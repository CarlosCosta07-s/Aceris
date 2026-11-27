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
type StreamStateFn = unsafe extern "C" fn(*mut c_void) -> i32;
type WaitForStateChangeFn =
    unsafe extern "C" fn(*mut c_void, i32, *mut i32, i64) -> i32;
type WriteFn = unsafe extern "C" fn(*mut c_void, *const c_void, i32, i64) -> i32;

const AAUDIO_OK: i32 = 0;
const STREAM_STATE_STARTING: i32 = 3;
const STREAM_STATE_STARTED: i32 = 4;
const STREAM_STATE_PAUSING: i32 = 5;
const STREAM_STATE_PAUSED: i32 = 6;
const STREAM_STATE_FLUSHING: i32 = 7;
const STREAM_STATE_FLUSHED: i32 = 8;
const STREAM_STATE_STOPPED: i32 = 10;
const STATE_CHANGE_TIMEOUT_NANOS: i64 = 100_000_000;

/// Plays the DSP's mixed audio on the default output device.
pub struct AAudioOutput {
    library: *mut c_void,
    stream: *mut c_void,
    write: WriteFn,
    start: StreamFn,
    pause: StreamFn,
    flush: StreamFn,
    get_state: StreamStateFn,
    wait_for_state_change: WaitForStateChangeFn,
    close: StreamFn,
    resampler: Resampler,
    resampled: Vec<i16>,
    muted: bool,
    paused: bool,
    game_usage: bool,
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
        let game_usage = set_usage.is_some();
        let open_stream: OpenStreamFn = load(library, "AAudioStreamBuilder_openStream")?;
        let delete_builder: BuilderDeleteFn = load(library, "AAudioStreamBuilder_delete")?;
        let start: StreamFn = load(library, "AAudioStream_requestStart")?;
        let pause: StreamFn = load(library, "AAudioStream_requestPause")?;
        let flush: StreamFn = load(library, "AAudioStream_requestFlush")?;
        let get_state: StreamStateFn = load(library, "AAudioStream_getState")?;
        let wait_for_state_change: WaitForStateChangeFn =
            load(library, "AAudioStream_waitForStateChange")?;
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
            start,
            pause,
            flush,
            get_state,
            wait_for_state_change,
            close,
            resampler: Resampler::new(System::audio_sample_rate(), OUTPUT_RATE),
            resampled: Vec::new(),
            muted: false,
            paused: false,
            game_usage,
        })
    }

    /// Waits for one asynchronous AAudio transition to leave its transient state.
    fn wait_for_transition(&self, transient: i32, operation: &str) -> Result<i32, String> {
        // SAFETY: the stream is open and owned by this output object.
        let mut state = unsafe { (self.get_state)(self.stream) };
        if state == transient {
            let mut next_state = state;
            // SAFETY: the stream is live; the output pointer is a valid local.
            let status = unsafe {
                (self.wait_for_state_change)(
                    self.stream,
                    transient,
                    &mut next_state,
                    STATE_CHANGE_TIMEOUT_NANOS,
                )
            };
            // Re-read the state even after timeout: the transition may have
            // completed at the boundary of the wait.
            // SAFETY: the stream remains open on this app thread.
            state = unsafe { (self.get_state)(self.stream) };
            if status != AAUDIO_OK && state == transient {
                return Err(format!(
                    "AAudio {operation} did not finish before the state-change timeout"
                ));
            }
        }
        Ok(state)
    }

    /// Pauses the output and discards queued samples; resumes from a clean queue.
    ///
    /// AAudio state changes are asynchronous, so each transition is observed
    /// before requesting the next one. This method is called only by the
    /// Android app thread, never by an audio callback.
    pub fn set_paused(&mut self, paused: bool) -> Result<(), String> {
        let mut state = {
            // SAFETY: the stream is open and owned by this output object.
            unsafe { (self.get_state)(self.stream) }
        };

        if paused {
            if state == STREAM_STATE_STARTING {
                state = self.wait_for_transition(STREAM_STATE_STARTING, "start")?;
            }
            if state == STREAM_STATE_STARTED {
                // SAFETY: requestPause is valid for this output stream.
                let status = unsafe { (self.pause)(self.stream) };
                if status != AAUDIO_OK {
                    return Err(format!("AAudio requestPause failed (error {status})"));
                }
                state = self.wait_for_transition(STREAM_STATE_PAUSING, "pause")?;
            } else if state == STREAM_STATE_PAUSING {
                state = self.wait_for_transition(STREAM_STATE_PAUSING, "pause")?;
            }

            if state == STREAM_STATE_PAUSED {
                // SAFETY: requestFlush is valid after the output has reached PAUSED.
                let status = unsafe { (self.flush)(self.stream) };
                if status != AAUDIO_OK {
                    return Err(format!("AAudio requestFlush failed (error {status})"));
                }
                state = self.wait_for_transition(STREAM_STATE_FLUSHING, "flush")?;
            }
            if state != STREAM_STATE_FLUSHED {
                return Err(format!("AAudio cannot pause from stream state {state}"));
            }
            self.paused = true;
        } else {
            if state == STREAM_STATE_STARTING {
                state = self.wait_for_transition(STREAM_STATE_STARTING, "start")?;
            }
            if state == STREAM_STATE_FLUSHED
                || state == STREAM_STATE_PAUSED
                || state == STREAM_STATE_STOPPED
            {
                // SAFETY: requestStart resumes a paused/flushed output stream.
                let status = unsafe { (self.start)(self.stream) };
                if status != AAUDIO_OK {
                    return Err(format!("AAudio requestStart failed (error {status})"));
                }
                state = self.wait_for_transition(STREAM_STATE_STARTING, "resume")?;
            }
            if state != STREAM_STATE_STARTED {
                return Err(format!("AAudio cannot resume from stream state {state}"));
            }
            self.paused = false;
        }
        Ok(())
    }

    /// Mutes only the emulator's output while keeping the stream clock alive.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// True when the platform accepted the dedicated game-audio usage.
    pub fn uses_game_usage(&self) -> bool {
        self.game_usage
    }

    /// Queues interleaved stereo samples (core rate) without blocking.
    pub fn feed(&mut self, samples: &[i16]) {
        if self.paused || samples.is_empty() {
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
    use super::{STREAM_STATE_FLUSHED, STREAM_STATE_PAUSED, STREAM_STATE_STARTED, mute_samples};

    fn is_silent_queue_state(state: i32) -> bool {
        state == STREAM_STATE_PAUSED || state == STREAM_STATE_FLUSHED
    }

    #[test]
    fn only_paused_or_flushed_streams_are_treated_as_silent_queue_states() {
        assert!(is_silent_queue_state(STREAM_STATE_PAUSED));
        assert!(is_silent_queue_state(STREAM_STATE_FLUSHED));
        assert!(!is_silent_queue_state(STREAM_STATE_STARTED));
    }

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
