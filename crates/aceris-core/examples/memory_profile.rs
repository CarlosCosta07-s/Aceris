//! Focused profiling for sparse GPU physical-memory access patterns.
//!
//! Run with:
//!   cargo run --release -p aceris-core --example memory_profile
//!
//! This measures the existing public GpuPhysicalMemory API. It does not change
//! the memory implementation or claim to measure physical DRAM bandwidth.

use std::{
    hint::black_box,
    time::{Duration, Instant},
};

use aceris_core::gpu::{FCRAM_BASE, GpuPhysicalMemory, PHYSICAL_PAGE_SIZE};

const ITERATIONS: usize = 1_000_000;
const MEASURED_REPETITIONS: usize = 5;
const WORKING_SET_BYTES: usize = 4 * 1024 * 1024;
const WORKING_SET_WORDS: usize = WORKING_SET_BYTES / 4;
const PAGE_WORDS: usize = PHYSICAL_PAGE_SIZE / 4;
const BASE: u32 = FCRAM_BASE + 0x0100_0000;

struct Pattern {
    label: &'static str,
    addresses: Vec<u32>,
}

fn patterns() -> [Pattern; 3] {
    let hot_page = (0..ITERATIONS)
        .map(|index| BASE + ((index & (PAGE_WORDS - 1)) * 4) as u32)
        .collect();

    let sequential = (0..ITERATIONS)
        .map(|index| BASE + ((index & (WORKING_SET_WORDS - 1)) * 4) as u32)
        .collect();

    let mut random_state = 0x6D2B_79F5u32;
    let random_pages = (0..ITERATIONS)
        .map(|_| {
            random_state ^= random_state << 13;
            random_state ^= random_state >> 17;
            random_state ^= random_state << 5;
            BASE + ((random_state as usize & (WORKING_SET_WORDS - 1)) * 4) as u32
        })
        .collect();

    [
        Pattern {
            label: "Hot 4 KiB page",
            addresses: hot_page,
        },
        Pattern {
            label: "Sequential 4 MiB",
            addresses: sequential,
        },
        Pattern {
            label: "Pseudo-random 4 MiB",
            addresses: random_pages,
        },
    ]
}

fn read_pattern(memory: &GpuPhysicalMemory, addresses: &[u32]) -> Result<Duration, String> {
    let started = Instant::now();
    for address in addresses {
        let value = memory
            .read_u32(*address)
            .map_err(|error| format!("physical-memory read failed: {error}"))?;
        black_box(value);
    }
    Ok(started.elapsed())
}

fn write_pattern(memory: &mut GpuPhysicalMemory, addresses: &[u32]) -> Result<Duration, String> {
    let started = Instant::now();
    for (index, address) in addresses.iter().enumerate() {
        memory
            .write_u32(*address, black_box(index as u32))
            .map_err(|error| format!("physical-memory write failed: {error}"))?;
    }
    Ok(started.elapsed())
}

fn median_duration(samples: &mut [Duration]) -> Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

fn print_rate(label: &str, duration: Duration) {
    let seconds = duration.as_secs_f64().max(f64::MIN_POSITIVE);
    let operations_per_second = ITERATIONS as f64 / seconds;
    let nanoseconds_per_operation = duration.as_nanos() as f64 / ITERATIONS as f64;
    let mebibytes_per_second = ITERATIONS as f64 * 4.0 / seconds / (1024.0 * 1024.0);

    println!(
        "{label:<32} {operations_per_second:>12.0} ops/s  \
         {nanoseconds_per_operation:>9.1} ns/op  {mebibytes_per_second:>9.1} MiB/s"
    );
}

fn run() -> Result<(), String> {
    let patterns = patterns();
    let mut memory = GpuPhysicalMemory::new();
    let zeroed_working_set = vec![0u8; WORKING_SET_BYTES];
    memory
        .write(BASE, &zeroed_working_set)
        .map_err(|error| format!("could not pre-materialize physical pages: {error}"))?;

    println!("Aceris sparse GPU physical-memory profile");
    println!(
        "Host: {} / {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("Working set: {} MiB", WORKING_SET_BYTES / (1024 * 1024));
    println!("Materialized pages: {}", memory.allocated_page_count());
    println!("Operations per case: {ITERATIONS}");
    println!("Measured repetitions: {MEASURED_REPETITIONS} (one warm-up discarded)");
    println!("Page allocation and address generation are outside timed regions.");
    println!();

    let mut samples: Vec<Vec<Duration>> = (0..patterns.len() * 2)
        .map(|_| Vec::with_capacity(MEASURED_REPETITIONS))
        .collect();

    for repetition in 0..=MEASURED_REPETITIONS {
        for (pattern_index, pattern) in patterns.iter().enumerate() {
            let read = read_pattern(&memory, &pattern.addresses)?;
            let write = write_pattern(&mut memory, &pattern.addresses)?;
            if repetition > 0 {
                samples[pattern_index * 2].push(read);
                samples[pattern_index * 2 + 1].push(write);
            }
        }
    }

    println!("Median of five measured repetitions:");
    for (index, pattern) in patterns.iter().enumerate() {
        let read = median_duration(&mut samples[index * 2]);
        let write = median_duration(&mut samples[index * 2 + 1]);
        print_rate(&format!("{} read32", pattern.label), read);
        print_rate(&format!("{} write32", pattern.label), write);
    }

    println!();
    println!("Interpretation:");
    println!("- All cases use the same pre-materialized 4 MiB backing and 4-byte operations.");
    println!(
        "- Hot-page accesses stay within one 4 KiB page; sequential and pseudo-random cases span 4 MiB."
    );
    println!("- Writes include the implementation's exclusive-reservation version updates.");
    println!(
        "- Reads/writes include range validation, RwLock acquisition and sparse BTreeMap lookup."
    );
    println!(
        "- This is a single-threaded host microbenchmark, not DRAM bandwidth or a full guest CPU/GPU workload."
    );
    println!(
        "- No memory implementation, synchronization policy or emulation semantics are changed."
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("memory profile failed: {error}");
        std::process::exit(1);
    }
}
