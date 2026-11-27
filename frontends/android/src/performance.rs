//! Opt-in Android frame timing diagnostics.
//!
//! This module measures host-side execution and presentation separately. It
//! never changes the emulated clock, frame budget, or guest framebuffer.

use std::fmt;
use std::time::{Duration, Instant};

const SAMPLE_CAPACITY: usize = 300;
const REPORT_INTERVAL: Duration = Duration::from_secs(5);
const NANOSECONDS_PER_MILLISECOND: f64 = 1_000_000.0;

#[derive(Clone, Copy, Debug, Default)]
struct FrameSample {
    frame_ns: u64,
    emulation_ns: u64,
    presentation_ns: u64,
}

/// Rolling host-side performance window for Android's render thread.
pub(crate) struct FramePerformanceMonitor {
    samples: [FrameSample; SAMPLE_CAPACITY],
    sample_count: usize,
    next_sample: usize,
    interval_started: Instant,
    frame_attempts: u64,
    frames_presented: u64,
    frames_presented_by_gpu: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct FramePerformanceReport {
    presented_fps: f64,
    sample_count: usize,
    frame_average_ms: f64,
    frame_p50_ms: f64,
    frame_p95_ms: f64,
    emulation_p95_ms: f64,
    presentation_p95_ms: f64,
    gpu_presentation_percent: f64,
    presentation_failures: u64,
}

impl FramePerformanceMonitor {
    pub(crate) fn new() -> Self {
        Self {
            samples: [FrameSample::default(); SAMPLE_CAPACITY],
            sample_count: 0,
            next_sample: 0,
            interval_started: Instant::now(),
            frame_attempts: 0,
            frames_presented: 0,
            frames_presented_by_gpu: 0,
        }
    }

    /// Records one non-empty render attempt and reports every five seconds.
    /// Times are host durations; they do not imply guest FPS or guest time.
    pub(crate) fn record(
        &mut self,
        frame_time: Duration,
        emulation_time: Duration,
        presentation_time: Duration,
        presented: bool,
        gpu_presented: bool,
    ) -> Option<FramePerformanceReport> {
        self.samples[self.next_sample] = FrameSample {
            frame_ns: duration_ns(frame_time),
            emulation_ns: duration_ns(emulation_time),
            presentation_ns: duration_ns(presentation_time),
        };
        self.next_sample = (self.next_sample + 1) % SAMPLE_CAPACITY;
        self.sample_count = (self.sample_count + 1).min(SAMPLE_CAPACITY);

        self.frame_attempts += 1;
        if presented {
            self.frames_presented += 1;
            if gpu_presented {
                self.frames_presented_by_gpu += 1;
            }
        }

        let interval = self.interval_started.elapsed();
        if interval < REPORT_INTERVAL {
            return None;
        }

        let report = summarize(
            &self.samples,
            self.sample_count,
            interval,
            self.frame_attempts,
            self.frames_presented,
            self.frames_presented_by_gpu,
        );

        self.sample_count = 0;
        self.next_sample = 0;
        self.interval_started = Instant::now();
        self.frame_attempts = 0;
        self.frames_presented = 0;
        self.frames_presented_by_gpu = 0;
        Some(report)
    }
}

impl fmt::Display for FramePerformanceReport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "presented_fps={:.1}, frame_ms_avg/p50/p95={:.2}/{:.2}/{:.2}, emulation_ms_p95={:.2}, presentation_ms_p95={:.2}, gpu_present={:.0}%, samples={}, presentation_failures={}",
            self.presented_fps,
            self.frame_average_ms,
            self.frame_p50_ms,
            self.frame_p95_ms,
            self.emulation_p95_ms,
            self.presentation_p95_ms,
            self.gpu_presentation_percent,
            self.sample_count,
            self.presentation_failures,
        )
    }
}

fn duration_ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

fn summarize(
    samples: &[FrameSample; SAMPLE_CAPACITY],
    sample_count: usize,
    interval: Duration,
    frame_attempts: u64,
    frames_presented: u64,
    frames_presented_by_gpu: u64,
) -> FramePerformanceReport {
    let count = sample_count.min(SAMPLE_CAPACITY);
    if count == 0 {
        return FramePerformanceReport {
            presented_fps: frames_presented as f64 / interval.as_secs_f64().max(f64::EPSILON),
            presentation_failures: frame_attempts.saturating_sub(frames_presented),
            ..FramePerformanceReport::default()
        };
    }

    let average_ns = |select: fn(FrameSample) -> u64| {
        let total: u128 = samples[..count]
            .iter()
            .copied()
            .map(select)
            .map(u128::from)
            .sum();
        total as f64 / count as f64
    };
    let percentile_ns = |select: fn(FrameSample) -> u64, percentile: usize| {
        let mut values = [0u64; SAMPLE_CAPACITY];
        for (destination, sample) in values[..count].iter_mut().zip(samples[..count].iter()) {
            *destination = select(*sample);
        }
        values[..count].sort_unstable();
        let rank = (count * percentile).div_ceil(100).saturating_sub(1);
        values[rank.min(count - 1)] as f64
    };
    let to_ms = |nanoseconds: f64| nanoseconds / NANOSECONDS_PER_MILLISECOND;
    let elapsed_seconds = interval.as_secs_f64().max(f64::EPSILON);

    FramePerformanceReport {
        interval_seconds: interval.as_secs_f64(),
        presented_fps: frames_presented as f64 / elapsed_seconds,
        sample_count: count,
        frame_average_ms: to_ms(average_ns(|sample| sample.frame_ns)),
        frame_p50_ms: to_ms(percentile_ns(|sample| sample.frame_ns, 50)),
        frame_p95_ms: to_ms(percentile_ns(|sample| sample.frame_ns, 95)),
        emulation_p95_ms: to_ms(percentile_ns(|sample| sample.emulation_ns, 95)),
        presentation_p95_ms: to_ms(percentile_ns(|sample| sample.presentation_ns, 95)),
        gpu_presentation_percent: if frames_presented == 0 {
            0.0
        } else {
            frames_presented_by_gpu as f64 * 100.0 / frames_presented as f64
        },
        presentation_failures: frame_attempts.saturating_sub(frames_presented),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(frame_ms: u64, emulation_ms: u64, presentation_ms: u64) -> FrameSample {
        FrameSample {
            frame_ns: frame_ms * 1_000_000,
            emulation_ns: emulation_ms * 1_000_000,
            presentation_ns: presentation_ms * 1_000_000,
        }
    }

    #[test]
    fn report_separates_emulation_presentation_and_frame_percentiles() {
        let samples = [
            sample(1, 1, 0),
            sample(2, 1, 1),
            sample(3, 2, 1),
            sample(4, 2, 2),
            sample(5, 3, 2),
        ];
        let mut storage = [FrameSample::default(); SAMPLE_CAPACITY];
        storage[..samples.len()].copy_from_slice(&samples);

        let report = summarize(
            &storage,
            samples.len(),
            Duration::from_secs(5),
            5,
            4,
            3,
        );

        assert_eq!(report.presented_fps, 0.8);
        assert_eq!(report.frame_average_ms, 3.0);
        assert_eq!(report.frame_p50_ms, 3.0);
        assert_eq!(report.frame_p95_ms, 5.0);
        assert_eq!(report.emulation_p95_ms, 3.0);
        assert_eq!(report.presentation_p95_ms, 2.0);
        assert_eq!(report.gpu_presentation_percent, 75.0);
        assert_eq!(report.presentation_failures, 1);
    }

    #[test]
    fn report_uses_only_the_latest_fixed_capacity_samples() {
        let mut storage = [FrameSample::default(); SAMPLE_CAPACITY];
        storage.fill(sample(1, 1, 1));
        storage[0] = sample(900, 900, 900);

        let report = summarize(
            &storage,
            SAMPLE_CAPACITY,
            Duration::from_secs(5),
            SAMPLE_CAPACITY as u64,
            SAMPLE_CAPACITY as u64,
            SAMPLE_CAPACITY as u64,
        );

        assert_eq!(report.frame_p95_ms, 1.0);
        assert_eq!(report.emulation_p95_ms, 1.0);
        assert_eq!(report.presentation_p95_ms, 1.0);
    }

    #[test]
    fn zero_samples_do_not_create_nan_percentages() {
        let storage = [FrameSample::default(); SAMPLE_CAPACITY];
        let report = summarize(
            &storage,
            0,
            Duration::ZERO,
            3,
            0,
            0,
        );

        assert_eq!(report.presented_fps, 0.0);
        assert_eq!(report.gpu_presentation_percent, 0.0);
        assert_eq!(report.presentation_failures, 3);
    }
}
