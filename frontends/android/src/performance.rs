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
    render_ns: u64,
    app_step_ns: u64,
    presentation_ns: u64,
    presentation_interval_ns: u64,
}

/// Rolling host-side performance window for Android's render thread.
pub(crate) struct FramePerformanceMonitor {
    samples: [FrameSample; SAMPLE_CAPACITY],
    sample_count: usize,
    next_sample: usize,
    interval_started: Instant,
    last_presentation: Option<Instant>,
    frame_attempts: u64,
    frames_presented: u64,
    frames_presented_by_gpu: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct FramePerformanceReport {
    presented_fps: f64,
    sample_count: usize,
    render_average_ms: f64,
    render_p50_ms: f64,
    render_p95_ms: f64,
    app_step_p95_ms: f64,
    presentation_p95_ms: f64,
    presentation_interval_p50_ms: f64,
    presentation_interval_p95_ms: f64,
    gpu_presentation_percent: f64,
    frames_not_presented: u64,
}

impl FramePerformanceMonitor {
    pub(crate) fn new() -> Self {
        Self {
            samples: [FrameSample::default(); SAMPLE_CAPACITY],
            sample_count: 0,
            next_sample: 0,
            interval_started: Instant::now(),
            last_presentation: None,
            frame_attempts: 0,
            frames_presented: 0,
            frames_presented_by_gpu: 0,
        }
    }

    /// Records one non-empty render attempt and reports every five seconds.
    /// Times are host durations; they do not imply guest FPS or guest time.
    pub(crate) fn record(
        &mut self,
        render_time: Duration,
        app_step_time: Duration,
        presentation_time: Duration,
        presented: bool,
        gpu_presented: bool,
    ) -> Option<FramePerformanceReport> {
        let presentation_interval = if presented {
            let now = Instant::now();
            let interval = self
                .last_presentation
                .map(|last| now.duration_since(last))
                .filter(|interval| *interval <= Duration::from_secs(2))
                .unwrap_or_default();
            self.last_presentation = Some(now);
            interval
        } else {
            Duration::ZERO
        };
        self.samples[self.next_sample] = FrameSample {
            render_ns: duration_ns(render_time),
            app_step_ns: duration_ns(app_step_time),
            presentation_ns: duration_ns(presentation_time),
            presentation_interval_ns: duration_ns(presentation_interval),
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
            "presented_fps={:.1}, render_ms_avg/p50/p95={:.2}/{:.2}/{:.2}, app_step_ms_p95={:.2}, present_ms_p95={:.2}, present_interval_ms_p50/p95={:.2}/{:.2}, gpu_present={:.0}%, samples={}, frames_not_presented={}",
            self.presented_fps,
            self.render_average_ms,
            self.render_p50_ms,
            self.render_p95_ms,
            self.app_step_p95_ms,
            self.presentation_p95_ms,
            self.presentation_interval_p50_ms,
            self.presentation_interval_p95_ms,
            self.gpu_presentation_percent,
            self.sample_count,
            self.frames_not_presented,
        )
    }
}

fn duration_ns(duration: Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
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
    interval: Duration,
    frame_attempts: u64,
    frames_presented: u64,
    frames_presented_by_gpu: u64,
) -> FramePerformanceReport {
    let count = sample_count.min(SAMPLE_CAPACITY);
    if count == 0 {
        return FramePerformanceReport {
            presented_fps: frames_presented as f64 / interval.as_secs_f64().max(f64::EPSILON),
            frames_not_presented: frame_attempts.saturating_sub(frames_presented),
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
        presented_fps: frames_presented as f64 / elapsed_seconds,
        sample_count: count,
        render_average_ms: to_ms(average_ns(|sample| sample.render_ns)),
        render_p50_ms: to_ms(percentile_ns(|sample| sample.render_ns, 50)),
        render_p95_ms: to_ms(percentile_ns(|sample| sample.render_ns, 95)),
        app_step_p95_ms: to_ms(percentile_ns(|sample| sample.app_step_ns, 95)),
        presentation_p95_ms: to_ms(percentile_ns(|sample| sample.presentation_ns, 95)),
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
        gpu_presentation_percent: if frames_presented == 0 {
            0.0
        } else {
            frames_presented_by_gpu as f64 * 100.0 / frames_presented as f64
        },
        frames_not_presented: frame_attempts.saturating_sub(frames_presented),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(
        render_ms: u64,
        app_step_ms: u64,
        presentation_ms: u64,
        presentation_interval_ms: u64,
    ) -> FrameSample {
        FrameSample {
            render_ns: render_ms * 1_000_000,
            app_step_ns: app_step_ms * 1_000_000,
            presentation_ns: presentation_ms * 1_000_000,
            presentation_interval_ns: presentation_interval_ms * 1_000_000,
        }
    }

    #[test]
    fn report_separates_app_step_presentation_and_frame_percentiles() {
        let samples = [
            sample(1, 1, 0, 0),
            sample(2, 1, 1, 15),
            sample(3, 2, 1, 16),
            sample(4, 2, 2, 17),
            sample(5, 3, 2, 18),
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
        assert_eq!(report.render_average_ms, 3.0);
        assert_eq!(report.render_p50_ms, 3.0);
        assert_eq!(report.render_p95_ms, 5.0);
        assert_eq!(report.app_step_p95_ms, 3.0);
        assert_eq!(report.presentation_p95_ms, 2.0);
        assert_eq!(report.presentation_interval_p50_ms, 16.0);
        assert_eq!(report.presentation_interval_p95_ms, 18.0);
        assert_eq!(report.gpu_presentation_percent, 75.0);
        assert_eq!(report.frames_not_presented, 1);
    }

    #[test]
    fn report_uses_only_the_latest_fixed_capacity_samples() {
        let mut storage = [FrameSample::default(); SAMPLE_CAPACITY];
        storage.fill(sample(1, 1, 1, 16));
        storage[0] = sample(900, 900, 900, 900);

        let report = summarize(
            &storage,
            SAMPLE_CAPACITY,
            Duration::from_secs(5),
            SAMPLE_CAPACITY as u64,
            SAMPLE_CAPACITY as u64,
            SAMPLE_CAPACITY as u64,
        );

        assert_eq!(report.render_p95_ms, 1.0);
        assert_eq!(report.app_step_p95_ms, 1.0);
        assert_eq!(report.presentation_p95_ms, 1.0);
        assert_eq!(report.presentation_interval_p95_ms, 16.0);
    }

    #[test]
    fn monitor_reports_host_metrics_after_the_configured_window() {
        let mut monitor = FramePerformanceMonitor::new();
        assert!(
            monitor
                .record(
                    Duration::from_millis(10),
                    Duration::from_millis(5),
                    Duration::from_millis(2),
                    true,
                    true,
                )
                .is_none()
        );

        monitor.interval_started = Instant::now() - REPORT_INTERVAL;
        let report = monitor
            .record(
                Duration::from_millis(12),
                Duration::from_millis(6),
                Duration::from_millis(3),
                true,
                true,
            )
            .unwrap();

        assert_eq!(report.sample_count, 2);
        assert_eq!(report.render_p95_ms, 12.0);
        assert_eq!(report.app_step_p95_ms, 6.0);
        assert_eq!(report.presentation_p95_ms, 3.0);
        assert_eq!(report.gpu_presentation_percent, 100.0);
        assert_eq!(report.frames_not_presented, 0);
        assert_eq!(monitor.sample_count, 0);
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
        assert_eq!(report.frames_not_presented, 3);
    }
}
