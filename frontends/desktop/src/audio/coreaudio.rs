//! macOS sink using AudioToolbox's `AudioQueue` output API.
//!
//! Written without a Mac: CI verifies that it compiles and links, nothing more.
//! A ring of buffers is refilled by [`Sink::write`]; the queue's completion
//! callback only flips a per-buffer "free" flag. Samples are dropped when every
//! buffer is still queued.

use super::{OUTPUT_RATE, Sink};
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

const FORMAT_LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
const FLAG_SIGNED_INTEGER: u32 = 0x4;
const FLAG_PACKED: u32 = 0x8;
const BUFFER_COUNT: usize = 8;
/// Stereo frames per queued buffer (about 21 ms at 48 kHz).
const BUFFER_FRAMES: usize = 1024;

#[repr(C)]
struct StreamDescription {
    sample_rate: f64,
    format_id: u32,
    format_flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels_per_frame: u32,
    bits_per_channel: u32,
    reserved: u32,
}

#[repr(C)]
struct QueueBuffer {
    capacity: u32,
    data: *mut c_void,
    byte_size: u32,
    user_data: *mut c_void,
    packet_description_capacity: u32,
    packet_descriptions: *mut c_void,
    packet_description_count: u32,
}

type OutputCallback = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut QueueBuffer);

#[cfg_attr(target_os = "macos", link(name = "AudioToolbox", kind = "framework"))]
unsafe extern "C" {
    fn AudioQueueNewOutput(
        format: *const StreamDescription,
        callback: OutputCallback,
        user_data: *mut c_void,
        run_loop: *mut c_void,
        run_loop_mode: *const c_void,
        flags: u32,
        queue: *mut *mut c_void,
    ) -> i32;
    fn AudioQueueAllocateBuffer(
        queue: *mut c_void,
        bytes: u32,
        buffer: *mut *mut QueueBuffer,
    ) -> i32;
    fn AudioQueueEnqueueBuffer(
        queue: *mut c_void,
        buffer: *mut QueueBuffer,
        packet_descriptions: u32,
        descriptions: *const c_void,
    ) -> i32;
    fn AudioQueueStart(queue: *mut c_void, start_time: *const c_void) -> i32;
    fn AudioQueueStop(queue: *mut c_void, immediate: u8) -> i32;
    fn AudioQueueDispose(queue: *mut c_void, immediate: u8) -> i32;
}

/// Runs on an AudioQueue thread: marks the finished buffer as reusable.
unsafe extern "C" fn buffer_finished(_: *mut c_void, _: *mut c_void, buffer: *mut QueueBuffer) {
    // SAFETY: `user_data` was set to a live `AtomicBool` owned by the sink, which
    // disposes the queue (stopping callbacks) before freeing the flags.
    unsafe {
        let flag = (*buffer).user_data as *const AtomicBool;
        if !flag.is_null() {
            (*flag).store(true, Ordering::Release);
        }
    }
}

pub struct AudioQueueSink {
    queue: *mut c_void,
    buffers: Vec<*mut QueueBuffer>,
    free: Box<[AtomicBool]>,
    carry: Vec<i16>,
}

impl AudioQueueSink {
    pub fn open() -> Result<Self, String> {
        let format = StreamDescription {
            sample_rate: f64::from(OUTPUT_RATE),
            format_id: FORMAT_LINEAR_PCM,
            format_flags: FLAG_SIGNED_INTEGER | FLAG_PACKED,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels_per_frame: 2,
            bits_per_channel: 16,
            reserved: 0,
        };
        let mut queue: *mut c_void = ptr::null_mut();
        // SAFETY: valid format and out-pointer; a null run loop makes the queue
        // call back on its own thread.
        let status = unsafe {
            AudioQueueNewOutput(
                &format,
                buffer_finished,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null(),
                0,
                &mut queue,
            )
        };
        if status != 0 || queue.is_null() {
            return Err(format!("AudioQueueNewOutput failed (error {status})"));
        }
        let free: Box<[AtomicBool]> = (0..BUFFER_COUNT).map(|_| AtomicBool::new(true)).collect();
        let mut buffers = Vec::with_capacity(BUFFER_COUNT);
        for flag in free.iter() {
            let mut buffer: *mut QueueBuffer = ptr::null_mut();
            // SAFETY: `queue` is live; the buffer is owned by the queue.
            let status =
                unsafe { AudioQueueAllocateBuffer(queue, (BUFFER_FRAMES * 4) as u32, &mut buffer) };
            if status != 0 || buffer.is_null() {
                // SAFETY: nothing else uses the queue yet.
                unsafe { AudioQueueDispose(queue, 1) };
                return Err(format!("AudioQueueAllocateBuffer failed (error {status})"));
            }
            // SAFETY: `buffer` is valid; `flag` lives in the boxed slice, whose
            // heap address stays fixed when the box moves into the sink.
            unsafe { (*buffer).user_data = flag as *const AtomicBool as *mut c_void };
            buffers.push(buffer);
        }
        // SAFETY: `queue` is live.
        let status = unsafe { AudioQueueStart(queue, ptr::null()) };
        if status != 0 {
            // SAFETY: nothing else uses the queue.
            unsafe { AudioQueueDispose(queue, 1) };
            return Err(format!("AudioQueueStart failed (error {status})"));
        }
        Ok(Self {
            queue,
            buffers,
            free,
            carry: Vec::new(),
        })
    }

    fn submit(&mut self, chunk: &[i16]) {
        for (index, &buffer) in self.buffers.iter().enumerate() {
            if !self.free[index].swap(false, Ordering::AcqRel) {
                continue;
            }
            // SAFETY: the buffer holds BUFFER_FRAMES * 4 bytes and `chunk` is that
            // many bytes of i16 data; the queue is not reading it (flag was set).
            unsafe {
                ptr::copy_nonoverlapping(
                    chunk.as_ptr().cast::<u8>(),
                    (*buffer).data.cast::<u8>(),
                    BUFFER_FRAMES * 4,
                );
                (*buffer).byte_size = (BUFFER_FRAMES * 4) as u32;
                if AudioQueueEnqueueBuffer(self.queue, buffer, 0, ptr::null()) != 0 {
                    self.free[index].store(true, Ordering::Release);
                }
            }
            return;
        }
        // Every buffer is still queued: drop this chunk.
    }
}

impl Sink for AudioQueueSink {
    fn flush(&mut self) -> Result<(), String> {
        self.carry.clear();
        // Immediate stop is synchronous and resets the queue, discarding buffers
        // that have not reached the output device.
        // SAFETY: `self.queue` is the live output queue owned by this sink.
        let status = unsafe { AudioQueueStop(self.queue, 1) };
        if status != 0 {
            return Err(format!("AudioQueueStop failed (error {status})"));
        }
        // Stop has completed and returned queued buffers through their callbacks.
        for free in self.free.iter() {
            free.store(true, Ordering::Release);
        }
        // SAFETY: a stopped queue can be started again after the reset.
        let status = unsafe { AudioQueueStart(self.queue, ptr::null()) };
        if status != 0 {
            return Err(format!("AudioQueueStart after reset failed (error {status})"));
        }
        Ok(())
    }

    fn write(&mut self, samples: &[i16]) {
        self.carry.extend_from_slice(samples);
        let chunk_len = BUFFER_FRAMES * 2;
        while self.carry.len() >= chunk_len {
            let chunk: Vec<i16> = self.carry.drain(..chunk_len).collect();
            self.submit(&chunk);
        }
    }
}

impl Drop for AudioQueueSink {
    fn drop(&mut self) {
        // SAFETY: immediate disposal stops callbacks before `free` is dropped.
        unsafe { AudioQueueDispose(self.queue, 1) };
    }
}
