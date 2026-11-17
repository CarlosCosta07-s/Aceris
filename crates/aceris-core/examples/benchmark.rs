//! Measures host-side throughput while executing a locally supplied 3DS title.
//!
//! Run with:
//!   cargo run --release -p aceris-core --example benchmark -- GAME.cia
//!
//! Retail encrypted images require caller-supplied key material through the
//! environment variables documented in docs/PERFORMANCE_TESTING.md. No key is
//! embedded, discovered, persisted, or printed by this example.

use std::{collections::BTreeMap, env, path::PathBuf, process::ExitCode, time::Instant};

use aceris_core::{
    DEFAULT_FRAME_INSTRUCTION_BUDGET, System,
    loader::{ImageFormat, cia::TicketCommonKeyStore, identify_file},
    system::SystemFrameReport,
};

#[derive(Debug)]
struct Options {
    image: PathBuf,
    frames: usize,
    warmup_frames: usize,
    instruction_budget: usize,
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
         OPTIONS:\n\
         --frames <N>              Measured frames (default: 120)\n\
         --warmup <N>              Warm-up frames excluded from results (default: 15)\n\
         --instruction-budget <N>  ARM11 instruction budget per frame (default: {DEFAULT_FRAME_INSTRUCTION_BUDGET})\n\
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

fn parse_args() -> Result<Option<Options>, String> {
    let mut args = env::args().skip(1);
    let Some(first) = args.next() else {
        return Err("missing game image path".to_string());
    };
    if first == "--help" || first == "-h" {
        return Ok(None);
    }

    let mut options = Options {
        image: PathBuf::from(first),
        frames: 120,
        warmup_frames: 15,
        instruction_budget: DEFAULT_FRAME_INSTRUCTION_BUDGET,
    };

    while let Some(argument) = args.next() {
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

fn run(options: Options) -> Result<(), String> {
    let mut system = System::new();
    let format = load_image(&mut system, &options.image)?;
    println!("Loaded {:?}: {}", format, options.image.display());
    println!(
        "Prepared ARM11 entry: PC={:#010X}, CPSR={:#010X}",
        system.cpu().pc(),
        system.cpu().cpsr.0
    );

    for frame in 0..options.warmup_frames {
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

    for frame in 0..options.frames {
        let started = Instant::now();
        let report = system
            .run_frame_with_budget(options.instruction_budget)
            .map_err(|error| format!("measured frame {} failed: {error}", frame + 1))?;
        samples_ns.push(started.elapsed().as_nanos());

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
        format,
        &samples_ns,
        instructions,
        gpu_lists,
        lcd_scans,
        zero_instruction_frames,
        &stop_reasons,
        first_report,
        system.cpu().pc(),
        system.cpu().cpsr.0,
    );
    Ok(())
}

fn main() -> ExitCode {
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
