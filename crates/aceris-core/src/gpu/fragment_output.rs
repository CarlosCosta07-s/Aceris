//! PICA200 framebuffer color-operation stage.
//!
//! This module models the color operation after TEV: fixed-function blending
//! or per-channel logic operations. It intentionally accepts and returns
//! normalized/8-bit RGBA values and does not yet model framebuffer formats
//! other than RGBA8, alpha testing, stencil, or depth/color write masks.

use super::pica200::Pica200;

/// GPUREG_COLOR_OPERATION.
pub const REG_COLOR_OPERATION: u16 = 0x100;
/// GPUREG_BLEND_FUNC.
pub const REG_BLEND_FUNC: u16 = 0x101;
/// GPUREG_LOGIC_OP.
pub const REG_LOGIC_OP: u16 = 0x102;
/// GPUREG_BLEND_COLOR.
pub const REG_BLEND_COLOR: u16 = 0x103;

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
            0 => Ok(Self::Add),
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
        })
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
        pica.write_register(REG_BLEND_FUNC, 5 | (15 << 16)).unwrap();
        assert_eq!(
            pica.fragment_output_state(),
            Err(PicaFragmentOutputError::UnsupportedBlendEquation {
                channel: PicaBlendChannel::Rgb,
                value: 5
            })
        );
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
}
