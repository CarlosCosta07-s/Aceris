//! Measures host-side throughput while executing a locally supplied 3DS title.
//!
//! Run with:
//!   cargo run --release -p aceris-core --example benchmark -- GAME.cia
//!
//! Retail encrypted images require caller-supplied key material through the
//! environment variables documented in docs/PERFORMANCE_TESTING.md. No key is
//! embedded, discovered, persisted, or printed by this example.

use std::{
    collections::BTreeMap, env, hint::black_box, path::PathBuf, process::ExitCode, time::Instant,
};

use aceris_core::{
    DEFAULT_FRAME_INSTRUCTION_BUDGET, System,
    cpu::{Arm11Core, StepEvent},
    gpu::{
        DepthFunction, FCRAM_BASE, GpuPhysicalMemory, Pica200, REG_DEPTHMAP_ENABLE,
        REG_SCISSORTEST_MODE, REG_VIEWPORT_HEIGHT, REG_VIEWPORT_WIDTH, RasterVertex,
        SoftwareRasterizer,
    },
    loader::{
        ImageFormat, cia::TicketCommonKeyStore, identify_file, ncsd::session::ipc::IpcMessage,
    },
    memory::{Bus, FlatMemory},
    system::SystemFrameReport,
};

#[derive(Debug)]
struct Options {
    image: Option<PathBuf>,
    frames: usize,
    warmup_frames: usize,
    instruction_budget: usize,
    dump_lcd: Option<PathBuf>,
    frame_trace: Option<PathBuf>,
    taps: Vec<Tap>,
    profile_components: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Tap {
    x: i32,
    y: i32,
    frame: usize,
    hold_frames: usize,
}

#[derive(Clone, Debug)]
struct FrameTraceSample {
    input_frame: usize,
    report_frame: u64,
    elapsed_ns: u128,
    instructions_executed: u64,
    gpu_command_lists_submitted: u64,
    lcd_screens_scanned_out: u8,
    gsp_interrupt_deliveries: u8,
    cpu_stop_reason: String,
    pc: u32,
    cpsr: u32,
}

struct BenchmarkSummary<'a> {
    format: ImageFormat,
    samples_ns: &'a [u128],
    instructions: u128,
    gpu_lists: u128,
    lcd_scans: u128,
    zero_instruction_frames: usize,
    stop_reasons: &'a BTreeMap<String, u64>,
    first_report: Option<SystemFrameReport>,
    final_pc: u32,
    final_cpsr: u32,
}

fn usage() {
    println!(
        "Aceris performance benchmark\n\
         USAGE:\n\
         cargo run --release -p aceris-core --example benchmark -- <GAME> [OPTIONS]\n\
         cargo run --release -p aceris-core --example benchmark -- --profile-components\n\
         OPTIONS:\n\
         --profile-components      Run standalone CPU/memory/GPU/IPC microbenchmarks (no game required)\n\
         --frames <N>              Measured frames (default: 120)\n\
         --warmup <N>              Warm-up frames excluded from results (default: 15)\n\
         --instruction-budget <N>  ARM11 instruction budget per frame (default: {DEFAULT_FRAME_INSTRUCTION_BUDGET})\n\
         --dump-lcd <DIR>          Save final LCD surfaces as PNG after timing\n\
         --frame-trace <CSV>       Save per-frame timing and guest activity to CSV\n\
         --tap <X,Y@FRAME[+HOLD]>  Script a bottom-screen touch (repeatable; default HOLD=3)\n\
         -h, --help                Show this help\n\
         \n\
         Supports 3DSX, NCSD (.3ds/.cci) and CIA files supported by the current loader."
    );
}

fn parse_positive(value: &str, option: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{option} expects a positive integer, got {value:?}"))?;
    if parsed == 0 {
        return Err(format!("{option} must be greater than zero"));
    }
    Ok(parsed)
}

fn parse_tap(value: &str) -> Result<Tap, String> {
    let invalid = || format!("--tap expects X,Y@FRAME[+HOLD], got {value:?}");
    let (point, timing) = value.split_once('@').ok_or_else(invalid)?;
    let (x, y) = point.split_once(',').ok_or_else(invalid)?;
    let (frame, hold_frames) = match timing.split_once('+') {
        Some((frame, hold)) => (
            frame,
            hold.trim().parse::<usize>().map_err(|_| invalid())?.max(1),
        ),
        None => (timing, 3),
    };

    Ok(Tap {
        x: x.trim().parse().map_err(|_| invalid())?,
        y: y.trim().parse().map_err(|_| invalid())?,
        frame: frame.trim().parse().map_err(|_| invalid())?,
        hold_frames,
    })
}

fn apply_taps(system: &mut System, taps: &[Tap], frame: usize) {
    for tap in taps {
        if frame == tap.frame {
            system.input_mut().touch(tap.x, tap.y);
        } else if frame == tap.frame.saturating_add(tap.hold_frames) {
            system.input_mut().touch_end();
        }
    }
}

fn parse_args() -> Result<Option<Options>, String> {
    let mut args = env::args().skip(1);
    let Some(first) = args.next() else {
        return Err("missing game image path (or use --profile-components)".to_string());
    };
    if first == "--help" || first == "-h" {
        return Ok(None);
    }

    let profile_only = first == "--profile-components";
    let mut options = Options {
        image: (!profile_only).then(|| PathBuf::from(first)),
        frames: 120,
        warmup_frames: 15,
        instruction_budget: DEFAULT_FRAME_INSTRUCTION_BUDGET,
        dump_lcd: None,
        frame_trace: None,
        taps: Vec::new(),
        profile_components: profile_only,
    };

    while let Some(argument) = args.next() {
        if argument == "--profile-components" {
            options.profile_components = true;
            continue;
        }
        if options.image.is_none() {
            return Err(format!(
                "{argument} requires a game image; standalone component profiling accepts no other options"
            ));
        }
        let value = args
            .next()
            .ok_or_else(|| format!("{argument} requires a value"))?;
        match argument.as_str() {
            "--frames" => options.frames = parse_positive(&value, "--frames")?,
            "--warmup" => {
                options.warmup_frames = value.parse::<usize>().map_err(|_| {
                    format!("--warmup expects a non-negative integer, got {value:?}")
                })?;
            }
            "--instruction-budget" => {
                options.instruction_budget = parse_positive(&value, "--instruction-budget")?;
            }
            "--dump-lcd" => options.dump_lcd = Some(PathBuf::from(value)),
            "--frame-trace" => options.frame_trace = Some(PathBuf::from(value)),
            "--tap" => options.taps.push(parse_tap(&value)?),
            _ => return Err(format!("unknown option: {argument}")),
        }
    }

    Ok(Some(options))
}

fn parse_key(name: &str) -> Result<Option<[u8; 16]>, String> {
    let Ok(value) = env::var(name) else {
        return Ok(None);
    };
    let value = value.trim();
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);

    if value.len() != 32 {
        return Err(format!(
            "{name} must contain exactly 32 hexadecimal characters (the value is not printed)"
        ));
    }

    let mut key = [0u8; 16];
    for (index, byte) in key.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&value[start..start + 2], 16)
            .map_err(|_| format!("{name} contains a non-hexadecimal character"))?;
    }
    Ok(Some(key))
}

fn load_image(system: &mut System, path: &std::path::Path) -> Result<ImageFormat, String> {
    let format = identify_file(path)
        .map_err(|error| format!("could not identify {}: {error}", path.display()))?;

    let menu_info_key = parse_key("ACERIS_MENU_INFO_KEY")?;
    let content_key = parse_key("ACERIS_CONTENT_KEY")?;

    match format {
        ImageFormat::Ncsd => match (menu_info_key, content_key) {
            (Some(menu), Some(content)) => system
                .load_ncsd_file_with_keys(path, menu, content)
                .map_err(|error| format!("could not load NCSD image: {error}")),
            (None, None) => system
                .load_executable_file(path)
                .map_err(|error| {
                    format!(
                        "could not load NCSD image: {error}. Encrypted cartridges require                          ACERIS_MENU_INFO_KEY and ACERIS_CONTENT_KEY supplied by the user."
                    )
                }),
            _ => Err(
                "encrypted NCSD loading requires both ACERIS_MENU_INFO_KEY and                  ACERIS_CONTENT_KEY".to_string(),
            ),
        },
        ImageFormat::Cia => {
            let common_index = env::var("ACERIS_COMMON_KEY_INDEX").ok();
            let common_key_x = parse_key("ACERIS_COMMON_KEY_X")?;
            let common_key_y = parse_key("ACERIS_COMMON_KEY_Y")?;
            let normal_common_key = parse_key("ACERIS_NORMAL_COMMON_KEY")?;

            let has_any_key_material = menu_info_key.is_some()
                || content_key.is_some()
                || common_index.is_some()
                || common_key_x.is_some()
                || common_key_y.is_some()
                || normal_common_key.is_some();

            if !has_any_key_material {
                return system.load_executable_file(path).map_err(|error| {
                    format!(
                        "could not load CIA image: {error}. Encrypted CIA content requires                          caller-supplied menu/content keys and ticket common-key material."
                    )
                });
            }

            let menu = menu_info_key.ok_or_else(|| {
                "encrypted CIA loading requires ACERIS_MENU_INFO_KEY".to_string()
            })?;
            let content = content_key.ok_or_else(|| {
                "encrypted CIA loading requires ACERIS_CONTENT_KEY".to_string()
            })?;
            let index_text = common_index.ok_or_else(|| {
                "encrypted CIA loading requires ACERIS_COMMON_KEY_INDEX".to_string()
            })?;
            let index = index_text
                .parse::<u8>()
                .map_err(|_| "ACERIS_COMMON_KEY_INDEX must be an integer from 0 to 5".to_string())?;
            if index >= 6 {
                return Err("ACERIS_COMMON_KEY_INDEX must be an integer from 0 to 5".to_string());
            }

            let mut key_store = TicketCommonKeyStore::default();
            if let Some(normal_key) = normal_common_key {
                if common_key_x.is_some() || common_key_y.is_some() {
                    return Err(
                        "provide either ACERIS_NORMAL_COMMON_KEY or the KeyX/KeyY pair, not both"
                            .to_string(),
                    );
                }
                key_store
                    .set_normal_key(index, normal_key)
                    .map_err(|error| error.to_string())?;
            } else {
                let key_x = common_key_x.ok_or_else(|| {
                    "encrypted CIA loading requires ACERIS_COMMON_KEY_X (or ACERIS_NORMAL_COMMON_KEY)"
                        .to_string()
                })?;
                let key_y = common_key_y.ok_or_else(|| {
                    "encrypted CIA loading requires ACERIS_COMMON_KEY_Y (or ACERIS_NORMAL_COMMON_KEY)"
                        .to_string()
                })?;
                key_store.set_key_x(key_x);
                key_store
                    .set_key_y(index, key_y)
                    .map_err(|error| error.to_string())?;
            }

            system
                .load_cia_file_with_keys(path, &key_store, menu, content)
                .map_err(|error| format!("could not load CIA image: {error}"))
        }
        ImageFormat::ThreeDsx => system
            .load_executable_file(path)
            .map_err(|error| format!("could not load 3DSX image: {error}")),
        other => Err(format!(
            "{} images are recognized but not executable through the current benchmark loader",
            other.name()
        )),
    }
}

fn percentile(sorted_samples: &[u128], percentage: usize) -> u128 {
    if sorted_samples.is_empty() {
        return 0;
    }
    let rank = sorted_samples
        .len()
        .saturating_mul(percentage)
        .saturating_add(99)
        / 100;
    sorted_samples[rank.saturating_sub(1).min(sorted_samples.len() - 1)]
}

fn print_summary(options: &Options, summary: BenchmarkSummary<'_>) {
    let BenchmarkSummary {
        format,
        samples_ns,
        instructions,
        gpu_lists,
        lcd_scans,
        zero_instruction_frames,
        stop_reasons,
        first_report,
        final_pc,
        final_cpsr,
    } = summary;
    let mut sorted = samples_ns.to_vec();
    sorted.sort_unstable();

    let total_ns: u128 = samples_ns.iter().copied().sum();
    let average_ns = total_ns / samples_ns.len() as u128;
    let elapsed_seconds = total_ns as f64 / 1_000_000_000.0;
    let frames_per_second = samples_ns.len() as f64 / elapsed_seconds;
    let instructions_per_second = instructions as f64 / elapsed_seconds;

    println!();
    println!("Aceris benchmark results");
    println!("Image format:             {}", format.name());
    println!(
        "Host:                     {} / {}",
        env::consts::OS,
        env::consts::ARCH
    );
    println!("Warm-up frames:           {}", options.warmup_frames);
    println!("Measured frames:          {}", samples_ns.len());
    println!("Instruction budget/frame: {}", options.instruction_budget);
    println!("Host elapsed:             {:.3} s", elapsed_seconds);
    println!(
        "Host throughput:          {:.2} frames/s",
        frames_per_second
    );
    println!(
        "Mean frame processing:    {:.3} ms",
        average_ns as f64 / 1_000_000.0
    );
    println!(
        "Frame time p50/p95/p99:   {:.3} / {:.3} / {:.3} ms",
        percentile(&sorted, 50) as f64 / 1_000_000.0,
        percentile(&sorted, 95) as f64 / 1_000_000.0,
        percentile(&sorted, 99) as f64 / 1_000_000.0
    );
    println!(
        "Frame time min/max:       {:.3} / {:.3} ms",
        sorted.first().copied().unwrap_or_default() as f64 / 1_000_000.0,
        sorted.last().copied().unwrap_or_default() as f64 / 1_000_000.0
    );
    println!("Guest instructions:       {instructions}");
    println!("Guest instructions/s:     {:.0}", instructions_per_second);
    println!("GSP command lists:        {gpu_lists}");
    println!("LCD scanout operations:   {lcd_scans}");
    println!("Zero-instruction frames:  {zero_instruction_frames}");

    if let Some(report) = first_report {
        println!("First measured stop:      {:?}", report.cpu_stop_reason);
        println!("Final guest PC/CPSR:      {final_pc:#010X} / {final_cpsr:#010X}");
    }
    println!("Stop reasons:");
    for (reason, count) in stop_reasons {
        println!("  {reason}: {count}");
    }

    if zero_instruction_frames == samples_ns.len() {
        println!(
            "WARNING: no guest instructions ran in the measured interval; this is not a meaningful game-performance result."
        );
    } else if zero_instruction_frames > 0 {
        println!(
            "WARNING: some frames executed no guest instructions; inspect stop reasons before comparing results."
        );
    }

    println!();
    println!("Interpretation:");
    println!(
        "- Host throughput is the time this build spent processing bounded frames, not a cycle-accurate 3DS FPS measurement."
    );
    println!(
        "- The instruction budget is a deterministic cap, not a hardware clock or proof that the game advanced one full frame."
    );
    println!(
        "- GPU-list and LCD counters measure submitted work; they do not prove complete PICA200 rendering, textures, audio, or gameplay."
    );
    println!(
        "- Compare runs only with the same image, build mode, frame budget, warm-up, host power mode, and settings."
    );
    println!(
        "- Key material is read only from this process environment and is never printed by this example."
    );
}

fn print_operation_rate(label: &str, operations: u64, elapsed: std::time::Duration) {
    let seconds = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    let operations_per_second = operations as f64 / seconds;
    let nanoseconds_per_operation = elapsed.as_nanos() as f64 / operations.max(1) as f64;
    println!(
        "{label:<34} {operations_per_second:>12.0} ops/s  {nanoseconds_per_operation:>10.1} ns/op"
    );
}

fn print_memory_rate(label: &str, operations: u64, elapsed: std::time::Duration) {
    let seconds = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    let operations_per_second = operations as f64 / seconds;
    let nanoseconds_per_operation = elapsed.as_nanos() as f64 / operations.max(1) as f64;
    let mebibytes_per_second = operations as f64 * 4.0 / seconds / (1024.0 * 1024.0);
    println!(
        "{label:<34} {operations_per_second:>12.0} ops/s  {nanoseconds_per_operation:>10.1} ns/op  {mebibytes_per_second:>9.1} MiB/s"
    );
}

fn profile_cpu(iterations: usize) -> Result<(), String> {
    const CODE_BASE: u32 = 0x1000;
    const INSTRUCTIONS: [u32; 4] = [
        0xE3A0_0001, // MOV r0, #1
        0xE280_0001, // ADD r0, r0, #1
        0xE350_0080, // CMP r0, #0x80
        0xE1A0_1000, // MOV r1, r0
    ];
    let code_bytes = (INSTRUCTIONS.len() * 4) as u32;
    let mut memory = FlatMemory::new(CODE_BASE, code_bytes as usize);
    for (index, instruction) in INSTRUCTIONS.iter().copied().enumerate() {
        Bus::write32(&mut memory, CODE_BASE + (index as u32 * 4), instruction)
            .map_err(|error| format!("CPU profile setup failed: {error}"))?;
    }

    let mut cpu = Arm11Core::new();
    cpu.regs[15] = CODE_BASE;
    let started = Instant::now();
    for _ in 0..iterations {
        if cpu.regs[15] >= CODE_BASE + code_bytes {
            cpu.regs[15] = CODE_BASE;
        }
        match cpu
            .step(&mut memory)
            .map_err(|error| format!("CPU profile instruction failed: {error}"))?
        {
            StepEvent::Executed => {}
            event => return Err(format!("CPU profile stopped unexpectedly: {event:?}")),
        }
    }
    let elapsed = started.elapsed();
    black_box((cpu.regs[0], cpu.regs[1], cpu.cpsr.0));
    print_operation_rate("ARM11 interpreter (A32 mix)", iterations as u64, elapsed);
    Ok(())
}

fn profile_cpu_thumb(iterations: usize) -> Result<std::time::Duration, String> {
    const CODE_BASE: u32 = 0x2000;
    const INSTRUCTIONS: [u16; 4] = [
        0x2001, // MOVS r0, #1
        0x3001, // ADDS r0, #1
        0x2880, // CMP r0, #0x80
        0x0001, // MOVS r1, r0
    ];
    let code_bytes = (INSTRUCTIONS.len() * 2) as u32;
    let mut memory = FlatMemory::new(CODE_BASE, code_bytes as usize);
    for (index, instruction) in INSTRUCTIONS.iter().copied().enumerate() {
        Bus::write16(&mut memory, CODE_BASE + (index as u32 * 2), instruction)
            .map_err(|error| format!("Thumb profile setup failed: {error}"))?;
    }

    let mut cpu = Arm11Core::new();
    cpu.regs[15] = CODE_BASE;
    cpu.cpsr.set_thumb(true);
    let started = Instant::now();
    for _ in 0..iterations {
        if cpu.regs[15] >= CODE_BASE + code_bytes {
            cpu.regs[15] = CODE_BASE;
        }
        match cpu
            .step(&mut memory)
            .map_err(|error| format!("Thumb profile instruction failed: {error}"))?
        {
            StepEvent::Executed => {}
            event => return Err(format!("Thumb profile stopped unexpectedly: {event:?}")),
        }
    }
    let elapsed = started.elapsed();
    black_box((cpu.regs[0], cpu.regs[1], cpu.cpsr.0));
    Ok(elapsed)
}

fn profile_pica_commands(command_lists: usize) -> Result<std::time::Duration, String> {
    const FULL_BYTE_MASK: u32 = 0xF << 16;
    const COMMANDS: [u32; 8] = [
        400,
        REG_VIEWPORT_WIDTH as u32 | FULL_BYTE_MASK,
        240,
        REG_VIEWPORT_HEIGHT as u32 | FULL_BYTE_MASK,
        0,
        REG_SCISSORTEST_MODE as u32 | FULL_BYTE_MASK,
        0,
        REG_DEPTHMAP_ENABLE as u32 | FULL_BYTE_MASK,
    ];

    let mut pica = Pica200::new();
    let started = Instant::now();
    for _ in 0..command_lists {
        let report = pica
            .execute_command_list(&COMMANDS)
            .map_err(|error| format!("PICA command profile failed: {error}"))?;
        black_box(report);
    }
    let elapsed = started.elapsed();
    black_box(pica.register(REG_VIEWPORT_WIDTH));
    Ok(elapsed)
}

fn profile_memory(iterations: usize) -> Result<(), String> {
    const BASE: u32 = 0x1000;
    const REGION_BYTES: usize = 64 * 1024;
    const WORD_COUNT: usize = REGION_BYTES / 4;

    let mut flat = FlatMemory::new(BASE, REGION_BYTES);
    let mut physical = GpuPhysicalMemory::new();
    let physical_base = FCRAM_BASE + 0x1000;

    for index in 0..WORD_COUNT {
        let value = index as u32 ^ 0xA5A5_5A5A;
        let address = BASE + (index as u32 * 4);
        Bus::write32(&mut flat, address, value)
            .map_err(|error| format!("FlatMemory profile initialization failed: {error}"))?;
        physical
            .write_u32(physical_base + (index as u32 * 4), value)
            .map_err(|error| format!("GPU memory profile initialization failed: {error}"))?;
    }

    let started = Instant::now();
    for index in 0..iterations {
        let slot = index & (WORD_COUNT - 1);
        let address = BASE + (slot as u32 * 4);
        Bus::write32(&mut flat, address, black_box(index as u32))
            .map_err(|error| format!("FlatMemory write benchmark failed: {error}"))?;
    }
    let elapsed = started.elapsed();
    print_memory_rate("FlatMemory write32", iterations as u64, elapsed);

    let started = Instant::now();
    for index in 0..iterations {
        let slot = index & (WORD_COUNT - 1);
        let address = BASE + (slot as u32 * 4);
        black_box(
            Bus::read32(&mut flat, address)
                .map_err(|error| format!("FlatMemory read benchmark failed: {error}"))?,
        );
    }
    let elapsed = started.elapsed();
    print_memory_rate("FlatMemory read32", iterations as u64, elapsed);

    let started = Instant::now();
    for index in 0..iterations {
        let slot = index & (WORD_COUNT - 1);
        let address = physical_base + (slot as u32 * 4);
        physical
            .write_u32(address, black_box(index as u32))
            .map_err(|error| format!("GPU backing write benchmark failed: {error}"))?;
    }
    let elapsed = started.elapsed();
    print_memory_rate("Sparse GPU backing write32", iterations as u64, elapsed);

    let started = Instant::now();
    for index in 0..iterations {
        let slot = index & (WORD_COUNT - 1);
        let address = physical_base + (slot as u32 * 4);
        black_box(
            physical
                .read_u32(address)
                .map_err(|error| format!("GPU backing read benchmark failed: {error}"))?,
        );
    }
    let elapsed = started.elapsed();
    print_memory_rate("Sparse GPU backing read32", iterations as u64, elapsed);
    Ok(())
}

fn profile_gpu(iterations: usize) -> Result<(), String> {
    const WIDTH: usize = 400;
    const HEIGHT: usize = 240;
    let mut rasterizer = SoftwareRasterizer::new(WIDTH, HEIGHT)
        .map_err(|error| format!("GPU profile target creation failed: {error}"))?;
    let triangle = [
        RasterVertex::new(0.0, 0.0, 0.5, [255, 64, 32, 255]),
        RasterVertex::new(WIDTH as f32, 0.0, 0.5, [64, 255, 32, 255]),
        RasterVertex::new(0.0, HEIGHT as f32, 0.5, [32, 64, 255, 255]),
    ];

    let started = Instant::now();
    let mut pixels_written = 0u64;
    for _ in 0..iterations {
        let report = rasterizer
            .draw_triangle(triangle, DepthFunction::Always)
            .map_err(|error| format!("GPU profile rasterization failed: {error}"))?;
        pixels_written = pixels_written.saturating_add(report.pixels_written);
    }
    let elapsed = started.elapsed();
    let seconds = elapsed.as_secs_f64().max(f64::MIN_POSITIVE);
    let triangles_per_second = iterations as f64 / seconds;
    let pixels_per_second = pixels_written as f64 / seconds;
    println!(
        "{:<34} {:>12.1} triangles/s  {:>12.0} pixels/s",
        "Software rasterizer (reference)", triangles_per_second, pixels_per_second
    );
    black_box(rasterizer.color_buffer());
    Ok(())
}

fn profile_ipc(iterations: usize) -> Result<(), String> {
    // Two normal parameters plus one mapped-buffer descriptor and its address.
    const MESSAGE: [u32; 5] = [
        0x1234_0082,
        0xAABB_CCDD,
        0x1122_3344,
        0x0001_000A,
        0x0040_0000,
    ];

    let started = Instant::now();
    let mut decoded_parameters = 0usize;
    for _ in 0..iterations {
        let message = IpcMessage::decode(&MESSAGE)
            .map_err(|error| format!("IPC profile message decode failed: {error:?}"))?;
        let parameters = message
            .translation_parameters()
            .map_err(|error| format!("IPC profile translation decode failed: {error:?}"))?;
        decoded_parameters = decoded_parameters.saturating_add(parameters.len());
        black_box(message.header().command_id());
        black_box(parameters);
    }
    let elapsed = started.elapsed();
    black_box(decoded_parameters);
    print_operation_rate(
        "IPC message + translation decode",
        iterations as u64,
        elapsed,
    );
    Ok(())
}

fn run_component_profile_once() -> Result<(), String> {
    const ITERATIONS: usize = 1_000_000;
    const GPU_TRIANGLES: usize = 64;
    const PICA_COMMAND_LISTS: usize = 250_000;

    println!();
    println!("Aceris standalone component profile");
    println!("Host: {} / {}", env::consts::OS, env::consts::ARCH);
    println!("CPU A32/Thumb instructions: {ITERATIONS} each");
    println!("PICA command lists: {PICA_COMMAND_LISTS} (four register writes/list)");
    println!("Memory operations per path: {ITERATIONS}");
    println!("GPU triangles: {GPU_TRIANGLES} at 400x240");
    println!("IPC messages: {ITERATIONS}");
    println!();
    profile_cpu(ITERATIONS)?;
    let thumb_elapsed = profile_cpu_thumb(ITERATIONS)?;
    print_operation_rate(
        "ARM11 interpreter (Thumb mix)",
        ITERATIONS as u64,
        thumb_elapsed,
    );
    let pica_elapsed = profile_pica_commands(PICA_COMMAND_LISTS)?;
    print_operation_rate(
        "PICA command lists (4 registers)",
        PICA_COMMAND_LISTS as u64,
        pica_elapsed,
    );
    profile_memory(ITERATIONS)?;
    profile_gpu(GPU_TRIANGLES)?;
    profile_ipc(ITERATIONS)?;
    println!();
    println!("Scope and interpretation:");
    println!(
        "- CPU measures bare A32 and Thumb instruction mixes; both exclude the process scheduler and guest MMU."
    );
    println!(
        "- PICA measures command-list decoding and four register writes per list; it excludes geometry, shaders, rasterization and GPU memory traffic."
    );
    println!(
        "- Memory compares FlatMemory with the sparse GPU physical backing; it does not measure the full ARM11 D-cache/session path."
    );
    println!(
        "- GPU measures the software reference rasterizer only; it is not the complete PICA200 shader/texture/TEV pipeline."
    );
    println!(
        "- IPC measures message and translation-descriptor decoding; it excludes SRV lookup, handle transfer, scheduling and service execution."
    );
    println!(
        "- These are host microbenchmarks. Use the game benchmark separately for end-to-end guest throughput."
    );
    println!();
    Ok(())
}

fn run_component_profile() -> Result<(), String> {
    const MEASURED_REPETITIONS: usize = 5;

    println!("Component profile warm-up");
    run_component_profile_once()?;

    for repetition in 1..=MEASURED_REPETITIONS {
        println!();
        println!("Component profile repetition {repetition}/{MEASURED_REPETITIONS}");
        run_component_profile_once()?;
    }

    println!();
    println!("Use the median of the five measured repetitions; discard the warm-up above.");
    Ok(())
}

fn dump_lcd(system: &System, directory: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(directory)
        .map_err(|error| format!("could not create LCD output directory: {error}"))?;
    for (name, surface) in [
        ("top_left.png", system.display().top_left()),
        ("top_right.png", system.display().top_right()),
        ("bottom.png", system.display().bottom()),
    ] {
        std::fs::write(directory.join(name), surface.to_png())
            .map_err(|error| format!("could not write {name}: {error}"))?;
    }
    Ok(())
}

fn run(options: Options) -> Result<(), String> {
    if options.profile_components {
        run_component_profile()?;
    }
    let Some(image_path) = options.image.as_deref() else {
        return Ok(());
    };

    let mut system = System::new();
    let format = load_image(&mut system, image_path)?;
    println!("Loaded {:?}: {}", format, image_path.display());
    println!(
        "Prepared ARM11 entry: PC={:#010X}, CPSR={:#010X}",
        system.cpu().pc(),
        system.cpu().cpsr.0
    );

    for frame in 0..options.warmup_frames {
        apply_taps(&mut system, &options.taps, frame);
        let report = system
            .run_frame_with_budget(options.instruction_budget)
            .map_err(|error| format!("warm-up frame {} failed: {error}", frame + 1))?;
        if report.instructions_executed == 0 {
            eprintln!(
                "warning: warm-up frame {} executed no guest instructions (stop: {:?})",
                frame + 1,
                report.cpu_stop_reason
            );
        }
    }

    let mut samples_ns = Vec::with_capacity(options.frames);
    let mut instructions = 0u128;
    let mut gpu_lists = 0u128;
    let mut lcd_scans = 0u128;
    let mut zero_instruction_frames = 0usize;
    let mut stop_reasons = BTreeMap::<String, u64>::new();
    let mut first_report = None;
    let mut frame_trace = options
        .frame_trace
        .as_ref()
        .map(|_| Vec::with_capacity(options.frames));

    for frame in 0..options.frames {
        let input_frame = options.warmup_frames.saturating_add(frame);
        apply_taps(&mut system, &options.taps, input_frame);
        let started = Instant::now();
        let report = system
            .run_frame_with_budget(options.instruction_budget)
            .map_err(|error| format!("measured frame {} failed: {error}", frame + 1))?;
        let elapsed_ns = started.elapsed().as_nanos();
        samples_ns.push(elapsed_ns);

        if let Some(trace) = frame_trace.as_mut() {
            trace.push(FrameTraceSample {
                input_frame,
                report_frame: report.frame_number,
                elapsed_ns,
                instructions_executed: report.instructions_executed,
                gpu_command_lists_submitted: report.gpu_command_lists_submitted,
                lcd_screens_scanned_out: report.lcd_screens_scanned_out,
                gsp_interrupt_deliveries: report.gsp_interrupt_deliveries,
                cpu_stop_reason: format!("{:?}", report.cpu_stop_reason),
                pc: system.cpu().pc(),
                cpsr: system.cpu().cpsr.0,
            });
        }

        instructions = instructions.saturating_add(u128::from(report.instructions_executed));
        gpu_lists = gpu_lists.saturating_add(u128::from(report.gpu_command_lists_submitted));
        lcd_scans = lcd_scans.saturating_add(u128::from(report.lcd_screens_scanned_out));
        if report.instructions_executed == 0 {
            zero_instruction_frames += 1;
        }
        *stop_reasons
            .entry(format!("{:?}", report.cpu_stop_reason))
            .or_default() += 1;
        first_report.get_or_insert(report);
    }

    print_summary(
        &options,
        BenchmarkSummary {
            format,
            samples_ns: &samples_ns,
            instructions,
            gpu_lists,
            lcd_scans,
            zero_instruction_frames,
            stop_reasons: &stop_reasons,
            first_report,
            final_pc: system.cpu().pc(),
            final_cpsr: system.cpu().cpsr.0,
        },
    );
    if let Some(session) = system.ncsd_session() {
        let memory = session.address_space().memory_usage_report();
        println!();
        println!("Internal memory diagnostics (not host-process RSS):");
        println!(
            "Static segment vector capacity: {} KiB",
            memory.static_segment_allocated_bytes / 1024
        );
        println!(
            "Application heap reservation:  {} KiB",
            memory.application_heap_virtual_bytes / 1024
        );
        println!(
            "Dynamic regions total:         {} KiB",
            memory.dynamic_region_virtual_bytes / 1024
        );
        println!(
            "Shared mapping virtual size:   {} KiB",
            memory.shared_mapping_virtual_bytes / 1024
        );
        println!(
            "Sparse backing:                {} pages / {} KiB across {} root(s)",
            memory.sparse_backing_pages,
            memory.sparse_backing_allocated_bytes / 1024,
            memory.unique_sparse_backing_count
        );
        println!(
            "ARM11 D-cache:                 {} lines / {} KiB",
            memory.data_cache_lines,
            memory.data_cache_bytes / 1024
        );
        println!(
            "D-cache write buffer:          {}/{} entries ({} bytes reserved)",
            memory.write_buffer_entries,
            memory.write_buffer_capacity,
            memory.write_buffer_reserved_bytes
        );
        println!(
            "Exclusive monitor entries:    {} virtual / {} shared",
            memory.virtual_exclusive_monitor_entries, memory.sparse_exclusive_monitor_entries
        );
        println!(
            "TLS slots:                     {}/{} in use; IPC aliases: {}",
            memory.tls_slots_in_use, memory.tls_slots_total, memory.ipc_temporary_mapping_count
        );
        println!(
            "GPU physical backing:          {} pages / {} KiB",
            memory.gpu_physical_pages,
            memory.gpu_physical_allocated_bytes / 1024
        );
    }
    if let Some(directory) = options.dump_lcd.as_ref() {
        dump_lcd(&system, directory)?;
        println!("LCD captures written to {}", directory.display());
    }
    Ok(())
}

pub fn run_cli() -> ExitCode {
    match parse_args() {
        Ok(None) => {
            usage();
            ExitCode::SUCCESS
        }
        Ok(Some(options)) => match run(options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("benchmark failed: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("error: {error}");
            usage();
            ExitCode::FAILURE
        }
    }
}

#[allow(dead_code)]
fn main() -> ExitCode {
    run_cli()
}
