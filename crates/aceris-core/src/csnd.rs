//! CSND sound unit: 32 hardware channels that play PCM8, PCM16 or IMA-ADPCM
//! samples straight from physical memory, plus the command-block protocol the
//! `csnd:SND` service exposes to applications through shared memory.
//!
//! The model is written independently for Aceris. Protocol facts (command IDs,
//! parameter layout, shared-state layout, timer clock) were cross-checked
//! against public 3dbrew pages ("CSND Services", "CSND Registers"), libctru's
//! public `csnd.h`, and a read-only study of Azahar (GPLv3, revision
//! `6d6e846afa7b02f3dd85183a968b40cd1561f700`, `src/audio_core/csnd.cpp` and
//! `src/core/hle/service/csnd/csnd_snd.cpp`). No code was copied or ported.
//!
//! Known simplifications: PSG square/noise channels output silence, capture
//! units are bookkeeping only, and volume fades use an evenly spaced ramp
//! instead of the exact timer cadence of the system module.

/// Number of hardware channels.
pub const CHANNEL_COUNT: usize = 32;
/// Number of capture units.
pub const CAPTURE_UNIT_COUNT: usize = 2;
/// Channels handed to applications by AcquireSoundChannels (0–7 are reserved
/// for the system's direct-sound path).
pub const APPLICATION_CHANNEL_MASK: u32 = 0xFFFF_FF00;
/// Size in bytes of one command in the shared command block.
pub const COMMAND_SIZE: usize = 0x20;
/// `next_command_offset` value that ends a command list.
pub const COMMAND_LIST_END: u16 = 0xFFFF;
/// Size of one per-channel state entry written by the UpdateState command.
pub const CHANNEL_STATE_SIZE: usize = 0xC;
/// Size of one capture-unit state entry.
pub const CAPTURE_STATE_SIZE: usize = 8;

/// Channel timer ticks per output sample: the channel clock runs at 2048× the
/// output rate, the same ~32728 Hz rate the DSP produces.
const TIMER_TICKS_PER_SAMPLE: u32 = 2048;
/// Shortest sample period accepted by the hardware (fastest rate).
const MIN_PERIOD: u32 = 0x42;
/// Full-scale channel volume (1.0 in 1.15 fixed point).
const FULL_VOLUME: i32 = 0x8000;
/// Number of steps a volume fade takes.
const FADE_STEPS: i32 = 40;
/// Output samples between two fade steps (~0.42 ms at 32728 Hz, so a full
/// fade lasts about 16.7 ms).
const SAMPLES_PER_FADE_STEP: u32 = 14;

const IMA_STEPS: [i16; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const IMA_INDEX_ADJUST: [i8; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Bit in the loop ADPCM state that reloads the decoder at the loop point.
const LOOP_ADPCM_RELOAD: u32 = 1 << 31;

/// Source of sample bytes (physical memory on the console).
pub trait SampleMemory {
    /// Fills `out` from `address`. Returns false if any byte is unmapped.
    fn read_bytes(&self, address: u32, out: &mut [u8]) -> bool;
}

impl SampleMemory for crate::gpu::GpuPhysicalMemory {
    fn read_bytes(&self, address: u32, out: &mut [u8]) -> bool {
        self.read(address, out).is_ok()
    }
}

/// Sample encoding of a channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SampleFormat {
    #[default]
    Pcm8,
    Pcm16,
    ImaAdpcm,
    /// PSG square wave or noise; not synthesised (silent).
    Psg,
}

impl SampleFormat {
    fn from_bits(bits: u32) -> Self {
        match bits & 3 {
            0 => Self::Pcm8,
            1 => Self::Pcm16,
            2 => Self::ImaAdpcm,
            _ => Self::Psg,
        }
    }
}

/// What a channel does when it reaches the end of its block.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LoopMode {
    /// Ignores the size and keeps reading.
    #[default]
    Manual,
    /// Jumps to the loop address and reloads the size register.
    Repeat,
    /// Stops.
    OneShot,
    /// Jumps to the loop address keeping the size latched at start.
    RepeatLatched,
}

impl LoopMode {
    fn from_bits(bits: u32) -> Self {
        match bits & 3 {
            0 => Self::Manual,
            1 => Self::Repeat,
            2 => Self::OneShot,
            _ => Self::RepeatLatched,
        }
    }
}

/// What happens to a channel once its fade to silence completes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterFade {
    Stop,
    Pause,
}

/// Small read-through window over sample memory so the mixer does not take
/// the physical-memory lock for every byte.
#[derive(Clone, Debug)]
struct FetchWindow {
    base: u32,
    valid: bool,
    bytes: [u8; Self::LEN],
}

impl Default for FetchWindow {
    fn default() -> Self {
        Self {
            base: 0,
            valid: false,
            bytes: [0; Self::LEN],
        }
    }
}

impl FetchWindow {
    const LEN: usize = 256;

    fn byte(&mut self, memory: &dyn SampleMemory, address: u32) -> u8 {
        let base = address & !(Self::LEN as u32 - 1);
        if !self.valid || base != self.base {
            self.base = base;
            self.valid = true;
            if !memory.read_bytes(base, &mut self.bytes) {
                // Partially mapped window: fall back to byte reads.
                for (index, slot) in self.bytes.iter_mut().enumerate() {
                    let mut one = [0u8];
                    *slot = if memory.read_bytes(base + index as u32, &mut one) {
                        one[0]
                    } else {
                        0
                    };
                }
            }
        }
        self.bytes[(address - base) as usize]
    }

    fn invalidate(&mut self) {
        self.valid = false;
    }
}

/// Register file and playback state of one channel.
#[derive(Clone, Debug, Default)]
pub struct SoundChannel {
    // Registers.
    enabled: bool,
    running: bool,
    hold: bool,
    linear: bool,
    format: SampleFormat,
    mode: LoopMode,
    period: u32,
    start_address: u32,
    loop_address: u32,
    size: u32,
    adpcm_state: u32,
    loop_adpcm_state: u32,
    capture_volume: [u16; 2],

    // Volume: what the application asked for, the ramp target and the value
    // the mixer applies.
    requested_volume: [i32; 2],
    target_volume: [i32; 2],
    volume: [i32; 2],
    fade_step: [i32; 2],
    fading: bool,
    after_fade: Option<AfterFade>,

    // Playback.
    block_address: u32,
    block_size: u32,
    cursor: u32,
    upper_nibble: bool,
    holding: bool,
    phase: u32,
    previous: i16,
    current: i16,
    predictor: i16,
    step_index: u8,
    window: FetchWindow,
}

impl SoundChannel {
    /// Whether the channel is started and its DMA is running.
    pub fn is_playing(&self) -> bool {
        self.enabled && self.running
    }

    /// Current IMA-ADPCM decoder state (predictor | index << 16).
    pub fn adpcm_state(&self) -> u32 {
        if self.enabled {
            u32::from(self.predictor as u16) | (u32::from(self.step_index) << 16)
        } else {
            self.adpcm_state
        }
    }

    fn set_timer(&mut self, value: u32) {
        // The register stores 0x10000 - period in 16 bits.
        let register = 0x1_0000u32.wrapping_sub(value) as u16;
        self.period = (0x1_0000 - u32::from(register)).max(MIN_PERIOD);
    }

    fn start(&mut self) {
        self.block_address = self.start_address;
        self.block_size = self.size;
        self.cursor = 0;
        self.upper_nibble = false;
        self.holding = false;
        self.phase = 0;
        self.previous = 0;
        self.current = 0;
        self.predictor = self.adpcm_state as u16 as i16;
        self.step_index = (((self.adpcm_state >> 16) & 0x7F) as u8).min(88);
        self.window.invalidate();
    }

    fn halt(&mut self) {
        if self.enabled {
            self.adpcm_state =
                u32::from(self.predictor as u16) | (u32::from(self.step_index) << 16);
        }
        self.enabled = false;
    }

    fn block_finished(&self) -> bool {
        self.format != SampleFormat::Psg
            && self.mode != LoopMode::Manual
            && self.cursor >= self.block_size
    }

    fn advance(&mut self, memory: &dyn SampleMemory) {
        if self.block_finished() {
            if self.mode == LoopMode::OneShot {
                self.halt();
                self.holding = self.hold;
                return;
            }
            self.block_address = self.loop_address;
            if self.mode == LoopMode::Repeat {
                self.block_size = self.size;
            }
            self.cursor = 0;
            self.upper_nibble = false;
            if self.loop_adpcm_state & LOOP_ADPCM_RELOAD != 0 {
                self.predictor = self.loop_adpcm_state as u16 as i16;
                self.step_index = (((self.loop_adpcm_state >> 16) & 0x7F) as u8).min(88);
            }
            if self.block_size == 0 {
                self.halt();
                return;
            }
        }
        self.previous = self.current;
        self.current = self.decode_next(memory);
    }

    fn decode_next(&mut self, memory: &dyn SampleMemory) -> i16 {
        let address = self.block_address.wrapping_add(self.cursor);
        match self.format {
            SampleFormat::Pcm8 => {
                self.cursor += 1;
                i16::from(self.window.byte(memory, address) as i8) << 8
            }
            SampleFormat::Pcm16 => {
                self.cursor += 2;
                let low = self.window.byte(memory, address);
                let high = self.window.byte(memory, address.wrapping_add(1));
                i16::from_le_bytes([low, high])
            }
            SampleFormat::ImaAdpcm => {
                let byte = self.window.byte(memory, address);
                let nibble = if self.upper_nibble {
                    self.cursor += 1;
                    byte >> 4
                } else {
                    byte & 0xF
                };
                self.upper_nibble = !self.upper_nibble;
                self.decode_ima(nibble)
            }
            SampleFormat::Psg => 0,
        }
    }

    fn decode_ima(&mut self, nibble: u8) -> i16 {
        let step = i32::from(IMA_STEPS[usize::from(self.step_index)]);
        let mut delta = step >> 3;
        if nibble & 1 != 0 {
            delta += step >> 2;
        }
        if nibble & 2 != 0 {
            delta += step >> 1;
        }
        if nibble & 4 != 0 {
            delta += step;
        }
        let value = if nibble & 8 != 0 {
            (i32::from(self.predictor) - delta).max(-0x7FFF)
        } else {
            (i32::from(self.predictor) + delta).min(0x7FFF)
        };
        self.predictor = value as i16;
        let index =
            i32::from(self.step_index) + i32::from(IMA_INDEX_ADJUST[usize::from(nibble & 7)]);
        self.step_index = index.clamp(0, 88) as u8;
        self.predictor
    }

    fn output_sample(&self) -> i32 {
        if !self.linear {
            return i32::from(self.current);
        }
        let delta = i32::from(self.current) - i32::from(self.previous);
        i32::from(self.previous)
            + (i64::from(delta) * i64::from(self.phase) / i64::from(self.period)) as i32
    }

    fn fade_to(&mut self, target: [i32; 2]) {
        self.target_volume = target;
        for ((step, volume), goal) in self
            .fade_step
            .iter_mut()
            .zip(self.volume.iter_mut())
            .zip(target)
        {
            *step = (goal - *volume) / FADE_STEPS;
            if *step == 0 {
                *volume = goal;
            }
        }
        self.fading = self.volume != self.target_volume || self.after_fade.is_some();
    }

    fn step_fade(&mut self) {
        if !self.fading {
            return;
        }
        let mut reached = true;
        for side in 0..2 {
            let step = self.fade_step[side];
            let target = self.target_volume[side];
            let next = (self.volume[side] + step).clamp(0, FULL_VOLUME);
            let done = step == 0 || (step > 0 && next >= target) || (step < 0 && next <= target);
            self.volume[side] = if done { target } else { next };
            reached &= done;
        }
        if reached {
            self.fading = false;
            match self.after_fade.take() {
                Some(AfterFade::Stop) => self.stop_now(),
                Some(AfterFade::Pause) => self.running = false,
                None => {}
            }
        }
    }

    fn stop_now(&mut self) {
        self.halt();
        self.running = false;
        self.hold = false;
        self.linear = false;
        self.holding = false;
    }

    /// Mixes one output sample into `mix`, advancing the channel.
    fn render_one(&mut self, memory: &dyn SampleMemory, mix: &mut [i32; 2]) {
        if self.enabled && self.running {
            self.phase += TIMER_TICKS_PER_SAMPLE;
            while self.phase >= self.period && self.enabled {
                self.phase -= self.period;
                self.advance(memory);
            }
        }
        let sample = if self.enabled {
            self.output_sample()
        } else if self.holding {
            i32::from(self.current)
        } else {
            return;
        };
        mix[0] += (sample * self.volume[0]) >> 15;
        mix[1] += (sample * self.volume[1]) >> 15;
    }

    fn is_audible(&self) -> bool {
        self.enabled || self.holding
    }
}

/// Register state of one capture unit (capture itself is not emulated).
#[derive(Clone, Copy, Debug, Default)]
struct CaptureUnit {
    control: u16,
    period: u32,
    address: u32,
    size: u32,
}

/// Per-session view of the shared command block, set by Initialize.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CsndClientLayout {
    pub master_state_offset: u32,
    pub channel_state_offset: u32,
    pub capture_state_offset: u32,
    pub direct_sound_offset: u32,
    pub channel_mask: u32,
    pub capture_mask: u32,
}

/// The whole sound unit: channels, capture units, global registers and the
/// command interpreter.
#[derive(Clone, Debug)]
pub struct SoundUnit {
    channels: Vec<SoundChannel>,
    captures: [CaptureUnit; CAPTURE_UNIT_COUNT],
    master_volume: i32,
    channel_mask_register: u32,
    capture_mask_register: u32,
    fade_clock: u32,
    commands_executed: u64,
}

impl Default for SoundUnit {
    fn default() -> Self {
        Self {
            channels: vec![SoundChannel::default(); CHANNEL_COUNT],
            captures: [CaptureUnit::default(); CAPTURE_UNIT_COUNT],
            master_volume: FULL_VOLUME,
            channel_mask_register: 0,
            capture_mask_register: 0,
            fade_clock: 0,
            commands_executed: 0,
        }
    }
}

fn read_u16(block: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        block.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn read_u32(block: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        block.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn write_bytes(block: &mut [u8], offset: u32, bytes: &[u8]) {
    let start = offset as usize;
    if let Some(slot) = block.get_mut(start..start.saturating_add(bytes.len())) {
        slot.copy_from_slice(bytes);
    }
}

fn split_volume(value: u32) -> [i32; 2] {
    [
        i32::from(value as u16).min(FULL_VOLUME),
        i32::from((value >> 16) as u16).min(FULL_VOLUME),
    ]
}

impl SoundUnit {
    pub fn new() -> Self {
        Self::default()
    }

    /// Read access to a channel, for diagnostics and tests.
    pub fn channel(&self, index: usize) -> Option<&SoundChannel> {
        self.channels.get(index)
    }

    /// Total number of commands interpreted so far.
    pub fn commands_executed(&self) -> u64 {
        self.commands_executed
    }

    /// True while any channel produces sound or a fade is in flight.
    pub fn is_active(&self) -> bool {
        self.channels.iter().any(|c| c.is_audible() || c.fading)
    }

    /// Stops every channel immediately (the Reset service call).
    pub fn reset_channels(&mut self) {
        for channel in &mut self.channels {
            channel.stop_now();
            channel.volume = [0; 2];
            channel.fading = false;
            channel.after_fade = None;
        }
    }

    /// Walks the command list that starts at `first_offset` inside the shared
    /// block, executes every command and sets the first command's `finished`
    /// byte. Returns the number of commands executed.
    pub fn execute_command_list(
        &mut self,
        block: &mut [u8],
        first_offset: u32,
        client: &CsndClientLayout,
    ) -> usize {
        let limit = block.len() / COMMAND_SIZE;
        let mut offset = first_offset;
        let mut executed = 0usize;
        while offset != u32::from(COMMAND_LIST_END) {
            let start = offset as usize;
            if start + COMMAND_SIZE > block.len() || executed >= limit {
                break;
            }
            let next = read_u16(block, start).unwrap_or(COMMAND_LIST_END);
            let id = read_u16(block, start + 2).unwrap_or(0);
            let mut params = [0u32; 6];
            for (index, param) in params.iter_mut().enumerate() {
                *param = read_u32(block, start + 8 + index * 4).unwrap_or(0);
            }
            self.execute_command(block, id, &params, client);
            executed += 1;
            offset = u32::from(next);
        }
        let first = first_offset as usize;
        if first + COMMAND_SIZE <= block.len() {
            block[first + 4] = 1;
        }
        self.commands_executed += executed as u64;
        executed
    }

    fn execute_command(
        &mut self,
        block: &mut [u8],
        id: u16,
        params: &[u32; 6],
        client: &CsndClientLayout,
    ) {
        let index = (params[0] & 0x1F) as usize;
        let value = params[1];
        match id {
            0x000..=0x0FF => {
                if client.channel_mask & (1 << index) == 0 {
                    return;
                }
                self.channel_command(block, id, index, params, client);
            }
            0x100..=0x1FF => {
                if index >= CAPTURE_UNIT_COUNT {
                    return;
                }
                self.capture_command(block, id, index, params, client);
            }
            0x200 => {
                self.channel_mask_register = client.channel_mask;
                self.capture_mask_register = client.capture_mask & 0xFF;
            }
            0x300 => self.publish_state(block, client),
            _ => {
                let _ = value;
            }
        }
    }

    fn channel_command(
        &mut self,
        block: &mut [u8],
        id: u16,
        index: usize,
        params: &[u32; 6],
        client: &CsndClientLayout,
    ) {
        let value = params[1];
        let channel = &mut self.channels[index];
        let busy = channel.enabled;
        let mut reset_state_entry = false;
        match id {
            // SetPlayStateRaw: start/stop without touching the volume.
            0x000 => {
                if value != 0 {
                    if !channel.enabled {
                        channel.enabled = true;
                        channel.start();
                    }
                } else if channel.is_playing() {
                    channel.after_fade = Some(AfterFade::Stop);
                    channel.fade_to([0; 2]);
                } else {
                    channel.stop_now();
                    channel.volume = [0; 2];
                }
            }
            // SetPlayState: start at the requested volume, or fade out and pause.
            0x001 => Self::set_play_state(channel, value != 0),
            0x002 => {
                if !busy {
                    channel.format = SampleFormat::from_bits(value);
                }
            }
            0x003 => {
                channel.loop_address = value;
                channel.size = params[2];
            }
            0x004 => {
                if !busy {
                    channel.mode = LoopMode::from_bits(value);
                }
            }
            0x005 => channel.hold = value != 0,
            0x006 => {
                if !busy {
                    channel.linear = value != 0;
                }
            }
            0x008 => channel.set_timer(value),
            0x009 => {
                channel.requested_volume = split_volume(value);
                channel.fade_to(channel.requested_volume);
                channel.capture_volume = [params[2] as u16, (params[2] >> 16) as u16];
            }
            0x00A => {
                channel.start_address = value;
                channel.size = params[2];
                reset_state_entry = true;
            }
            0x00B => {
                channel.adpcm_state = (value & 0xFFFF) | ((params[2] & 0xFF) << 16);
                channel.loop_adpcm_state = 0;
            }
            0x00C => {
                channel.loop_adpcm_state =
                    (value & 0xFFFF) | ((params[2] & 0xFF) << 16) | LOOP_ADPCM_RELOAD;
            }
            0x00D => {
                let base = channel.loop_adpcm_state & !LOOP_ADPCM_RELOAD;
                channel.loop_adpcm_state = base | if value != 0 { LOOP_ADPCM_RELOAD } else { 0 };
            }
            // ConfigureChannel: full register setup in one command.
            0x00E => {
                let flags = params[0];
                channel.stop_now();
                channel.set_timer(flags >> 16);
                channel.volume = [0; 2];
                channel.capture_volume = [params[2] as u16, (params[2] >> 16) as u16];
                channel.start_address = params[3];
                channel.loop_address = params[4];
                channel.size = params[5] & 0x07FF_FFFF;
                channel.format = SampleFormat::from_bits(flags >> 12);
                channel.mode = LoopMode::from_bits(flags >> 10);
                channel.linear = flags & (1 << 6) != 0;
                if channel.format != SampleFormat::ImaAdpcm {
                    channel.adpcm_state = 0;
                    channel.loop_adpcm_state = 0;
                }
                channel.requested_volume = split_volume(value);
                channel.target_volume = channel.requested_volume;
                channel.fading = false;
                channel.after_fade = None;
                Self::set_play_state(channel, flags & (1 << 14) != 0);
                reset_state_entry = true;
            }
            // PSG duty / PSG / noise configuration: accepted, not synthesised.
            0x007 | 0x00F | 0x010 => {}
            _ => {}
        }
        if reset_state_entry {
            let slot = (client.channel_mask & ((1u32 << index) - 1)).count_ones();
            write_bytes(
                block,
                client.channel_state_offset + slot * CHANNEL_STATE_SIZE as u32 + 8,
                &0u32.to_le_bytes(),
            );
        }
    }

    fn set_play_state(channel: &mut SoundChannel, play: bool) {
        if play {
            channel.volume = channel.requested_volume;
            channel.target_volume = channel.requested_volume;
            channel.fading = false;
            channel.after_fade = None;
            if !channel.enabled {
                channel.enabled = true;
                channel.start();
            }
            channel.running = true;
        } else if channel.is_playing() {
            channel.after_fade = Some(AfterFade::Pause);
            channel.fade_to([0; 2]);
        } else {
            channel.running = false;
            channel.volume = [0; 2];
        }
    }

    fn capture_command(
        &mut self,
        block: &mut [u8],
        id: u16,
        unit: usize,
        params: &[u32; 6],
        client: &CsndClientLayout,
    ) {
        let value = params[1];
        let capture = &mut self.captures[unit];
        let mut reset_state_entry = false;
        match id {
            0x100 => {
                capture.control = (capture.control & !0x8000) | if value != 0 { 0x8000 } else { 0 }
            }
            0x101 => capture.control = (capture.control & !1) | (value as u16 & 1),
            0x102 => capture.control = (capture.control & !2) | ((value as u16 & 1) << 1),
            0x103 => capture.control = (capture.control & !4) | if value != 0 { 4 } else { 0 },
            0x104 => {
                let register = 0x1_0000u32.wrapping_sub(value) as u16;
                capture.period = 0x1_0000 - u32::from(register);
            }
            0x105 => {
                capture.address = value;
                capture.size = params[2];
                reset_state_entry = true;
            }
            0x106 => {
                let register = 0x1_0000u32.wrapping_sub(value >> 16) as u16;
                capture.period = 0x1_0000 - u32::from(register);
                capture.address = params[2];
                capture.size = params[3];
                capture.control = (value as u16) & 0x8007;
                reset_state_entry = true;
            }
            _ => {}
        }
        if reset_state_entry {
            write_bytes(
                block,
                client.capture_state_offset + (unit * CAPTURE_STATE_SIZE) as u32 + 4,
                &0u32.to_le_bytes(),
            );
        }
    }

    /// UpdateState: publishes the master mask words, one entry per acquired
    /// channel and one per acquired capture unit.
    fn publish_state(&self, block: &mut [u8], client: &CsndClientLayout) {
        let mut master = [0u8; 8];
        master[..4]
            .copy_from_slice(&(self.channel_mask_register & client.channel_mask).to_le_bytes());
        master[4..]
            .copy_from_slice(&(self.capture_mask_register & client.capture_mask).to_le_bytes());
        write_bytes(block, client.master_state_offset, &master);

        let mut slot = 0u32;
        for (index, channel) in self.channels.iter().enumerate() {
            if client.channel_mask & (1 << index) == 0 {
                continue;
            }
            let adpcm = channel.adpcm_state();
            let mut entry = [0u8; CHANNEL_STATE_SIZE];
            entry[0] = u8::from(channel.is_playing());
            entry[4..6].copy_from_slice(&(adpcm as u16).to_le_bytes());
            entry[6] = ((adpcm >> 16) & 0x7F) as u8;
            write_bytes(
                block,
                client.channel_state_offset + slot * CHANNEL_STATE_SIZE as u32,
                &entry,
            );
            slot += 1;
        }
        for unit in 0..CAPTURE_UNIT_COUNT {
            if client.capture_mask & (1 << unit) == 0 {
                continue;
            }
            write_bytes(
                block,
                client.capture_state_offset + (unit * CAPTURE_STATE_SIZE) as u32,
                &[0u8; CAPTURE_STATE_SIZE],
            );
        }
    }

    /// Renders interleaved stereo samples into `out` (`out.len() / 2`
    /// frames), advancing every channel and every volume fade.
    pub fn render(&mut self, memory: &dyn SampleMemory, out: &mut [i16]) {
        for frame in out.chunks_exact_mut(2) {
            self.fade_clock += 1;
            if self.fade_clock >= SAMPLES_PER_FADE_STEP {
                self.fade_clock = 0;
                for channel in &mut self.channels {
                    channel.step_fade();
                }
            }
            let mut mix = [0i32; 2];
            for channel in &mut self.channels {
                channel.render_one(memory, &mut mix);
            }
            for (slot, value) in frame.iter_mut().zip(mix) {
                let scaled = (i64::from(value) * i64::from(self.master_volume)) >> 15;
                *slot = scaled.clamp(-0x8000, 0x7FFF) as i16;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Flat physical memory starting at `base`.
    struct FlatMemory {
        base: u32,
        bytes: Vec<u8>,
    }

    impl SampleMemory for FlatMemory {
        fn read_bytes(&self, address: u32, out: &mut [u8]) -> bool {
            let Some(start) = address.checked_sub(self.base) else {
                return false;
            };
            let start = start as usize;
            match self.bytes.get(start..start + out.len()) {
                Some(src) => {
                    out.copy_from_slice(src);
                    true
                }
                None => false,
            }
        }
    }

    const BASE: u32 = 0x2000_0000;

    fn client() -> CsndClientLayout {
        CsndClientLayout {
            master_state_offset: 0x100,
            channel_state_offset: 0x108,
            capture_state_offset: 0x288,
            direct_sound_offset: 0x298,
            channel_mask: APPLICATION_CHANNEL_MASK,
            capture_mask: 0,
        }
    }

    fn put_command(block: &mut [u8], offset: usize, next: u16, id: u16, params: [u32; 6]) {
        block[offset..offset + 2].copy_from_slice(&next.to_le_bytes());
        block[offset + 2..offset + 4].copy_from_slice(&id.to_le_bytes());
        block[offset + 4] = 0;
        for (index, param) in params.iter().enumerate() {
            let at = offset + 8 + index * 4;
            block[at..at + 4].copy_from_slice(&param.to_le_bytes());
        }
    }

    /// Flags word of ConfigureChannel.
    fn configure_flags(channel: u32, format: u32, mode: u32, play: bool, period: u32) -> u32 {
        channel | (mode << 10) | (format << 12) | (u32::from(play) << 14) | (period << 16)
    }

    #[test]
    fn configure_channel_plays_pcm16_at_output_rate() {
        // Period 2048 ticks = exactly one source sample per output sample.
        let samples: Vec<i16> = vec![1000, -2000, 3000, -4000];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let memory = FlatMemory { base: BASE, bytes };
        let mut unit = SoundUnit::new();
        let mut block = vec![0u8; 0x400];
        put_command(
            &mut block,
            0,
            COMMAND_LIST_END,
            0x00E,
            [
                configure_flags(8, 1, 2, true, 2048),
                0x8000_8000,
                0,
                BASE,
                BASE,
                8,
            ],
        );
        assert_eq!(unit.execute_command_list(&mut block, 0, &client()), 1);
        assert_eq!(block[4], 1, "first command is marked finished");
        assert!(unit.channel(8).unwrap().is_playing());

        let mut out = vec![0i16; 12];
        unit.render(&memory, &mut out);
        let left: Vec<i16> = out.chunks(2).map(|f| f[0]).collect();
        assert_eq!(&left[..4], &[1000, -2000, 3000, -4000]);
        // One-shot without hold: silent and stopped after the block.
        assert_eq!(&left[4..], &[0, 0]);
        assert!(!unit.channel(8).unwrap().is_playing());
    }

    #[test]
    fn pcm8_is_scaled_and_volume_is_applied_per_side() {
        let memory = FlatMemory {
            base: BASE,
            bytes: vec![0x40, 0xC0],
        };
        let mut unit = SoundUnit::new();
        let mut block = vec![0u8; 0x400];
        // Left full, right half.
        put_command(
            &mut block,
            0,
            COMMAND_LIST_END,
            0x00E,
            [
                configure_flags(9, 0, 2, true, 2048),
                0x4000_8000,
                0,
                BASE,
                BASE,
                2,
            ],
        );
        unit.execute_command_list(&mut block, 0, &client());
        let mut out = vec![0i16; 4];
        unit.render(&memory, &mut out);
        assert_eq!(out, vec![0x4000, 0x2000, -0x4000, -0x2000]);
    }

    #[test]
    fn channels_outside_the_acquired_mask_are_ignored() {
        let mut unit = SoundUnit::new();
        let mut block = vec![0u8; 0x400];
        put_command(
            &mut block,
            0,
            COMMAND_LIST_END,
            0x00E,
            [
                configure_flags(3, 1, 2, true, 2048),
                0x8000_8000,
                0,
                BASE,
                BASE,
                8,
            ],
        );
        unit.execute_command_list(&mut block, 0, &client());
        assert!(!unit.channel(3).unwrap().is_playing());
    }

    #[test]
    fn repeat_mode_loops_from_the_loop_address() {
        let samples: Vec<i16> = vec![1, 2, 3, 4];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let memory = FlatMemory { base: BASE, bytes };
        let mut unit = SoundUnit::new();
        let mut block = vec![0u8; 0x400];
        // Block = samples 0..4, loop restarts at sample 2 with size 4 bytes.
        put_command(
            &mut block,
            0,
            0x20,
            0x00E,
            [
                configure_flags(10, 1, 1, false, 2048),
                0x8000_8000,
                0,
                BASE,
                BASE + 4,
                8,
            ],
        );
        put_command(&mut block, 0x20, 0x40, 0x003, [10, BASE + 4, 4, 0, 0, 0]);
        put_command(
            &mut block,
            0x40,
            COMMAND_LIST_END,
            0x001,
            [10, 1, 0, 0, 0, 0],
        );
        assert_eq!(unit.execute_command_list(&mut block, 0, &client()), 3);
        let mut out = vec![0i16; 16];
        unit.render(&memory, &mut out);
        let left: Vec<i16> = out.chunks(2).map(|f| f[0]).collect();
        assert_eq!(left, vec![1, 2, 3, 4, 3, 4, 3, 4]);
    }

    #[test]
    fn timer_halves_the_rate_and_linear_interpolation_blends() {
        let samples: Vec<i16> = vec![0, 1000, 1000];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let memory = FlatMemory { base: BASE, bytes };
        let mut unit = SoundUnit::new();
        let mut block = vec![0u8; 0x400];
        // Period 4096: one source sample every two output samples.
        put_command(
            &mut block,
            0,
            COMMAND_LIST_END,
            0x00E,
            [
                configure_flags(11, 1, 0, true, 4096) | (1 << 6),
                0x8000_8000,
                0,
                BASE,
                BASE,
                6,
            ],
        );
        unit.execute_command_list(&mut block, 0, &client());
        let mut out = vec![0i16; 12];
        unit.render(&memory, &mut out);
        let left: Vec<i16> = out.chunks(2).map(|f| f[0]).collect();
        // A new source sample is latched every second output; interpolation
        // ramps from the previous to the current one, so 1000 arrives as the
        // midpoint first and settles one output later.
        assert_eq!(left, vec![0, 0, 0, 0, 500, 1000]);
    }

    #[test]
    fn ima_adpcm_decodes_both_nibbles() {
        // Nibble 7 (max positive at index 0), then nibble 0.
        let memory = FlatMemory {
            base: BASE,
            bytes: vec![0x07],
        };
        let mut unit = SoundUnit::new();
        let mut block = vec![0u8; 0x400];
        put_command(
            &mut block,
            0,
            COMMAND_LIST_END,
            0x00E,
            [
                configure_flags(12, 2, 2, true, 2048),
                0x8000_8000,
                0,
                BASE,
                BASE,
                1,
            ],
        );
        unit.execute_command_list(&mut block, 0, &client());
        let mut out = vec![0i16; 4];
        unit.render(&memory, &mut out);
        // step 7: diff = 0 + 1 + 3 + 7 = 11; index -> 8 (step 16): diff = 2.
        assert_eq!(out[0], 11);
        assert_eq!(out[2], 13);
    }

    #[test]
    fn pause_fades_out_before_stopping_the_dma() {
        let bytes: Vec<u8> = std::iter::repeat_n(0x40u8, 4096).collect();
        let memory = FlatMemory { base: BASE, bytes };
        let mut unit = SoundUnit::new();
        let mut block = vec![0u8; 0x400];
        put_command(
            &mut block,
            0,
            COMMAND_LIST_END,
            0x00E,
            [
                configure_flags(13, 0, 2, true, 2048),
                0x8000_8000,
                0,
                BASE,
                BASE,
                4096,
            ],
        );
        unit.execute_command_list(&mut block, 0, &client());
        put_command(&mut block, 0, COMMAND_LIST_END, 0x001, [13, 0, 0, 0, 0, 0]);
        unit.execute_command_list(&mut block, 0, &client());
        assert!(unit.channel(13).unwrap().is_playing(), "still fading");
        let mut out = vec![0i16; 2 * 1024];
        unit.render(&memory, &mut out);
        assert!(out[0] > 0, "fade starts from the playing volume");
        assert!(out[0] > out[2 * 300], "volume ramps down");
        assert_eq!(out[2 * 1023], 0);
        assert!(!unit.channel(13).unwrap().is_playing());
    }

    #[test]
    fn update_state_reports_playing_channels_in_acquired_order() {
        let memory = FlatMemory {
            base: BASE,
            bytes: vec![0; 64],
        };
        let mut unit = SoundUnit::new();
        let layout = client();
        let mut block = vec![0u8; 0x400];
        put_command(
            &mut block,
            0,
            0x20,
            0x00E,
            [
                configure_flags(9, 1, 0, true, 2048),
                0x8000_8000,
                0,
                BASE,
                BASE,
                64,
            ],
        );
        put_command(&mut block, 0x20, 0x40, 0x200, [0; 6]);
        put_command(&mut block, 0x40, COMMAND_LIST_END, 0x300, [0; 6]);
        unit.execute_command_list(&mut block, 0, &layout);
        let mut out = vec![0i16; 2];
        unit.render(&memory, &mut out);
        let master = read_u32(&block, 0x100).unwrap();
        assert_eq!(master, APPLICATION_CHANNEL_MASK);
        // Channel 8 is slot 0, channel 9 slot 1.
        assert_eq!(block[0x108], 0);
        assert_eq!(block[0x108 + CHANNEL_STATE_SIZE], 1);
    }

    #[test]
    fn malformed_lists_stop_at_the_block_end() {
        let mut unit = SoundUnit::new();
        let mut block = vec![0u8; 0x40];
        // A self-referencing command must not loop forever.
        put_command(&mut block, 0, 0, 0x300, [0; 6]);
        assert_eq!(unit.execute_command_list(&mut block, 0, &client()), 2);
        assert_eq!(unit.execute_command_list(&mut block, 0x30, &client()), 0);
    }

    #[test]
    fn timer_register_wraps_like_sixteen_bit_hardware() {
        let mut channel = SoundChannel::default();
        channel.set_timer(2048);
        assert_eq!(channel.period, 2048);
        channel.set_timer(0);
        assert_eq!(channel.period, 0x1_0000);
        channel.set_timer(1);
        assert_eq!(channel.period, MIN_PERIOD);
    }
}
