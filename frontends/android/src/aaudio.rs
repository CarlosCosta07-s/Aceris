//! Android audio output through AAudio (`libaaudio.so`, opened with `dlopen`).
//!
//! STATUS: compiled by CI for `aarch64-linux-android` and type-checked on Linux,
//! never run on a device. `libaaudio.so` exists from Android 8 (API 26); on older
//! devices (minSdk is 24) `open` returns `None` and the game runs silent.
//! The stream is never paused while the app is in the background.

use aceris_core::System;
use aceris_core::dsp::Resampler;
use std::ffi::{CString, c_char, c_int, c_void};
use std::ptr;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

const RTLD_NOW: c_int = 2;
const AAUDIO_DIRECTION_OUTPUT: i32 = 0;
const AAUDIO_FORMAT_PCM_I16: i32 = 1;
const AAUDIO_PERFORMANCE_MODE_LOW_LATENCY: i32 = 12;
const AAUDIO_USAGE_GAME: i32 = 14;
/// Rate requested from the device; the core's 32728 Hz is resampled to it.
const OUTPUT_RATE: u32 = 48_000;

type CreateBuilderFn = unsafe extern "C" fn(*mut *mut c_void) -> i32;
type SetI32Fn = unsafe extern "C" fn(*mut c_void, i32);
type OpenStreamFn = unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> i32;
type BuilderDeleteFn = unsafe extern "C" fn(*mut c_void) -> i32;
type StreamFn = unsafe extern "C" fn(*mut c_void) -> i32;
type WriteFn = unsafe extern "C" fn(*mut c_void, *const c_void, i32, i64) -> i32;

/// Plays the DSP's mixed audio on the default output device.
pub struct AAudioOutput {
    library: *mut c_void,
    stream: *mut c_void,
    write: WriteFn,
    close: StreamFn,
    resampler: Resampler,
    resampled: Vec<i16>,
    muted: bool,
}

fn mute_samples(samples: &mut [i16], muted: bool) {
    if muted {
        samples.fill(0);
    }
}

fn load<F: Copy>(library: *mut c_void, name: &str) -> Option<F> {
    assert_eq!(size_of::<F>(), size_of::<*mut c_void>());
    let symbol = CString::new(name).ok()?;
    // SAFETY: `library` is a live dlopen handle and `symbol` is NUL-terminated.
    let address = unsafe { dlsym(library, symbol.as_ptr()) };
    if address.is_null() {
        return None;
    }
    // SAFETY: `F` is the matching `extern "C"` function-pointer type for the
    // AAudio prototype named by `name` and has the size of `address`.
    Some(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&address) })
}

impl AAudioOutput {
    /// Opens a 48 kHz stereo `i16` output stream, or `None` without AAudio.
    pub fn open() -> Option<Self> {
        let name = CString::new("libaaudio.so").ok()?;
        // SAFETY: plain dlopen of a system library by name.
        let library = unsafe { dlopen(name.as_ptr(), RTLD_NOW) };
        if library.is_null() {
            return None;
        }
        let output = Self::open_stream(library);
        if output.is_none() {
            // SAFETY: `library` came from dlopen above and nothing uses it now.
            unsafe { dlclose(library) };
        }
        output
    }

    fn open_stream(library: *mut c_void) -> Option<Self> {
        let create: CreateBuilderFn = load(library, "AAudio_createStreamBuilder")?;
        let set_direction: SetI32Fn = load(library, "AAudioStreamBuilder_setDirection")?;
        let set_rate: SetI32Fn = load(library, "AAudioStreamBuilder_setSampleRate")?;
        let set_channels: SetI32Fn = load(library, "AAudioStreamBuilder_setChannelCount")?;
        let set_format: SetI32Fn = load(library, "AAudioStreamBuilder_setFormat")?;
        let set_mode: SetI32Fn = load(library, "AAudioStreamBuilder_setPerformanceMode")?;
        // API 28+: identify Aceris output as game audio rather than media.
        // Older Android releases simply keep the system default.
        let set_usage: Option<SetI32Fn> = load(library, "AAudioStreamBuilder_setUsage");
        let open_stream: OpenStreamFn = load(library, "AAudioStreamBuilder_openStream")?;
        let delete_builder: BuilderDeleteFn = load(library, "AAudioStreamBuilder_delete")?;
        let start: StreamFn = load(library, "AAudioStream_requestStart")?;
        let write: WriteFn = load(library, "AAudioStream_write")?;
        let close: StreamFn = load(library, "AAudioStream_close")?;

        let mut builder: *mut c_void = ptr::null_mut();
        // SAFETY: `builder` is a valid out-pointer.
        if unsafe { create(&mut builder) } != 0 || builder.is_null() {
            return None;
        }
        let mut stream: *mut c_void = ptr::null_mut();
        // SAFETY: `builder` is the live builder created above; it is deleted exactly once.
        let status = unsafe {
            set_direction(builder, AAUDIO_DIRECTION_OUTPUT);
            set_rate(builder, OUTPUT_RATE as i32);
            set_channels(builder, 2);
            set_format(builder, AAUDIO_FORMAT_PCM_I16);
            set_mode(builder, AAUDIO_PERFORMANCE_MODE_LOW_LATENCY);
            if let Some(set_usage) = set_usage {
                set_usage(builder, AAUDIO_USAGE_GAME);
            }
            let status = open_stream(builder, &mut stream);
            delete_builder(builder);
            status
        };
        if status != 0 || stream.is_null() {
            return None;
        }
        // SAFETY: `stream` is the stream just opened.
        if unsafe { start(stream) } != 0 {
            // SAFETY: `stream` is open and unused afterwards.
            unsafe { close(stream) };
            return None;
        }
        Some(Self {
            library,
            stream,
            write,
            close,
            resampler: Resampler::new(System::audio_sample_rate(), OUTPUT_RATE),
            resampled: Vec::new(),
            muted: false,
        })
    }

    /// Mutes only the emulator's output while keeping the stream clock alive.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Queues interleaved stereo samples (core rate) without blocking.
    pub fn feed(&mut self, samples: &[i16]) {
        if samples.is_empty() {
            return;
        }
        self.resampled.clear();
        self.resampler.process(samples, &mut self.resampled);
        mute_samples(&mut self.resampled, self.muted);
        let frames = i32::try_from(self.resampled.len() / 2).unwrap_or(i32::MAX);
        // SAFETY: `resampled` holds `frames` interleaved stereo frames; a zero
        // timeout makes the write non-blocking and extra frames are dropped.
        unsafe {
            (self.write)(self.stream, self.resampled.as_ptr().cast(), frames, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mute_samples;

    #[test]
    fn muting_keeps_a_silent_buffer_with_the_same_length() {
        let mut samples = [1200, -800, 300, -100];
        let len = samples.len();
        mute_samples(&mut samples, true);
        assert_eq!(samples, [0; 4]);
        assert_eq!(samples.len(), len);

        samples.copy_from_slice(&[1200, -800, 300, -100]);
        mute_samples(&mut samples, false);
        assert_eq!(samples, [1200, -800, 300, -100]);
    }
}

impl Drop for AAudioOutput {
    fn drop(&mut self) {
        // SAFETY: both handles are live and not used after this point.
        unsafe {
            (self.close)(self.stream);
            dlclose(self.library);
        }
    }
}
