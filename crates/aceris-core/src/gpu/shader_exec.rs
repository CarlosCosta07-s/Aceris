//! Reference execution of a documented subset of the PICA200 vertex shader ISA.
//!
//! Supported: the arithmetic instructions without address-register indexing
//! (ADD, MUL, DP3, DP4, DPH, DPHI, SLT, SLTI, SGE, SGEI, MAX, MIN, FLR, RCP,
//! RSQ, EX2, LG2, MOV, MAD, MADI) plus NOP and END. Programs are straight-line:
//! flow control (CALL/IF/LOOP/JMP/BREAK), comparisons that set condition codes
//! (CMP), MOVA, address-register indexing, LITP/DST and geometry-shader emits
//! are rejected with an explicit error instead of being guessed.
//!
//! Arithmetic is performed in IEEE `f32`. The hardware's internal float24
//! precision and its approximated reciprocal/inverse-square-root results are
//! not modeled. Multiplication follows the documented rule that a zero
//! operand yields zero, even when the other operand is infinite or NaN.

use super::pica200::VSH_OPDESC_TRANSFER_WORDS;
use super::pica200::{Pica200, VSH_CODE_TRANSFER_WORDS, VSH_FLOAT_UNIFORM_COUNT};
use super::vertex_shader::{
    PicaShaderOperandDescriptor, PicaVertexShaderInstruction, PicaVertexShaderInstructionFormat,
};

/// Number of shader input registers (v0..v15).
pub const SHADER_INPUT_REGISTERS: usize = 16;
/// Number of shader output registers (o0..o15).
pub const SHADER_OUTPUT_REGISTERS: usize = 16;
/// Number of shader temporary registers (r0..r15).
pub const SHADER_TEMPORARY_REGISTERS: usize = 16;

/// Instructions this executor understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaShaderOpcode {
    Add,
    Dp3,
    Dp4,
    Dph,
    Ex2,
    Lg2,
    Mul,
    Sge,
    Slt,
    Flr,
    Max,
    Min,
    Rcp,
    Rsq,
    Mov,
    Dphi,
    Sgei,
    Slti,
    Nop,
    End,
    Mad,
    Madi,
}

impl PicaShaderOpcode {
    /// Identifies a supported instruction from a raw 32-bit shader word.
    ///
    /// Multiply-add forms use only the top three bits of the opcode field
    /// (0b110 = MADI, 0b111 = MAD); everything else uses the full 6-bit field.
    pub const fn from_word(word: u32) -> Option<Self> {
        match word >> 29 {
            0b111 => return Some(Self::Mad),
            0b110 => return Some(Self::Madi),
            _ => {}
        }
        match word >> 26 {
            0x00 => Some(Self::Add),
            0x01 => Some(Self::Dp3),
            0x02 => Some(Self::Dp4),
            0x03 => Some(Self::Dph),
            0x05 => Some(Self::Ex2),
            0x06 => Some(Self::Lg2),
            0x08 => Some(Self::Mul),
            0x09 => Some(Self::Sge),
            0x0A => Some(Self::Slt),
            0x0B => Some(Self::Flr),
            0x0C => Some(Self::Max),
            0x0D => Some(Self::Min),
            0x0E => Some(Self::Rcp),
            0x0F => Some(Self::Rsq),
            0x13 => Some(Self::Mov),
            0x18 => Some(Self::Dphi),
            0x1A => Some(Self::Sgei),
            0x1B => Some(Self::Slti),
            0x21 => Some(Self::Nop),
            0x22 => Some(Self::End),
            _ => None,
        }
    }

    const fn format(self) -> Option<PicaVertexShaderInstructionFormat> {
        match self {
            Self::Add
            | Self::Dp3
            | Self::Dp4
            | Self::Dph
            | Self::Mul
            | Self::Sge
            | Self::Slt
            | Self::Max
            | Self::Min => Some(PicaVertexShaderInstructionFormat::Format1),
            Self::Ex2 | Self::Lg2 | Self::Flr | Self::Rcp | Self::Rsq | Self::Mov => {
                Some(PicaVertexShaderInstructionFormat::Format1Unary)
            }
            Self::Dphi | Self::Sgei | Self::Slti => {
                Some(PicaVertexShaderInstructionFormat::Format1Inverted)
            }
            Self::Mad => Some(PicaVertexShaderInstructionFormat::Format5MultiplyAdd),
            Self::Madi => Some(PicaVertexShaderInstructionFormat::Format5InvertedMultiplyAdd),
            Self::Nop | Self::End => None,
        }
    }
}

/// Failure while running a vertex shader program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicaShaderExecError {
    /// The program counter reached a word that was never uploaded (or the end of code space).
    InstructionNotUploaded { index: usize },
    /// The instruction names an operand descriptor that was never uploaded.
    DescriptorNotUploaded { index: usize, descriptor: usize },
    /// The opcode is not part of the supported subset.
    UnsupportedOpcode { index: usize, word: u32 },
    /// The instruction indexes a register through a0/a1/aL.
    AddressRegisterUnsupported { index: usize },
}

impl std::fmt::Display for PicaShaderExecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InstructionNotUploaded { index } => {
                write!(formatter, "shader instruction {index} was not uploaded")
            }
            Self::DescriptorNotUploaded { index, descriptor } => write!(
                formatter,
                "shader instruction {index} uses operand descriptor {descriptor}, which was not uploaded"
            ),
            Self::UnsupportedOpcode { index, word } => write!(
                formatter,
                "shader instruction {index} ({word:#010x}) is not in the supported subset"
            ),
            Self::AddressRegisterUnsupported { index } => write!(
                formatter,
                "shader instruction {index} uses address-register indexing, which is unsupported"
            ),
        }
    }
}

impl std::error::Error for PicaShaderExecError {}

/// Registers produced by one shader run.
#[derive(Clone, Debug, PartialEq)]
pub struct PicaVertexShaderOutput {
    pub outputs: [[f32; 4]; SHADER_OUTPUT_REGISTERS],
    pub executed_instructions: usize,
}

/// Snapshot of an uploaded vertex shader program, ready to run per vertex.
#[derive(Clone, Debug)]
pub struct PicaVertexShaderProgram {
    code: Vec<Option<u32>>,
    descriptors: Vec<Option<u32>>,
    uniforms: Vec<[f32; 4]>,
}

impl PicaVertexShaderProgram {
    /// Creates a program where `None` marks words that were never uploaded.
    pub fn new(
        code: Vec<Option<u32>>,
        descriptors: Vec<Option<u32>>,
        uniforms: Vec<[f32; 4]>,
    ) -> Self {
        Self {
            code,
            descriptors,
            uniforms,
        }
    }

    /// Creates a program from fully uploaded code and descriptor words.
    pub fn from_words(code: &[u32], descriptors: &[u32], uniforms: &[[f32; 4]]) -> Self {
        Self::new(
            code.iter().copied().map(Some).collect(),
            descriptors.iter().copied().map(Some).collect(),
            uniforms.to_vec(),
        )
    }

    /// Runs the program from `entry` until END with the given v0..v15 inputs.
    ///
    /// Temporaries and outputs start at zero. Constants that were never
    /// uploaded read as zero.
    pub fn run(
        &self,
        entry: usize,
        inputs: &[[f32; 4]; SHADER_INPUT_REGISTERS],
    ) -> Result<PicaVertexShaderOutput, PicaShaderExecError> {
        let mut machine = Machine {
            program: self,
            inputs,
            temporaries: [[0.0; 4]; SHADER_TEMPORARY_REGISTERS],
            outputs: [[0.0; 4]; SHADER_OUTPUT_REGISTERS],
        };
        let mut index = entry;
        let mut executed_instructions = 0usize;
        loop {
            let word = self
                .code
                .get(index)
                .copied()
                .flatten()
                .ok_or(PicaShaderExecError::InstructionNotUploaded { index })?;
            let opcode = PicaShaderOpcode::from_word(word)
                .ok_or(PicaShaderExecError::UnsupportedOpcode { index, word })?;
            executed_instructions += 1;
            match opcode {
                PicaShaderOpcode::Nop => {}
                PicaShaderOpcode::End => {
                    return Ok(PicaVertexShaderOutput {
                        outputs: machine.outputs,
                        executed_instructions,
                    });
                }
                _ => machine.execute(index, word, opcode)?,
            }
            index += 1;
        }
    }
}

impl Pica200 {
    /// Snapshots the uploaded vertex shader code, descriptors and float constants.
    pub fn vertex_shader_program(&self) -> PicaVertexShaderProgram {
        PicaVertexShaderProgram::new(
            (0..VSH_CODE_TRANSFER_WORDS)
                .map(|index| self.vertex_shader_instruction(index))
                .collect(),
            (0..VSH_OPDESC_TRANSFER_WORDS)
                .map(|index| self.vertex_shader_operand_descriptor(index))
                .collect(),
            (0..VSH_FLOAT_UNIFORM_COUNT)
                .map(|index| self.vertex_shader_uniform(index).unwrap_or([0.0; 4]))
                .collect(),
        )
    }
}

/// Source operand: a 5- or 7-bit register number and its address-index field.
#[derive(Clone, Copy)]
struct Source {
    register: u8,
    address_index: u8,
}

struct Machine<'a> {
    program: &'a PicaVertexShaderProgram,
    inputs: &'a [[f32; 4]; SHADER_INPUT_REGISTERS],
    temporaries: [[f32; 4]; SHADER_TEMPORARY_REGISTERS],
    outputs: [[f32; 4]; SHADER_OUTPUT_REGISTERS],
}

impl Machine<'_> {
    fn execute(
        &mut self,
        index: usize,
        word: u32,
        opcode: PicaShaderOpcode,
    ) -> Result<(), PicaShaderExecError> {
        let format = opcode
            .format()
            .expect("Nop/End are handled before operand decoding");
        let instruction = PicaVertexShaderInstruction::decode(word, format);
        let (destination, descriptor_index, sources) = operands(instruction);

        let descriptor_word = self
            .program
            .descriptors
            .get(descriptor_index)
            .copied()
            .flatten()
            .ok_or(PicaShaderExecError::DescriptorNotUploaded {
                index,
                descriptor: descriptor_index,
            })?;
        let descriptor = PicaShaderOperandDescriptor::decode(descriptor_word);

        let selectors = [
            (descriptor.source1_selector, descriptor.source1_negate),
            (descriptor.source2_selector, descriptor.source2_negate),
            (descriptor.source3_selector, descriptor.source3_negate),
        ];
        let mut values = [[0.0f32; 4]; 3];
        for (slot, source) in sources.iter().enumerate() {
            if let Some(source) = source {
                let raw = self.read_register(index, *source)?;
                values[slot] = swizzle(raw, selectors[slot].0, selectors[slot].1);
            }
        }
        let [a, b, c] = values;

        let result = match opcode {
            PicaShaderOpcode::Add => lanewise(a, b, |x, y| x + y),
            PicaShaderOpcode::Mul => lanewise(a, b, pica_mul),
            PicaShaderOpcode::Max => lanewise(a, b, |x, y| if x >= y { x } else { y }),
            PicaShaderOpcode::Min => lanewise(a, b, |x, y| if x < y { x } else { y }),
            PicaShaderOpcode::Slt | PicaShaderOpcode::Slti => {
                lanewise(a, b, |x, y| if x < y { 1.0 } else { 0.0 })
            }
            PicaShaderOpcode::Sge | PicaShaderOpcode::Sgei => {
                lanewise(a, b, |x, y| if x >= y { 1.0 } else { 0.0 })
            }
            PicaShaderOpcode::Dp3 => [dot(a, b, 3); 4],
            PicaShaderOpcode::Dp4 => [dot(a, b, 4); 4],
            PicaShaderOpcode::Dph | PicaShaderOpcode::Dphi => {
                // Homogeneous dot product: the first source's w is treated as 1.0.
                [dot([a[0], a[1], a[2], 1.0], b, 4); 4]
            }
            PicaShaderOpcode::Flr => [a[0].floor(), a[1].floor(), a[2].floor(), a[3].floor()],
            PicaShaderOpcode::Ex2 => [a[0].exp2(); 4],
            PicaShaderOpcode::Lg2 => [a[0].log2(); 4],
            PicaShaderOpcode::Rcp => [1.0 / a[0]; 4],
            PicaShaderOpcode::Rsq => [1.0 / a[0].sqrt(); 4],
            PicaShaderOpcode::Mov => a,
            PicaShaderOpcode::Mad | PicaShaderOpcode::Madi => {
                let product = lanewise(a, b, pica_mul);
                lanewise(product, c, |x, y| x + y)
            }
            PicaShaderOpcode::Nop | PicaShaderOpcode::End => unreachable!("handled by the caller"),
        };

        let target = if destination < 0x10 {
            &mut self.outputs[usize::from(destination)]
        } else {
            &mut self.temporaries[usize::from(destination - 0x10)]
        };
        for lane in 0..4u8 {
            if descriptor.destination_lane_enabled(lane) {
                target[usize::from(lane)] = result[usize::from(lane)];
            }
        }
        Ok(())
    }

    /// Reads a source register: v0..v15, then r0..r15, then float constants.
    fn read_register(&self, index: usize, source: Source) -> Result<[f32; 4], PicaShaderExecError> {
        if source.address_index != 0 {
            return Err(PicaShaderExecError::AddressRegisterUnsupported { index });
        }
        let register = usize::from(source.register);
        Ok(if register < 0x10 {
            self.inputs[register]
        } else if register < 0x20 {
            self.temporaries[register - 0x10]
        } else {
            self.program
                .uniforms
                .get(register - 0x20)
                .copied()
                .unwrap_or([0.0; 4])
        })
    }
}

/// Extracts destination, descriptor index and up to three sources.
fn operands(instruction: PicaVertexShaderInstruction) -> (u8, usize, [Option<Source>; 3]) {
    let plain = |register: u8| Source {
        register,
        address_index: 0,
    };
    match instruction {
        PicaVertexShaderInstruction::Format1(value) => (
            value.destination,
            usize::from(value.descriptor),
            [
                Some(Source {
                    register: value.source1,
                    address_index: value.source1_address_index,
                }),
                Some(plain(value.source2)),
                None,
            ],
        ),
        PicaVertexShaderInstruction::Format1Unary(value) => (
            value.destination,
            usize::from(value.descriptor),
            [
                Some(Source {
                    register: value.source1,
                    address_index: value.source1_address_index,
                }),
                None,
                None,
            ],
        ),
        PicaVertexShaderInstruction::Format1Inverted(value) => (
            value.destination,
            usize::from(value.descriptor),
            [
                Some(plain(value.source1)),
                Some(Source {
                    register: value.source2,
                    address_index: value.source2_address_index,
                }),
                None,
            ],
        ),
        PicaVertexShaderInstruction::Format5MultiplyAdd(value) => (
            value.destination,
            usize::from(value.descriptor),
            [
                Some(plain(value.source1)),
                Some(Source {
                    register: value.source2,
                    address_index: value.source2_address_index,
                }),
                Some(plain(value.source3)),
            ],
        ),
        PicaVertexShaderInstruction::Format5InvertedMultiplyAdd(value) => (
            value.destination,
            usize::from(value.descriptor),
            [
                Some(plain(value.source1)),
                Some(plain(value.source2)),
                Some(Source {
                    register: value.source3,
                    address_index: value.source3_address_index,
                }),
            ],
        ),
        PicaVertexShaderInstruction::Format1Compare(_)
        | PicaVertexShaderInstruction::Format2Flow(_)
        | PicaVertexShaderInstruction::Format3BooleanFlow(_)
        | PicaVertexShaderInstruction::Format4Emit(_) => {
            unreachable!("opcode table never selects these formats")
        }
    }
}

/// Applies a source swizzle (x lane in selector bits 6..7) and optional negation.
fn swizzle(value: [f32; 4], selector: u8, negate: bool) -> [f32; 4] {
    let mut result = [0.0f32; 4];
    for lane in 0..4u8 {
        let component =
            PicaShaderOperandDescriptor::source_component(selector, lane).expect("lane is in 0..4");
        let component_value = value[usize::from(component)];
        result[usize::from(lane)] = if negate {
            -component_value
        } else {
            component_value
        };
    }
    result
}

fn lanewise(a: [f32; 4], b: [f32; 4], operation: impl Fn(f32, f32) -> f32) -> [f32; 4] {
    [
        operation(a[0], b[0]),
        operation(a[1], b[1]),
        operation(a[2], b[2]),
        operation(a[3], b[3]),
    ]
}

/// PICA multiplication: a zero operand gives zero even against infinity or NaN.
fn pica_mul(a: f32, b: f32) -> f32 {
    if a == 0.0 || b == 0.0 { 0.0 } else { a * b }
}

fn dot(a: [f32; 4], b: [f32; 4], lanes: usize) -> f32 {
    (0..lanes).fold(0.0, |sum, lane| sum + pica_mul(a[lane], b[lane]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{
        REG_VSH_CODETRANSFER_DATA0, REG_VSH_CODETRANSFER_INDEX, REG_VSH_FLOATUNIFORM_DATA0,
        REG_VSH_FLOATUNIFORM_INDEX, REG_VSH_OPDESCS_DATA0, REG_VSH_OPDESCS_INDEX,
    };

    // Register numbers used by the hand-assembled test programs.
    const V0: u32 = 0x00;
    const V1: u32 = 0x01;
    const R0: u32 = 0x10;
    const C0: u32 = 0x20;
    const O0: u32 = 0x00;
    const O1: u32 = 0x01;
    const R0_DEST: u32 = 0x10;

    const IDENTITY: u32 = 0x1B; // x, y, z, w
    const REVERSE: u32 = 0xE4; // w, z, y, x
    const END: u32 = 0x22 << 26;
    const NOP: u32 = 0x21 << 26;

    /// Operand descriptor: mask bit 3 = x ... bit 0 = w, per-source negate/swizzle.
    fn descriptor(mask: u32, sources: [(bool, u32); 3]) -> u32 {
        mask | (u32::from(sources[0].0) << 4)
            | (sources[0].1 << 5)
            | (u32::from(sources[1].0) << 13)
            | (sources[1].1 << 14)
            | (u32::from(sources[2].0) << 22)
            | (sources[2].1 << 23)
    }

    fn plain_descriptor(mask: u32) -> u32 {
        descriptor(
            mask,
            [(false, IDENTITY), (false, IDENTITY), (false, IDENTITY)],
        )
    }

    fn format1(opcode: u32, destination: u32, source1: u32, source2: u32) -> u32 {
        (opcode << 26) | (destination << 21) | (source1 << 12) | (source2 << 7)
    }

    fn format1_unary(opcode: u32, destination: u32, source1: u32) -> u32 {
        (opcode << 26) | (destination << 21) | (source1 << 12)
    }

    fn format1_inverted(opcode: u32, destination: u32, source1: u32, source2: u32) -> u32 {
        (opcode << 26) | (destination << 21) | (source1 << 14) | (source2 << 7)
    }

    fn mad(destination: u32, source1: u32, source2: u32, source3: u32) -> u32 {
        (7 << 29) | (destination << 24) | (source1 << 17) | (source2 << 10) | (source3 << 5)
    }

    fn mad_inverted(destination: u32, source1: u32, source2: u32, source3: u32) -> u32 {
        (6 << 29) | (destination << 24) | (source1 << 17) | (source2 << 12) | (source3 << 5)
    }

    fn inputs(v0: [f32; 4], v1: [f32; 4]) -> [[f32; 4]; SHADER_INPUT_REGISTERS] {
        let mut registers = [[0.0; 4]; SHADER_INPUT_REGISTERS];
        registers[0] = v0;
        registers[1] = v1;
        registers
    }

    /// Runs one instruction followed by END; returns o0.
    fn run_one(word: u32, descriptor_word: u32, v0: [f32; 4], v1: [f32; 4]) -> [f32; 4] {
        let program =
            PicaVertexShaderProgram::from_words(&[word, END], &[descriptor_word], &[[0.0; 4]; 96]);
        program.run(0, &inputs(v0, v1)).unwrap().outputs[0]
    }

    #[test]
    fn add_sums_lanes_and_mask_preserves_disabled_lanes() {
        // Full mask first.
        let all = run_one(
            format1(0x00, O0, V0, V1),
            plain_descriptor(0b1111),
            [1.0, 2.0, 3.0, 4.0],
            [10.0, 20.0, 30.0, 40.0],
        );
        assert_eq!(all, [11.0, 22.0, 33.0, 44.0]);

        // Only x and w written; y and z stay at the initial zero of o0.
        let masked = run_one(
            format1(0x00, O0, V0, V1),
            plain_descriptor(0b1001),
            [1.0, 2.0, 3.0, 4.0],
            [10.0, 20.0, 30.0, 40.0],
        );
        assert_eq!(masked, [11.0, 0.0, 0.0, 44.0]);
    }

    #[test]
    fn mov_applies_swizzle_then_negation() {
        let word = format1_unary(0x13, O0, V0);
        let reversed_negated = descriptor(
            0b1111,
            [(true, REVERSE), (false, IDENTITY), (false, IDENTITY)],
        );
        assert_eq!(
            run_one(word, reversed_negated, [1.0, 2.0, 3.0, 4.0], [0.0; 4]),
            [-4.0, -3.0, -2.0, -1.0]
        );
    }

    #[test]
    fn mul_treats_zero_as_absorbing_even_against_infinity_and_nan() {
        let result = run_one(
            format1(0x08, O0, V0, V1),
            plain_descriptor(0b1111),
            [0.0, 2.0, f32::INFINITY, 3.0],
            [f32::INFINITY, 0.5, 0.0, f32::NAN],
        );
        assert_eq!(result[0], 0.0);
        assert_eq!(result[1], 1.0);
        assert_eq!(result[2], 0.0);
        assert!(result[3].is_nan());
    }

    #[test]
    fn dot_products_broadcast_to_every_enabled_lane() {
        let a = [1.0, 2.0, 3.0, 4.0];
        let b = [5.0, 6.0, 7.0, 8.0];
        let mask = plain_descriptor(0b1111);
        assert_eq!(run_one(format1(0x01, O0, V0, V1), mask, a, b), [38.0; 4]);
        assert_eq!(run_one(format1(0x02, O0, V0, V1), mask, a, b), [70.0; 4]);
        // DPH: a.w is replaced by 1.0, so 5 + 12 + 21 + 8 = 46.
        assert_eq!(run_one(format1(0x03, O0, V0, V1), mask, a, b), [46.0; 4]);
        // DPHI uses the inverted field layout but the same operation.
        assert_eq!(
            run_one(format1_inverted(0x18, O0, V0, V1), mask, a, b),
            [46.0; 4]
        );
    }

    #[test]
    fn set_on_less_than_and_greater_equal_produce_one_or_zero() {
        let a = [1.0, 5.0, 3.0, f32::NAN];
        let b = [2.0, 5.0, 1.0, 0.0];
        let mask = plain_descriptor(0b1111);
        assert_eq!(
            run_one(format1(0x0A, O0, V0, V1), mask, a, b),
            [1.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(
            run_one(format1(0x09, O0, V0, V1), mask, a, b),
            [0.0, 1.0, 1.0, 0.0]
        );
        assert_eq!(
            run_one(format1_inverted(0x1B, O0, V0, V1), mask, a, b),
            [1.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(
            run_one(format1_inverted(0x1A, O0, V0, V1), mask, a, b),
            [0.0, 1.0, 1.0, 0.0]
        );
    }

    #[test]
    fn max_min_and_floor_work_per_lane() {
        let a = [1.0, 5.0, -3.0, 2.0];
        let b = [2.0, 4.0, -4.0, 2.0];
        let mask = plain_descriptor(0b1111);
        assert_eq!(
            run_one(format1(0x0C, O0, V0, V1), mask, a, b),
            [2.0, 5.0, -3.0, 2.0]
        );
        assert_eq!(
            run_one(format1(0x0D, O0, V0, V1), mask, a, b),
            [1.0, 4.0, -4.0, 2.0]
        );
        assert_eq!(
            run_one(
                format1_unary(0x0B, O0, V0),
                mask,
                [1.5, -1.5, 2.0, 0.25],
                [0.0; 4]
            ),
            [1.0, -2.0, 2.0, 0.0]
        );
    }

    #[test]
    fn reciprocal_and_inverse_square_root_use_the_first_swizzled_lane() {
        let mask = plain_descriptor(0b1111);
        assert_eq!(
            run_one(
                format1_unary(0x0E, O0, V0),
                mask,
                [4.0, 9.0, 9.0, 9.0],
                [0.0; 4]
            ),
            [0.25; 4]
        );
        assert_eq!(
            run_one(
                format1_unary(0x0F, O0, V0),
                mask,
                [16.0, 9.0, 9.0, 9.0],
                [0.0; 4]
            ),
            [0.25; 4]
        );
        // Broadcasting y into the x slot of the swizzle picks the y value.
        let pick_y = descriptor(
            0b1111,
            [(false, 0x55), (false, IDENTITY), (false, IDENTITY)],
        );
        assert_eq!(
            run_one(
                format1_unary(0x0E, O0, V0),
                pick_y,
                [9.0, 2.0, 9.0, 9.0],
                [0.0; 4]
            ),
            [0.5; 4]
        );
        assert!(
            run_one(
                format1_unary(0x0E, O0, V0),
                mask,
                [0.0, 0.0, 0.0, 0.0],
                [0.0; 4]
            )[0]
            .is_infinite()
        );
    }

    #[test]
    fn ex2_and_lg2_broadcast_the_first_swizzled_source_lane() {
        let mask = plain_descriptor(0b1111);
        assert_eq!(
            run_one(
                format1_unary(0x05, O0, V0),
                mask,
                [3.0, 0.0, 0.0, 0.0],
                [0.0; 4]
            ),
            [8.0; 4]
        );
        assert_eq!(
            run_one(
                format1_unary(0x05, O0, V0),
                mask,
                [-2.0, 0.0, 0.0, 0.0],
                [0.0; 4]
            ),
            [0.25; 4]
        );
        assert_eq!(
            run_one(
                format1_unary(0x06, O0, V0),
                mask,
                [8.0, 0.0, 0.0, 0.0],
                [0.0; 4]
            ),
            [3.0; 4]
        );

        let pick_y = descriptor(
            0b1111,
            [(false, 0x55), (false, IDENTITY), (false, IDENTITY)],
        );
        assert_eq!(
            run_one(
                format1_unary(0x06, O0, V0),
                pick_y,
                [9.0, 2.0, 9.0, 9.0],
                [0.0; 4]
            ),
            [1.0; 4]
        );
    }

    #[test]
    fn ex2_and_lg2_respect_destination_masks_and_ieee_domain_results() {
        let only_y_and_w = plain_descriptor(0b0101);
        let ex2 = run_one(
            format1_unary(0x05, O0, V0),
            only_y_and_w,
            [2.0, 0.0, 0.0, 0.0],
            [0.0; 4],
        );
        assert_eq!(ex2, [0.0, 4.0, 0.0, 4.0]);

        let log_zero = run_one(
            format1_unary(0x06, O0, V0),
            plain_descriptor(0b1111),
            [0.0, 0.0, 0.0, 0.0],
            [0.0; 4],
        );
        assert!(
            log_zero
                .iter()
                .all(|value| value.is_infinite() && value.is_sign_negative())
        );

        let log_negative = run_one(
            format1_unary(0x06, O0, V0),
            plain_descriptor(0b1111),
            [-1.0, 0.0, 0.0, 0.0],
            [0.0; 4],
        );
        assert!(log_negative.iter().all(|value| value.is_nan()));
    }

    #[test]
    fn multiply_add_forms_read_constants_through_their_wide_source_field() {
        let mut uniforms = [[0.0; 4]; 96];
        uniforms[0] = [2.0, 2.0, 2.0, 2.0];
        let program = PicaVertexShaderProgram::from_words(
            &[
                // o0 = v0 * c0 + v1 (MAD: second source may be a constant)
                mad(O0, V0, C0, V1),
                // o1 = v0 * v1 + c0 (MADI: third source may be a constant)
                mad_inverted(O1, V0, V1, C0),
                END,
            ],
            &[plain_descriptor(0b1111)],
            &uniforms,
        );
        let result = program
            .run(0, &inputs([1.0, 2.0, 3.0, 4.0], [10.0, 10.0, 10.0, 10.0]))
            .unwrap();
        assert_eq!(result.outputs[0], [12.0, 14.0, 16.0, 18.0]);
        assert_eq!(result.outputs[1], [12.0, 22.0, 32.0, 42.0]);
        assert_eq!(result.executed_instructions, 3);
    }

    #[test]
    fn temporaries_carry_results_between_instructions() {
        let program = PicaVertexShaderProgram::from_words(
            &[
                format1(0x00, R0_DEST, V0, V1), // r0 = v0 + v1
                format1_unary(0x13, O0, R0),    // o0 = r0
                NOP,
                format1(0x08, O1, R0, R0), // o1 = r0 * r0
                END,
            ],
            &[plain_descriptor(0b1111)],
            &[],
        );
        let result = program
            .run(0, &inputs([1.0, 2.0, 3.0, 4.0], [1.0, 1.0, 1.0, 1.0]))
            .unwrap();
        assert_eq!(result.outputs[0], [2.0, 3.0, 4.0, 5.0]);
        assert_eq!(result.outputs[1], [4.0, 9.0, 16.0, 25.0]);
        assert_eq!(result.executed_instructions, 5);
    }

    #[test]
    fn execution_can_start_at_a_nonzero_entry_point() {
        let program = PicaVertexShaderProgram::from_words(
            &[
                format1_unary(0x13, O0, V0),
                END,
                format1_unary(0x13, O0, V1),
                END,
            ],
            &[plain_descriptor(0b1111)],
            &[],
        );
        let input = inputs([1.0; 4], [7.0; 4]);
        assert_eq!(program.run(0, &input).unwrap().outputs[0], [1.0; 4]);
        assert_eq!(program.run(2, &input).unwrap().outputs[0], [7.0; 4]);
    }

    #[test]
    fn unsupported_and_incomplete_programs_report_precise_errors() {
        let descriptors = [plain_descriptor(0b1111)];
        let input = inputs([0.0; 4], [0.0; 4]);

        // CALL (0x24) is flow control and not part of the supported subset.
        let call = 0x24u32 << 26;
        let program = PicaVertexShaderProgram::from_words(&[call, END], &descriptors, &[]);
        assert_eq!(
            program.run(0, &input),
            Err(PicaShaderExecError::UnsupportedOpcode {
                index: 0,
                word: call
            })
        );

        // No END: execution runs off the end of the uploaded code.
        let program =
            PicaVertexShaderProgram::from_words(&[format1_unary(0x13, O0, V0)], &descriptors, &[]);
        assert_eq!(
            program.run(0, &input),
            Err(PicaShaderExecError::InstructionNotUploaded { index: 1 })
        );

        // Descriptor 5 was never uploaded.
        let word = format1_unary(0x13, O0, V0) | 5;
        let program = PicaVertexShaderProgram::from_words(&[word, END], &descriptors, &[]);
        assert_eq!(
            program.run(0, &input),
            Err(PicaShaderExecError::DescriptorNotUploaded {
                index: 0,
                descriptor: 5
            })
        );

        // Address-register indexing on source 1 (bits 19..20) is rejected.
        let word = format1_unary(0x13, O0, V0) | (1 << 19);
        let program = PicaVertexShaderProgram::from_words(&[word, END], &descriptors, &[]);
        assert_eq!(
            program.run(0, &input),
            Err(PicaShaderExecError::AddressRegisterUnsupported { index: 0 })
        );
    }

    #[test]
    fn opcode_table_identifies_supported_words_only() {
        assert_eq!(
            PicaShaderOpcode::from_word(0x00 << 26),
            Some(PicaShaderOpcode::Add)
        );
        assert_eq!(
            PicaShaderOpcode::from_word(0x13 << 26),
            Some(PicaShaderOpcode::Mov)
        );
        assert_eq!(
            PicaShaderOpcode::from_word(0x38 << 26),
            Some(PicaShaderOpcode::Mad)
        );
        assert_eq!(
            PicaShaderOpcode::from_word(0x05 << 26),
            Some(PicaShaderOpcode::Ex2)
        );
        assert_eq!(
            PicaShaderOpcode::from_word(0x06 << 26),
            Some(PicaShaderOpcode::Lg2)
        );
        assert_eq!(
            PicaShaderOpcode::from_word(0x37 << 26),
            Some(PicaShaderOpcode::Madi)
        );
        assert_eq!(
            PicaShaderOpcode::from_word(0x30 << 26),
            Some(PicaShaderOpcode::Madi)
        );
        // MOVA, CMP, CALL, DST, LITP and other unimplemented opcodes stay rejected.
        for opcode in [0x12u32, 0x2E, 0x2F, 0x24, 0x04, 0x07, 0x19] {
            assert_eq!(
                PicaShaderOpcode::from_word(opcode << 26),
                None,
                "{opcode:#x}"
            );
        }
    }

    #[test]
    fn uploaded_program_runs_through_the_pica200_registers() {
        let mut gpu = Pica200::new();

        // Operand descriptor 0: identity swizzle, full mask.
        gpu.write_register(REG_VSH_OPDESCS_INDEX, 0).unwrap();
        gpu.write_register(REG_VSH_OPDESCS_DATA0, plain_descriptor(0b1111))
            .unwrap();

        // Float32 constant c0 = (2, 3, 4, 5); words arrive in w, z, y, x order.
        gpu.write_register(REG_VSH_FLOATUNIFORM_INDEX, 0x8000_0000)
            .unwrap();
        for value in [5.0f32, 4.0, 3.0, 2.0] {
            gpu.write_register(REG_VSH_FLOATUNIFORM_DATA0, value.to_bits())
                .unwrap();
        }

        // o0 = v0 * c0 + v1 ; END
        gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 0).unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, mad(O0, V0, C0, V1))
            .unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, END).unwrap();

        let program = gpu.vertex_shader_program();
        let result = program
            .run(0, &inputs([1.0, 2.0, 3.0, 4.0], [2.0, 2.0, 2.0, 2.0]))
            .unwrap();
        // (1*2+2, 2*3+2, 3*4+2, 4*5+2)
        assert_eq!(result.outputs[0], [4.0, 8.0, 14.0, 22.0]);
        assert_eq!(result.executed_instructions, 2);
    }

    #[test]
    fn snapshot_of_an_empty_gpu_reports_missing_code() {
        let program = Pica200::new().vertex_shader_program();
        assert_eq!(
            program.run(0, &inputs([0.0; 4], [0.0; 4])),
            Err(PicaShaderExecError::InstructionNotUploaded { index: 0 })
        );
    }
}
