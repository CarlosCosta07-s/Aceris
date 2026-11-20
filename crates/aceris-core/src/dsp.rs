//! High-level emulation of the 3DS audio DSP (`dsp::DSP`).
//!
//! There is no DSP firmware: this module models the *shared-memory protocol*
//! libctru's `ndsp` uses. The guest sees a 0x80000-byte DSP RAM mapped at
//! `0x1FF00000` holding two banks ("regions") of a `SharedMemory` block. Each
//! DSP frame (160 stereo samples at about 32728 Hz) the DSP picks the bank with
//! the newer frame counter, applies the dirty configuration, mixes up to 24
//! voices (PCM8, PCM16, DSP-ADPCM mono) with linear interpolation, writes the
//! voice status and the final stereo mix back, and queues the samples for the
//! frontend.
//!
//! All layouts come from public documentation and open-source emulators
//! (Azahar's `shared_memory.h`, libctru's `ndsp`). NOTHING here is verified on
//! hardware or against retail games. Not modelled: biquad/simple filters,
//! polyphase interpolation, aux busses, reverb/delay effects, surround, the
//! compressor, stereo ADPCM and the capture path.

use std::collections::VecDeque;

use crate::gpu::GpuPhysicalMemory;
use crate::loader::ncsd::process::SharedMemoryBacking;

/// Size of the DSP RAM block.
pub const DSP_RAM_SIZE: usize = 0x80000;
/// Guest address where the kernel maps DSP RAM.
pub const DSP_RAM_GUEST_BASE: u32 = 0x1FF0_0000;
/// Stereo samples produced per DSP frame.
pub const SAMPLES_PER_FRAME: usize = 160;
/// Number of mixer voices.
pub const SOURCE_COUNT: usize = 24;
/// Nominal output rate in Hz.
pub const SAMPLE_RATE_HZ: f64 = 32_728.498_046_875;
/// Duration of one DSP frame in nanoseconds (160 / 32728.498 Hz).
pub const FRAME_PERIOD_NS: u64 = 4_888_706;
/// Pipe-interrupt type and pipe number libctru registers an event for.
pub const INTERRUPT_PIPE: u32 = 2;
/// Audio pipe index.
pub const AUDIO_PIPE: u32 = 2;

const REGION_OFFSETS: [usize; 2] = [0x50000, 0x70000];
const REGION_SIZE: usize = 0x8000;

const OFF_DSP_STATUS: usize = 0x0800;
const OFF_FINAL_SAMPLES: usize = 0x0A80;
const OFF_SOURCE_STATUSES: usize = 0x0D00;
const OFF_DSP_CONFIGURATION: usize = 0x2860;
const OFF_SOURCE_CONFIGURATIONS: usize = 0x3D24;
const OFF_ADPCM_COEFFICIENTS: usize = 0x4F24;
const OFF_FRAME_COUNTER: usize = 0x7FFE;

/// Region offsets reported through pipe 2, in libctru's `ndspVars` order.
const PIPE_STRUCT_OFFSETS: [usize; 15] = [
    OFF_FRAME_COUNTER,
    OFF_SOURCE_CONFIGURATIONS,
    OFF_SOURCE_STATUSES,
    OFF_ADPCM_COEFFICIENTS,
    OFF_DSP_CONFIGURATION,
    OFF_DSP_STATUS,
    OFF_FINAL_SAMPLES,
    0x2924, // intermediate_mix_samples
    0x0E20, // compressor
    0x0820, // dsp_debug
    0x5224,
    0x5424,
    0x55A4,
    0x58A4,
    0x58B8,
];

const SOURCE_CONFIG_SIZE: usize = 192;
const SOURCE_STATUS_SIZE: usize = 12;
const MAX_BUFFER_SAMPLES: u32 = 0x0010_0000;
const MAX_QUEUED_OUTPUT_FRAMES: usize = 16_384;

/// Converts a DSP word address to the guest address of the same byte.
pub const fn convert_dsp_address(dsp_address: u32) -> u32 {
    dsp_address
        .wrapping_mul(2)
        .wrapping_add(DSP_RAM_GUEST_BASE + 0x40000)
}

/// Encodes a value in the DSP's swapped-halves 32-bit format.
pub const fn encode_u32_dsp(value: u32) -> u32 {
    value.rotate_left(16)
}

/// Decodes the DSP's swapped-halves 32-bit format.
pub const fn decode_u32_dsp(raw: u32) -> u32 {
    raw.rotate_left(16)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Format {
    #[default]
    Pcm8,
    Pcm16,
    Adpcm,
}

#[derive(Clone, Debug)]
struct QueuedBuffer {
    physical_address: u32,
    length: u32,
    adpcm_yn: [i16; 2],
    adpcm_dirty: bool,
    looping: bool,
    id: u16,
    format: Format,
    stereo: bool,
}

#[derive(Clone, Debug)]
struct Playing {
    id: u16,
    frames: Vec<[i16; 2]>,
    looping: bool,
    position: f64,
}

#[derive(Clone)]
struct Source {
    enabled: bool,
    format: Format,
    stereo: bool,
    rate: f32,
    nearest: bool,
    gain: [f32; 2],
    coefficients: [i16; 16],
    queue: VecDeque<QueuedBuffer>,
    playing: Option<Playing>,
    adpcm_yn: [i16; 2],
    sync_count: u16,
    current_buffer_id: u16,
    last_buffer_id: u16,
    buffer_finished: bool,
}

impl Default for Source {
    fn default() -> Self {
        Self {
            enabled: false,
            format: Format::Pcm8,
            stereo: false,
            rate: 1.0,
            nearest: false,
            gain: [0.0; 2],
            coefficients: [0; 16],
            queue: VecDeque::new(),
            playing: None,
            adpcm_yn: [0; 2],
            sync_count: 0,
            current_buffer_id: 0,
            last_buffer_id: 0,
            buffer_finished: false,
        }
    }
}

/// HLE DSP: shared RAM, pipes, voices and the output sample queue.
#[derive(Debug)]
pub struct Dsp {
    ram: SharedMemoryBacking,
    pipe2: VecDeque<u8>,
    running: bool,
    sources: Vec<Source>,
    master_volume: f32,
    output: VecDeque<i16>,
    frame_accumulator_ns: u64,
    frames_mixed: u64,
}

impl std::fmt::Debug for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Source")
            .field("enabled", &self.enabled)
            .field("queued", &self.queue.len())
            .finish()
    }
}

impl Dsp {
    /// Allocates DSP RAM and resets all state.
    pub fn new() -> std::result::Result<Self, String> {
        Ok(Self {
            ram: SharedMemoryBacking::zeroed(DSP_RAM_SIZE)?,
            pipe2: VecDeque::new(),
            running: false,
            sources: vec![Source::default(); SOURCE_COUNT],
            master_volume: 1.0,
            output: VecDeque::new(),
            frame_accumulator_ns: 0,
            frames_mixed: 0,
        })
    }

    /// The DSP RAM backing (map it at [`DSP_RAM_GUEST_BASE`]).
    pub fn ram(&self) -> &SharedMemoryBacking {
        &self.ram
    }

    /// Whether the audio pipe has started the DSP (command 0 or 2, not yet 1 or 3).
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// Number of DSP frames mixed so far.
    pub fn frames_mixed(&self) -> u64 {
        self.frames_mixed
    }

    /// Handles a guest write to a DSP pipe. Returns true when the pipe
    /// interrupt should be signalled (the DSP produced a pipe-2 reply).
    pub fn write_pipe(&mut self, pipe: u32, data: &[u8]) -> bool {
        if pipe != AUDIO_PIPE || data.len() < 2 {
            return false;
        }
        match u16::from_le_bytes([data[0], data[1]]) {
            0 | 2 => {
                self.running = true;
                self.frame_accumulator_ns = 0;
                self.pipe2.clear();
                self.pipe2
                    .extend((PIPE_STRUCT_OFFSETS.len() as u16).to_le_bytes());
                for offset in PIPE_STRUCT_OFFSETS {
                    let address = 0x8000 + (offset as u16) / 2;
                    self.pipe2.extend(address.to_le_bytes());
                }
                true
            }
            1 | 3 => {
                self.running = false;
                true
            }
            _ => false,
        }
    }

    /// Bytes currently readable from a pipe.
    pub fn pipe_readable(&self, pipe: u32) -> usize {
        if pipe == AUDIO_PIPE {
            self.pipe2.len()
        } else {
            0
        }
    }

    /// Reads up to `max` bytes from a pipe.
    pub fn read_pipe(&mut self, pipe: u32, max: usize) -> Vec<u8> {
        if pipe != AUDIO_PIPE {
            return Vec::new();
        }
        let count = max.min(self.pipe2.len());
        self.pipe2.drain(..count).collect()
    }

    /// Advances virtual time and returns how many DSP frames are due.
    pub fn frames_due(&mut self, elapsed_ns: u64) -> u32 {
        if !self.running {
            return 0;
        }
        self.frame_accumulator_ns = self.frame_accumulator_ns.saturating_add(elapsed_ns);
        let frames = self.frame_accumulator_ns / FRAME_PERIOD_NS;
        self.frame_accumulator_ns %= FRAME_PERIOD_NS;
        u32::try_from(frames).unwrap_or(u32::MAX)
    }

    /// Moves queued interleaved stereo samples into `out`.
    pub fn take_samples(&mut self, out: &mut Vec<i16>) {
        out.extend(self.output.drain(..));
    }

    /// Number of queued interleaved samples.
    pub fn queued_samples(&self) -> usize {
        self.output.len()
    }

    fn read_region(&self, region: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; REGION_SIZE];
        let _ = self.ram.read_bytes(REGION_OFFSETS[region], &mut bytes);
        bytes
    }

    fn frame_counter(&self, region: usize) -> u16 {
        let mut bytes = [0u8; 2];
        let _ = self
            .ram
            .read_bytes(REGION_OFFSETS[region] + OFF_FRAME_COUNTER, &mut bytes);
        u16::from_le_bytes(bytes)
    }

    /// Picks the bank with the newer frame counter (handling 16-bit wrap).
    fn current_region(&self) -> usize {
        let (c0, c1) = (self.frame_counter(0), self.frame_counter(1));
        if c0 == 0xFFFF && c1 != 0xFFFE {
            return 0;
        }
        if c1 == 0xFFFF && c0 != 0xFFFE {
            return 1;
        }
        usize::from(c1 > c0)
    }

    /// Mixes one DSP frame from the current bank. Returns the bank index used.
    pub fn mix_frame(&mut self, physical: &GpuPhysicalMemory) -> usize {
        let region = self.current_region();
        let base = REGION_OFFSETS[region];
        let mut bank = self.read_region(region);

        // Global configuration.
        let config = OFF_DSP_CONFIGURATION;
        let dirty = read_u32(&bank, config);
        if dirty & (1 << 16) != 0 {
            self.master_volume = read_f32(&bank, config + 4);
        }
        write_u32(&mut bank, config, 0);

        for index in 0..SOURCE_COUNT {
            let offset = OFF_SOURCE_CONFIGURATIONS + index * SOURCE_CONFIG_SIZE;
            self.apply_source_config(index, &mut bank, offset);
        }

        // Mix.
        let mut accumulator = [0f32; SAMPLES_PER_FRAME * 2];
        for source in &mut self.sources {
            mix_source(source, physical, &mut accumulator);
        }
        let mut samples = [0i16; SAMPLES_PER_FRAME * 2];
        for (out, value) in samples.iter_mut().zip(accumulator) {
            *out = (value * self.master_volume).clamp(-32768.0, 32767.0) as i16;
        }

        // Publish results into the same bank.
        for (i, sample) in samples.iter().enumerate() {
            bank[OFF_FINAL_SAMPLES + i * 2..OFF_FINAL_SAMPLES + i * 2 + 2]
                .copy_from_slice(&sample.to_le_bytes());
        }
        for (index, source) in self.sources.iter_mut().enumerate() {
            let status = OFF_SOURCE_STATUSES + index * SOURCE_STATUS_SIZE;
            let position = source.playing.as_ref().map_or(0, |p| p.position as u32);
            bank[status] = u8::from(source.enabled);
            bank[status + 1] = u8::from(source.buffer_finished);
            bank[status + 2..status + 4].copy_from_slice(&source.sync_count.to_le_bytes());
            write_u32(&mut bank, status + 4, encode_u32_dsp(position));
            bank[status + 8..status + 10].copy_from_slice(&source.current_buffer_id.to_le_bytes());
            bank[status + 10..status + 12].copy_from_slice(&source.last_buffer_id.to_le_bytes());
            source.buffer_finished = false;
        }
        // Dropped frames stay zero.
        bank[OFF_DSP_STATUS + 2..OFF_DSP_STATUS + 4].copy_from_slice(&0u16.to_le_bytes());

        let _ = self.ram.write_bytes(base, &bank);

        for pair in samples {
            if self.output.len() >= MAX_QUEUED_OUTPUT_FRAMES * 2 {
                self.output.pop_front();
            }
            self.output.push_back(pair);
        }
        self.frames_mixed += 1;
        region
    }

    fn apply_source_config(&mut self, index: usize, bank: &mut [u8], offset: usize) {
        let dirty = read_u32(bank, offset);
        let source = &mut self.sources[index];
        if dirty == 0 {
            return;
        }
        let bit = |n: u32| dirty & (1 << n) != 0;
        let flags1 = read_u16(bank, offset + 0xB4);

        if bit(29) {
            *source = Source::default();
        } else if bit(4) {
            source.queue.clear();
            source.playing = None;
        }
        if bit(0) || bit(30) {
            source.format = match (flags1 >> 2) & 3 {
                1 => Format::Pcm16,
                2 => Format::Adpcm,
                _ => Format::Pcm8,
            };
        }
        if bit(1) || bit(30) {
            match flags1 & 3 {
                1 => source.stereo = false,
                2 => source.stereo = true,
                _ => {}
            }
        }
        if bit(2) {
            let start = OFF_ADPCM_COEFFICIENTS + index * 32;
            for (i, coefficient) in source.coefficients.iter_mut().enumerate() {
                *coefficient = read_u16(bank, start + i * 2) as i16;
            }
        }
        if bit(17) {
            source.nearest = bank[offset + 0x38] == 2;
        }
        if bit(18) {
            let rate = read_f32(bank, offset + 0x34);
            source.rate = if rate.is_finite() && rate > 0.0 {
                rate.min(8.0)
            } else {
                1.0
            };
        }
        if bit(25) {
            source.gain = [read_f32(bank, offset + 0x04), read_f32(bank, offset + 0x08)];
        }
        if bit(28) {
            source.sync_count = read_u16(bank, offset + 0xA2);
        }
        if bit(19) {
            let queue_dirty = read_u16(bank, offset + 0x4A);
            for slot in 0..4 {
                if queue_dirty & (1 << slot) == 0 {
                    continue;
                }
                let buffer = offset + 0x4C + slot * 20;
                let flags = bank[buffer + 0xE];
                let looping = bank[buffer + 0xF] != 0;
                let queued = QueuedBuffer {
                    physical_address: decode_u32_dsp(read_u32(bank, buffer)),
                    length: decode_u32_dsp(read_u32(bank, buffer + 4)),
                    adpcm_yn: [
                        read_u16(bank, buffer + 0xA) as i16,
                        read_u16(bank, buffer + 0xC) as i16,
                    ],
                    adpcm_dirty: flags != 0,
                    looping,
                    id: read_u16(bank, buffer + 0x10),
                    format: source.format,
                    stereo: source.stereo,
                };
                source.queue.push_back(queued);
            }
            write_u16(bank, offset + 0x4A, 0);
        }
        if bit(30) {
            let flags2 = read_u16(bank, offset + 0xBC);
            source.queue.push_back(QueuedBuffer {
                physical_address: decode_u32_dsp(read_u32(bank, offset + 0xAC)),
                length: decode_u32_dsp(read_u32(bank, offset + 0xB0)),
                adpcm_yn: [
                    read_u16(bank, offset + 0xB8) as i16,
                    read_u16(bank, offset + 0xBA) as i16,
                ],
                adpcm_dirty: flags2 & 1 != 0,
                looping: flags2 & 2 != 0,
                id: read_u16(bank, offset + 0xBE),
                format: source.format,
                stereo: source.stereo,
            });
        }
        if bit(21) {
            let position = decode_u32_dsp(read_u32(bank, offset + 0xA4));
            if let Some(playing) = source.playing.as_mut() {
                playing.position = f64::from(position);
            }
        }
        if bit(16) {
            source.enabled = bank[offset + 0xA0] != 0;
        }
        write_u32(bank, offset, 0);
    }
}

fn mix_source(source: &mut Source, physical: &GpuPhysicalMemory, out: &mut [f32]) {
    if !source.enabled {
        return;
    }
    let step = f64::from(source.rate);
    for sample in 0..SAMPLES_PER_FRAME {
        if source.playing.is_none() && !start_next_buffer(source, physical) {
            // Out of buffers: the voice stops, like the real DSP's status shows.
            source.enabled = false;
            return;
        }
        let Some(playing) = source.playing.as_mut() else {
            return;
        };
        let length = playing.frames.len();
        let index = (playing.position as usize).min(length - 1);
        let fraction = (playing.position - index as f64) as f32;
        let current = playing.frames[index];
        let value = if source.nearest || index + 1 >= length {
            [f32::from(current[0]), f32::from(current[1])]
        } else {
            let next = playing.frames[index + 1];
            [
                f32::from(current[0]) + (f32::from(next[0]) - f32::from(current[0])) * fraction,
                f32::from(current[1]) + (f32::from(next[1]) - f32::from(current[1])) * fraction,
            ]
        };
        out[sample * 2] += value[0] * source.gain[0];
        out[sample * 2 + 1] += value[1] * source.gain[1];

        playing.position += step;
        if playing.position >= length as f64 {
            if playing.looping {
                playing.position -= length as f64;
                if playing.position >= length as f64 {
                    playing.position = 0.0;
                }
            } else {
                let id = playing.id;
                source.last_buffer_id = id;
                source.buffer_finished = true;
                source.playing = None;
                if source.queue.is_empty() {
                    // Nothing left to play: the voice stops in this same frame.
                    source.enabled = false;
                    return;
                }
            }
        }
    }
}

/// Moves the next queued buffer into playback. Returns false when none is left.
fn start_next_buffer(source: &mut Source, physical: &GpuPhysicalMemory) -> bool {
    while let Some(buffer) = source.queue.pop_front() {
        if buffer.adpcm_dirty {
            source.adpcm_yn = buffer.adpcm_yn;
        }
        let Some(frames) = decode_buffer(source, &buffer, physical) else {
            continue;
        };
        if frames.is_empty() {
            continue;
        }
        source.current_buffer_id = buffer.id;
        source.playing = Some(Playing {
            id: buffer.id,
            frames,
            looping: buffer.looping,
            position: 0.0,
        });
        return true;
    }
    false
}

fn decode_buffer(
    source: &mut Source,
    buffer: &QueuedBuffer,
    physical: &GpuPhysicalMemory,
) -> Option<Vec<[i16; 2]>> {
    let length = buffer.length.min(MAX_BUFFER_SAMPLES) as usize;
    if length == 0 {
        return None;
    }
    let channels = if buffer.stereo { 2 } else { 1 };
    let byte_len = match buffer.format {
        Format::Pcm8 => length * channels,
        Format::Pcm16 => length * 2 * channels,
        Format::Adpcm => length.div_ceil(14) * 8,
    };
    let mut bytes = vec![0u8; byte_len];
    physical.read(buffer.physical_address, &mut bytes).ok()?;
    let mut frames = Vec::with_capacity(length);
    match buffer.format {
        Format::Pcm8 => {
            for i in 0..length {
                let left = i16::from(bytes[i * channels] as i8) << 8;
                let right = if channels == 2 {
                    i16::from(bytes[i * 2 + 1] as i8) << 8
                } else {
                    left
                };
                frames.push([left, right]);
            }
        }
        Format::Pcm16 => {
            for i in 0..length {
                let at = i * 2 * channels;
                let left = i16::from_le_bytes([bytes[at], bytes[at + 1]]);
                let right = if channels == 2 {
                    i16::from_le_bytes([bytes[at + 2], bytes[at + 3]])
                } else {
                    left
                };
                frames.push([left, right]);
            }
        }
        Format::Adpcm => {
            let [mut y1, mut y2] = [i32::from(source.adpcm_yn[0]), i32::from(source.adpcm_yn[1])];
            'frames: for frame in bytes.chunks_exact(8) {
                let scale = u32::from(frame[0] & 0xF);
                let predictor = usize::from((frame[0] >> 4) & 7);
                let c0 = i32::from(source.coefficients[predictor * 2]);
                let c1 = i32::from(source.coefficients[predictor * 2 + 1]);
                for byte in &frame[1..] {
                    for nibble in [byte >> 4, byte & 0xF] {
                        if frames.len() >= length {
                            break 'frames;
                        }
                        let signed = i32::from(((nibble << 4) as i8) >> 4);
                        let value = (((signed << scale) << 11) + 0x400 + c0 * y1 + c1 * y2) >> 11;
                        let value = value.clamp(-32768, 32767);
                        y2 = y1;
                        y1 = value;
                        frames.push([value as i16, value as i16]);
                    }
                }
            }
            source.adpcm_yn = [y1 as i16, y2 as i16];
        }
    }
    Some(frames)
}

/// Linear-interpolation resampler for interleaved stereo audio.
pub struct Resampler {
    step: f64,
    position: f64,
    pending: Vec<[i16; 2]>,
}

impl Resampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Self {
        Self {
            step: f64::from(input_rate) / f64::from(output_rate),
            position: 0.0,
            pending: Vec::new(),
        }
    }

    /// Resamples `input` (interleaved stereo) and appends the result to `output`.
    pub fn process(&mut self, input: &[i16], output: &mut Vec<i16>) {
        self.pending
            .extend(input.chunks_exact(2).map(|pair| [pair[0], pair[1]]));
        while self.position + 1.0 < self.pending.len() as f64 {
            let index = self.position as usize;
            let fraction = (self.position - index as f64) as f32;
            let (a, b) = (self.pending[index], self.pending[index + 1]);
            for channel in 0..2 {
                let value = f32::from(a[channel])
                    + (f32::from(b[channel]) - f32::from(a[channel])) * fraction;
                output.push(value as i16);
            }
            self.position += self.step;
        }
        let consumed = (self.position as usize).min(self.pending.len());
        self.pending.drain(..consumed);
        self.position -= consumed as f64;
    }
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("4 bytes"))
}

fn read_f32(bytes: &[u8], offset: usize) -> f32 {
    f32::from_bits(read_u32(bytes, offset))
}

fn write_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn write_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGION_BASE: [usize; 2] = REGION_OFFSETS;

    fn put(dsp: &Dsp, region: usize, offset: usize, bytes: &[u8]) {
        assert!(dsp.ram.write_bytes(REGION_BASE[region] + offset, bytes));
    }

    fn put_u32(dsp: &Dsp, region: usize, offset: usize, value: u32) {
        put(dsp, region, offset, &value.to_le_bytes());
    }

    fn put_u16(dsp: &Dsp, region: usize, offset: usize, value: u16) {
        put(dsp, region, offset, &value.to_le_bytes());
    }

    fn put_f32(dsp: &Dsp, region: usize, offset: usize, value: f32) {
        put(dsp, region, offset, &value.to_bits().to_le_bytes());
    }

    fn get(dsp: &Dsp, region: usize, offset: usize, len: usize) -> Vec<u8> {
        let mut out = vec![0; len];
        assert!(dsp.ram.read_bytes(REGION_BASE[region] + offset, &mut out));
        out
    }

    fn final_samples(dsp: &Dsp, region: usize) -> Vec<i16> {
        get(dsp, region, OFF_FINAL_SAMPLES, 640)
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect()
    }

    fn physical_with(bytes: &[u8]) -> (GpuPhysicalMemory, u32) {
        let mut memory = GpuPhysicalMemory::new();
        let address = memory.allocate_linear_fcram(0x1000).expect("fcram");
        memory.write(address, bytes).expect("write");
        (memory, address)
    }

    /// Configures voice `index` in `region` to play one embedded PCM16 mono buffer.
    fn start_pcm16_voice(
        dsp: &Dsp,
        region: usize,
        index: usize,
        address: u32,
        length: u32,
        gain: f32,
    ) {
        let base = OFF_SOURCE_CONFIGURATIONS + index * SOURCE_CONFIG_SIZE;
        let dirty: u32 = (1 << 0) | (1 << 1) | (1 << 16) | (1 << 25) | (1 << 30);
        put_u32(dsp, region, base, dirty);
        put_f32(dsp, region, base + 0x04, gain);
        put_f32(dsp, region, base + 0x08, gain);
        put(dsp, region, base + 0xA0, &[1]);
        put_u32(dsp, region, base + 0xAC, encode_u32_dsp(address));
        put_u32(dsp, region, base + 0xB0, encode_u32_dsp(length));
        // mono (1) | PCM16 (1 << 2)
        put_u16(dsp, region, base + 0xB4, 1 | (1 << 2));
        put_u16(dsp, region, base + 0xBE, 7);
    }

    #[test]
    fn u32_dsp_swaps_halves_and_round_trips() {
        assert_eq!(encode_u32_dsp(0x1234_5678), 0x5678_1234);
        assert_eq!(decode_u32_dsp(encode_u32_dsp(0xDEAD_BEEF)), 0xDEAD_BEEF);
    }

    #[test]
    fn dsp_addresses_convert_to_both_banks() {
        // Frame counter of bank 0 and bank 1 (bit 16 selects the second bank).
        assert_eq!(convert_dsp_address(0x8000 + 0x7FFE / 2), 0x1FF5_7FFE);
        assert_eq!(
            convert_dsp_address((0x8000 + 0x7FFE / 2) | 0x10000),
            0x1FF7_7FFE
        );
    }

    #[test]
    fn pipe2_start_replies_with_fifteen_struct_addresses() {
        let mut dsp = Dsp::new().unwrap();
        assert!(!dsp.is_running());
        assert!(dsp.write_pipe(AUDIO_PIPE, &[0, 0, 0, 0]));
        assert!(dsp.is_running());
        assert_eq!(dsp.pipe_readable(AUDIO_PIPE), 2 + 15 * 2);
        let count = dsp.read_pipe(AUDIO_PIPE, 2);
        assert_eq!(u16::from_le_bytes([count[0], count[1]]), 15);
        let addresses = dsp.read_pipe(AUDIO_PIPE, 30);
        let first = u16::from_le_bytes([addresses[0], addresses[1]]);
        assert_eq!(first, 0x8000 + 0x7FFE / 2);
        assert_eq!(dsp.pipe_readable(AUDIO_PIPE), 0);
        assert!(dsp.write_pipe(AUDIO_PIPE, &[1, 0, 0, 0]));
        assert!(!dsp.is_running());
    }

    #[test]
    fn other_pipes_stay_empty_and_ignore_writes() {
        let mut dsp = Dsp::new().unwrap();
        assert!(!dsp.write_pipe(0, &[0, 0, 0, 0]));
        assert!(!dsp.is_running());
        assert_eq!(dsp.pipe_readable(0), 0);
        assert!(dsp.read_pipe(3, 8).is_empty());
    }

    #[test]
    fn frames_due_follow_virtual_time_only_while_running() {
        let mut dsp = Dsp::new().unwrap();
        assert_eq!(dsp.frames_due(100_000_000), 0);
        dsp.write_pipe(AUDIO_PIPE, &[0, 0]);
        // 16.67 ms video frame: three DSP frames, remainder carried.
        assert_eq!(dsp.frames_due(16_666_667), 3);
        let mut total = 3;
        for _ in 0..59 {
            total += dsp.frames_due(16_666_667);
        }
        // 60 video frames = 1 second = ~204.5 DSP frames.
        assert!((204..=205).contains(&total), "total {total}");
    }

    #[test]
    fn pcm16_voice_plays_scaled_samples_and_reports_status() {
        let mut dsp = Dsp::new().unwrap();
        let samples: Vec<u8> = (0..200i16).flat_map(|i| (i * 100).to_le_bytes()).collect();
        let (physical, address) = physical_with(&samples);
        put_u16(&dsp, 0, OFF_FRAME_COUNTER, 5);
        start_pcm16_voice(&dsp, 0, 3, address, 200, 0.5);
        let region = dsp.mix_frame(&physical);
        assert_eq!(region, 0);
        let mix = final_samples(&dsp, 0);
        assert_eq!(mix[0], 0);
        assert_eq!(mix[2], 50); // 100 * 0.5
        assert_eq!(mix[2 * 159], 7950); // 15900 * 0.5
        assert_eq!(mix[2 * 159 + 1], 7950);
        let status = OFF_SOURCE_STATUSES + 3 * SOURCE_STATUS_SIZE;
        let raw = get(&dsp, 0, status, 12);
        assert_eq!(raw[0], 1, "enabled");
        assert_eq!(decode_u32_dsp(read_u32(&raw, 4)), 160);
        assert_eq!(read_u16(&raw, 8), 7);
        // Dirty flags were consumed by the DSP.
        let cfg = OFF_SOURCE_CONFIGURATIONS + 3 * SOURCE_CONFIG_SIZE;
        assert_eq!(get(&dsp, 0, cfg, 4), vec![0; 4]);
        // Second frame consumes the last 40 samples and finishes the buffer.
        dsp.mix_frame(&physical);
        let mix = final_samples(&dsp, 0);
        assert_eq!(mix[2 * 39], 9950); // sample 199 * 100 * 0.5
        assert_eq!(mix[2 * 40], 0, "silence after the buffer ends");
        let raw = get(&dsp, 0, status, 12);
        assert_eq!(raw[0], 0, "voice disabled when queue empties");
        assert_eq!(raw[1], 1, "buffer finished flag");
        assert_eq!(read_u16(&raw, 10), 7, "last buffer id");
    }

    #[test]
    fn output_queue_receives_each_frame_and_is_bounded() {
        let mut dsp = Dsp::new().unwrap();
        let physical = GpuPhysicalMemory::new();
        dsp.mix_frame(&physical);
        assert_eq!(dsp.queued_samples(), 320);
        let mut out = Vec::new();
        dsp.take_samples(&mut out);
        assert_eq!(out.len(), 320);
        assert_eq!(dsp.queued_samples(), 0);
        for _ in 0..200 {
            dsp.mix_frame(&physical);
        }
        assert!(dsp.queued_samples() <= MAX_QUEUED_OUTPUT_FRAMES * 2);
    }

    #[test]
    fn newer_frame_counter_selects_the_bank_including_wraparound() {
        let dsp = Dsp::new().unwrap();
        put_u16(&dsp, 0, OFF_FRAME_COUNTER, 4);
        put_u16(&dsp, 1, OFF_FRAME_COUNTER, 5);
        assert_eq!(dsp.current_region(), 1);
        put_u16(&dsp, 0, OFF_FRAME_COUNTER, 6);
        assert_eq!(dsp.current_region(), 0);
        put_u16(&dsp, 0, OFF_FRAME_COUNTER, 0xFFFF);
        put_u16(&dsp, 1, OFF_FRAME_COUNTER, 2);
        assert_eq!(dsp.current_region(), 0, "0xFFFF precedes the wrapped 2");
    }

    #[test]
    fn pcm8_voice_scales_to_sixteen_bits_and_rate_two_skips_samples() {
        let mut dsp = Dsp::new().unwrap();
        let data: Vec<u8> = (0..400u16).map(|i| (i % 100) as u8).collect();
        let (physical, address) = physical_with(&data);
        put_u16(&dsp, 0, OFF_FRAME_COUNTER, 1);
        let base = OFF_SOURCE_CONFIGURATIONS;
        let dirty: u32 = (1 << 0) | (1 << 1) | (1 << 16) | (1 << 18) | (1 << 25) | (1 << 30);
        put_u32(&dsp, 0, base, dirty);
        put_f32(&dsp, 0, base + 0x04, 1.0);
        put_f32(&dsp, 0, base + 0x08, 1.0);
        put_f32(&dsp, 0, base + 0x34, 2.0);
        put(&dsp, 0, base + 0xA0, &[1]);
        put_u32(&dsp, 0, base + 0xAC, encode_u32_dsp(address));
        put_u32(&dsp, 0, base + 0xB0, encode_u32_dsp(400));
        put_u16(&dsp, 0, base + 0xB4, 1); // mono PCM8
        dsp.mix_frame(&physical);
        let mix = final_samples(&dsp, 0);
        assert_eq!(mix[2], 2 << 8, "second output is input sample 2");
        assert_eq!(mix[2 * 10], 20 << 8);
    }

    #[test]
    fn adpcm_voice_decodes_with_coefficients() {
        let mut dsp = Dsp::new().unwrap();
        // One ADPCM frame: predictor 0, scale 0, nibbles 1,2,3,... 14 samples.
        let mut frame = vec![0x00u8];
        for pair in 0..7u8 {
            frame.push(((pair * 2 + 1) << 4) | (pair * 2 + 2));
        }
        let (physical, address) = physical_with(&frame);
        put_u16(&dsp, 0, OFF_FRAME_COUNTER, 1);
        // Coefficients: c0 = 0, c1 = 0 (pure scaled nibbles).
        let base = OFF_SOURCE_CONFIGURATIONS;
        let dirty: u32 = (1 << 0) | (1 << 1) | (1 << 2) | (1 << 16) | (1 << 25) | (1 << 30);
        put_u32(&dsp, 0, base, dirty);
        put_f32(&dsp, 0, base + 0x04, 1.0);
        put_f32(&dsp, 0, base + 0x08, 1.0);
        put(&dsp, 0, base + 0xA0, &[1]);
        put_u32(&dsp, 0, base + 0xAC, encode_u32_dsp(address));
        put_u32(&dsp, 0, base + 0xB0, encode_u32_dsp(14));
        put_u16(&dsp, 0, base + 0xB4, 1 | (2 << 2)); // mono ADPCM
        dsp.mix_frame(&physical);
        let mix = final_samples(&dsp, 0);
        // value = (nibble << 11 + 0x400) >> 11 == sign-extended nibble.
        let signed = |n: usize| i16::from(((n as u8) << 4) as i8 >> 4);
        for n in 0..14usize {
            assert_eq!(mix[2 * n], signed(n + 1), "sample {n}");
        }
    }

    #[test]
    fn looping_buffer_wraps_instead_of_finishing() {
        let mut dsp = Dsp::new().unwrap();
        let samples: Vec<u8> = (0..50i16).flat_map(|i| (i + 1).to_le_bytes()).collect();
        let (physical, address) = physical_with(&samples);
        put_u16(&dsp, 0, OFF_FRAME_COUNTER, 1);
        start_pcm16_voice(&dsp, 0, 0, address, 50, 1.0);
        // Looping flag lives in flags2 bit 1 of the embedded buffer.
        put_u16(&dsp, 0, OFF_SOURCE_CONFIGURATIONS + 0xBC, 2);
        dsp.mix_frame(&physical);
        let mix = final_samples(&dsp, 0);
        assert_eq!(mix[2 * 50], 1, "restarts at the first sample");
        assert_eq!(mix[2 * 159], 10, "159 % 50 = 9 -> sample value 10");
        let raw = get(&dsp, 0, OFF_SOURCE_STATUSES, 12);
        assert_eq!(raw[0], 1, "still enabled");
    }

    #[test]
    fn master_volume_and_clipping_apply_to_the_final_mix() {
        let mut dsp = Dsp::new().unwrap();
        let samples: Vec<u8> = std::iter::repeat_n(30000i16, 160)
            .flat_map(i16::to_le_bytes)
            .collect();
        let (physical, address) = physical_with(&samples);
        put_u16(&dsp, 0, OFF_FRAME_COUNTER, 1);
        start_pcm16_voice(&dsp, 0, 0, address, 160, 1.0);
        start_pcm16_voice(&dsp, 0, 1, address, 160, 1.0);
        dsp.mix_frame(&physical);
        assert_eq!(final_samples(&dsp, 0)[0], 32767, "60000 clips");
        // Halve the master volume next frame.
        put_u16(&dsp, 0, OFF_FRAME_COUNTER, 2);
        put_u32(&dsp, 0, OFF_DSP_CONFIGURATION, 1 << 16);
        put_f32(&dsp, 0, OFF_DSP_CONFIGURATION + 4, 0.5);
        start_pcm16_voice(&dsp, 0, 0, address, 160, 1.0);
        dsp.mix_frame(&physical);
        assert_eq!(final_samples(&dsp, 0)[0], 15000);
    }

    #[test]
    fn reset_flag_clears_a_playing_voice() {
        let mut dsp = Dsp::new().unwrap();
        let samples: Vec<u8> = vec![0x10; 2000];
        let (physical, address) = physical_with(&samples);
        put_u16(&dsp, 0, OFF_FRAME_COUNTER, 1);
        start_pcm16_voice(&dsp, 0, 0, address, 1000, 1.0);
        dsp.mix_frame(&physical);
        assert_eq!(get(&dsp, 0, OFF_SOURCE_STATUSES, 1)[0], 1);
        put_u32(&dsp, 0, OFF_SOURCE_CONFIGURATIONS, 1 << 29);
        dsp.mix_frame(&physical);
        assert_eq!(get(&dsp, 0, OFF_SOURCE_STATUSES, 1)[0], 0);
        assert_eq!(final_samples(&dsp, 0)[0], 0);
    }

    #[test]
    fn resampler_produces_the_expected_output_count_across_chunks() {
        let mut resampler = Resampler::new(32_728, 48_000);
        let input: Vec<i16> = vec![100; 545 * 2];
        let mut output = Vec::new();
        for _ in 0..60 {
            resampler.process(&input, &mut output);
        }
        // One second of input becomes about one second of output.
        let frames = output.len() / 2;
        assert!((47_900..=48_100).contains(&frames), "frames {frames}");
        assert!(output.iter().all(|sample| *sample == 100));
    }

    #[test]
    fn resampler_interpolates_linearly() {
        let mut resampler = Resampler::new(1, 2); // upsample by two
        let mut output = Vec::new();
        resampler.process(&[0, 0, 100, 200, 200, 400], &mut output);
        assert_eq!(&output[..6], &[0, 0, 50, 100, 100, 200]);
    }
}
