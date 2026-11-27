//! Opt-in desktop frame timing diagnostics.
//!
//! This monitor measures host-side work around the desktop render loop. It
//! does not change the 3DS guest clock, instruction budget, or frame pacing.

use std::path::Path;
use std::time::{Duration, Instant};

const SAMPLE_CAPACITY: usize = 300;
const REPORT_INTERVAL: Duration = Duration::from_secs(5);
const NANOSECONDS_PER_MILLISECOND: f64 = 1_000_000.0;

/// The host window currently being measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DesktopFrameKind {
    Intro,
    Library,
    Game,
}

impl DesktopFrameKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Intro => "intro",
            Self::Library => "library",
            Self::Game => "game",
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct FrameSample {
    frame_ns: u64,
    emulation_ns: u64,
    composition_ns: u64,
    audio_ns: u64,
    presentation_ns: u64,
    presentation_interval_ns: u64,
}

/// Rolling host-side timing window for the desktop render loop.
pub(crate) struct DesktopPerformanceMonitor {
    kind: Option<DesktopFrameKind>,
    samples: [FrameSample; SAMPLE_CAPACITY],
    sample_count: usize,
    next_sample: usize,
    interval_started: Instant,
    last_presentation: Option<Instant>,
    presented_frames: u64,
    gpu_presented_frames: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct DesktopPerformanceReport {
    presented_fps: f64,
    sample_count: usize,
    frame_average_ms: f64,
    frame_p50_ms: f64,
    frame_p95_ms: f64,
    emulation_p95_ms: f64,
    composition_p95_ms: f64,
    audio_p95_ms: f64,
    presentation_p95_ms: f64,
    presentation_interval_p50_ms: f64,
    presentation_interval_p95_ms: f64,
    gpu_presentation_percent: f64,
}

impl DesktopPerformanceMonitor {
    pub(crate) fn new() -> Self {
        Self {
            kind: None,
            samples: [FrameSample::default(); SAMPLE_CAPACITY],
            sample_count: 0,
            next_sample: 0,
            interval_started: Instant::now(),
            last_presentation: None,
            presented_frames: 0,
            gpu_presented_frames: 0,
        }
    }

    /// Records one successfully presented host frame. The render-loop mode is
    /// tracked separately so intro, library, and gameplay results are not mixed.
    pub(crate) fn record(
        &mut self,
        kind: DesktopFrameKind,
        frame_time: Duration,
        emulation_time: Duration,
        composition_time: Duration,
        audio_time: Duration,
        presentation_time: Duration,
        gpu_presented: bool,
    ) {
        if self.kind != Some(kind) {
            self.reset(kind);
        }

        let now = Instant::now();
        let presentation_interval = self
            .last_presentation
            .map(|last| now.duration_since(last))
            .filter(|interval| *interval <= Duration::from_secs(2))
            .unwrap_or_default();
        self.last_presentation = Some(now);

        self.samples[self.next_sample] = FrameSample {
            frame_ns: duration_ns(frame_time),
            emulation_ns: duration_ns(emulation_time),
            composition_ns: duration_ns(composition_time),
            audio_ns: duration_ns(audio_time),
            presentation_ns: duration_ns(presentation_time),
            presentation_interval_ns: duration_ns(presentation_interval),
        };
        self.next_sample = (self.next_sample + 1) % SAMPLE_CAPACITY;
        self.sample_count = (self.sample_count + 1).min(SAMPLE_CAPACITY);
        self.presented_frames += 1;
        if gpu_presented {
            self.gpu_presented_frames += 1;
        }

        let elapsed = self.interval_started.elapsed();
        if elapsed < REPORT_INTERVAL {
            return;
        }

        let report = summarize(
            &self.samples,
            self.sample_count,
            elapsed,
            self.presented_frames,
            self.gpu_presented_frames,
        );
        eprintln!(
            "Aceris desktop performance [{}]: {report}",
            kind.label()
        );
        self.reset(kind);
    }

    fn reset(&mut self, kind: DesktopFrameKind) {
        self.kind = Some(kind);
        self.sample_count = 0;
        self.next_sample = 0;
        self.interval_started = Instant::now();
        self.last_presentation = None;
        self.presented_frames = 0;
        self.gpu_presented_frames = 0;
    }
}

impl std::fmt::Display for DesktopPerformanceReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "presented_fps={:.1}, frame_ms_avg/p50/p95={:.2}/{:.2}/{:.2}, emulation_ms_p95={:.2}, composition_ms_p95={:.2}, audio_ms_p95={:.2}, presentation_ms_p95={:.2}, presentation_interval_ms_p50/p95={:.2}/{:.2}, gpu_present={:.0}%, samples={}",
            self.presented_fps,
            self.frame_average_ms,
            self.frame_p50_ms,
            self.frame_p95_ms,
            self.emulation_p95_ms,
            self.composition_p95_ms,
            self.audio_p95_ms,
            self.presentation_p95_ms,
            self.presentation_interval_p50_ms,
            self.presentation_interval_p95_ms,
            self.gpu_presentation_percent,
            self.sample_count,
        )
    }
}

/// Enables desktop timing logs with ACERIS_PERFORMANCE_LOG=1 or when
/// <games>/.aceris/desktop_performance_log_v1.txt contains exactly "1".
pub(crate) fn performance_logging_enabled(settings_directory: &Path) -> bool {
    std::env::var("ACERIS_PERFORMANCE_LOG").is_ok_and(|value| value == "1")
        || settings_file_enabled(settings_directory)
}

fn settings_file_enabled(settings_directory: &Path) -> bool {
    std::fs::read_to_string(settings_directory.join("desktop_performance_log_v1.txt"))
        .is_ok_and(|value| value.trim() == "1")
}

fn duration_ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn percentile_ns(
    samples: &[FrameSample; SAMPLE_CAPACITY],
    sample_count: usize,
    select: fn(FrameSample) -> u64,
    percentile: usize,
) -> f64 {
    let count = sample_count.min(SAMPLE_CAPACITY);
    if count == 0 {
        return 0.0;
    }
    let mut values = [0u64; SAMPLE_CAPACITY];
    for (destination, sample) in values[..count].iter_mut().zip(samples[..count].iter()) {
        *destination = select(*sample);
    }
    values[..count].sort_unstable();
    let rank = (count * percentile).div_ceil(100).saturating_sub(1);
    values[rank.min(count - 1)] as f64
}

fn percentile_nonzero_ns(
    samples: &[FrameSample; SAMPLE_CAPACITY],
    sample_count: usize,
    select: fn(FrameSample) -> u64,
    percentile: usize,
) -> f64 {
    let mut values = [0u64; SAMPLE_CAPACITY];
    let mut count = 0usize;
    for sample in samples.iter().take(sample_count.min(SAMPLE_CAPACITY)) {
        let value = select(*sample);
        if value != 0 {
            values[count] = value;
            count += 1;
        }
    }
    if count == 0 {
        return 0.0;
    }
    values[..count].sort_unstable();
    let rank = (count * percentile).div_ceil(100).saturating_sub(1);
    values[rank.min(count - 1)] as f64
}

fn summarize(
    samples: &[FrameSample; SAMPLE_CAPACITY],
    sample_count: usize,
    elapsed: Duration,
    presented_frames: u64,
    gpu_presented_frames: u64,
) -> DesktopPerformanceReport {
    let count = sample_count.min(SAMPLE_CAPACITY);
    if count == 0 {
        return DesktopPerformanceReport::default();
    }

    let total_ns = |select: fn(FrameSample) -> u64| {
        samples[..count]
            .iter()
            .copied()
            .map(select)
            .map(u128::from)
            .sum::<u128>() as f64
            / count as f64
    };
    let to_ms = |nanoseconds: f64| nanoseconds / NANOSECONDS_PER_MILLISECOND;
    let elapsed_seconds = elapsed.as_secs_f64().max(f64::EPSILON);

    DesktopPerformanceReport {
        presented_fps: presented_frames as f64 / elapsed_seconds,
        sample_count: count,
        frame_average_ms: to_ms(total_ns(|sample| sample.frame_ns)),
        frame_p50_ms: to_ms(percentile_ns(samples, count, |sample| sample.frame_ns, 50)),
        frame_p95_ms: to_ms(percentile_ns(samples, count, |sample| sample.frame_ns, 95)),
        emulation_p95_ms: to_ms(percentile_ns(
            samples,
            count,
            |sample| sample.emulation_ns,
            95,
        )),
        composition_p95_ms: to_ms(percentile_ns(
            samples,
            count,
            |sample| sample.composition_ns,
            95,
        )),
        audio_p95_ms: to_ms(percentile_ns(samples, count, |sample| sample.audio_ns, 95)),
        presentation_p95_ms: to_ms(percentile_ns(
            samples,
            count,
            |sample| sample.presentation_ns,
            95,
        )),
        presentation_interval_p50_ms: to_ms(percentile_nonzero_ns(
            samples,
            count,
            |sample| sample.presentation_interval_ns,
            50,
        )),
        presentation_interval_p95_ms: to_ms(percentile_nonzero_ns(
            samples,
            count,
            |sample| sample.presentation_interval_ns,
            95,
        )),
        gpu_presentation_percent: if presented_frames == 0 {
            0.0
        } else {
            gpu_presented_frames as f64 * 100.0 / presented_frames as f64
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(
        frame_ms: u64,
        emulation_ms: u64,
        composition_ms: u64,
        audio_ms: u64,
        presentation_ms: u64,
        interval_ms: u64,
    ) -> FrameSample {
        FrameSample {
            frame_ns: frame_ms * 1_000_000,
            emulation_ns: emulation_ms * 1_000_000,
            composition_ns: composition_ms * 1_000_000,
            audio_ns: audio_ms * 1_000_000,
            presentation_ns: presentation_ms * 1_000_000,
            presentation_interval_ns: interval_ms * 1_000_000,
        }
    }

    #[test]
    fn settings_file_requires_the_exact_enabled_value() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let settings_directory = std::env::temp_dir().join(format!(
            "aceris-desktop-performance-test-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&settings_directory).unwrap();

        assert!(!settings_file_enabled(&settings_directory));
        let path = settings_directory.join("desktop_performance_log_v1.txt");
        std::fs::write(&path, "true").unwrap();
        assert!(!settings_file_enabled(&settings_directory));
        std::fs::write(&path, "1\n").unwrap();
        assert!(settings_file_enabled(&settings_directory));
        std::fs::write(&path, "0").unwrap();
        assert!(!settings_file_enabled(&settings_directory));

        std::fs::remove_dir_all(settings_directory).unwrap();
    }

    #[test]
    fn report_separates_emulation_composition_audio_and_presentation() {
        let samples = [
            sample(4, 2, 1, 0, 1, 0),
            sample(5, 3, 1, 0, 1, 15),
            sample(6, 3, 2, 1, 2, 16),
            sample(7, 4, 2, 1, 2, 17),
            sample(8, 5, 3, 1, 3, 18),
        ];
        let mut storage = [FrameSample::default(); SAMPLE_CAPACITY];
        storage[..samples.len()].copy_from_slice(&samples);

        let report = summarize(
            &storage,
            samples.len(),
            Duration::from_secs(5),
            5,
            3,
        );

        assert_eq!(report.presented_fps, 1.0);
        assert_eq!(report.frame_average_ms, 6.0);
        assert_eq!(report.frame_p50_ms, 6.0);
        assert_eq!(report.frame_p95_ms, 8.0);
        assert_eq!(report.emulation_p95_ms, 5.0);
        assert_eq!(report.composition_p95_ms, 3.0);
        assert_eq!(report.audio_p95_ms, 1.0);
        assert_eq!(report.presentation_p95_ms, 3.0);
        assert_eq!(report.presentation_interval_p50_ms, 16.0);
        assert_eq!(report.presentation_interval_p95_ms, 18.0);
        assert_eq!(report.gpu_presentation_percent, 60.0);
    }

    #[test]
    fn monitor_resets_metrics_when_the_desktop_mode_changes() {
        let mut monitor = DesktopPerformanceMonitor::new();
        monitor.kind = Some(DesktopFrameKind::Game);
        monitor.sample_count = 5;
        monitor.presented_frames = 5;
        monitor.gpu_presented_frames = 5;

        monitor.record(
            DesktopFrameKind::Library,
            Duration::from_millis(5),
            Duration::ZERO,
            Duration::from_millis(2),
            Duration::ZERO,
            Duration::from_millis(1),
            false,
        );
        assert_eq!(monitor.kind, Some(DesktopFrameKind::Library));
        assert_eq!(monitor.sample_count, 1);
        assert_eq!(monitor.presented_frames, 1);
        assert_eq!(monitor.gpu_presented_frames, 0);
    }

    #[test]
    fn monitor_emits_and_resets_each_five_second_window() {
        let mut monitor = DesktopPerformanceMonitor::new();
        monitor.interval_started = Instant::now() - REPORT_INTERVAL;

        monitor.record(
            DesktopFrameKind::Game,
            Duration::from_millis(12),
            Duration::from_millis(6),
            Duration::from_millis(3),
            Duration::from_millis(1),
            Duration::from_millis(2),
            true,
        );

        assert_eq!(monitor.kind, Some(DesktopFrameKind::Game));
        assert_eq!(monitor.sample_count, 0);
        assert_eq!(monitor.presented_frames, 0);
        assert_eq!(monitor.gpu_presented_frames, 0);
        assert_eq!(monitor.next_sample, 0);
    }

    #[test]
    fn reports_use_only_the_latest_fixed_capacity_samples() {
        let mut storage = [FrameSample::default(); SAMPLE_CAPACITY];
        storage.fill(sample(1, 1, 1, 1, 1, 16));
        storage[0] = sample(900, 900, 900, 900, 900, 900);

        let report = summarize(
            &storage,
            SAMPLE_CAPACITY,
            Duration::from_secs(5),
            SAMPLE_CAPACITY as u64,
            SAMPLE_CAPACITY as u64,
        );

        assert_eq!(report.frame_p95_ms, 1.0);
        assert_eq!(report.emulation_p95_ms, 1.0);
        assert_eq!(report.presentation_interval_p95_ms, 16.0);
        assert_eq!(report.gpu_presentation_percent, 100.0);
    }
}
