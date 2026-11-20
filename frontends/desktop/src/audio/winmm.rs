//! Windows sink using the `waveOut` API from `winmm.dll`.
//!
//! Written without a Windows machine: CI verifies that it compiles, nothing
//! more. Buffers are a small ring of `WAVEHDR`s; samples are dropped when all
//! of them are still queued.

use super::{OUTPUT_RATE, Sink};
use std::ffi::c_void;
use std::ptr;

const WAVE_MAPPER: u32 = 0xFFFF_FFFF;
const WAVE_FORMAT_PCM: u16 = 1;
const WHDR_DONE: u32 = 0x0000_0001;
const MMSYSERR_NOERROR: u32 = 0;
const BUFFER_COUNT: usize = 8;
/// Stereo frames per queued buffer (about 21 ms at 48 kHz).
const BUFFER_FRAMES: usize = 1024;

#[repr(C)]
struct WaveFormatEx {
    format_tag: u16,
    channels: u16,
    samples_per_sec: u32,
    avg_bytes_per_sec: u32,
    block_align: u16,
    bits_per_sample: u16,
    size: u16,
}

#[repr(C)]
struct WaveHdr {
    data: *mut u8,
    buffer_length: u32,
    bytes_recorded: u32,
    user: usize,
    flags: u32,
    loops: u32,
    next: *mut WaveHdr,
    reserved: usize,
}

#[link(name = "winmm")]
unsafe extern "system" {
    fn waveOutOpen(
        device: *mut *mut c_void,
        device_id: u32,
        format: *const WaveFormatEx,
        callback: usize,
        instance: usize,
        flags: u32,
    ) -> u32;
    fn waveOutClose(device: *mut c_void) -> u32;
    fn waveOutReset(device: *mut c_void) -> u32;
    fn waveOutPrepareHeader(device: *mut c_void, header: *mut WaveHdr, size: u32) -> u32;
    fn waveOutUnprepareHeader(device: *mut c_void, header: *mut WaveHdr, size: u32) -> u32;
    fn waveOutWrite(device: *mut c_void, header: *mut WaveHdr, size: u32) -> u32;
}

struct Slot {
    header: Box<WaveHdr>,
    data: Box<[i16]>,
    prepared: bool,
}

pub struct WaveOutSink {
    device: *mut c_void,
    slots: Vec<Slot>,
    /// Samples not yet forming a whole buffer.
    carry: Vec<i16>,
}

impl WaveOutSink {
    pub fn open() -> Result<Self, String> {
        let format = WaveFormatEx {
            format_tag: WAVE_FORMAT_PCM,
            channels: 2,
            samples_per_sec: OUTPUT_RATE,
            avg_bytes_per_sec: OUTPUT_RATE * 4,
            block_align: 4,
            bits_per_sample: 16,
            size: 0,
        };
        let mut device: *mut c_void = ptr::null_mut();
        // SAFETY: `device` and `format` are valid for the call; no callback is used.
        let status = unsafe { waveOutOpen(&mut device, WAVE_MAPPER, &format, 0, 0, 0) };
        if status != MMSYSERR_NOERROR || device.is_null() {
            return Err(format!("waveOutOpen failed (error {status})"));
        }
        let slots = (0..BUFFER_COUNT)
            .map(|_| {
                let mut data = vec![0i16; BUFFER_FRAMES * 2].into_boxed_slice();
                let header = Box::new(WaveHdr {
                    data: data.as_mut_ptr().cast(),
                    buffer_length: (BUFFER_FRAMES * 4) as u32,
                    bytes_recorded: 0,
                    user: 0,
                    flags: 0,
                    loops: 0,
                    next: ptr::null_mut(),
                    reserved: 0,
                });
                Slot {
                    header,
                    data,
                    prepared: false,
                }
            })
            .collect();
        Ok(Self {
            device,
            slots,
            carry: Vec::new(),
        })
    }

    fn submit(&mut self, chunk: &[i16]) {
        let device = self.device;
        for slot in &mut self.slots {
            let idle = !slot.prepared || slot.header.flags & WHDR_DONE != 0;
            if !idle {
                continue;
            }
            if slot.prepared {
                // SAFETY: the header was prepared on this device and has finished playing.
                unsafe {
                    waveOutUnprepareHeader(device, &mut *slot.header, size_of::<WaveHdr>() as u32);
                }
                slot.prepared = false;
            }
            slot.data.copy_from_slice(chunk);
            slot.header.flags = 0;
            // SAFETY: `header` points into a live box whose `data` buffer outlives
            // playback (both are dropped only after waveOutReset in Drop).
            unsafe {
                if waveOutPrepareHeader(device, &mut *slot.header, size_of::<WaveHdr>() as u32)
                    != MMSYSERR_NOERROR
                {
                    return;
                }
                slot.prepared = true;
                waveOutWrite(device, &mut *slot.header, size_of::<WaveHdr>() as u32);
            }
            return;
        }
        // Every buffer is still queued: drop this chunk.
    }
}

impl Sink for WaveOutSink {
    fn write(&mut self, samples: &[i16]) {
        self.carry.extend_from_slice(samples);
        let chunk_len = BUFFER_FRAMES * 2;
        while self.carry.len() >= chunk_len {
            let chunk: Vec<i16> = self.carry.drain(..chunk_len).collect();
            self.submit(&chunk);
        }
    }
}

impl Drop for WaveOutSink {
    fn drop(&mut self) {
        // SAFETY: the device is open; reset returns every queued buffer before the
        // headers are unprepared and the device closed.
        unsafe {
            waveOutReset(self.device);
            for slot in &mut self.slots {
                if slot.prepared {
                    waveOutUnprepareHeader(
                        self.device,
                        &mut *slot.header,
                        size_of::<WaveHdr>() as u32,
                    );
                }
            }
            waveOutClose(self.device);
        }
    }
}
