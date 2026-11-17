//! PICA200 texture-environment (TEV) fragment combiners.
//!
//! The PICA200 does not expose a programmable pixel-shader ISA. Its fragment
//! color path is configured through six fixed-function texture-environment
//! stages. This module decodes those stage registers and evaluates the
//! documented source/operand/combine operations.
//!
//! Texture fetching, coordinate generation, lighting, fog, alpha/stencil tests,
//! framebuffer blending and rasterizer integration remain separate stages.
//! Inputs are normalized RGBA values supplied by the caller; no texture memory
//! is read here.

use super::pica200::Pica200;

/// Number of fixed-function texture-environment stages.
pub const PICA_TEV_STAGE_COUNT: usize = 6;
/// First texture-environment stage register.
pub const REG_TEXENV0_SOURCE: u16 = 0x0C0;
/// Operand selectors for the first texture-environment stage.
pub const REG_TEXENV0_OPERAND: u16 = REG_TEXENV0_SOURCE + 1;
/// RGB/alpha combine operations for the first texture-environment stage.
pub const REG_TEXENV0_COMBINER: u16 = REG_TEXENV0_SOURCE + 2;
/// Constant RGBA color for the first texture-environment stage.
pub const REG_TEXENV0_COLOR: u16 = REG_TEXENV0_SOURCE + 3;
/// RGB/alpha output scale for the first texture-environment stage.
pub const REG_TEXENV0_SCALE: u16 = REG_TEXENV0_SOURCE + 4;
/// Source-register addresses for all six TEV stages. The final two stages
/// occupy the 0xF0 and 0xF8 register groups rather than continuing the first
/// four-stage 8-register sequence.
pub const TEXENV_STAGE_SOURCE_REGISTERS: [u16; PICA_TEV_STAGE_COUNT] =
    [0x0C0, 0x0C8, 0x0D0, 0x0D8, 0x0F0, 0x0F8];
/// Register spacing between the first four texture-environment stages.
pub const TEXENV_STAGE_REGISTER_STRIDE: u16 = 8;

/// Sources accepted by the texture-environment combiner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaTevSource {
    PrimaryColor,
    FragmentPrimaryColor,
    FragmentSecondaryColor,
    Texture0,
    Texture1,
    Texture2,
    Texture3,
    PreviousBuffer,
    Constant,
    Previous,
}

impl PicaTevSource {
    fn decode(
        stage: usize,
        channel: PicaTevChannel,
        slot: usize,
        value: u32,
    ) -> Result<Self, PicaTevError> {
        match value {
            0 => Ok(Self::PrimaryColor),
            1 => Ok(Self::FragmentPrimaryColor),
            2 => Ok(Self::FragmentSecondaryColor),
            3 => Ok(Self::Texture0),
            4 => Ok(Self::Texture1),
            5 => Ok(Self::Texture2),
            6 => Ok(Self::Texture3),
            13 => Ok(Self::PreviousBuffer),
            14 => Ok(Self::Constant),
            15 => Ok(Self::Previous),
            _ => Err(PicaTevError::UnsupportedSource {
                stage,
                channel,
                slot,
                value: value as u8,
            }),
        }
    }
}

/// Whether a malformed TEV register field belongs to RGB or alpha processing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaTevChannel {
    Rgb,
    Alpha,
}

/// RGB operand selectors described by GPUREG_TEXENVi_OPERAND.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaTevRgbOperand {
    SourceColor,
    OneMinusSourceColor,
    SourceAlpha,
    OneMinusSourceAlpha,
    SourceRed,
    OneMinusSourceRed,
    SourceGreen,
    OneMinusSourceGreen,
    SourceBlue,
    OneMinusSourceBlue,
}

impl PicaTevRgbOperand {
    fn decode(stage: usize, slot: usize, value: u32) -> Result<Self, PicaTevError> {
        match value {
            0 => Ok(Self::SourceColor),
            1 => Ok(Self::OneMinusSourceColor),
            2 => Ok(Self::SourceAlpha),
            3 => Ok(Self::OneMinusSourceAlpha),
            4 => Ok(Self::SourceRed),
            5 => Ok(Self::OneMinusSourceRed),
            8 => Ok(Self::SourceGreen),
            9 => Ok(Self::OneMinusSourceGreen),
            12 => Ok(Self::SourceBlue),
            13 => Ok(Self::OneMinusSourceBlue),
            _ => Err(PicaTevError::UnsupportedOperand {
                stage,
                channel: PicaTevChannel::Rgb,
                slot,
                value: value as u8,
            }),
        }
    }

    fn apply(self, source: [f32; 4]) -> [f32; 3] {
        match self {
            Self::SourceColor => [source[0], source[1], source[2]],
            Self::OneMinusSourceColor => [1.0 - source[0], 1.0 - source[1], 1.0 - source[2]],
            Self::SourceAlpha => [source[3]; 3],
            Self::OneMinusSourceAlpha => [1.0 - source[3]; 3],
            Self::SourceRed => [source[0]; 3],
            Self::OneMinusSourceRed => [1.0 - source[0]; 3],
            Self::SourceGreen => [source[1]; 3],
            Self::OneMinusSourceGreen => [1.0 - source[1]; 3],
            Self::SourceBlue => [source[2]; 3],
            Self::OneMinusSourceBlue => [1.0 - source[2]; 3],
        }
    }
}

/// Alpha operand selectors described by GPUREG_TEXENVi_OPERAND.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaTevAlphaOperand {
    SourceAlpha,
    OneMinusSourceAlpha,
    SourceRed,
    OneMinusSourceRed,
    SourceGreen,
    OneMinusSourceGreen,
    SourceBlue,
    OneMinusSourceBlue,
}

impl PicaTevAlphaOperand {
    fn decode(stage: usize, slot: usize, value: u32) -> Result<Self, PicaTevError> {
        match value {
            0 => Ok(Self::SourceAlpha),
            1 => Ok(Self::OneMinusSourceAlpha),
            2 => Ok(Self::SourceRed),
            3 => Ok(Self::OneMinusSourceRed),
            4 => Ok(Self::SourceGreen),
            5 => Ok(Self::OneMinusSourceGreen),
            6 => Ok(Self::SourceBlue),
            7 => Ok(Self::OneMinusSourceBlue),
            _ => Err(PicaTevError::UnsupportedOperand {
                stage,
                channel: PicaTevChannel::Alpha,
                slot,
                value: value as u8,
            }),
        }
    }

    fn apply(self, source: [f32; 4]) -> f32 {
        match self {
            Self::SourceAlpha => source[3],
            Self::OneMinusSourceAlpha => 1.0 - source[3],
            Self::SourceRed => source[0],
            Self::OneMinusSourceRed => 1.0 - source[0],
            Self::SourceGreen => source[1],
            Self::OneMinusSourceGreen => 1.0 - source[1],
            Self::SourceBlue => source[2],
            Self::OneMinusSourceBlue => 1.0 - source[2],
        }
    }
}

/// Combine operations supported by the PICA texture-environment registers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaTevCombine {
    Replace,
    Modulate,
    Add,
    AddSigned,
    Interpolate,
    Subtract,
    Dot3Rgb,
    Dot3Rgba,
    MultiplyThenAdd,
    AddThenMultiply,
}

impl PicaTevCombine {
    fn decode(stage: usize, channel: PicaTevChannel, value: u32) -> Result<Self, PicaTevError> {
        if channel == PicaTevChannel::Alpha && (value == 6 || value == 7) {
            return Err(PicaTevError::UnsupportedCombine {
                stage,
                channel,
                value: value as u8,
            });
        }
        match value {
            0 => Ok(Self::Replace),
            1 => Ok(Self::Modulate),
            2 => Ok(Self::Add),
            3 => Ok(Self::AddSigned),
            4 => Ok(Self::Interpolate),
            5 => Ok(Self::Subtract),
            6 => Ok(Self::Dot3Rgb),
            7 => Ok(Self::Dot3Rgba),
            8 => Ok(Self::MultiplyThenAdd),
            9 => Ok(Self::AddThenMultiply),
            _ => Err(PicaTevError::UnsupportedCombine {
                stage,
                channel,
                value: value as u8,
            }),
        }
    }

    fn apply_rgb(self, sources: [[f32; 3]; 3]) -> [f32; 3] {
        let [a, b, c] = sources;
        match self {
            Self::Replace => a,
            Self::Modulate => zip3(a, b, |x, y| x * y),
            Self::Add => zip3(a, b, |x, y| x + y),
            Self::AddSigned => zip3(a, b, |x, y| x + y - 0.5),
            Self::Interpolate => zip3x3(a, b, c, |x, y, z| x * z + y * (1.0 - z)),
            Self::Subtract => zip3(a, b, |x, y| x - y),
            Self::Dot3Rgb | Self::Dot3Rgba => {
                let dot = 4.0
                    * ((a[0] - 0.5) * (b[0] - 0.5)
                        + (a[1] - 0.5) * (b[1] - 0.5)
                        + (a[2] - 0.5) * (b[2] - 0.5));
                [dot; 3]
            }
            Self::MultiplyThenAdd => zip3x3(a, b, c, |x, y, z| x * y + z),
            Self::AddThenMultiply => zip3x3(a, b, c, |x, y, z| (x + y) * z),
        }
    }

    fn apply_alpha(self, sources: [f32; 3]) -> Option<f32> {
        let [a, b, c] = sources;
        match self {
            Self::Replace => Some(a),
            Self::Modulate => Some(a * b),
            Self::Add => Some(a + b),
            Self::AddSigned => Some(a + b - 0.5),
            Self::Interpolate => Some(a * c + b * (1.0 - c)),
            Self::Subtract => Some(a - b),
            Self::Dot3Rgb | Self::Dot3Rgba => None,
            Self::MultiplyThenAdd => Some(a * b + c),
            Self::AddThenMultiply => Some((a + b) * c),
        }
    }
}

fn zip3(a: [f32; 3], b: [f32; 3], f: impl Fn(f32, f32) -> f32) -> [f32; 3] {
    [f(a[0], b[0]), f(a[1], b[1]), f(a[2], b[2])]
}

fn zip3x3(a: [f32; 3], b: [f32; 3], c: [f32; 3], f: impl Fn(f32, f32, f32) -> f32) -> [f32; 3] {
    [
        f(a[0], b[0], c[0]),
        f(a[1], b[1], c[1]),
        f(a[2], b[2], c[2]),
    ]
}

/// One decoded PICA TEV stage.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaTevStage {
    pub rgb_sources: [PicaTevSource; 3],
    pub alpha_sources: [PicaTevSource; 3],
    pub rgb_operands: [PicaTevRgbOperand; 3],
    pub alpha_operands: [PicaTevAlphaOperand; 3],
    pub rgb_combine: PicaTevCombine,
    pub alpha_combine: PicaTevCombine,
    pub constant_color: [f32; 4],
    pub rgb_scale: f32,
    pub alpha_scale: f32,
}

impl PicaTevStage {
    fn from_registers(pica: &Pica200, index: usize) -> Result<Self, PicaTevError> {
        let base = TEXENV_STAGE_SOURCE_REGISTERS[index];
        let source = pica.register(base).unwrap_or(0);
        let operand = pica.register(base + 1).unwrap_or(0);
        let combine = pica.register(base + 2).unwrap_or(0);
        let color = pica.register(base + 3).unwrap_or(0);
        let scale = pica.register(base + 4).unwrap_or(0);

        let rgb_sources = [
            PicaTevSource::decode(index, PicaTevChannel::Rgb, 0, source & 0xF)?,
            PicaTevSource::decode(index, PicaTevChannel::Rgb, 1, (source >> 4) & 0xF)?,
            PicaTevSource::decode(index, PicaTevChannel::Rgb, 2, (source >> 8) & 0xF)?,
        ];
        let alpha_sources = [
            PicaTevSource::decode(index, PicaTevChannel::Alpha, 0, (source >> 16) & 0xF)?,
            PicaTevSource::decode(index, PicaTevChannel::Alpha, 1, (source >> 20) & 0xF)?,
            PicaTevSource::decode(index, PicaTevChannel::Alpha, 2, (source >> 24) & 0xF)?,
        ];
        let rgb_operands = [
            PicaTevRgbOperand::decode(index, 0, operand & 0xF)?,
            PicaTevRgbOperand::decode(index, 1, (operand >> 4) & 0xF)?,
            PicaTevRgbOperand::decode(index, 2, (operand >> 8) & 0xF)?,
        ];
        let alpha_operands = [
            PicaTevAlphaOperand::decode(index, 0, (operand >> 12) & 0x7)?,
            PicaTevAlphaOperand::decode(index, 1, (operand >> 16) & 0x7)?,
            PicaTevAlphaOperand::decode(index, 2, (operand >> 20) & 0x7)?,
        ];
        let rgb_combine = PicaTevCombine::decode(index, PicaTevChannel::Rgb, combine & 0xF)?;
        let alpha_combine =
            PicaTevCombine::decode(index, PicaTevChannel::Alpha, (combine >> 16) & 0xF)?;
        let rgb_scale = decode_scale(index, PicaTevChannel::Rgb, scale & 0x3)?;
        let alpha_scale = decode_scale(index, PicaTevChannel::Alpha, (scale >> 16) & 0x3)?;

        Ok(Self {
            rgb_sources,
            alpha_sources,
            rgb_operands,
            alpha_operands,
            rgb_combine,
            alpha_combine,
            constant_color: [
                (color & 0xFF) as f32 / 255.0,
                ((color >> 8) & 0xFF) as f32 / 255.0,
                ((color >> 16) & 0xFF) as f32 / 255.0,
                ((color >> 24) & 0xFF) as f32 / 255.0,
            ],
            rgb_scale,
            alpha_scale,
        })
    }

    fn evaluate(
        self,
        stage_index: usize,
        previous: [f32; 4],
        inputs: &PicaFragmentInputs,
    ) -> Result<[f32; 4], PicaTevError> {
        let rgb_sources = self.rgb_sources.map(|source| {
            resolve_source(
                source,
                self.rgb_sources,
                stage_index,
                previous,
                inputs,
                self.constant_color,
            )
        });
        let alpha_sources = self.alpha_sources.map(|source| {
            resolve_source(
                source,
                self.alpha_sources,
                stage_index,
                previous,
                inputs,
                self.constant_color,
            )
        });

        let rgb_operands = [
            self.rgb_operands[0].apply(rgb_sources[0]),
            self.rgb_operands[1].apply(rgb_sources[1]),
            self.rgb_operands[2].apply(rgb_sources[2]),
        ];
        let alpha_operands = [
            self.alpha_operands[0].apply(alpha_sources[0]),
            self.alpha_operands[1].apply(alpha_sources[1]),
            self.alpha_operands[2].apply(alpha_sources[2]),
        ];

        let unscaled_rgb = self.rgb_combine.apply_rgb(rgb_operands);
        let mut rgb = unscaled_rgb;
        for lane in &mut rgb {
            *lane = (*lane * self.rgb_scale).clamp(0.0, 1.0);
        }

        let alpha = if self.rgb_combine == PicaTevCombine::Dot3Rgba {
            (unscaled_rgb[0] * self.alpha_scale).clamp(0.0, 1.0)
        } else {
            self.alpha_combine
                .apply_alpha(alpha_operands)
                .ok_or(PicaTevError::UnsupportedCombine {
                    stage: stage_index,
                    channel: PicaTevChannel::Alpha,
                    value: 6,
                })?
                .mul_add(self.alpha_scale, 0.0)
                .clamp(0.0, 1.0)
        };

        Ok([rgb[0], rgb[1], rgb[2], alpha])
    }
}

fn decode_scale(stage: usize, channel: PicaTevChannel, value: u32) -> Result<f32, PicaTevError> {
    match value {
        0 => Ok(1.0),
        1 => Ok(2.0),
        2 => Ok(4.0),
        _ => Err(PicaTevError::UnsupportedScale {
            stage,
            channel,
            value: value as u8,
        }),
    }
}

fn resolve_source(
    source: PicaTevSource,
    stage_sources: [PicaTevSource; 3],
    stage_index: usize,
    previous: [f32; 4],
    inputs: &PicaFragmentInputs,
    constant: [f32; 4],
) -> [f32; 4] {
    match source {
        PicaTevSource::PrimaryColor => inputs.primary_color,
        PicaTevSource::FragmentPrimaryColor => inputs.fragment_primary_color,
        PicaTevSource::FragmentSecondaryColor => inputs.fragment_secondary_color,
        PicaTevSource::Texture0 => inputs.textures[0],
        PicaTevSource::Texture1 => inputs.textures[1],
        PicaTevSource::Texture2 => inputs.textures[2],
        PicaTevSource::Texture3 => inputs.textures[3],
        PicaTevSource::PreviousBuffer => [0.0; 4],
        PicaTevSource::Constant => constant,
        PicaTevSource::Previous if stage_index != 0 => previous,
        PicaTevSource::Previous if stage_sources[2] == PicaTevSource::Previous => [0.0; 4],
        PicaTevSource::Previous => resolve_source(
            stage_sources[2],
            [PicaTevSource::PrimaryColor; 3],
            stage_index,
            previous,
            inputs,
            constant,
        ),
    }
}

/// Per-fragment values supplied to the TEV stages after interpolation/fetch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaFragmentInputs {
    pub primary_color: [f32; 4],
    pub fragment_primary_color: [f32; 4],
    pub fragment_secondary_color: [f32; 4],
    pub textures: [[f32; 4]; 4],
}

impl Default for PicaFragmentInputs {
    fn default() -> Self {
        Self {
            primary_color: [1.0; 4],
            fragment_primary_color: [1.0; 4],
            fragment_secondary_color: [0.0; 4],
            textures: [[0.0; 4]; 4],
        }
    }
}

/// Six decoded texture-environment stages.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaTextureEnvironment {
    pub stages: [PicaTevStage; PICA_TEV_STAGE_COUNT],
}

impl PicaTextureEnvironment {
    /// Decodes the six guest-configured stages from the PICA register file.
    ///
    /// Reserved selectors return an error; they are never silently treated as
    /// another source, operand, operation, or scale.
    pub fn from_pica(pica: &Pica200) -> Result<Self, PicaTevError> {
        let mut stages = [PicaTevStage::from_registers(pica, 0)?; PICA_TEV_STAGE_COUNT];
        for (index, stage) in stages.iter_mut().enumerate().skip(1) {
            *stage = PicaTevStage::from_registers(pica, index)?;
        }
        Ok(Self { stages })
    }

    /// Evaluates all six stages in order, returning normalized RGBA.
    pub fn evaluate(&self, inputs: &PicaFragmentInputs) -> Result<[f32; 4], PicaTevError> {
        let mut previous = [0.0; 4];
        for (index, stage) in self.stages.iter().copied().enumerate() {
            previous = stage.evaluate(index, previous, inputs)?;
        }
        Ok(previous)
    }
}

impl Pica200 {
    /// Decodes the currently programmed six-stage fragment color combiner.
    pub fn texture_environment(&self) -> Result<PicaTextureEnvironment, PicaTevError> {
        PicaTextureEnvironment::from_pica(self)
    }
}

/// Malformed or not-yet-supported texture-environment register field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaTevError {
    UnsupportedSource {
        stage: usize,
        channel: PicaTevChannel,
        slot: usize,
        value: u8,
    },
    UnsupportedOperand {
        stage: usize,
        channel: PicaTevChannel,
        slot: usize,
        value: u8,
    },
    UnsupportedCombine {
        stage: usize,
        channel: PicaTevChannel,
        value: u8,
    },
    UnsupportedScale {
        stage: usize,
        channel: PicaTevChannel,
        value: u8,
    },
}

impl std::fmt::Display for PicaTevError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSource {
                stage,
                channel,
                slot,
                value,
            } => write!(
                formatter,
                "PICA TEV stage {stage} {channel:?} source {slot} uses unsupported selector {value}"
            ),
            Self::UnsupportedOperand {
                stage,
                channel,
                slot,
                value,
            } => write!(
                formatter,
                "PICA TEV stage {stage} {channel:?} operand {slot} uses unsupported selector {value}"
            ),
            Self::UnsupportedCombine {
                stage,
                channel,
                value,
            } => write!(
                formatter,
                "PICA TEV stage {stage} {channel:?} uses unsupported combine operation {value}"
            ),
            Self::UnsupportedScale {
                stage,
                channel,
                value,
            } => write!(
                formatter,
                "PICA TEV stage {stage} {channel:?} uses unsupported scale {value}"
            ),
        }
    }
}

impl std::error::Error for PicaTevError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn simple_stage(
        sources: [PicaTevSource; 3],
        operands: [PicaTevRgbOperand; 3],
        combine: PicaTevCombine,
        scale: f32,
    ) -> PicaTevStage {
        PicaTevStage {
            rgb_sources: sources,
            alpha_sources: sources,
            rgb_operands: operands,
            alpha_operands: [PicaTevAlphaOperand::SourceAlpha; 3],
            rgb_combine: combine,
            alpha_combine: PicaTevCombine::Replace,
            constant_color: [0.25, 0.5, 0.75, 1.0],
            rgb_scale: scale,
            alpha_scale: 1.0,
        }
    }

    #[test]
    fn decodes_guest_tev_registers_and_constants() {
        let mut pica = Pica200::new();
        let stage = 2u16;
        let base = TEXENV_STAGE_SOURCE_REGISTERS[stage as usize];
        pica.write_register(
            base,
            3 | (4 << 4) | (14 << 8) | (6 << 16) | (15 << 20) | (13 << 24),
        )
        .unwrap();
        pica.write_register(base + 1, (1 << 4) | (12 << 8) | (1 << 16) | (7 << 20))
            .unwrap();
        pica.write_register(base + 2, 4 | (1 << 16)).unwrap();
        pica.write_register(base + 3, 0x8040_2010).unwrap();
        pica.write_register(base + 4, 1 | (2 << 16)).unwrap();

        let decoded = PicaTevStage::from_registers(&pica, 2).unwrap();
        assert_eq!(
            decoded.rgb_sources,
            [
                PicaTevSource::Texture0,
                PicaTevSource::Texture1,
                PicaTevSource::Constant
            ]
        );
        assert_eq!(
            decoded.alpha_sources,
            [
                PicaTevSource::Texture3,
                PicaTevSource::Previous,
                PicaTevSource::PreviousBuffer
            ]
        );
        assert_eq!(decoded.rgb_combine, PicaTevCombine::Interpolate);
        assert_eq!(decoded.alpha_combine, PicaTevCombine::Modulate);
        assert_eq!(
            decoded.constant_color,
            [16.0 / 255.0, 32.0 / 255.0, 64.0 / 255.0, 128.0 / 255.0]
        );
        assert_eq!(decoded.rgb_scale, 2.0);
        assert_eq!(decoded.alpha_scale, 4.0);
    }

    #[test]
    fn evaluates_replace_modulate_add_and_subtract() {
        let a = [0.2, 0.4, 0.6, 0.8];
        let b = [0.5, 0.25, 0.1, 0.5];
        let inputs = PicaFragmentInputs {
            primary_color: a,
            textures: [b, [0.0; 4], [0.0; 4], [0.0; 4]],
            ..PicaFragmentInputs::default()
        };

        let replace = simple_stage(
            [
                PicaTevSource::PrimaryColor,
                PicaTevSource::Texture0,
                PicaTevSource::Texture0,
            ],
            [PicaTevRgbOperand::SourceColor; 3],
            PicaTevCombine::Replace,
            1.0,
        );
        assert_eq!(replace.evaluate(0, [0.0; 4], &inputs).unwrap(), a);

        let modulate = PicaTevStage {
            rgb_combine: PicaTevCombine::Modulate,
            alpha_combine: PicaTevCombine::Modulate,
            ..replace
        };
        let modulated = modulate.evaluate(0, [0.0; 4], &inputs).unwrap();
        assert!((modulated[0] - 0.1).abs() < 1.0e-6);
        assert!((modulated[1] - 0.1).abs() < 1.0e-6);
        assert!((modulated[2] - 0.06).abs() < 1.0e-6);
        assert!((modulated[3] - 0.4).abs() < 1.0e-6);

        let add = PicaTevStage {
            rgb_combine: PicaTevCombine::Add,
            alpha_combine: PicaTevCombine::Add,
            ..replace
        };
        assert_eq!(
            add.evaluate(0, [0.0; 4], &inputs).unwrap(),
            [0.7, 0.65, 0.7, 1.0]
        );

        let subtract = PicaTevStage {
            rgb_combine: PicaTevCombine::Subtract,
            alpha_combine: PicaTevCombine::Subtract,
            ..replace
        };
        assert_eq!(
            subtract.evaluate(0, [0.0; 4], &inputs).unwrap(),
            [0.0, 0.15, 0.5, 0.3]
        );
    }

    #[test]
    fn evaluates_interpolate_signed_add_dot3_and_scales() {
        let inputs = PicaFragmentInputs {
            primary_color: [0.75, 0.25, 0.5, 0.75],
            textures: [[0.25, 0.75, 0.5, 0.25], [0.0; 4], [0.0; 4], [0.0; 4]],
            ..PicaFragmentInputs::default()
        };

        let interpolate = simple_stage(
            [
                PicaTevSource::PrimaryColor,
                PicaTevSource::Texture0,
                PicaTevSource::PrimaryColor,
            ],
            [PicaTevRgbOperand::SourceColor; 3],
            PicaTevCombine::Interpolate,
            1.0,
        );
        assert_eq!(
            interpolate.evaluate(0, [0.0; 4], &inputs).unwrap(),
            [0.625, 0.625, 0.5, 0.75]
        );

        let add_signed = PicaTevStage {
            rgb_combine: PicaTevCombine::AddSigned,
            ..interpolate
        };
        assert_eq!(
            add_signed.evaluate(0, [0.0; 4], &inputs).unwrap(),
            [0.5, 0.5, 0.5, 0.75]
        );

        let dot_inputs = PicaFragmentInputs {
            primary_color: [0.75, 0.75, 0.75, 1.0],
            textures: [[0.75, 0.75, 0.75, 1.0], [0.0; 4], [0.0; 4], [0.0; 4]],
            ..PicaFragmentInputs::default()
        };
        let dot = PicaTevStage {
            rgb_sources: [
                PicaTevSource::PrimaryColor,
                PicaTevSource::Texture0,
                PicaTevSource::PrimaryColor,
            ],
            rgb_operands: [PicaTevRgbOperand::SourceColor; 3],
            rgb_combine: PicaTevCombine::Dot3Rgba,
            alpha_combine: PicaTevCombine::Replace,
            ..interpolate
        };
        let dot = dot.evaluate(0, [0.0; 4], &dot_inputs);
        assert_eq!(dot.unwrap(), [0.75, 0.75, 0.75, 0.75]);

        let scaled = PicaTevStage {
            rgb_scale: 4.0,
            rgb_combine: PicaTevCombine::Add,
            ..interpolate
        };
        assert_eq!(
            scaled.evaluate(0, [0.0; 4], &inputs).unwrap(),
            [1.0, 1.0, 1.0, 0.75]
        );
    }

    #[test]
    fn previous_source_chains_stages_and_first_stage_uses_third_source() {
        let mut first = simple_stage(
            [
                PicaTevSource::Previous,
                PicaTevSource::PrimaryColor,
                PicaTevSource::Texture0,
            ],
            [PicaTevRgbOperand::SourceColor; 3],
            PicaTevCombine::Replace,
            1.0,
        );
        let mut second = first;
        second.rgb_sources = [PicaTevSource::Previous; 3];
        second.rgb_combine = PicaTevCombine::Replace;
        second.alpha_sources = [PicaTevSource::Previous; 3];

        let mut stages = [first; PICA_TEV_STAGE_COUNT];
        stages[1] = second;
        let env = PicaTextureEnvironment { stages };
        let mut inputs = PicaFragmentInputs::default();
        inputs.textures[0] = [0.1, 0.2, 0.3, 0.4];
        assert_eq!(env.evaluate(&inputs).unwrap(), [0.1, 0.2, 0.3, 0.4]);

        first.rgb_sources = [PicaTevSource::Previous; 3];
        first.alpha_sources = [PicaTevSource::Previous; 3];
        let first_only_previous = PicaTevStage { ..first };
        assert_eq!(
            first_only_previous.evaluate(0, [0.0; 4], &inputs).unwrap(),
            [0.0; 4]
        );
    }

    #[test]
    fn stages_four_and_five_use_their_non_contiguous_hardware_register_groups() {
        let mut pica = Pica200::new();
        let stage_four = TEXENV_STAGE_SOURCE_REGISTERS[4];
        let stage_five = TEXENV_STAGE_SOURCE_REGISTERS[5];

        pica.write_register(stage_four, 5 | (5 << 16)).unwrap();
        pica.write_register(stage_five, 14 | (14 << 16)).unwrap();
        pica.write_register(stage_five + 3, 0x4433_2211).unwrap();

        // The unused 0xE0/0xE8 area must not be mistaken for TEV stages 4/5.
        pica.write_register(0x0E0, 0).unwrap();
        pica.write_register(0x0E8, 0).unwrap();

        let environment = PicaTextureEnvironment::from_pica(&pica).unwrap();
        assert_eq!(
            environment.stages[4].rgb_sources[0],
            PicaTevSource::Texture2
        );
        assert_eq!(
            environment.stages[5].rgb_sources[0],
            PicaTevSource::Constant
        );
        assert_eq!(
            environment.stages[5].constant_color,
            [
                0x11 as f32 / 255.0,
                0x22 as f32 / 255.0,
                0x33 as f32 / 255.0,
                0x44 as f32 / 255.0,
            ]
        );
    }

    #[test]
    fn rejects_reserved_register_selectors_instead_of_guessing() {
        let mut pica = Pica200::new();
        pica.write_register(REG_TEXENV0_SOURCE, 7).unwrap();
        assert_eq!(
            pica.texture_environment(),
            Err(PicaTevError::UnsupportedSource {
                stage: 0,
                channel: PicaTevChannel::Rgb,
                slot: 0,
                value: 7,
            })
        );

        pica.write_register(REG_TEXENV0_SOURCE, 0).unwrap();
        pica.write_register(REG_TEXENV0_OPERAND, 6).unwrap();
        assert!(matches!(
            pica.texture_environment(),
            Err(PicaTevError::UnsupportedOperand {
                channel: PicaTevChannel::Rgb,
                value: 6,
                ..
            })
        ));
    }
}
