//! PICA200 framebuffer color-operation stage.
//!
//! This module models the color operation after TEV: alpha/stencil tests,
//! fixed-function blending or per-channel logic operations. It accepts and
//! returns normalized/8-bit RGBA values; framebuffer format conversion and
//! physical depth/stencil storage are handled by their own GPU modules.

use super::pica200::Pica200;

/// GPUREG_COLOR_OPERATION.
pub const REG_COLOR_OPERATION: u16 = 0x100;
/// GPUREG_BLEND_FUNC.
pub const REG_BLEND_FUNC: u16 = 0x101;
/// GPUREG_LOGIC_OP.
pub const REG_LOGIC_OP: u16 = 0x102;
/// GPUREG_BLEND_COLOR.
pub const REG_BLEND_COLOR: u16 = 0x103;
/// GPUREG_FRAGOP_ALPHA_TEST.
pub const REG_FRAGOP_ALPHA_TEST: u16 = 0x104;
/// GPUREG_STENCIL_TEST.
pub const REG_STENCIL_TEST: u16 = 0x105;
/// GPUREG_STENCIL_OP.
pub const REG_STENCIL_OP: u16 = 0x106;

/// Alpha-test comparison configured by GPUREG_FRAGOP_ALPHA_TEST.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaAlphaTestFunction {
    Never,
    Always,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl PicaAlphaTestFunction {
    fn decode(value: u32) -> Self {
        match value & 0x7 {
            0 => Self::Never,
            1 => Self::Always,
            2 => Self::Equal,
            3 => Self::NotEqual,
            4 => Self::Less,
            5 => Self::LessEqual,
            6 => Self::Greater,
            _ => Self::GreaterEqual,
        }
    }

    fn compare(self, alpha: u8, reference: u8) -> bool {
        match self {
            Self::Never => false,
            Self::Always => true,
            Self::Equal => alpha == reference,
            Self::NotEqual => alpha != reference,
            Self::Less => alpha < reference,
            Self::LessEqual => alpha <= reference,
            Self::Greater => alpha > reference,
            Self::GreaterEqual => alpha >= reference,
        }
    }
}

/// Comparison function configured by GPUREG_STENCIL_TEST.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaStencilFunction {
    Never,
    Always,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
}

impl PicaStencilFunction {
    fn decode(value: u32) -> Self {
        match value & 0x7 {
            0 => Self::Never,
            1 => Self::Always,
            2 => Self::Equal,
            3 => Self::NotEqual,
            4 => Self::Less,
            5 => Self::LessEqual,
            6 => Self::Greater,
            _ => Self::GreaterEqual,
        }
    }

    fn compare(self, reference: u8, value: u8) -> bool {
        match self {
            Self::Never => false,
            Self::Always => true,
            Self::Equal => reference == value,
            Self::NotEqual => reference != value,
            Self::Less => reference < value,
            Self::LessEqual => reference <= value,
            Self::Greater => reference > value,
            Self::GreaterEqual => reference >= value,
        }
    }
}

/// Stencil actions configured by GPUREG_STENCIL_OP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaStencilOperation {
    Keep,
    Zero,
    Replace,
    IncrementClamp,
    DecrementClamp,
    Invert,
    IncrementWrap,
    DecrementWrap,
}

impl PicaStencilOperation {
    fn decode(value: u32) -> Self {
        match value & 0x7 {
            0 => Self::Keep,
            1 => Self::Zero,
            2 => Self::Replace,
            3 => Self::IncrementClamp,
            4 => Self::DecrementClamp,
            5 => Self::Invert,
            6 => Self::IncrementWrap,
            _ => Self::DecrementWrap,
        }
    }

    fn apply(self, current: u8, reference: u8) -> u8 {
        match self {
            Self::Keep => current,
            Self::Zero => 0,
            Self::Replace => reference,
            Self::IncrementClamp => current.saturating_add(1),
            Self::DecrementClamp => current.saturating_sub(1),
            Self::Invert => !current,
            Self::IncrementWrap => current.wrapping_add(1),
            Self::DecrementWrap => current.wrapping_sub(1),
        }
    }
}

/// Guest stencil-test state captured at the PICA draw trigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaStencilState {
    pub enabled: bool,
    pub function: PicaStencilFunction,
    pub write_mask: u8,
    pub reference: u8,
    pub compare_mask: u8,
    pub stencil_fail: PicaStencilOperation,
    pub depth_fail: PicaStencilOperation,
    pub depth_pass: PicaStencilOperation,
}

impl PicaStencilState {
    /// Compares the masked reference value with the masked stored stencil value.
    pub fn test_passes(self, stored: u8) -> bool {
        if !self.enabled {
            return true;
        }
        self.function.compare(
            self.reference & self.compare_mask,
            stored & self.compare_mask,
        )
    }

    /// Applies an action and merges it through the guest's stencil write mask.
    pub fn apply(self, action: PicaStencilOperation, stored: u8) -> u8 {
        let updated = action.apply(stored, self.reference);
        (stored & !self.write_mask) | (updated & self.write_mask)
    }
}

/// Supported fixed-function fragment operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaFragmentOperation {
    Default,
}

/// How the color operation combines source and destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaColorOperationMode {
    Logic,
    Blend,
}

/// Blend equations configured independently for RGB and alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaBlendEquation {
    Add,
    Subtract,
    ReverseSubtract,
    Minimum,
    Maximum,
}

impl PicaBlendEquation {
    fn decode(channel: PicaBlendChannel, value: u32) -> Result<Self, PicaFragmentOutputError> {
        match value {
            0 | 5 | 6 | 7 => Ok(Self::Add),
            1 => Ok(Self::Subtract),
            2 => Ok(Self::ReverseSubtract),
            3 => Ok(Self::Minimum),
            4 => Ok(Self::Maximum),
            other => Err(PicaFragmentOutputError::UnsupportedBlendEquation {
                channel,
                value: other as u8,
            }),
        }
    }

    fn apply(
        self,
        source: f32,
        destination: f32,
        source_factor: f32,
        destination_factor: f32,
    ) -> f32 {
        match self {
            Self::Add => source * source_factor + destination * destination_factor,
            Self::Subtract => source * source_factor - destination * destination_factor,
            Self::ReverseSubtract => destination * destination_factor - source * source_factor,
            // The minimum/maximum equations operate on the un-factored colors.
            Self::Minimum => source.min(destination),
            Self::Maximum => source.max(destination),
        }
    }
}

/// Which blend equation failed to decode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaBlendChannel {
    Rgb,
    Alpha,
}

/// Blend factors from GPUREG_BLEND_FUNC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaBlendFactor {
    Zero,
    One,
    SourceColor,
    OneMinusSourceColor,
    DestinationColor,
    OneMinusDestinationColor,
    SourceAlpha,
    OneMinusSourceAlpha,
    DestinationAlpha,
    OneMinusDestinationAlpha,
    ConstantColor,
    OneMinusConstantColor,
    ConstantAlpha,
    OneMinusConstantAlpha,
    SourceAlphaSaturate,
}

impl PicaBlendFactor {
    fn decode(channel: PicaBlendChannel, value: u32) -> Result<Self, PicaFragmentOutputError> {
        match value {
            0 => Ok(Self::Zero),
            1 => Ok(Self::One),
            2 => Ok(Self::SourceColor),
            3 => Ok(Self::OneMinusSourceColor),
            4 => Ok(Self::DestinationColor),
            5 => Ok(Self::OneMinusDestinationColor),
            6 => Ok(Self::SourceAlpha),
            7 => Ok(Self::OneMinusSourceAlpha),
            8 => Ok(Self::DestinationAlpha),
            9 => Ok(Self::OneMinusDestinationAlpha),
            10 => Ok(Self::ConstantColor),
            11 => Ok(Self::OneMinusConstantColor),
            12 => Ok(Self::ConstantAlpha),
            13 => Ok(Self::OneMinusConstantAlpha),
            14 => Ok(Self::SourceAlphaSaturate),
            other => Err(PicaFragmentOutputError::UnsupportedBlendFactor {
                channel,
                value: other as u8,
            }),
        }
    }

    fn evaluate(
        self,
        channel: usize,
        source: [f32; 4],
        destination: [f32; 4],
        constant: [f32; 4],
    ) -> f32 {
        match self {
            Self::Zero => 0.0,
            Self::One => 1.0,
            Self::SourceColor => source[channel],
            Self::OneMinusSourceColor => 1.0 - source[channel],
            Self::DestinationColor => destination[channel],
            Self::OneMinusDestinationColor => 1.0 - destination[channel],
            Self::SourceAlpha => source[3],
            Self::OneMinusSourceAlpha => 1.0 - source[3],
            Self::DestinationAlpha => destination[3],
            Self::OneMinusDestinationAlpha => 1.0 - destination[3],
            Self::ConstantColor => constant[channel],
            Self::OneMinusConstantColor => 1.0 - constant[channel],
            Self::ConstantAlpha => constant[3],
            Self::OneMinusConstantAlpha => 1.0 - constant[3],
            Self::SourceAlphaSaturate if channel == 3 => 1.0,
            Self::SourceAlphaSaturate => source[3].min(1.0 - destination[3]),
        }
    }
}

/// Logic operations configured by GPUREG_LOGIC_OP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaLogicOperation {
    Clear,
    And,
    ReverseAnd,
    Copy,
    Set,
    InvertedCopy,
    Noop,
    Invert,
    Nand,
    Or,
    Nor,
    Xor,
    Equivalent,
    InvertedAnd,
    ReverseOr,
    InvertedOr,
}

impl PicaLogicOperation {
    fn decode(value: u32) -> Result<Self, PicaFragmentOutputError> {
        match value {
            0 => Ok(Self::Clear),
            1 => Ok(Self::And),
            2 => Ok(Self::ReverseAnd),
            3 => Ok(Self::Copy),
            4 => Ok(Self::Set),
            5 => Ok(Self::InvertedCopy),
            6 => Ok(Self::Noop),
            7 => Ok(Self::Invert),
            8 => Ok(Self::Nand),
            9 => Ok(Self::Or),
            10 => Ok(Self::Nor),
            11 => Ok(Self::Xor),
            12 => Ok(Self::Equivalent),
            13 => Ok(Self::InvertedAnd),
            14 => Ok(Self::ReverseOr),
            15 => Ok(Self::InvertedOr),
            other => Err(PicaFragmentOutputError::UnsupportedLogicOperation { value: other as u8 }),
        }
    }

    fn apply(self, source: u8, destination: u8) -> u8 {
        match self {
            Self::Clear => 0,
            Self::And => source & destination,
            Self::ReverseAnd => source & !destination,
            Self::Copy => source,
            Self::Set => u8::MAX,
            Self::InvertedCopy => !source,
            Self::Noop => destination,
            Self::Invert => !destination,
            Self::Nand => !(source & destination),
            Self::Or => source | destination,
            Self::Nor => !(source | destination),
            Self::Xor => source ^ destination,
            Self::Equivalent => !(source ^ destination),
            Self::InvertedAnd => !source & destination,
            Self::ReverseOr => source | !destination,
            Self::InvertedOr => !source | destination,
        }
    }
}

/// Fully decoded PICA200 color-operation state for a draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaFragmentOutputState {
    pub operation: PicaFragmentOperation,
    pub mode: PicaColorOperationMode,
    pub rgb_equation: PicaBlendEquation,
    pub alpha_equation: PicaBlendEquation,
    pub rgb_source_factor: PicaBlendFactor,
    pub rgb_destination_factor: PicaBlendFactor,
    pub alpha_source_factor: PicaBlendFactor,
    pub alpha_destination_factor: PicaBlendFactor,
    pub logic_operation: PicaLogicOperation,
    pub constant_color: [u8; 4],
    pub alpha_test_enabled: bool,
    pub alpha_test_function: PicaAlphaTestFunction,
    pub alpha_test_reference: u8,
    pub stencil: PicaStencilState,
}

impl PicaFragmentOutputState {
    /// Decodes the PICA color operation, blend, logic-op and blend-color registers.
    pub fn from_pica(pica: &Pica200) -> Result<Self, PicaFragmentOutputError> {
        let color_operation = pica.register(REG_COLOR_OPERATION).unwrap_or(0);
        let operation = match color_operation & 0x3 {
            0 => PicaFragmentOperation::Default,
            value => {
                return Err(PicaFragmentOutputError::UnsupportedFragmentOperation {
                    value: value as u8,
                });
            }
        };
        let mode = if color_operation & (1 << 8) != 0 {
            PicaColorOperationMode::Blend
        } else {
            PicaColorOperationMode::Logic
        };

        let blend_color = pica.register(REG_BLEND_COLOR).unwrap_or(0);
        let alpha_test = pica.register(REG_FRAGOP_ALPHA_TEST).unwrap_or(0);
        let alpha_test_enabled = alpha_test & 1 != 0;
        let stencil_test = pica.register(REG_STENCIL_TEST).unwrap_or(0);
        let stencil_operation = pica.register(REG_STENCIL_OP).unwrap_or(0);
        let (
            rgb_equation,
            alpha_equation,
            rgb_source_factor,
            rgb_destination_factor,
            alpha_source_factor,
            alpha_destination_factor,
            logic_operation,
        ) = match mode {
            PicaColorOperationMode::Logic => (
                PicaBlendEquation::Add,
                PicaBlendEquation::Add,
                PicaBlendFactor::One,
                PicaBlendFactor::Zero,
                PicaBlendFactor::One,
                PicaBlendFactor::Zero,
                PicaLogicOperation::decode(pica.register(REG_LOGIC_OP).unwrap_or(0) & 0xF)?,
            ),
            PicaColorOperationMode::Blend => {
                let blend = pica.register(REG_BLEND_FUNC).unwrap_or(0);
                (
                    PicaBlendEquation::decode(PicaBlendChannel::Rgb, blend & 0x7)?,
                    PicaBlendEquation::decode(PicaBlendChannel::Alpha, (blend >> 8) & 0x7)?,
                    PicaBlendFactor::decode(PicaBlendChannel::Rgb, (blend >> 16) & 0xF)?,
                    PicaBlendFactor::decode(PicaBlendChannel::Rgb, (blend >> 20) & 0xF)?,
                    PicaBlendFactor::decode(PicaBlendChannel::Alpha, (blend >> 24) & 0xF)?,
                    PicaBlendFactor::decode(PicaBlendChannel::Alpha, (blend >> 28) & 0xF)?,
                    PicaLogicOperation::Copy,
                )
            }
        };

        Ok(Self {
            operation,
            mode,
            rgb_equation,
            alpha_equation,
            rgb_source_factor,
            rgb_destination_factor,
            alpha_source_factor,
            alpha_destination_factor,
            logic_operation,
            constant_color: [
                blend_color as u8,
                (blend_color >> 8) as u8,
                (blend_color >> 16) as u8,
                (blend_color >> 24) as u8,
            ],
            alpha_test_enabled,
            alpha_test_function: if alpha_test_enabled {
                PicaAlphaTestFunction::decode(alpha_test >> 4)
            } else {
                PicaAlphaTestFunction::Always
            },
            alpha_test_reference: (alpha_test >> 8) as u8,
            stencil: PicaStencilState {
                enabled: stencil_test & 1 != 0,
                function: PicaStencilFunction::decode(stencil_test >> 4),
                write_mask: (stencil_test >> 8) as u8,
                reference: (stencil_test >> 16) as u8,
                compare_mask: (stencil_test >> 24) as u8,
                stencil_fail: PicaStencilOperation::decode(stencil_operation),
                depth_fail: PicaStencilOperation::decode(stencil_operation >> 4),
                depth_pass: PicaStencilOperation::decode(stencil_operation >> 8),
            },
        })
    }

    /// Tests fragment alpha before depth/stencil and color operations.
    ///
    /// The PICA alpha reference is 8-bit, so fragment alpha is quantized to
    /// target precision before comparison.
    pub fn alpha_test_passes(&self, alpha: f32) -> Result<bool, PicaFragmentOutputError> {
        if !alpha.is_finite() {
            return Err(PicaFragmentOutputError::NonFiniteColor {
                name: "source",
                channel: 3,
            });
        }
        if !self.alpha_test_enabled {
            return Ok(true);
        }
        Ok(self
            .alpha_test_function
            .compare(to_u8(alpha), self.alpha_test_reference))
    }

    /// Applies blend/logic operations to one normalized source/destination RGBA pair.
    ///
    /// Logic operations quantize both inputs to RGBA8 before applying bitwise
    /// operations. Blending uses normalized floating-point channels and rounds
    /// the final result to RGBA8.
    pub fn apply(
        &self,
        source: [f32; 4],
        destination: [f32; 4],
    ) -> Result<[u8; 4], PicaFragmentOutputError> {
        validate_color("source", source)?;
        validate_color("destination", destination)?;

        match self.mode {
            PicaColorOperationMode::Logic => {
                let source = source.map(to_u8);
                let destination = destination.map(to_u8);
                Ok(std::array::from_fn(|channel| {
                    self.logic_operation
                        .apply(source[channel], destination[channel])
                }))
            }
            PicaColorOperationMode::Blend => {
                let destination = destination.map(|value| value.clamp(0.0, 1.0));
                let constant = self.constant_color.map(|value| f32::from(value) / 255.0);
                let source = source.map(|value| value.clamp(0.0, 1.0));
                let mut result = [0; 4];

                for channel in 0..4 {
                    let (equation, source_factor, destination_factor) = if channel == 3 {
                        (
                            self.alpha_equation,
                            self.alpha_source_factor,
                            self.alpha_destination_factor,
                        )
                    } else {
                        (
                            self.rgb_equation,
                            self.rgb_source_factor,
                            self.rgb_destination_factor,
                        )
                    };

                    let source_factor =
                        source_factor.evaluate(channel, source, destination, constant);
                    let destination_factor =
                        destination_factor.evaluate(channel, source, destination, constant);
                    let value = equation.apply(
                        source[channel],
                        destination[channel],
                        source_factor,
                        destination_factor,
                    );
                    result[channel] = to_u8(value.clamp(0.0, 1.0));
                }
                Ok(result)
            }
        }
    }
}

impl Pica200 {
    /// Decodes the current PICA200 framebuffer color-operation state.
    pub fn fragment_output_state(
        &self,
    ) -> Result<PicaFragmentOutputState, PicaFragmentOutputError> {
        PicaFragmentOutputState::from_pica(self)
    }
}

fn to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn validate_color(name: &'static str, color: [f32; 4]) -> Result<(), PicaFragmentOutputError> {
    for (channel, value) in color.into_iter().enumerate() {
        if !value.is_finite() {
            return Err(PicaFragmentOutputError::NonFiniteColor { name, channel });
        }
    }
    Ok(())
}

/// Decode or evaluation error for the PICA framebuffer color operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaFragmentOutputError {
    UnsupportedFragmentOperation {
        value: u8,
    },
    UnsupportedBlendEquation {
        channel: PicaBlendChannel,
        value: u8,
    },
    UnsupportedBlendFactor {
        channel: PicaBlendChannel,
        value: u8,
    },
    UnsupportedLogicOperation {
        value: u8,
    },
    NonFiniteColor {
        name: &'static str,
        channel: usize,
    },
}

impl std::fmt::Display for PicaFragmentOutputError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedFragmentOperation { value } => write!(
                formatter,
                "PICA fragment operation {value} is not implemented"
            ),
            Self::UnsupportedBlendEquation { channel, value } => write!(
                formatter,
                "PICA {channel:?} blend equation {value} is not implemented"
            ),
            Self::UnsupportedBlendFactor { channel, value } => write!(
                formatter,
                "PICA {channel:?} blend factor {value} is not implemented"
            ),
            Self::UnsupportedLogicOperation { value } => {
                write!(formatter, "PICA logic operation {value} is not implemented")
            }
            Self::NonFiniteColor { name, channel } => {
                write!(
                    formatter,
                    "PICA fragment {name} channel {channel} is not finite"
                )
            }
        }
    }
}

impl std::error::Error for PicaFragmentOutputError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_guest_configured_alpha_blend_state() {
        let mut pica = Pica200::new();
        pica.write_register(REG_COLOR_OPERATION, 1 << 8).unwrap();
        pica.write_register(REG_BLEND_FUNC, (6 << 16) | (7 << 20) | (1 << 24))
            .unwrap();
        pica.write_register(REG_LOGIC_OP, 3).unwrap();
        pica.write_register(REG_BLEND_COLOR, 0x8040_2010).unwrap();

        let state = PicaFragmentOutputState::from_pica(&pica).unwrap();
        assert_eq!(state.mode, PicaColorOperationMode::Blend);
        assert_eq!(state.rgb_equation, PicaBlendEquation::Add);
        assert_eq!(state.alpha_equation, PicaBlendEquation::Add);
        assert_eq!(state.rgb_source_factor, PicaBlendFactor::SourceAlpha);
        assert_eq!(
            state.rgb_destination_factor,
            PicaBlendFactor::OneMinusSourceAlpha
        );
        assert_eq!(state.alpha_source_factor, PicaBlendFactor::One);
        assert_eq!(state.alpha_destination_factor, PicaBlendFactor::Zero);
        assert_eq!(state.logic_operation, PicaLogicOperation::Copy);
        assert_eq!(state.constant_color, [0x10, 0x20, 0x40, 0x80]);
    }

    #[test]
    fn observed_blend_equation_aliases_decode_as_add() {
        for equation in 5..=7 {
            let mut pica = Pica200::new();
            pica.write_register(REG_COLOR_OPERATION, 1 << 8).unwrap();
            pica.write_register(REG_BLEND_FUNC, equation).unwrap();
            let state = PicaFragmentOutputState::from_pica(&pica).unwrap();
            assert_eq!(state.rgb_equation, PicaBlendEquation::Add);
        }
    }

    #[test]
    fn applies_source_alpha_blending_with_independent_alpha_equation() {
        let state = PicaFragmentOutputState {
            operation: PicaFragmentOperation::Default,
            mode: PicaColorOperationMode::Blend,
            rgb_equation: PicaBlendEquation::Add,
            alpha_equation: PicaBlendEquation::Add,
            rgb_source_factor: PicaBlendFactor::SourceAlpha,
            rgb_destination_factor: PicaBlendFactor::OneMinusSourceAlpha,
            alpha_source_factor: PicaBlendFactor::One,
            alpha_destination_factor: PicaBlendFactor::Zero,
            logic_operation: PicaLogicOperation::Copy,
            constant_color: [0; 4],
            alpha_test_enabled: false,
            alpha_test_function: PicaAlphaTestFunction::Always,
            alpha_test_reference: 0,
            stencil: PicaStencilState {
                enabled: false,
                function: PicaStencilFunction::Always,
                write_mask: 0,
                reference: 0,
                compare_mask: 0xFF,
                stencil_fail: PicaStencilOperation::Keep,
                depth_fail: PicaStencilOperation::Keep,
                depth_pass: PicaStencilOperation::Keep,
            },
        };
        assert_eq!(
            state
                .apply([1.0, 0.0, 0.0, 0.5], [0.0, 0.0, 1.0, 1.0])
                .unwrap(),
            [128, 0, 128, 128]
        );
    }

    #[test]
    fn min_max_ignore_factors_and_reverse_subtract_reverses_operands() {
        let mut state = PicaFragmentOutputState {
            operation: PicaFragmentOperation::Default,
            mode: PicaColorOperationMode::Blend,
            rgb_equation: PicaBlendEquation::Minimum,
            alpha_equation: PicaBlendEquation::Maximum,
            rgb_source_factor: PicaBlendFactor::Zero,
            rgb_destination_factor: PicaBlendFactor::Zero,
            alpha_source_factor: PicaBlendFactor::Zero,
            alpha_destination_factor: PicaBlendFactor::Zero,
            logic_operation: PicaLogicOperation::Copy,
            constant_color: [0; 4],
            alpha_test_enabled: false,
            alpha_test_function: PicaAlphaTestFunction::Always,
            alpha_test_reference: 0,
            stencil: PicaStencilState {
                enabled: false,
                function: PicaStencilFunction::Always,
                write_mask: 0,
                reference: 0,
                compare_mask: 0xFF,
                stencil_fail: PicaStencilOperation::Keep,
                depth_fail: PicaStencilOperation::Keep,
                depth_pass: PicaStencilOperation::Keep,
            },
        };
        assert_eq!(
            state
                .apply([0.25, 0.75, 0.5, 0.25], [0.5, 0.5, 0.25, 0.75])
                .unwrap(),
            [64, 128, 64, 191]
        );

        state.rgb_equation = PicaBlendEquation::ReverseSubtract;
        state.rgb_source_factor = PicaBlendFactor::One;
        state.rgb_destination_factor = PicaBlendFactor::One;
        state.alpha_equation = PicaBlendEquation::Add;
        state.alpha_source_factor = PicaBlendFactor::One;
        state.alpha_destination_factor = PicaBlendFactor::Zero;
        assert_eq!(
            state
                .apply([0.25, 0.75, 0.5, 0.25], [0.5, 0.5, 0.25, 0.75])
                .unwrap(),
            [64, 0, 0, 64]
        );
    }

    #[test]
    fn applies_logic_operations_to_all_rgba8_channels() {
        let mut state = PicaFragmentOutputState::from_pica(&Pica200::new()).unwrap();
        assert_eq!(state.mode, PicaColorOperationMode::Logic);
        state.logic_operation = PicaLogicOperation::Xor;
        assert_eq!(
            state
                .apply(
                    [
                        0xF0 as f32 / 255.0,
                        0x55 as f32 / 255.0,
                        0xAA as f32 / 255.0,
                        1.0,
                    ],
                    [
                        0x0F as f32 / 255.0,
                        0x33 as f32 / 255.0,
                        0xAA as f32 / 255.0,
                        0.0,
                    ],
                )
                .unwrap(),
            [0xFF, 0x66, 0x00, 0xFF]
        );
        state.logic_operation = PicaLogicOperation::Noop;
        assert_eq!(
            state.apply([1.0; 4], [0.0, 0.25, 0.5, 1.0]).unwrap(),
            [0, 64, 128, 255]
        );
    }

    #[test]
    fn rejects_unimplemented_fragment_modes_and_reserved_blend_fields() {
        let mut pica = Pica200::new();
        pica.write_register(REG_COLOR_OPERATION, 1).unwrap();
        assert_eq!(
            pica.fragment_output_state(),
            Err(PicaFragmentOutputError::UnsupportedFragmentOperation { value: 1 })
        );

        pica.write_register(REG_COLOR_OPERATION, 1 << 8).unwrap();
        pica.write_register(REG_BLEND_FUNC, 15 << 16).unwrap();
        assert_eq!(
            pica.fragment_output_state(),
            Err(PicaFragmentOutputError::UnsupportedBlendFactor {
                channel: PicaBlendChannel::Rgb,
                value: 15
            })
        );
    }

    #[test]
    fn decodes_and_applies_guest_configured_alpha_test_functions() {
        let mut pica = Pica200::new();
        pica.write_register(REG_FRAGOP_ALPHA_TEST, 1 | (4 << 4) | (128 << 8))
            .unwrap();
        let mut state = PicaFragmentOutputState::from_pica(&pica).unwrap();
        assert!(state.alpha_test_enabled);
        assert_eq!(state.alpha_test_function, PicaAlphaTestFunction::Less);
        assert_eq!(state.alpha_test_reference, 128);
        assert!(state.alpha_test_passes(127.0 / 255.0).unwrap());
        assert!(!state.alpha_test_passes(128.0 / 255.0).unwrap());

        state.alpha_test_function = PicaAlphaTestFunction::GreaterEqual;
        assert!(state.alpha_test_passes(128.0 / 255.0).unwrap());
        state.alpha_test_function = PicaAlphaTestFunction::Never;
        assert!(!state.alpha_test_passes(1.0).unwrap());
        state.alpha_test_function = PicaAlphaTestFunction::Always;
        assert!(state.alpha_test_passes(0.0).unwrap());
    }

    #[test]
    fn decodes_stencil_masks_comparison_and_three_result_operations() {
        let mut pica = Pica200::new();
        pica.write_register(
            REG_STENCIL_TEST,
            1 | (2 << 4) | (0x0F << 8) | (0xA5 << 16) | (0xF0 << 24),
        )
        .unwrap();
        pica.write_register(REG_STENCIL_OP, 2 | (3 << 4) | (7 << 8))
            .unwrap();

        let state = PicaFragmentOutputState::from_pica(&pica).unwrap().stencil;
        assert!(state.enabled);
        assert_eq!(state.function, PicaStencilFunction::Equal);
        assert_eq!(state.write_mask, 0x0F);
        assert_eq!(state.reference, 0xA5);
        assert_eq!(state.compare_mask, 0xF0);
        assert_eq!(state.stencil_fail, PicaStencilOperation::Replace);
        assert_eq!(state.depth_fail, PicaStencilOperation::IncrementClamp);
        assert_eq!(state.depth_pass, PicaStencilOperation::DecrementWrap);
        assert!(state.test_passes(0xA1));
        assert!(!state.test_passes(0xB1));
        assert_eq!(state.apply(PicaStencilOperation::Replace, 0x5A), 0x55);
    }

    #[test]
    fn stencil_operations_cover_clamp_wrap_invert_and_mask_merge() {
        let state = PicaStencilState {
            enabled: true,
            function: PicaStencilFunction::Always,
            write_mask: 0xFF,
            reference: 0x34,
            compare_mask: 0xFF,
            stencil_fail: PicaStencilOperation::Keep,
            depth_fail: PicaStencilOperation::Keep,
            depth_pass: PicaStencilOperation::Keep,
        };
        assert_eq!(
            state.apply(PicaStencilOperation::IncrementClamp, 0xFF),
            0xFF
        );
        assert_eq!(state.apply(PicaStencilOperation::DecrementClamp, 0), 0);
        assert_eq!(state.apply(PicaStencilOperation::IncrementWrap, 0xFF), 0);
        assert_eq!(state.apply(PicaStencilOperation::DecrementWrap, 0), 0xFF);
        assert_eq!(state.apply(PicaStencilOperation::Invert, 0x0F), 0xF0);
        assert_eq!(state.apply(PicaStencilOperation::Zero, 0xAA), 0);

        let masked = PicaStencilState {
            write_mask: 0x0F,
            ..state
        };
        assert_eq!(masked.apply(PicaStencilOperation::Replace, 0xA0), 0xA4);
    }

    #[test]
    fn rejects_non_finite_source_or_destination_colors() {
        let state = PicaFragmentOutputState::from_pica(&Pica200::new()).unwrap();
        assert_eq!(
            state.apply([f32::NAN, 0.0, 0.0, 0.0], [0.0; 4]),
            Err(PicaFragmentOutputError::NonFiniteColor {
                name: "source",
                channel: 0,
            })
        );
    }

    #[test]
    fn all_sixteen_logic_operations_match_their_truth_tables() {
        use PicaLogicOperation as Op;
        // Source 0b1100, destination 0b1010 on the low nibble.
        let (s, d) = (0b1100u8, 0b1010u8);
        let expected = [
            (Op::Clear, 0),
            (Op::And, s & d),
            (Op::ReverseAnd, s & !d),
            (Op::Copy, s),
            (Op::Set, 0xFF),
            (Op::InvertedCopy, !s),
            (Op::Noop, d),
            (Op::Invert, !d),
            (Op::Nand, !(s & d)),
            (Op::Or, s | d),
            (Op::Nor, !(s | d)),
            (Op::Xor, s ^ d),
            (Op::Equivalent, !(s ^ d)),
            (Op::InvertedAnd, !s & d),
            (Op::ReverseOr, s | !d),
            (Op::InvertedOr, !s | d),
        ];
        for (index, (operation, value)) in expected.into_iter().enumerate() {
            assert_eq!(PicaLogicOperation::decode(index as u32).unwrap(), operation);
            assert_eq!(operation.apply(s, d), value, "{operation:?}");
        }
    }

    #[test]
    fn every_blend_factor_evaluates_per_channel_and_alpha() {
        use PicaBlendFactor as F;
        let source = [0.2, 0.4, 0.6, 0.5];
        let destination = [0.9, 0.8, 0.7, 0.25];
        let constant = [0.1, 0.3, 0.5, 0.75];
        let close = |a: f32, b: f32| (a - b).abs() < 1.0e-6;
        for (factor, red, alpha) in [
            (F::Zero, 0.0, 0.0),
            (F::One, 1.0, 1.0),
            (F::SourceColor, 0.2, 0.5),
            (F::OneMinusSourceColor, 0.8, 0.5),
            (F::DestinationColor, 0.9, 0.25),
            (F::OneMinusDestinationColor, 0.1, 0.75),
            (F::SourceAlpha, 0.5, 0.5),
            (F::OneMinusSourceAlpha, 0.5, 0.5),
            (F::DestinationAlpha, 0.25, 0.25),
            (F::OneMinusDestinationAlpha, 0.75, 0.75),
            (F::ConstantColor, 0.1, 0.75),
            (F::OneMinusConstantColor, 0.9, 0.25),
            (F::ConstantAlpha, 0.75, 0.75),
            (F::OneMinusConstantAlpha, 0.25, 0.25),
            // min(source alpha, 1 - destination alpha) for color, 1 for alpha.
            (F::SourceAlphaSaturate, 0.5, 1.0),
        ] {
            assert!(
                close(factor.evaluate(0, source, destination, constant), red),
                "{factor:?} red"
            );
            assert!(
                close(factor.evaluate(3, source, destination, constant), alpha),
                "{factor:?} alpha"
            );
        }
        // The saturate factor limits itself by the remaining destination alpha.
        let nearly_full = [0.9, 0.8, 0.7, 0.9];
        assert!(close(
            F::SourceAlphaSaturate.evaluate(0, source, nearly_full, constant),
            0.1
        ));
    }

    #[test]
    fn blend_results_are_clamped_and_equations_use_independent_channels() {
        let mut pica = Pica200::new();
        // Blend mode; RGB: add, source color + destination color (overflows);
        // alpha: subtract with both factors one.
        pica.write_register(REG_COLOR_OPERATION, 1 << 8).unwrap();
        pica.write_register(
            REG_BLEND_FUNC,
            (1 << 8) | (1 << 16) | (1 << 20) | (1 << 24) | (1 << 28),
        )
        .unwrap();
        let state = pica.fragment_output_state().unwrap();
        let out = state
            .apply([0.8, 0.8, 0.8, 0.6], [0.7, 0.7, 0.7, 0.25])
            .unwrap();
        // RGB saturates at 1.0; alpha is 0.6 - 0.25.
        assert_eq!(out[..3], [255, 255, 255]);
        assert_eq!(out[3], to_u8(0.35));
    }
}
