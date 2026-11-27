//! Audio output for the desktop frontend, without external crates.
//!
//! The core mixes at 32728 Hz (see `System::take_audio_samples`); this module
//! resamples to 48 kHz with linear interpolation and hands the samples to a
//! platform sink: ALSA through `dlopen` on Linux, `waveOut` on Windows, AudioQueue
//! on macOS. Sinks never block: when the device
//! queue is full the newest samples are dropped, and a missing device only
//! disables sound.
//!
//! Nothing here has been heard on real hardware by the authors: the Linux sink
//! was exercised only on a machine without a sound device and the Windows and macOS
//! sinks are verified by compilation in CI only.

use aceris_core::{System, dsp::Resampler};

#[cfg(target_os = "linux")]
mod alsa;
#[cfg(target_os = "macos")]
mod coreaudio;
#[cfg(all(aceris_typecheck_native, not(target_os = "macos")))]
#[allow(dead_code)]
mod coreaudio;
#[cfg(target_os = "windows")]
mod winmm;
// Type-checked on any host with `--cfg aceris_typecheck_native`; never linked there.
#[cfg(all(aceris_typecheck_native, not(target_os = "windows")))]
#[allow(dead_code)]
mod winmm;

/// Sample rate delivered to the platform sinks.
pub const OUTPUT_RATE: u32 = 48_000;

/// A platform audio device accepting interleaved stereo `i16` at [`OUTPUT_RATE`].
pub trait Sink {
    /// Queues samples without blocking; excess samples are dropped.
    fn write(&mut self, samples: &[i16]);

    /// Discards queued and partially assembled samples, preserving the device.
    fn flush(&mut self) -> Result<(), String>;
}

/// Feeds the core's mixed audio to the platform device.
pub struct AudioOutput {
    sink: Box<dyn Sink>,
    resampler: Resampler,
    mixed: Vec<i16>,
    resampled: Vec<i16>,
}

impl AudioOutput {
    /// Opens the platform sink. Returns `None` (after printing why) without a device.
    pub fn open() -> Option<Self> {
        match open_sink() {
            Ok(sink) => Some(Self {
                sink,
                resampler: Resampler::new(System::audio_sample_rate(), OUTPUT_RATE),
                mixed: Vec::new(),
                resampled: Vec::new(),
            }),
            Err(message) => {
                eprintln!("audio disabled: {message}");
                None
            }
        }
    }

    /// Discards any samples still held by the frontend and its host device.
    ///
    /// Recreating the resampler also drops its interpolation history so audio
    /// from before a pause cannot bleed into the first samples after resuming.
    pub fn flush(&mut self) -> Result<(), String> {
        self.mixed.clear();
        self.resampled.clear();
        self.resampler = Resampler::new(System::audio_sample_rate(), OUTPUT_RATE);
        self.sink.flush()
    }

    /// Moves everything the DSP mixed since the last call to the device.
    pub fn feed(&mut self, system: &mut System) {
        self.mixed.clear();
        system.take_audio_samples(&mut self.mixed);
        if self.mixed.is_empty() {
            return;
        }
        self.resampled.clear();
        self.resampler.process(&self.mixed, &mut self.resampled);
        self.sink.write(&self.resampled);
    }
}

#[cfg(target_os = "linux")]
fn open_sink() -> Result<Box<dyn Sink>, String> {
    Ok(Box::new(alsa::AlsaSink::open()?))
}

#[cfg(target_os = "windows")]
fn open_sink() -> Result<Box<dyn Sink>, String> {
    Ok(Box::new(winmm::WaveOutSink::open()?))
}

#[cfg(target_os = "macos")]
fn open_sink() -> Result<Box<dyn Sink>, String> {
    Ok(Box::new(coreaudio::AudioQueueSink::open()?))
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn open_sink() -> Result<Box<dyn Sink>, String> {
    Err("no audio backend for this platform yet".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    struct FlushRecordingSink(Arc<AtomicBool>);

    impl Sink for FlushRecordingSink {
        fn write(&mut self, _samples: &[i16]) {}

        fn flush(&mut self) -> Result<(), String> {
            self.0.store(true, Ordering::Release);
            Ok(())
        }
    }

    #[test]
    fn flush_clears_frontend_buffers_and_calls_the_host_sink() {
        let flushed = Arc::new(AtomicBool::new(false));
        let mut output = AudioOutput {
            sink: Box::new(FlushRecordingSink(flushed.clone())),
            resampler: Resampler::new(System::audio_sample_rate(), OUTPUT_RATE),
            mixed: vec![1, 2, 3, 4],
            resampled: vec![5, 6, 7, 8],
        };

        output.flush().expect("test sink flushes");
        assert!(flushed.load(Ordering::Acquire));
        assert!(output.mixed.is_empty());
        assert!(output.resampled.is_empty());
    }

    #[test]
    fn opening_the_device_never_panics_even_without_sound_hardware() {
        // CI machines usually have no sound card; the result may be None or Some.
        drop(AudioOutput::open());
    }
}
