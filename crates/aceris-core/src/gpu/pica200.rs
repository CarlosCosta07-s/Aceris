//! Original PICA200 command processor foundation.
//!
//! This module models the documented command-list register-write protocol.
//! Vertex rasterization, shader execution and undocumented register side
//! effects are deliberately not represented as implemented.

use std::fmt;

use super::DepthFunction;
use super::draw::{
    MAX_PENDING_DRAW_REQUESTS, PicaCommandListRunError, PicaDrawKind, PicaDrawRequest,
    REG_DRAWARRAYS, REG_DRAWELEMENTS,
};

type DrawObserver<'a, E> = dyn FnMut(&Pica200, &PicaDrawRequest) -> Result<(), E> + 'a;
use super::fixed_attribute::{
    FIXED_ATTRIBUTE_COUNT, FixedVertexAttribute, Float24, REG_FIXEDATTRIB_DATA0,
    REG_FIXEDATTRIB_DATA1, REG_FIXEDATTRIB_DATA2, REG_FIXEDATTRIB_INDEX,
};
use super::vertex_shader::{
    PicaShaderOperandDescriptor, PicaVertexShaderInstruction, PicaVertexShaderInstructionFormat,
};

/// Number of 32-bit internal PICA200 registers addressable by command ID.
pub const REGISTER_COUNT: usize = 0x400;

/// PICA viewport horizontal half-size, encoded as float24.
pub const REG_VIEWPORT_WIDTH: u16 = 0x041;
/// Reciprocal viewport width (retained for diagnostics and future derivative rules).
pub const REG_VIEWPORT_INV_WIDTH: u16 = 0x042;
/// PICA viewport vertical half-size, encoded as float24.
pub const REG_VIEWPORT_HEIGHT: u16 = 0x043;
/// Reciprocal viewport height (retained for diagnostics and future derivative rules).
pub const REG_VIEWPORT_INV_HEIGHT: u16 = 0x044;
/// PICA normalized-depth scale, encoded as float24.
pub const REG_DEPTHMAP_SCALE: u16 = 0x04D;
/// PICA normalized-depth offset, encoded as float24.
pub const REG_DEPTHMAP_OFFSET: u16 = 0x04E;
/// PICA viewport origin; signed 10-bit X and Y fields.
pub const REG_VIEWPORT_XY: u16 = 0x068;
/// PICA depth-buffer control: the current model treats bit 0 as Z (1) / W (0).
pub const REG_DEPTHMAP_ENABLE: u16 = 0x06D;
/// GPUREG_DEPTH_COLOR_MASK: depth test/function, channel masks and depth write.
pub const REG_DEPTH_COLOR_MASK: u16 = 0x107;
/// GPUREG_COLORBUFFER_WRITE: per-channel color-buffer write access.
pub const REG_COLORBUFFER_WRITE: u16 = 0x113;
/// GPUREG_DEPTHBUFFER_WRITE: stencil/depth write access.
pub const REG_DEPTHBUFFER_WRITE: u16 = 0x115;

/// Decoded viewport data written by the guest through PICA registers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaViewportState {
    /// Half of the viewport width, in pixels.
    pub half_width: f32,
    /// Half of the viewport height, in pixels.
    pub half_height: f32,
    /// Left edge in PICA screen coordinates.
    pub origin_x: i32,
    /// Bottom edge in PICA screen coordinates (Y increases upward).
    pub origin_y: i32,
    /// Raw reciprocal-width register, retained without interpretation.
    pub inverse_width_raw: u32,
    /// Raw reciprocal-height register, retained without interpretation.
    pub inverse_height_raw: u32,
}

/// PICA's depth interpolation mode selected by GPUREG_DEPTHMAP_ENABLE.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaDepthBuffering {
    WBuffer,
    ZBuffer,
}

/// Decoded guest depth-test configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaDepthTestState {
    pub enabled: bool,
    pub function: DepthFunction,
}

/// Decoded guest controls for PICA depth range mapping and interpolation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaDepthState {
    /// Float24 depth-range scale applied to interpolated Z/W.
    pub scale: f32,
    /// Float24 depth-range offset added after scaling Z/W.
    pub offset: f32,
    /// Z/W selection from register 0x06D.
    pub buffering: PicaDepthBuffering,
}

fn sign_extend_10(value: u32) -> i32 {
    ((value & 0x03FF) << 22) as i32 >> 22
}
/// First documented framebuffer-state register.
pub const REG_COLOR_OPERATION: u16 = 0x100;
/// First documented geometry-state register.
pub const REG_ATTRIBUTE_BUFFERS: u16 = 0x200;
/// Vertex shader float-uniform index/transfer-mode register.
pub const REG_VSH_FLOATUNIFORM_INDEX: u16 = 0x2C0;
/// First of eight vertex shader float-uniform data FIFO registers.
pub const REG_VSH_FLOATUNIFORM_DATA0: u16 = 0x2C1;
/// Vertex shader program transfer completion register.
pub const REG_VSH_CODETRANSFER_END: u16 = 0x2BF;
/// Vertex shader instruction upload index register.
pub const REG_VSH_CODETRANSFER_INDEX: u16 = 0x2CB;
/// First of eight vertex shader instruction upload FIFO registers.
pub const REG_VSH_CODETRANSFER_DATA0: u16 = 0x2CC;
/// Vertex shader operand-descriptor upload index register.
pub const REG_VSH_OPDESCS_INDEX: u16 = 0x2D5;
/// First of three vertex shader operand-descriptor upload FIFO registers.
pub const REG_VSH_OPDESCS_DATA0: u16 = 0x2D6;
/// 12-bit address space exposed by the shader instruction transfer index.
pub const VSH_CODE_TRANSFER_WORDS: usize = 0x1000;
/// 12-bit address space exposed by the shader operand-descriptor transfer index.
pub const VSH_OPDESC_TRANSFER_WORDS: usize = 0x1000;
/// Number of float4 constant registers documented for the vertex shader.
pub const VSH_FLOAT_UNIFORM_COUNT: usize = 96;

/// First command-buffer size register.
pub const REG_COMMAND_BUFFER_SIZE_0: u16 = 0x238;
/// First command-buffer address register.
pub const REG_COMMAND_BUFFER_ADDRESS_0: u16 = 0x23A;
/// First command-buffer trigger register.
pub const REG_COMMAND_BUFFER_JUMP_0: u16 = 0x23C;
/// Second command-buffer size register.
pub const REG_COMMAND_BUFFER_SIZE_1: u16 = 0x239;
/// Second command-buffer address register.
pub const REG_COMMAND_BUFFER_ADDRESS_1: u16 = 0x23B;
/// Second command-buffer trigger register.
pub const REG_COMMAND_BUFFER_JUMP_1: u16 = 0x23D;

/// A command buffer configured through the PICA200 internal register file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaCommandBufferDescriptor {
    pub channel: u8,
    /// Command-buffer base address as encoded by the 3DS register (address >> 3).
    /// The system bridge currently treats the decoded address as process-visible.
    pub address: u32,
    /// Byte length after decoding the size register (size >> 3).
    pub size_bytes: u32,
    /// Non-zero trigger value captured when the descriptor was read.
    pub trigger_value: u32,
}

impl PicaCommandBufferDescriptor {
    pub const fn word_count(self) -> usize {
        (self.size_bytes / 4) as usize
    }
}

/// PICA200 command-list parsing, register-write or command-buffer failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicaCommandError {
    IncompleteCommandHeader {
        word_offset: usize,
    },
    TruncatedParameters {
        word_offset: usize,
        required_words: usize,
        remaining_words: usize,
    },
    InvalidRegister {
        register: u32,
        parameter_index: usize,
    },
    InvalidCommandBufferChannel {
        channel: u8,
    },
    InvalidCommandBufferConfiguration {
        channel: u8,
        size_register: u32,
        address_register: u32,
    },
}

impl fmt::Display for PicaCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompleteCommandHeader { word_offset } => {
                write!(
                    formatter,
                    "PICA200 command at word {word_offset} has no header"
                )
            }
            Self::TruncatedParameters {
                word_offset,
                required_words,
                remaining_words,
            } => write!(
                formatter,
                "PICA200 command at word {word_offset} needs {required_words} words, only {remaining_words} remain"
            ),
            Self::InvalidRegister {
                register,
                parameter_index,
            } => write!(
                formatter,
                "PICA200 command parameter {parameter_index} targets invalid register {register:#x}"
            ),
            Self::InvalidCommandBufferChannel { channel } => {
                write!(
                    formatter,
                    "PICA200 command buffer channel {channel} is invalid"
                )
            }
            Self::InvalidCommandBufferConfiguration {
                channel,
                size_register,
                address_register,
            } => write!(
                formatter,
                "PICA200 command buffer {channel} has invalid size/address registers ({size_register:#x}, {address_register:#x})"
            ),
        }
    }
}

impl std::error::Error for PicaCommandError {}

/// Counters returned after parsing one command list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PicaCommandListReport {
    pub commands: u64,
    pub register_writes: u64,
}

/// PICA200 internal register file and command-list interpreter.
#[derive(Clone, Debug)]
pub struct Pica200 {
    registers: [u32; REGISTER_COUNT],
    fixed_attributes: [[u32; 3]; FIXED_ATTRIBUTE_COUNT],
    immediate_mode_attribute: [u32; 3],
    vertex_shader_code: [u32; VSH_CODE_TRANSFER_WORDS],
    vertex_shader_code_uploaded: [bool; VSH_CODE_TRANSFER_WORDS],
    vertex_shader_operand_descriptors: [u32; VSH_OPDESC_TRANSFER_WORDS],
    vertex_shader_operand_descriptors_uploaded: [bool; VSH_OPDESC_TRANSFER_WORDS],
    vertex_shader_uniforms: [[f32; 4]; VSH_FLOAT_UNIFORM_COUNT],
    vertex_shader_uniforms_uploaded: [bool; VSH_FLOAT_UNIFORM_COUNT],
    pending_uniform_words: Vec<u32>,
    vertex_shader_program_ready: bool,
    pending_draws: Vec<PicaDrawRequest>,
    draws_triggered: u64,
    draws_dropped: u64,
    command_lists_submitted: u64,
    register_writes: u64,
}

impl Default for Pica200 {
    fn default() -> Self {
        Self {
            registers: [0; REGISTER_COUNT],
            fixed_attributes: [[0; 3]; FIXED_ATTRIBUTE_COUNT],
            immediate_mode_attribute: [0; 3],
            vertex_shader_code: [0; VSH_CODE_TRANSFER_WORDS],
            vertex_shader_code_uploaded: [false; VSH_CODE_TRANSFER_WORDS],
            vertex_shader_operand_descriptors: [0; VSH_OPDESC_TRANSFER_WORDS],
            vertex_shader_operand_descriptors_uploaded: [false; VSH_OPDESC_TRANSFER_WORDS],
            vertex_shader_uniforms: [[0.0; 4]; VSH_FLOAT_UNIFORM_COUNT],
            vertex_shader_uniforms_uploaded: [false; VSH_FLOAT_UNIFORM_COUNT],
            pending_uniform_words: Vec::with_capacity(4),
            vertex_shader_program_ready: false,
            pending_draws: Vec::new(),
            draws_triggered: 0,
            draws_dropped: 0,
            command_lists_submitted: 0,
            register_writes: 0,
        }
    }
}

impl Pica200 {
    /// Creates a reset PICA200 command processor.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads an internal register, or returns None for an invalid ID.
    pub fn register(&self, id: u16) -> Option<u32> {
        self.registers.get(usize::from(id)).copied()
    }

    /// Decodes the live guest viewport registers.
    ///
    /// PICA stores half-width/half-height in float24 registers and a signed
    /// 10-bit origin. The origin's Y coordinate is measured from the bottom;
    /// the presentation surface uses a top-left origin, so the render session
    /// performs that coordinate conversion after it knows the target height.
    pub fn viewport_state(&self) -> PicaViewportState {
        let origin = self.register(REG_VIEWPORT_XY).unwrap_or(0);
        PicaViewportState {
            half_width: Float24::from_bits(self.register(REG_VIEWPORT_WIDTH).unwrap_or(0)).to_f32(),
            half_height: Float24::from_bits(self.register(REG_VIEWPORT_HEIGHT).unwrap_or(0))
                .to_f32(),
            origin_x: sign_extend_10(origin),
            origin_y: sign_extend_10(origin >> 16),
            inverse_width_raw: self.register(REG_VIEWPORT_INV_WIDTH).unwrap_or(0),
            inverse_height_raw: self.register(REG_VIEWPORT_INV_HEIGHT).unwrap_or(0),
        }
    }

    /// Decodes the live guest depth-map registers.
    ///
    /// The scale and offset are float24 values. Bit zero of the mode register
    /// selects Z buffering when set and W buffering when clear.
    pub fn depth_state(&self) -> PicaDepthState {
        let mode = self.register(REG_DEPTHMAP_ENABLE).unwrap_or(0);
        PicaDepthState {
            scale: Float24::from_bits(self.register(REG_DEPTHMAP_SCALE).unwrap_or(0)).to_f32(),
            offset: Float24::from_bits(self.register(REG_DEPTHMAP_OFFSET).unwrap_or(0)).to_f32(),
            buffering: if mode & 1 == 0 {
                PicaDepthBuffering::WBuffer
            } else {
                PicaDepthBuffering::ZBuffer
            },
        }
    }

    /// Decodes the live guest depth-test configuration (GPUREG_DEPTH_COLOR_MASK).
    ///
    /// Only the enable bit and comparison function are modeled; the color
    /// write mask and the depth write enable are ignored. Function codes:
    /// 0 never, 1 always, 2 equal, 3 not equal, 4 less, 5 less-equal,
    /// 6 greater, 7 greater-equal. A disabled test always passes.
    pub fn depth_test_state(&self) -> PicaDepthTestState {
        let raw = self.register(REG_DEPTH_COLOR_MASK).unwrap_or(0);
        let function = match (raw >> 4) & 7 {
            0 => DepthFunction::Never,
            1 => DepthFunction::Always,
            2 => DepthFunction::Equal,
            3 => DepthFunction::NotEqual,
            4 => DepthFunction::Less,
            5 => DepthFunction::LessEqual,
            6 => DepthFunction::Greater,
            _ => DepthFunction::GreaterEqual,
        };
        PicaDepthTestState {
            enabled: raw & 1 != 0,
            function,
        }
    }

    /// Writes one complete internal register.
    pub fn write_register(&mut self, id: u16, value: u32) -> Result<(), PicaCommandError> {
        let Some(register) = self.registers.get_mut(usize::from(id)) else {
            return Err(PicaCommandError::InvalidRegister {
                register: u32::from(id),
                parameter_index: 0,
            });
        };
        *register = value;
        self.apply_fixed_attribute_register_write(id);
        self.apply_vertex_shader_register_write(id);
        self.apply_draw_trigger_register_write(id, 0xF);
        self.register_writes = self.register_writes.saturating_add(1);
        Ok(())
    }

    /// Removes and returns the draws triggered since the last call, in order.
    ///
    /// Each request holds the register state captured at its trigger write.
    pub fn take_draw_requests(&mut self) -> Vec<PicaDrawRequest> {
        std::mem::take(&mut self.pending_draws)
    }

    /// Number of accepted draw triggers, including ones dropped by a full queue.
    pub const fn draw_requests_triggered(&self) -> u64 {
        self.draws_triggered
    }

    /// Number of draw requests discarded because the pending queue was full.
    pub const fn draw_requests_dropped(&self) -> u64 {
        self.draws_dropped
    }

    /// Records a draw when a trigger register's low byte is written with bit 0 set.
    ///
    /// `written_mask` is the command byte-lane mask (bit 0 = lowest byte). The
    /// bit-0 condition follows the documented "write 1 to draw" behavior; the
    /// effect of other trigger values is not documented and is ignored.
    fn apply_draw_trigger_register_write(&mut self, id: u16, written_mask: u8) {
        if let Some(request) = self.capture_draw_trigger(id, written_mask) {
            if self.pending_draws.len() >= MAX_PENDING_DRAW_REQUESTS {
                self.draws_dropped = self.draws_dropped.saturating_add(1);
            } else {
                self.pending_draws.push(request);
            }
        }
    }

    /// Counts an accepted trigger and captures its request without queueing it.
    fn capture_draw_trigger(&mut self, id: u16, written_mask: u8) -> Option<PicaDrawRequest> {
        let kind = match id {
            REG_DRAWARRAYS => PicaDrawKind::Arrays,
            REG_DRAWELEMENTS => PicaDrawKind::Elements,
            _ => return None,
        };
        let value = self.registers[usize::from(id)];
        if written_mask & 1 == 0 || value & 1 == 0 {
            return None;
        }
        let sequence = self.draws_triggered;
        self.draws_triggered = self.draws_triggered.saturating_add(1);
        Some(PicaDrawRequest::capture(
            kind,
            value,
            &self.registers,
            sequence,
        ))
    }

    /// Returns the three packed data words for a fixed vertex attribute.
    pub fn fixed_attribute_words(&self, index: usize) -> Option<[u32; 3]> {
        self.fixed_attributes.get(index).copied()
    }

    /// Returns the last immediate-mode attribute payload (index register 0xF).
    pub fn immediate_mode_attribute_words(&self) -> [u32; 3] {
        self.immediate_mode_attribute
    }

    /// Returns one uploaded vertex shader instruction word.
    pub fn vertex_shader_instruction(&self, index: usize) -> Option<u32> {
        self.vertex_shader_code.get(index).copied().filter(|_| {
            self.vertex_shader_code_uploaded
                .get(index)
                .copied()
                .unwrap_or(false)
        })
    }

    /// Decodes a previously uploaded instruction using a caller-supplied format.
    ///
    /// This method only extracts documented bit fields. It intentionally does
    /// not infer the instruction format from the opcode or execute the result.
    pub fn decode_vertex_shader_instruction(
        &self,
        index: usize,
        format: PicaVertexShaderInstructionFormat,
    ) -> Option<PicaVertexShaderInstruction> {
        self.vertex_shader_instruction(index)
            .map(|word| PicaVertexShaderInstruction::decode(word, format))
    }

    /// Returns one uploaded vertex shader operand descriptor word.
    pub fn vertex_shader_operand_descriptor(&self, index: usize) -> Option<u32> {
        self.vertex_shader_operand_descriptors
            .get(index)
            .copied()
            .filter(|_| {
                self.vertex_shader_operand_descriptors_uploaded
                    .get(index)
                    .copied()
                    .unwrap_or(false)
            })
    }

    /// Decodes an uploaded instruction and resolves its optional operand descriptor.
    ///
    /// This is a read-only join between the instruction stream and descriptor
    /// table. It does not validate opcode semantics or execute the instruction.
    pub fn decode_vertex_shader_instruction_with_descriptor(
        &self,
        index: usize,
        format: PicaVertexShaderInstructionFormat,
    ) -> Option<(
        PicaVertexShaderInstruction,
        Option<PicaShaderOperandDescriptor>,
    )> {
        let instruction = self.decode_vertex_shader_instruction(index, format)?;
        let descriptor = instruction
            .operand_descriptor_index()
            .and_then(|descriptor_index| {
                self.decode_vertex_shader_operand_descriptor(descriptor_index)
            });
        Some((instruction, descriptor))
    }

    /// Decodes one uploaded operand descriptor's mask, negation and swizzles.
    pub fn decode_vertex_shader_operand_descriptor(
        &self,
        index: usize,
    ) -> Option<PicaShaderOperandDescriptor> {
        self.vertex_shader_operand_descriptor(index)
            .map(PicaShaderOperandDescriptor::decode)
    }

    /// Returns one decoded vertex shader float4 constant register.
    pub fn vertex_shader_uniform(&self, index: usize) -> Option<[f32; 4]> {
        self.vertex_shader_uniforms.get(index).copied().filter(|_| {
            self.vertex_shader_uniforms_uploaded
                .get(index)
                .copied()
                .unwrap_or(false)
        })
    }

    /// Returns whether the guest signaled completion of the latest code upload.
    ///
    /// This is only an upload-state flag; it does not mean that the program has
    /// been validated or can be executed by Aceris.
    pub const fn vertex_shader_program_ready(&self) -> bool {
        self.vertex_shader_program_ready
    }

    /// Returns the number of float-uniform transfer words waiting for a complete vector.
    pub fn pending_vertex_shader_uniform_words(&self) -> usize {
        self.pending_uniform_words.len()
    }

    fn apply_fixed_attribute_register_write(&mut self, id: u16) {
        let data_index = match id {
            REG_FIXEDATTRIB_DATA0 => 0,
            REG_FIXEDATTRIB_DATA1 => 1,
            REG_FIXEDATTRIB_DATA2 => 2,
            _ => return,
        };
        let selected = (self.registers[usize::from(REG_FIXEDATTRIB_INDEX)] & 0xF) as u8;
        let value = self.registers[usize::from(id)];
        match selected {
            0..=11 => {
                self.fixed_attributes[usize::from(selected)][data_index] = value;
            }
            0xF => {
                self.immediate_mode_attribute[data_index] = value;
            }
            _ => {}
        }
    }

    fn apply_vertex_shader_register_write(&mut self, id: u16) {
        if id == REG_VSH_CODETRANSFER_INDEX {
            self.registers[usize::from(id)] &= 0x0FFF;
            return;
        }

        let code_end = REG_VSH_CODETRANSFER_DATA0 + 7;
        if (REG_VSH_CODETRANSFER_DATA0..=code_end).contains(&id) {
            let index_register = usize::from(REG_VSH_CODETRANSFER_INDEX);
            let index = (self.registers[index_register] & 0x0FFF) as usize;
            self.vertex_shader_code[index] = self.registers[usize::from(id)];
            self.vertex_shader_code_uploaded[index] = true;
            self.registers[index_register] = ((index + 1) % VSH_CODE_TRANSFER_WORDS) as u32;
            self.vertex_shader_program_ready = false;
            return;
        }

        if id == REG_VSH_CODETRANSFER_END {
            self.vertex_shader_program_ready = self.registers[usize::from(id)] != 0;
            return;
        }

        if id == REG_VSH_OPDESCS_INDEX {
            self.registers[usize::from(id)] &= 0x0FFF;
            return;
        }

        let descriptor_end = REG_VSH_OPDESCS_DATA0 + 2;
        if (REG_VSH_OPDESCS_DATA0..=descriptor_end).contains(&id) {
            let index_register = usize::from(REG_VSH_OPDESCS_INDEX);
            let index = (self.registers[index_register] & 0x0FFF) as usize;
            self.vertex_shader_operand_descriptors[index] = self.registers[usize::from(id)];
            self.vertex_shader_operand_descriptors_uploaded[index] = true;
            self.registers[index_register] = ((index + 1) % VSH_OPDESC_TRANSFER_WORDS) as u32;
            return;
        }

        if id == REG_VSH_FLOATUNIFORM_INDEX {
            self.registers[usize::from(id)] &= 0x8000_00FF;
            self.pending_uniform_words.clear();
            return;
        }

        let uniform_data_end = REG_VSH_FLOATUNIFORM_DATA0 + 7;
        if (REG_VSH_FLOATUNIFORM_DATA0..=uniform_data_end).contains(&id) {
            let index_register = usize::from(REG_VSH_FLOATUNIFORM_INDEX);
            let index_value = self.registers[index_register];
            let float32_mode = index_value & 0x8000_0000 != 0;
            self.pending_uniform_words
                .push(self.registers[usize::from(id)]);
            let words_per_uniform = if float32_mode { 4 } else { 3 };

            if self.pending_uniform_words.len() == words_per_uniform {
                let index = (index_value & 0xFF) as usize;
                if index < VSH_FLOAT_UNIFORM_COUNT {
                    self.vertex_shader_uniforms[index] = if float32_mode {
                        [
                            f32::from_bits(self.pending_uniform_words[3]),
                            f32::from_bits(self.pending_uniform_words[2]),
                            f32::from_bits(self.pending_uniform_words[1]),
                            f32::from_bits(self.pending_uniform_words[0]),
                        ]
                    } else {
                        FixedVertexAttribute::from_register_words(
                            self.pending_uniform_words[0],
                            self.pending_uniform_words[1],
                            self.pending_uniform_words[2],
                        )
                        .to_numeric()
                        .components
                    };
                    self.vertex_shader_uniforms_uploaded[index] = true;
                }
                self.pending_uniform_words.clear();
                let next_index = (index_value & 0xFF).saturating_add(1);
                self.registers[index_register] = (index_value & 0x8000_0000) | next_index;
            }
        }
    }

    /// Parses and executes a PICA200 command list from little-endian words.
    ///
    /// A command is an initial parameter, a header, then the extra parameters
    /// counted in header bits 20..27. Bits 0..15 select the register,
    /// bits 16..19 select byte lanes, and bit 31 enables consecutive writes.
    pub fn execute_command_list(
        &mut self,
        words: &[u32],
    ) -> Result<PicaCommandListReport, PicaCommandError> {
        match self.run_command_list(
            words,
            None::<&mut DrawObserver<'_, std::convert::Infallible>>,
        ) {
            Ok(report) => Ok(report),
            Err(PicaCommandListRunError::Command(error)) => Err(error),
            Err(PicaCommandListRunError::Draw(never)) => match never {},
        }
    }

    /// Executes a command list and hands every accepted draw trigger to `on_draw`.
    ///
    /// `on_draw` runs at the instant of the trigger write with the register
    /// file, shader program and uniforms exactly as the draw sees them, so a
    /// list that reprograms state between draws renders each draw correctly.
    /// Draws handled this way are not added to the `take_draw_requests` queue.
    /// The first `Err` from `on_draw` stops the list: writes made so far stay
    /// applied, but the list is not counted as submitted.
    pub fn execute_command_list_with_draws<E>(
        &mut self,
        words: &[u32],
        mut on_draw: impl FnMut(&Pica200, &PicaDrawRequest) -> Result<(), E>,
    ) -> Result<PicaCommandListReport, PicaCommandListRunError<E>> {
        self.run_command_list(words, Some(&mut on_draw))
    }

    fn run_command_list<E>(
        &mut self,
        words: &[u32],
        mut observer: Option<&mut DrawObserver<'_, E>>,
    ) -> Result<PicaCommandListReport, PicaCommandListRunError<E>> {
        validate_command_list(words)?;
        let mut report = PicaCommandListReport::default();
        let mut cursor = 0usize;

        while cursor < words.len() {
            let command_offset = cursor;
            if words.len() - cursor < 2 {
                return Err(PicaCommandError::IncompleteCommandHeader {
                    word_offset: command_offset,
                }
                .into());
            }

            let first_parameter = words[cursor];
            let header = words[cursor + 1];
            let register_id = header & 0xFFFF;
            let byte_mask = ((header >> 16) & 0xF) as u8;
            let extra_parameters = ((header >> 20) & 0xFF) as usize;
            let consecutive = header & (1 << 31) != 0;

            let parameter_count = extra_parameters + 1;
            let command_words = 2usize.saturating_add(extra_parameters);
            let remaining_words = words.len() - cursor;
            if remaining_words < command_words {
                return Err(PicaCommandError::TruncatedParameters {
                    word_offset: command_offset,
                    required_words: command_words,
                    remaining_words,
                }
                .into());
            }

            let final_register = if consecutive {
                register_id.saturating_add(extra_parameters as u32)
            } else {
                register_id
            };
            if register_id >= REGISTER_COUNT as u32 || final_register >= REGISTER_COUNT as u32 {
                return Err(PicaCommandError::InvalidRegister {
                    register: final_register,
                    parameter_index: extra_parameters,
                }
                .into());
            }

            for parameter_index in 0..parameter_count {
                let register = if consecutive {
                    register_id + parameter_index as u32
                } else {
                    register_id
                };
                let value = if parameter_index == 0 {
                    first_parameter
                } else {
                    words[cursor + 1 + parameter_index]
                };
                apply_byte_mask(&mut self.registers[register as usize], value, byte_mask);
                self.apply_fixed_attribute_register_write(register as u16);
                self.apply_vertex_shader_register_write(register as u16);
                match observer.as_deref_mut() {
                    Some(on_draw) => {
                        if let Some(request) = self.capture_draw_trigger(register as u16, byte_mask)
                        {
                            on_draw(&*self, &request).map_err(PicaCommandListRunError::Draw)?;
                        }
                    }
                    None => self.apply_draw_trigger_register_write(register as u16, byte_mask),
                }
                report.register_writes = report.register_writes.saturating_add(1);
            }

            report.commands = report.commands.saturating_add(1);
            cursor += command_words;
        }

        self.command_lists_submitted = self.command_lists_submitted.saturating_add(1);
        self.register_writes = self.register_writes.saturating_add(report.register_writes);
        Ok(report)
    }

    /// Returns a triggered command-buffer descriptor for channel 0 or 1.
    ///
    /// Size and address registers are decoded using their documented >> 3
    /// units. The hardware requires command-buffer byte sizes to be 16-byte
    /// aligned; malformed configurations remain pending for diagnostics.
    pub fn command_buffer_descriptor(
        &self,
        channel: u8,
    ) -> Result<Option<PicaCommandBufferDescriptor>, PicaCommandError> {
        let (size_register, address_register, jump_register) = match channel {
            0 => (
                REG_COMMAND_BUFFER_SIZE_0,
                REG_COMMAND_BUFFER_ADDRESS_0,
                REG_COMMAND_BUFFER_JUMP_0,
            ),
            1 => (
                REG_COMMAND_BUFFER_SIZE_1,
                REG_COMMAND_BUFFER_ADDRESS_1,
                REG_COMMAND_BUFFER_JUMP_1,
            ),
            _ => return Err(PicaCommandError::InvalidCommandBufferChannel { channel }),
        };

        let size_units = self.registers[usize::from(size_register)];
        let address_units = self.registers[usize::from(address_register)];
        let trigger_value = self.registers[usize::from(jump_register)];
        if trigger_value == 0 {
            return Ok(None);
        }

        let size_is_valid =
            size_units != 0 && size_units & !0x001F_FFFF == 0 && size_units & 1 == 0;
        let address_is_valid = address_units & !0x1FFF_FFFF == 0;
        let size_bytes = size_units.checked_mul(8);
        let address = address_units.checked_mul(8);
        let range_is_valid = match (address, size_bytes) {
            (Some(address), Some(size_bytes)) => {
                u64::from(address) + u64::from(size_bytes) <= 0x1_0000_0000
            }
            _ => false,
        };
        if !size_is_valid || !address_is_valid || !range_is_valid {
            return Err(PicaCommandError::InvalidCommandBufferConfiguration {
                channel,
                size_register: size_units,
                address_register: address_units,
            });
        }

        Ok(Some(PicaCommandBufferDescriptor {
            channel,
            address: address.expect("validated command-buffer address"),
            size_bytes: size_bytes.expect("validated command-buffer size"),
            trigger_value,
        }))
    }

    /// Acknowledges one completed command buffer without clearing a newer trigger.
    ///
    /// Returns false if the guest changed the trigger register after the
    /// descriptor was captured, leaving the newer request pending.
    pub fn acknowledge_command_buffer(
        &mut self,
        descriptor: PicaCommandBufferDescriptor,
    ) -> Result<bool, PicaCommandError> {
        let jump_register = match descriptor.channel {
            0 => REG_COMMAND_BUFFER_JUMP_0,
            1 => REG_COMMAND_BUFFER_JUMP_1,
            channel => {
                return Err(PicaCommandError::InvalidCommandBufferChannel { channel });
            }
        };
        let trigger = &mut self.registers[usize::from(jump_register)];
        if *trigger != descriptor.trigger_value {
            return Ok(false);
        }
        *trigger = 0;
        Ok(true)
    }

    pub const fn command_lists_submitted(&self) -> u64 {
        self.command_lists_submitted
    }

    pub const fn register_writes(&self) -> u64 {
        self.register_writes
    }

    pub fn reset(&mut self) {
        self.registers.fill(0);
        self.fixed_attributes.fill([0; 3]);
        self.immediate_mode_attribute.fill(0);
        self.vertex_shader_code.fill(0);
        self.vertex_shader_code_uploaded.fill(false);
        self.vertex_shader_operand_descriptors.fill(0);
        self.vertex_shader_operand_descriptors_uploaded.fill(false);
        self.vertex_shader_uniforms.fill([0.0; 4]);
        self.vertex_shader_uniforms_uploaded.fill(false);
        self.pending_uniform_words.clear();
        self.vertex_shader_program_ready = false;
        self.pending_draws.clear();
        self.draws_triggered = 0;
        self.draws_dropped = 0;
        self.command_lists_submitted = 0;
        self.register_writes = 0;
    }
}

fn validate_command_list(words: &[u32]) -> Result<(), PicaCommandError> {
    let mut cursor = 0usize;
    while cursor < words.len() {
        let command_offset = cursor;
        if words.len() - cursor < 2 {
            return Err(PicaCommandError::IncompleteCommandHeader {
                word_offset: command_offset,
            });
        }

        let header = words[cursor + 1];
        let register_id = header & 0xFFFF;
        let extra_parameters = ((header >> 20) & 0xFF) as usize;
        let consecutive = header & (1 << 31) != 0;
        let command_words = 2usize.saturating_add(extra_parameters);
        let remaining_words = words.len() - cursor;
        if remaining_words < command_words {
            return Err(PicaCommandError::TruncatedParameters {
                word_offset: command_offset,
                required_words: command_words,
                remaining_words,
            });
        }

        let final_register = if consecutive {
            register_id.saturating_add(extra_parameters as u32)
        } else {
            register_id
        };
        if register_id >= REGISTER_COUNT as u32 || final_register >= REGISTER_COUNT as u32 {
            return Err(PicaCommandError::InvalidRegister {
                register: final_register,
                parameter_index: extra_parameters,
            });
        }
        cursor += command_words;
    }
    Ok(())
}

fn apply_byte_mask(target: &mut u32, value: u32, mask: u8) {
    let mut byte_mask = 0u32;
    for lane in 0..4 {
        if mask & (1 << lane) != 0 {
            byte_mask |= 0xFF << (lane * 8);
        }
    }
    *target = (*target & !byte_mask) | (value & byte_mask);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(register: u16, mask: u8, consecutive: bool, extra: u8) -> u32 {
        u32::from(register)
            | (u32::from(mask & 0xF) << 16)
            | (u32::from(extra) << 20)
            | (u32::from(consecutive) << 31)
    }

    #[test]
    fn vertex_shader_code_fifo_upload_increments_index_and_requires_finalize() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 0x12)
            .unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, 0x1122_3344)
            .unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0 + 7, 0x5566_7788)
            .unwrap();

        assert_eq!(gpu.vertex_shader_instruction(0x12), Some(0x1122_3344));
        assert_eq!(gpu.vertex_shader_instruction(0x13), Some(0x5566_7788));
        assert_eq!(gpu.register(REG_VSH_CODETRANSFER_INDEX), Some(0x14));
        assert!(!gpu.vertex_shader_program_ready());

        gpu.write_register(REG_VSH_CODETRANSFER_END, 1).unwrap();
        assert!(gpu.vertex_shader_program_ready());

        gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 0x40)
            .unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0 + 3, 0xAABB_CCDD)
            .unwrap();
        assert_eq!(gpu.vertex_shader_instruction(0x40), Some(0xAABB_CCDD));
        assert!(!gpu.vertex_shader_program_ready());
    }

    #[test]
    fn pica200_joins_uploaded_instruction_with_its_operand_descriptor() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VSH_OPDESCS_INDEX, 0x21).unwrap();
        let descriptor_word = 0b1111u32 | (0x1B << 5) | (0x55 << 14) | (0xE4 << 23);
        gpu.write_register(REG_VSH_OPDESCS_DATA0, descriptor_word)
            .unwrap();

        gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 0x10)
            .unwrap();
        let instruction = (0x03u32 << 26) | 0x21;
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, instruction)
            .unwrap();

        let (decoded, descriptor) = gpu
            .decode_vertex_shader_instruction_with_descriptor(
                0x10,
                PicaVertexShaderInstructionFormat::Format1,
            )
            .unwrap();

        assert_eq!(decoded.operand_descriptor_index(), Some(0x21));
        let descriptor = descriptor.unwrap();
        assert_eq!(descriptor.destination_mask, 0xF);
        assert_eq!(descriptor.source1_component(0), Some(0));
        assert_eq!(descriptor.source1_component(3), Some(3));
        assert_eq!(descriptor.source2_component(0), Some(1));
        assert_eq!(descriptor.source3_component(0), Some(3));
    }

    #[test]
    fn pica200_decodes_uploaded_operand_descriptor_words() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VSH_OPDESCS_INDEX, 0x07).unwrap();
        let descriptor_word =
            0b1010u32 | (1 << 4) | (0x1B << 5) | (0x55 << 14) | (1 << 22) | (0xE4 << 23);
        gpu.write_register(REG_VSH_OPDESCS_DATA0, descriptor_word)
            .unwrap();

        let descriptor = gpu.decode_vertex_shader_operand_descriptor(7).unwrap();
        assert_eq!(descriptor.destination_mask, 0b1010);
        assert!(descriptor.source1_negate);
        assert_eq!(descriptor.source1_component(0), Some(0));
        assert_eq!(descriptor.source1_component(1), Some(1));
        assert_eq!(descriptor.source1_component(2), Some(2));
        assert_eq!(descriptor.source1_component(3), Some(3));
        assert_eq!(descriptor.source2_component(0), Some(1));
        assert_eq!(descriptor.source3_component(0), Some(3));
    }

    #[test]
    fn pica200_decodes_uploaded_shader_words_only_with_explicit_format() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 4).unwrap();
        let instruction =
            (0x03u32 << 26) | (0x02u32 << 21) | (0x01u32 << 12) | (0x10u32 << 7) | 0x21;
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, instruction)
            .unwrap();

        assert_eq!(
            gpu.decode_vertex_shader_instruction(4, PicaVertexShaderInstructionFormat::Format1),
            Some(PicaVertexShaderInstruction::Format1(
                super::super::vertex_shader::PicaShaderFormat1 {
                    descriptor: 0x21,
                    source2: 0x10,
                    source1: 0x01,
                    source1_address_index: 0,
                    destination: 0x02,
                    opcode: 0x03,
                }
            ))
        );
        assert_eq!(
            gpu.decode_vertex_shader_instruction(
                VSH_CODE_TRANSFER_WORDS,
                PicaVertexShaderInstructionFormat::Format1,
            ),
            None
        );
    }

    #[test]
    fn shader_upload_readers_distinguish_unwritten_zero_from_uploaded_zero() {
        let mut gpu = Pica200::new();
        assert_eq!(gpu.vertex_shader_instruction(3), None);
        assert_eq!(gpu.vertex_shader_operand_descriptor(7), None);

        gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 3).unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, 0).unwrap();
        gpu.write_register(REG_VSH_OPDESCS_INDEX, 7).unwrap();
        gpu.write_register(REG_VSH_OPDESCS_DATA0, 0).unwrap();

        assert_eq!(gpu.vertex_shader_instruction(3), Some(0));
        assert_eq!(gpu.vertex_shader_operand_descriptor(7), Some(0));
        assert_eq!(gpu.vertex_shader_instruction(4), None);
        assert_eq!(gpu.vertex_shader_operand_descriptor(8), None);
    }

    #[test]
    fn vertex_shader_code_and_operand_descriptors_accept_command_list_fifo_writes() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 0x20)
            .unwrap();
        let code_report = gpu
            .execute_command_list(&[
                0x0102_0304,
                command(REG_VSH_CODETRANSFER_DATA0, 0xF, true, 1),
                0xA0B0_C0D0,
            ])
            .unwrap();
        assert_eq!(code_report.register_writes, 2);
        assert_eq!(gpu.vertex_shader_instruction(0x20), Some(0x0102_0304));
        assert_eq!(gpu.vertex_shader_instruction(0x21), Some(0xA0B0_C0D0));
        assert_eq!(gpu.register(REG_VSH_CODETRANSFER_INDEX), Some(0x22));

        gpu.write_register(REG_VSH_OPDESCS_INDEX, 0x31).unwrap();
        gpu.execute_command_list(&[
            0x1234_5678,
            command(REG_VSH_OPDESCS_DATA0, 0xF, true, 2),
            0x9ABC_DEF0,
            0x0BAD_C0DE,
        ])
        .unwrap();
        assert_eq!(
            gpu.vertex_shader_operand_descriptor(0x31),
            Some(0x1234_5678)
        );
        assert_eq!(
            gpu.vertex_shader_operand_descriptor(0x32),
            Some(0x9ABC_DEF0)
        );
        assert_eq!(
            gpu.vertex_shader_operand_descriptor(0x33),
            Some(0x0BAD_C0DE)
        );
        assert_eq!(gpu.register(REG_VSH_OPDESCS_INDEX), Some(0x34));
    }

    #[test]
    fn vertex_shader_float24_uniform_fifo_decodes_one_vector_and_advances_index() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VSH_FLOATUNIFORM_INDEX, 5).unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0, 0x4100_0040)
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 1, 0x0000_4000)
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 2, 0x003F_0000)
            .unwrap();

        assert_eq!(gpu.vertex_shader_uniform(5), Some([1.0, 2.0, 2.0, 4.0]));
        assert_eq!(gpu.pending_vertex_shader_uniform_words(), 0);
        assert_eq!(gpu.register(REG_VSH_FLOATUNIFORM_INDEX), Some(6));
    }

    #[test]
    fn vertex_shader_float32_uniform_fifo_decodes_in_reverse_transfer_order() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VSH_FLOATUNIFORM_INDEX, 0x8000_0009)
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0, 4.0f32.to_bits())
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 1, 3.0f32.to_bits())
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 2, 2.0f32.to_bits())
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 3, 1.0f32.to_bits())
            .unwrap();

        assert_eq!(gpu.vertex_shader_uniform(9), Some([1.0, 2.0, 3.0, 4.0]));
        assert_eq!(gpu.pending_vertex_shader_uniform_words(), 0);
        assert_eq!(gpu.register(REG_VSH_FLOATUNIFORM_INDEX), Some(0x8000_000A));
    }

    #[test]
    fn uniforms_distinguish_unwritten_constants_from_uploaded_zero_vectors() {
        let mut gpu = Pica200::new();
        assert_eq!(gpu.vertex_shader_uniform(0), None);

        gpu.write_register(REG_VSH_FLOATUNIFORM_INDEX, 0).unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0, 0).unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 1, 0)
            .unwrap();
        assert_eq!(gpu.pending_vertex_shader_uniform_words(), 2);
        assert_eq!(gpu.vertex_shader_uniform(0), None);
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 2, 0)
            .unwrap();

        assert_eq!(gpu.pending_vertex_shader_uniform_words(), 0);
        assert_eq!(gpu.vertex_shader_uniform(0), Some([0.0; 4]));
        assert_eq!(gpu.vertex_shader_uniform(1), None);
    }

    #[test]
    fn changing_vertex_shader_uniform_index_discards_incomplete_transfer() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VSH_FLOATUNIFORM_INDEX, 2).unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0, 0x1111_1111)
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 1, 0x2222_2222)
            .unwrap();
        assert_eq!(gpu.pending_vertex_shader_uniform_words(), 2);

        gpu.write_register(REG_VSH_FLOATUNIFORM_INDEX, 3).unwrap();
        assert_eq!(gpu.pending_vertex_shader_uniform_words(), 0);
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0, 0x4100_0040)
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 1, 0x0000_4000)
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 2, 0x003F_0000)
            .unwrap();
        assert_eq!(gpu.vertex_shader_uniform(2), None);
        assert_eq!(gpu.vertex_shader_uniform(3), Some([1.0, 2.0, 2.0, 4.0]));
    }

    #[test]
    fn vertex_shader_transfer_indices_wrap_at_their_12_bit_boundary() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 0x0FFF)
            .unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, 0x1111_1111)
            .unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0 + 1, 0x2222_2222)
            .unwrap();
        assert_eq!(
            gpu.vertex_shader_instruction(VSH_CODE_TRANSFER_WORDS - 1),
            Some(0x1111_1111)
        );
        assert_eq!(gpu.vertex_shader_instruction(0), Some(0x2222_2222));
        assert_eq!(gpu.register(REG_VSH_CODETRANSFER_INDEX), Some(1));

        gpu.write_register(REG_VSH_OPDESCS_INDEX, 0x0FFF).unwrap();
        gpu.write_register(REG_VSH_OPDESCS_DATA0, 0x3333_3333)
            .unwrap();
        gpu.write_register(REG_VSH_OPDESCS_DATA0 + 1, 0x4444_4444)
            .unwrap();
        assert_eq!(
            gpu.vertex_shader_operand_descriptor(VSH_OPDESC_TRANSFER_WORDS - 1),
            Some(0x3333_3333)
        );
        assert_eq!(gpu.vertex_shader_operand_descriptor(0), Some(0x4444_4444));
        assert_eq!(gpu.register(REG_VSH_OPDESCS_INDEX), Some(1));
    }

    #[test]
    fn vertex_shader_float_uniform_index_is_bounded_to_documented_c_registers() {
        let mut gpu = Pica200::new();
        gpu.write_register(
            REG_VSH_FLOATUNIFORM_INDEX,
            0x8000_0000 | (VSH_FLOAT_UNIFORM_COUNT - 1) as u32,
        )
        .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0, 4.0f32.to_bits())
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 1, 3.0f32.to_bits())
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 2, 2.0f32.to_bits())
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 3, 1.0f32.to_bits())
            .unwrap();
        assert_eq!(
            gpu.vertex_shader_uniform(VSH_FLOAT_UNIFORM_COUNT - 1),
            Some([1.0, 2.0, 3.0, 4.0])
        );
        assert_eq!(
            gpu.register(REG_VSH_FLOATUNIFORM_INDEX),
            Some(0x8000_0000 | VSH_FLOAT_UNIFORM_COUNT as u32)
        );

        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0, 4.0f32.to_bits())
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 1, 3.0f32.to_bits())
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 2, 2.0f32.to_bits())
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0 + 3, 1.0f32.to_bits())
            .unwrap();
        assert_eq!(gpu.vertex_shader_uniform(VSH_FLOAT_UNIFORM_COUNT), None);
        assert_eq!(gpu.pending_vertex_shader_uniform_words(), 0);
    }

    #[test]
    fn decodes_guest_viewport_half_sizes_and_signed_origin() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VIEWPORT_WIDTH, 0x0041_0000).unwrap(); // 4.0
        gpu.write_register(REG_VIEWPORT_HEIGHT, 0x0040_0000)
            .unwrap(); // 2.0
        gpu.write_register(REG_VIEWPORT_INV_WIDTH, 0x1234_5678)
            .unwrap();
        gpu.write_register(REG_VIEWPORT_INV_HEIGHT, 0x8765_4321)
            .unwrap();
        let origin_x = (-7i32 as u32) & 0x03FF;
        let origin_y = 11u32 & 0x03FF;
        gpu.write_register(REG_VIEWPORT_XY, origin_x | (origin_y << 16))
            .unwrap();

        assert_eq!(
            gpu.viewport_state(),
            PicaViewportState {
                half_width: 4.0,
                half_height: 2.0,
                origin_x: -7,
                origin_y: 11,
                inverse_width_raw: 0x1234_5678,
                inverse_height_raw: 0x8765_4321,
            }
        );
    }

    #[test]
    fn decodes_guest_z_and_w_depth_modes_and_float24_mapping() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_DEPTHMAP_SCALE, 0x003E_0000).unwrap(); // 0.5
        gpu.write_register(REG_DEPTHMAP_OFFSET, 0x003E_0000)
            .unwrap(); // 0.5

        assert_eq!(
            gpu.depth_state(),
            PicaDepthState {
                scale: 0.5,
                offset: 0.5,
                buffering: PicaDepthBuffering::WBuffer,
            }
        );

        gpu.write_register(REG_DEPTHMAP_ENABLE, 1).unwrap();
        assert_eq!(
            gpu.depth_state(),
            PicaDepthState {
                scale: 0.5,
                offset: 0.5,
                buffering: PicaDepthBuffering::ZBuffer,
            }
        );
    }

    #[test]
    fn reset_clears_vertex_shader_uploads_uniforms_and_transfer_state() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 0).unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, 0xCAFE_BABE)
            .unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_END, 1).unwrap();
        gpu.write_register(REG_VSH_OPDESCS_INDEX, 7).unwrap();
        gpu.write_register(REG_VSH_OPDESCS_DATA0, 0x1234_5678)
            .unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_INDEX, 0).unwrap();
        gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0, 0x4100_0040)
            .unwrap();

        gpu.reset();

        assert_eq!(gpu.vertex_shader_instruction(0), None);
        assert_eq!(gpu.vertex_shader_operand_descriptor(7), None);
        assert_eq!(gpu.vertex_shader_uniform(0), None);
        assert_eq!(gpu.pending_vertex_shader_uniform_words(), 0);
        assert!(!gpu.vertex_shader_program_ready());
        assert_eq!(gpu.register(REG_VSH_CODETRANSFER_INDEX), Some(0));
    }

    #[test]
    fn command_list_writes_one_register_with_a_full_mask() {
        let mut gpu = Pica200::new();
        let report = gpu
            .execute_command_list(&[0xDEAD_BEEF, command(0x100, 0xF, false, 0)])
            .unwrap();
        assert_eq!(report.commands, 1);
        assert_eq!(report.register_writes, 1);
        assert_eq!(gpu.register(0x100), Some(0xDEAD_BEEF));
        assert_eq!(gpu.command_lists_submitted(), 1);
    }

    #[test]
    fn command_list_applies_byte_mask_without_changing_other_lanes() {
        let mut gpu = Pica200::new();
        gpu.write_register(0x100, 0x1122_3344).unwrap();
        gpu.execute_command_list(&[0xAABB_CCDD, command(0x100, 0b0101, false, 0)])
            .unwrap();
        assert_eq!(gpu.register(0x100), Some(0x11BB_33DD));
    }

    #[test]
    fn command_list_can_write_consecutive_registers() {
        let mut gpu = Pica200::new();
        let report = gpu
            .execute_command_list(&[0x10, command(0x200, 0xF, true, 2), 0x20, 0x30])
            .unwrap();
        assert_eq!(report.commands, 1);
        assert_eq!(report.register_writes, 3);
        assert_eq!(gpu.register(0x200), Some(0x10));
        assert_eq!(gpu.register(0x201), Some(0x20));
        assert_eq!(gpu.register(0x202), Some(0x30));
    }

    #[test]
    fn command_list_reports_truncated_extra_parameters() {
        let mut gpu = Pica200::new();
        let error = gpu
            .execute_command_list(&[0x10, command(0x200, 0xF, true, 2), 0x20])
            .unwrap_err();
        assert_eq!(
            error,
            PicaCommandError::TruncatedParameters {
                word_offset: 0,
                required_words: 4,
                remaining_words: 3,
            }
        );
        assert_eq!(gpu.command_lists_submitted(), 0);
    }

    #[test]
    fn command_list_rejects_register_overflow_before_any_write() {
        let mut gpu = Pica200::new();
        let error = gpu
            .execute_command_list(&[0x10, command(0x3FF, 0xF, true, 1), 0x20])
            .unwrap_err();
        assert!(matches!(error, PicaCommandError::InvalidRegister { .. }));
        assert_eq!(gpu.register(0x3FF), Some(0));
        assert_eq!(gpu.command_lists_submitted(), 0);
    }

    #[test]
    fn invalid_trailing_command_does_not_partially_apply_earlier_register_writes() {
        let mut gpu = Pica200::new();
        gpu.write_register(0x100, 0x1122_3344).unwrap();

        let error = gpu
            .execute_command_list(&[
                0xAABB_CCDD,
                command(0x101, 0xF, false, 0),
                0xDEAD_BEEF,
                command(0x3FF, 0xF, true, 1),
                0x1234_5678,
            ])
            .unwrap_err();

        assert!(matches!(error, PicaCommandError::InvalidRegister { .. }));
        assert_eq!(gpu.register(0x100), Some(0x1122_3344));
        assert_eq!(gpu.register(0x101), Some(0));
        assert_eq!(gpu.register(0x3FF), Some(0));
        assert_eq!(gpu.command_lists_submitted(), 0);
        assert_eq!(gpu.register_writes(), 1);
    }

    #[test]
    fn command_buffer_channels_decode_size_address_and_trigger() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_COMMAND_BUFFER_SIZE_0, 2).unwrap();
        gpu.write_register(REG_COMMAND_BUFFER_ADDRESS_0, 0x0010_2000 >> 3)
            .unwrap();
        gpu.write_register(REG_COMMAND_BUFFER_JUMP_0, 1).unwrap();
        gpu.write_register(REG_COMMAND_BUFFER_SIZE_1, 4).unwrap();
        gpu.write_register(REG_COMMAND_BUFFER_ADDRESS_1, 0x0020_0000 >> 3)
            .unwrap();
        gpu.write_register(REG_COMMAND_BUFFER_JUMP_1, 0x80).unwrap();

        assert_eq!(
            gpu.command_buffer_descriptor(0).unwrap(),
            Some(PicaCommandBufferDescriptor {
                channel: 0,
                address: 0x0010_2000,
                size_bytes: 16,
                trigger_value: 1,
            })
        );
        assert_eq!(
            gpu.command_buffer_descriptor(1).unwrap(),
            Some(PicaCommandBufferDescriptor {
                channel: 1,
                address: 0x0020_0000,
                size_bytes: 32,
                trigger_value: 0x80,
            })
        );
        assert!(matches!(
            gpu.command_buffer_descriptor(2),
            Err(PicaCommandError::InvalidCommandBufferChannel { channel: 2 })
        ));
    }

    #[test]
    fn command_buffer_requires_a_nonzero_trigger_and_aligned_nonzero_size() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_COMMAND_BUFFER_SIZE_0, 1).unwrap();
        gpu.write_register(REG_COMMAND_BUFFER_ADDRESS_0, 0x1000 >> 3)
            .unwrap();
        assert_eq!(gpu.command_buffer_descriptor(0).unwrap(), None);

        gpu.write_register(REG_COMMAND_BUFFER_JUMP_0, 1).unwrap();
        assert!(matches!(
            gpu.command_buffer_descriptor(0),
            Err(PicaCommandError::InvalidCommandBufferConfiguration { channel: 0, .. })
        ));
    }

    #[test]
    fn command_buffer_acknowledgement_does_not_clear_a_newer_trigger() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_COMMAND_BUFFER_SIZE_0, 2).unwrap();
        gpu.write_register(REG_COMMAND_BUFFER_ADDRESS_0, 0x1000 >> 3)
            .unwrap();
        gpu.write_register(REG_COMMAND_BUFFER_JUMP_0, 1).unwrap();
        let descriptor = gpu.command_buffer_descriptor(0).unwrap().unwrap();

        gpu.write_register(REG_COMMAND_BUFFER_JUMP_0, 2).unwrap();
        assert!(!gpu.acknowledge_command_buffer(descriptor).unwrap());
        assert_eq!(gpu.register(REG_COMMAND_BUFFER_JUMP_0), Some(2));
    }

    #[test]
    fn empty_command_list_is_a_successful_noop() {
        let mut gpu = Pica200::new();
        assert_eq!(
            gpu.execute_command_list(&[]).unwrap(),
            PicaCommandListReport::default()
        );
        assert_eq!(gpu.command_lists_submitted(), 1);
    }
}
