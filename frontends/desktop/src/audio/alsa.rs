//! ALSA sink: `libasound.so.2` is opened with `dlopen`, so no development
//! package is needed and a machine without ALSA just has no sound.

use super::{OUTPUT_RATE, Sink};
use std::ffi::{CString, c_char, c_int, c_long, c_uint, c_ulong, c_void};
use std::ptr;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flags: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

const RTLD_NOW: c_int = 2;
const SND_PCM_STREAM_PLAYBACK: c_int = 0;
const SND_PCM_NONBLOCK: c_int = 1;
const SND_PCM_FORMAT_S16_LE: c_int = 2;
const SND_PCM_ACCESS_RW_INTERLEAVED: c_int = 3;
/// Device latency requested from ALSA (100 ms).
const LATENCY_US: c_uint = 100_000;

type OpenFn = unsafe extern "C" fn(*mut *mut c_void, *const c_char, c_int, c_int) -> c_int;
type SetParamsFn =
    unsafe extern "C" fn(*mut c_void, c_int, c_int, c_uint, c_uint, c_int, c_uint) -> c_int;
type WriteFn = unsafe extern "C" fn(*mut c_void, *const c_void, c_ulong) -> c_long;
type RecoverFn = unsafe extern "C" fn(*mut c_void, c_int, c_int) -> c_int;
type DropPcmFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type PrepareFn = unsafe extern "C" fn(*mut c_void) -> c_int;
type CloseFn = unsafe extern "C" fn(*mut c_void) -> c_int;

pub struct AlsaSink {
    library: *mut c_void,
    pcm: *mut c_void,
    write: WriteFn,
    recover: RecoverFn,
    drop_pcm: DropPcmFn,
    prepare: PrepareFn,
    close: CloseFn,
}

fn load<F: Copy>(library: *mut c_void, name: &str) -> Result<F, String> {
    assert_eq!(size_of::<F>(), size_of::<*mut c_void>());
    let symbol = CString::new(name).expect("symbol names contain no NUL");
    // SAFETY: `library` is a live dlopen handle and `symbol` is NUL-terminated.
    let address = unsafe { dlsym(library, symbol.as_ptr()) };
    if address.is_null() {
        return Err(format!("libasound lacks {name}"));
    }
    // SAFETY: `F` is the matching `extern "C"` function-pointer type for the
    // symbol's ALSA prototype and has the size of `address`.
    Ok(unsafe { std::mem::transmute_copy::<*mut c_void, F>(&address) })
}

impl AlsaSink {
    pub fn open() -> Result<Self, String> {
        let name = CString::new("libasound.so.2").expect("no NUL");
        // SAFETY: plain dlopen of a system library by name.
        let library = unsafe { dlopen(name.as_ptr(), RTLD_NOW) };
        if library.is_null() {
            return Err("libasound.so.2 is not available".to_string());
        }
        let result = Self::open_device(library);
        if result.is_err() {
            // SAFETY: `library` came from dlopen above and nothing uses it now.
            unsafe { dlclose(library) };
        }
        result
    }

    fn open_device(library: *mut c_void) -> Result<Self, String> {
        let open: OpenFn = load(library, "snd_pcm_open")?;
        let set_params: SetParamsFn = load(library, "snd_pcm_set_params")?;
        let write: WriteFn = load(library, "snd_pcm_writei")?;
        let recover: RecoverFn = load(library, "snd_pcm_recover")?;
        let drop_pcm: DropPcmFn = load(library, "snd_pcm_drop")?;
        let prepare: PrepareFn = load(library, "snd_pcm_prepare")?;
        let close: CloseFn = load(library, "snd_pcm_close")?;
        let device = CString::new("default").expect("no NUL");
        let mut pcm: *mut c_void = ptr::null_mut();
        // SAFETY: `pcm` is a valid out-pointer and `device` is NUL-terminated.
        let status = unsafe {
            open(
                &mut pcm,
                device.as_ptr(),
                SND_PCM_STREAM_PLAYBACK,
                SND_PCM_NONBLOCK,
            )
        };
        if status < 0 || pcm.is_null() {
            return Err(format!("no ALSA playback device (error {status})"));
        }
        // SAFETY: `pcm` is the handle just opened.
        let status = unsafe {
            set_params(
                pcm,
                SND_PCM_FORMAT_S16_LE,
                SND_PCM_ACCESS_RW_INTERLEAVED,
                2,
                OUTPUT_RATE,
                1,
                LATENCY_US,
            )
        };
        if status < 0 {
            // SAFETY: `pcm` is open and unused afterwards.
            unsafe { close(pcm) };
            return Err(format!("ALSA rejected 48 kHz stereo (error {status})"));
        }
        Ok(Self {
            library,
            pcm,
            write,
            recover,
            drop_pcm,
            prepare,
            close,
        })
    }
}

impl Sink for AlsaSink {
    fn flush(&mut self) -> Result<(), String> {
        // Dropping stops playback immediately and discards all queued frames.
        // Preparing returns the PCM handle to a state that accepts new writes.
        // SAFETY: `self.pcm` is the live nonblocking playback handle.
        let status = unsafe { (self.drop_pcm)(self.pcm) };
        if status < 0 {
            return Err(format!("ALSA snd_pcm_drop failed (error {status})"));
        }
        // SAFETY: a dropped PCM stream can be prepared for playback again.
        let status = unsafe { (self.prepare)(self.pcm) };
        if status < 0 {
            return Err(format!("ALSA snd_pcm_prepare failed (error {status})"));
        }
        Ok(())
    }

    fn write(&mut self, samples: &[i16]) {
        let mut frames = samples.len() / 2;
        let mut offset = 0;
        while frames > 0 {
            // SAFETY: `offset * 2 + frames * 2 <= samples.len()`, so the pointer
            // and frame count describe initialised, in-bounds interleaved data.
            let written = unsafe {
                (self.write)(
                    self.pcm,
                    samples[offset * 2..].as_ptr().cast(),
                    frames as c_ulong,
                )
            };
            if written < 0 {
                // Underruns and suspends are recoverable; a full queue (EAGAIN)
                // is not, and drops the rest of this chunk.
                // SAFETY: `self.pcm` is open.
                let recovered = unsafe { (self.recover)(self.pcm, written as c_int, 1) };
                if recovered < 0 {
                    return;
                }
                continue;
            }
            if written == 0 {
                return;
            }
            offset += written as usize;
            frames -= written as usize;
        }
    }
}

impl Drop for AlsaSink {
    fn drop(&mut self) {
        // SAFETY: both handles are live and not used after this point.
        unsafe {
            (self.close)(self.pcm);
            dlclose(self.library);
        }
    }
}
