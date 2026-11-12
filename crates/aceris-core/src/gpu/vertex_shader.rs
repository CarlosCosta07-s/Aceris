//! Typed decoding of documented PICA200 vertex-shader instruction fields.
//!
//! The PICA instruction set is partially reverse-engineered. This module only
//! extracts bit fields when the caller supplies the expected format; it does
//! not guess an instruction's format, validate opcode semantics or execute it.

/// Bitfield layout selected by the caller for one 32-bit shader word.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaVertexShaderInstructionFormat {
    Format1,
    Format1Inverted,
    Format1Unary,
    Format1Compare,
    Format2Flow,
    Format3BooleanFlow,
    Format4Emit,
    Format5MultiplyAdd,
    Format5InvertedMultiplyAdd,
}

/// Raw fields for register-operation instruction Format 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaShaderFormat1 {
    pub descriptor: u8,
    pub source2: u8,
    pub source1: u8,
    pub source1_address_index: u8,
    pub destination: u8,
    pub opcode: u8,
}

/// Raw fields for inverted register-operation instruction Format 1i.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaShaderFormat1Inverted {
    pub descriptor: u8,
    pub source2: u8,
    pub source1: u8,
    pub source2_address_index: u8,
    pub destination: u8,
    pub opcode: u8,
}

/// Raw fields for unary register-operation instruction Format 1u.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaShaderFormat1Unary {
    pub descriptor: u8,
    pub source1: u8,
    pub source1_address_index: u8,
    pub destination: u8,
    pub opcode: u8,
}

/// Raw fields for comparison instruction Format 1c.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaShaderFormat1Compare {
    pub descriptor: u8,
    pub source2: u8,
    pub source1: u8,
    pub source1_address_index: u8,
    pub compare_y: u8,
    pub compare_x: u8,
    pub opcode: u8,
}

/// Raw fields for flow-control instruction Format 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaShaderFormat2Flow {
    pub instruction_count: u8,
    pub destination_offset: u16,
    pub condition_operator: u8,
    pub reference_y: bool,
    pub reference_x: bool,
    pub opcode: u8,
}

/// Raw fields for boolean-constant flow-control instruction Format 3.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaShaderFormat3BooleanFlow {
    pub instruction_count: u8,
    pub destination_offset: u16,
    pub constant_id: u8,
    pub opcode: u8,
}

/// Raw fields for geometry-shader emit instruction Format 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaShaderFormat4Emit {
    pub winding: bool,
    pub primitive_emit: bool,
    pub vertex_id: u8,
    pub opcode: u8,
}

/// Raw fields for multiply-add instruction Format 5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaShaderFormat5MultiplyAdd {
    pub descriptor: u8,
    pub source3: u8,
    pub source2: u8,
    pub source1: u8,
    pub source2_address_index: u8,
    pub destination: u8,
    pub opcode: u8,
}

/// Raw fields for inverted multiply-add instruction Format 5i.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaShaderFormat5InvertedMultiplyAdd {
    pub descriptor: u8,
    pub source3: u8,
    pub source2: u8,
    pub source1: u8,
    pub source3_address_index: u8,
    pub destination: u8,
    pub opcode: u8,
}

/// One instruction word decoded according to an explicitly supplied format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaVertexShaderInstruction {
    Format1(PicaShaderFormat1),
    Format1Inverted(PicaShaderFormat1Inverted),
    Format1Unary(PicaShaderFormat1Unary),
    Format1Compare(PicaShaderFormat1Compare),
    Format2Flow(PicaShaderFormat2Flow),
    Format3BooleanFlow(PicaShaderFormat3BooleanFlow),
    Format4Emit(PicaShaderFormat4Emit),
    Format5MultiplyAdd(PicaShaderFormat5MultiplyAdd),
    Format5InvertedMultiplyAdd(PicaShaderFormat5InvertedMultiplyAdd),
}

const fn field(word: u32, shift: u32, width: u32) -> u32 {
    (word >> shift) & ((1u32 << width) - 1)
}

impl PicaVertexShaderInstruction {
    /// Decodes raw fields without assigning execution semantics to the opcode.
    ///
    /// The caller supplies the format because opcode-to-format identification
    /// has ambiguities and undocumented combinations in the available ISA notes.
    pub const fn decode(word: u32, format: PicaVertexShaderInstructionFormat) -> Self {
        match format {
            PicaVertexShaderInstructionFormat::Format1 => Self::Format1(PicaShaderFormat1 {
                descriptor: field(word, 0, 7) as u8,
                source2: field(word, 7, 5) as u8,
                source1: field(word, 12, 7) as u8,
                source1_address_index: field(word, 19, 2) as u8,
                destination: field(word, 21, 5) as u8,
                opcode: field(word, 26, 6) as u8,
            }),
            PicaVertexShaderInstructionFormat::Format1Inverted => {
                Self::Format1Inverted(PicaShaderFormat1Inverted {
                    descriptor: field(word, 0, 7) as u8,
                    source2: field(word, 7, 7) as u8,
                    source1: field(word, 14, 5) as u8,
                    source2_address_index: field(word, 19, 2) as u8,
                    destination: field(word, 21, 5) as u8,
                    opcode: field(word, 26, 6) as u8,
                })
            }
            PicaVertexShaderInstructionFormat::Format1Unary => {
                Self::Format1Unary(PicaShaderFormat1Unary {
                    descriptor: field(word, 0, 7) as u8,
                    source1: field(word, 12, 7) as u8,
                    source1_address_index: field(word, 19, 2) as u8,
                    destination: field(word, 21, 5) as u8,
                    opcode: field(word, 26, 6) as u8,
                })
            }
            PicaVertexShaderInstructionFormat::Format1Compare => {
                Self::Format1Compare(PicaShaderFormat1Compare {
                    descriptor: field(word, 0, 7) as u8,
                    source2: field(word, 7, 5) as u8,
                    source1: field(word, 12, 7) as u8,
                    source1_address_index: field(word, 19, 2) as u8,
                    compare_y: field(word, 21, 3) as u8,
                    compare_x: field(word, 24, 3) as u8,
                    opcode: field(word, 27, 5) as u8,
                })
            }
            PicaVertexShaderInstructionFormat::Format2Flow => {
                Self::Format2Flow(PicaShaderFormat2Flow {
                    instruction_count: field(word, 0, 8) as u8,
                    destination_offset: field(word, 10, 12) as u16,
                    condition_operator: field(word, 22, 2) as u8,
                    reference_y: field(word, 24, 1) != 0,
                    reference_x: field(word, 25, 1) != 0,
                    opcode: field(word, 26, 6) as u8,
                })
            }
            PicaVertexShaderInstructionFormat::Format3BooleanFlow => {
                Self::Format3BooleanFlow(PicaShaderFormat3BooleanFlow {
                    instruction_count: field(word, 0, 8) as u8,
                    destination_offset: field(word, 10, 12) as u16,
                    constant_id: field(word, 22, 4) as u8,
                    opcode: field(word, 26, 6) as u8,
                })
            }
            PicaVertexShaderInstructionFormat::Format4Emit => {
                Self::Format4Emit(PicaShaderFormat4Emit {
                    winding: field(word, 22, 1) != 0,
                    primitive_emit: field(word, 23, 1) != 0,
                    vertex_id: field(word, 24, 2) as u8,
                    opcode: field(word, 26, 6) as u8,
                })
            }
            PicaVertexShaderInstructionFormat::Format5MultiplyAdd => {
                Self::Format5MultiplyAdd(PicaShaderFormat5MultiplyAdd {
                    descriptor: field(word, 0, 5) as u8,
                    source3: field(word, 5, 5) as u8,
                    source2: field(word, 10, 7) as u8,
                    source1: field(word, 17, 5) as u8,
                    source2_address_index: field(word, 22, 2) as u8,
                    destination: field(word, 24, 5) as u8,
                    opcode: field(word, 29, 3) as u8,
                })
            }
            PicaVertexShaderInstructionFormat::Format5InvertedMultiplyAdd => {
                Self::Format5InvertedMultiplyAdd(PicaShaderFormat5InvertedMultiplyAdd {
                    descriptor: field(word, 0, 5) as u8,
                    source3: field(word, 5, 7) as u8,
                    source2: field(word, 12, 5) as u8,
                    source1: field(word, 17, 5) as u8,
                    source3_address_index: field(word, 22, 2) as u8,
                    destination: field(word, 24, 5) as u8,
                    opcode: field(word, 29, 3) as u8,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn bits(value: u32, shift: u32) -> u32 {
        value << shift
    }

    #[test]
    fn decodes_format1_register_operation_fields() {
        let word = bits(0x55, 0)
            | bits(0x12, 7)
            | bits(0x65, 12)
            | bits(2, 19)
            | bits(0x1A, 21)
            | bits(0x13, 26);
        assert_eq!(
            PicaVertexShaderInstruction::decode(word, PicaVertexShaderInstructionFormat::Format1,),
            PicaVertexShaderInstruction::Format1(PicaShaderFormat1 {
                descriptor: 0x55,
                source2: 0x12,
                source1: 0x65,
                source1_address_index: 2,
                destination: 0x1A,
                opcode: 0x13,
            })
        );
    }

    #[test]
    fn decodes_inverted_and_unary_register_formats() {
        let inverted = bits(0x21, 0)
            | bits(0x65, 7)
            | bits(0x12, 14)
            | bits(3, 19)
            | bits(0x0A, 21)
            | bits(0x18, 26);
        assert_eq!(
            PicaVertexShaderInstruction::decode(
                inverted,
                PicaVertexShaderInstructionFormat::Format1Inverted,
            ),
            PicaVertexShaderInstruction::Format1Inverted(PicaShaderFormat1Inverted {
                descriptor: 0x21,
                source2: 0x65,
                source1: 0x12,
                source2_address_index: 3,
                destination: 0x0A,
                opcode: 0x18,
            })
        );

        let unary = bits(0x32, 0) | bits(0x65, 12) | bits(1, 19) | bits(0x1F, 21) | bits(0x0E, 26);
        assert_eq!(
            PicaVertexShaderInstruction::decode(
                unary,
                PicaVertexShaderInstructionFormat::Format1Unary,
            ),
            PicaVertexShaderInstruction::Format1Unary(PicaShaderFormat1Unary {
                descriptor: 0x32,
                source1: 0x65,
                source1_address_index: 1,
                destination: 0x1F,
                opcode: 0x0E,
            })
        );
    }

    #[test]
    fn decodes_compare_and_flow_control_formats() {
        let compare = bits(0x5A, 0)
            | bits(0x12, 7)
            | bits(0x65, 12)
            | bits(2, 19)
            | bits(5, 21)
            | bits(3, 24)
            | bits(0x16, 27);
        assert_eq!(
            PicaVertexShaderInstruction::decode(
                compare,
                PicaVertexShaderInstructionFormat::Format1Compare,
            ),
            PicaVertexShaderInstruction::Format1Compare(PicaShaderFormat1Compare {
                descriptor: 0x5A,
                source2: 0x12,
                source1: 0x65,
                source1_address_index: 2,
                compare_y: 5,
                compare_x: 3,
                opcode: 0x16,
            })
        );

        let flow = bits(0xA5, 0)
            | bits(0xABC, 10)
            | bits(2, 22)
            | bits(1, 24)
            | bits(1, 25)
            | bits(0x24, 26);
        assert_eq!(
            PicaVertexShaderInstruction::decode(
                flow,
                PicaVertexShaderInstructionFormat::Format2Flow,
            ),
            PicaVertexShaderInstruction::Format2Flow(PicaShaderFormat2Flow {
                instruction_count: 0xA5,
                destination_offset: 0xABC,
                condition_operator: 2,
                reference_y: true,
                reference_x: true,
                opcode: 0x24,
            })
        );

        let bool_flow = bits(0x5C, 0) | bits(0x321, 10) | bits(0xA, 22) | bits(0x27, 26);
        assert_eq!(
            PicaVertexShaderInstruction::decode(
                bool_flow,
                PicaVertexShaderInstructionFormat::Format3BooleanFlow,
            ),
            PicaVertexShaderInstruction::Format3BooleanFlow(PicaShaderFormat3BooleanFlow {
                instruction_count: 0x5C,
                destination_offset: 0x321,
                constant_id: 0xA,
                opcode: 0x27,
            })
        );
    }

    #[test]
    fn decodes_emit_and_multiply_add_formats() {
        let emit = bits(1, 22) | bits(1, 23) | bits(2, 24) | bits(0x2B, 26);
        assert_eq!(
            PicaVertexShaderInstruction::decode(
                emit,
                PicaVertexShaderInstructionFormat::Format4Emit,
            ),
            PicaVertexShaderInstruction::Format4Emit(PicaShaderFormat4Emit {
                winding: true,
                primitive_emit: true,
                vertex_id: 2,
                opcode: 0x2B,
            })
        );

        let mad = bits(0x1B, 0)
            | bits(0x12, 5)
            | bits(0x65, 10)
            | bits(0x14, 17)
            | bits(2, 22)
            | bits(0x1A, 24)
            | bits(0x06, 29);
        assert_eq!(
            PicaVertexShaderInstruction::decode(
                mad,
                PicaVertexShaderInstructionFormat::Format5MultiplyAdd,
            ),
            PicaVertexShaderInstruction::Format5MultiplyAdd(PicaShaderFormat5MultiplyAdd {
                descriptor: 0x1B,
                source3: 0x12,
                source2: 0x65,
                source1: 0x14,
                source2_address_index: 2,
                destination: 0x1A,
                opcode: 0x06,
            })
        );

        let madi = bits(0x0B, 0)
            | bits(0x65, 5)
            | bits(0x12, 12)
            | bits(0x14, 17)
            | bits(1, 22)
            | bits(0x1A, 24)
            | bits(0x03, 29);
        assert_eq!(
            PicaVertexShaderInstruction::decode(
                madi,
                PicaVertexShaderInstructionFormat::Format5InvertedMultiplyAdd,
            ),
            PicaVertexShaderInstruction::Format5InvertedMultiplyAdd(
                PicaShaderFormat5InvertedMultiplyAdd {
                    descriptor: 0x0B,
                    source3: 0x65,
                    source2: 0x12,
                    source1: 0x14,
                    source3_address_index: 1,
                    destination: 0x1A,
                    opcode: 0x03,
                }
            )
        );
    }
}
