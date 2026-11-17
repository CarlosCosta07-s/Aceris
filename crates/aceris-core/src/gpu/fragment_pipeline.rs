//! Connects PICA200 2D texture sampling to the six-stage TEV combiner.
//!
//! This is a reusable per-fragment fixed-function stage. Rasterization must
//! supply perspective-correct interpolated colors and texture coordinates.
//! The existing reference rasterizer is intentionally left unchanged until
//! its varying/interpolation contract is extended in a separate step.

use super::{
    GpuPhysicalMemory, Pica200, PicaFragmentInputs, PicaFragmentOutputError,
    PicaFragmentOutputState, PicaTevError, PicaTextureEnvironment, PicaTextureError,
    PicaTextureUnit, REG_TEXUNIT_CONFIG,
};

/// Perspective-correct attributes at one fragment sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaFragmentVaryings {
    pub primary_color: [f32; 4],
    pub fragment_primary_color: [f32; 4],
    pub fragment_secondary_color: [f32; 4],
    /// oTexCoord0, oTexCoord1 and oTexCoord2 in shader-output-map order.
    pub texture_coordinates: [[f32; 2]; 3],
    /// Caller-provided LOD classification for texture units 0, 1 and 2.
    pub minifying: [bool; 3],
}

impl Default for PicaFragmentVaryings {
    fn default() -> Self {
        Self {
            primary_color: [1.0; 4],
            fragment_primary_color: [1.0; 4],
            fragment_secondary_color: [0.0; 4],
            texture_coordinates: [[0.0; 2]; 3],
            minifying: [false; 3],
        }
    }
}

/// Decoded fixed-function fragment state for one PICA draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaFragmentPipeline {
    texture_environment: PicaTextureEnvironment,
    texture_units: [PicaTextureUnit; 3],
    output_state: PicaFragmentOutputState,
    /// Unit 2 can read either oTexCoord2 or oTexCoord1.
    texture2_coordinate_index: usize,
}

impl PicaFragmentPipeline {
    /// Captures TEV and texture-unit registers before a draw is executed.
    pub fn from_pica(pica: &Pica200) -> Result<Self, PicaFragmentPipelineError> {
        let config = pica.register(REG_TEXUNIT_CONFIG).unwrap_or(0);
        if config & (1 << 10) != 0 {
            return Err(PicaFragmentPipelineError::ProceduralTextureUnitUnsupported);
        }

        Ok(Self {
            texture_environment: pica
                .texture_environment()
                .map_err(PicaFragmentPipelineError::Tev)?,
            texture_units: [
                PicaTextureUnit::from_pica(pica, 0).map_err(PicaFragmentPipelineError::Texture)?,
                PicaTextureUnit::from_pica(pica, 1).map_err(PicaFragmentPipelineError::Texture)?,
                PicaTextureUnit::from_pica(pica, 2).map_err(PicaFragmentPipelineError::Texture)?,
            ],
            output_state: pica
                .fragment_output_state()
                .map_err(PicaFragmentPipelineError::Output)?,
            texture2_coordinate_index: if config & (1 << 13) != 0 { 1 } else { 2 },
        })
    }

    /// Evaluates one fragment after texture fetch, using the captured draw state.
    ///
    /// Texture unit 3 is the PICA procedural-texture generator and is not
    /// silently substituted with a normal 2D sampler. Enabling it is rejected
    /// by from_pica; when disabled, its TEV source resolves to zero.
    pub fn shade_fragment(
        &self,
        memory: &GpuPhysicalMemory,
        varyings: &PicaFragmentVaryings,
    ) -> Result<[f32; 4], PicaFragmentPipelineError> {
        validate_color("primary color", varyings.primary_color)?;
        validate_color("fragment primary color", varyings.fragment_primary_color)?;
        validate_color(
            "fragment secondary color",
            varyings.fragment_secondary_color,
        )?;

        let mut inputs = PicaFragmentInputs {
            primary_color: varyings.primary_color,
            fragment_primary_color: varyings.fragment_primary_color,
            fragment_secondary_color: varyings.fragment_secondary_color,
            ..PicaFragmentInputs::default()
        };

        for (index, unit) in self.texture_units.iter().enumerate() {
            if !unit.enabled {
                continue;
            }
            let coordinate_index = if index == 2 {
                self.texture2_coordinate_index
            } else {
                index
            };
            let [s, t] = varyings.texture_coordinates[coordinate_index];
            inputs.textures[index] = unit
                .sample(memory, s, t, varyings.minifying[index])
                .map_err(PicaFragmentPipelineError::Texture)?;
        }

        self.texture_environment
            .evaluate(&inputs)
            .map_err(PicaFragmentPipelineError::Tev)
    }

    /// Applies the captured blend or logic operation to a shaded fragment and
    /// the destination pixel already stored in the RGBA8 color target.
    pub fn apply_fragment_output(
        &self,
        source: [f32; 4],
        destination: [f32; 4],
    ) -> Result<[u8; 4], PicaFragmentPipelineError> {
        self.output_state
            .apply(source, destination)
            .map_err(PicaFragmentPipelineError::Output)
    }
}

impl Pica200 {
    /// Decodes the current fragment-stage state for reuse across one draw.
    pub fn fragment_pipeline(&self) -> Result<PicaFragmentPipeline, PicaFragmentPipelineError> {
        PicaFragmentPipeline::from_pica(self)
    }
}

fn validate_color(name: &'static str, color: [f32; 4]) -> Result<(), PicaFragmentPipelineError> {
    for (channel, value) in color.into_iter().enumerate() {
        if !value.is_finite() {
            return Err(PicaFragmentPipelineError::NonFiniteColor {
                attribute: name,
                channel,
            });
        }
    }
    Ok(())
}

/// Failures while preparing or evaluating the fixed-function fragment stage.
#[derive(Clone, Debug, PartialEq)]
pub enum PicaFragmentPipelineError {
    Texture(PicaTextureError),
    Tev(PicaTevError),
    Output(PicaFragmentOutputError),
    ProceduralTextureUnitUnsupported,
    NonFiniteColor {
        attribute: &'static str,
        channel: usize,
    },
}

impl std::fmt::Display for PicaFragmentPipelineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Texture(error) => {
                write!(formatter, "PICA fragment texture stage failed: {error}")
            }
            Self::Tev(error) => write!(formatter, "PICA TEV combiner failed: {error}"),
            Self::Output(error) => write!(formatter, "PICA fragment output stage failed: {error}"),
            Self::ProceduralTextureUnitUnsupported => {
                formatter.write_str("PICA procedural texture unit 3 is not implemented")
            }
            Self::NonFiniteColor { attribute, channel } => write!(
                formatter,
                "PICA fragment {attribute} channel {channel} is not finite"
            ),
        }
    }
}

impl std::error::Error for PicaFragmentPipelineError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::physical_memory::VRAM_BASE;
    use crate::gpu::{
        PicaTevSource, REG_BLEND_FUNC, REG_COLOR_OPERATION, TEXENV_STAGE_SOURCE_REGISTERS,
    };

    fn configure_texture(pica: &mut Pica200, unit: usize, address: u32, width: u16, height: u16) {
        let registers = match unit {
            0 => [0x081, 0x082, 0x083, 0x085, 0x08E],
            1 => [0x091, 0x092, 0x093, 0x095, 0x096],
            2 => [0x099, 0x09A, 0x09B, 0x09D, 0x09E],
            _ => panic!("test texture unit out of range"),
        };
        pica.write_register(registers[0], 0).unwrap();
        pica.write_register(registers[1], (u32::from(width) << 16) | u32::from(height))
            .unwrap();
        pica.write_register(registers[2], 0).unwrap();
        pica.write_register(registers[3], address >> 3).unwrap();
        pica.write_register(registers[4], 0).unwrap();
    }

    fn configure_tev(pica: &mut Pica200, first_source: PicaTevSource) {
        for stage in 0..6u16 {
            let base = TEXENV_STAGE_SOURCE_REGISTERS[stage as usize];
            let source = if stage == 0 {
                let value = match first_source {
                    PicaTevSource::PrimaryColor => 0,
                    PicaTevSource::FragmentPrimaryColor => 1,
                    PicaTevSource::FragmentSecondaryColor => 2,
                    PicaTevSource::Texture0 => 3,
                    PicaTevSource::Texture1 => 4,
                    PicaTevSource::Texture2 => 5,
                    PicaTevSource::Texture3 => 6,
                    PicaTevSource::PreviousBuffer => 13,
                    PicaTevSource::Constant => 14,
                    PicaTevSource::Previous => 15,
                };
                value | (value << 16)
            } else {
                15 | (15 << 16)
            };
            pica.write_register(base, source).unwrap();
            pica.write_register(base + 1, 0).unwrap();
            pica.write_register(base + 2, 0).unwrap();
            pica.write_register(base + 3, 0).unwrap();
            pica.write_register(base + 4, 0).unwrap();
        }
    }

    #[test]
    fn texture_fetch_flows_through_all_six_tev_stages() {
        let mut pica = Pica200::new();
        pica.write_register(REG_TEXUNIT_CONFIG, 1).unwrap();
        configure_texture(&mut pica, 0, VRAM_BASE, 8, 8);
        configure_tev(&mut pica, PicaTevSource::Texture0);

        let mut memory = GpuPhysicalMemory::new();
        let mut texture = Vec::with_capacity(8 * 8 * 4);
        for _ in 0..64 {
            texture.extend_from_slice(&[10, 20, 30, 40]);
        }
        memory.write(VRAM_BASE, &texture).unwrap();

        let pipeline = pica.fragment_pipeline().unwrap();
        let varyings = PicaFragmentVaryings {
            texture_coordinates: [[2.5 / 8.0, 3.5 / 8.0]; 3],
            ..PicaFragmentVaryings::default()
        };
        assert_eq!(
            pipeline.shade_fragment(&memory, &varyings).unwrap(),
            [10.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0, 40.0 / 255.0]
        );
    }

    #[test]
    fn texture_two_uses_the_guest_selected_coordinate_set() {
        let mut pica = Pica200::new();
        pica.write_register(REG_TEXUNIT_CONFIG, (1 << 2) | (1 << 13))
            .unwrap();
        configure_texture(&mut pica, 2, VRAM_BASE, 8, 8);
        configure_tev(&mut pica, PicaTevSource::Texture2);

        let mut memory = GpuPhysicalMemory::new();
        let mut texture = vec![0; 8 * 8 * 4];
        for y in 0..8 {
            for x in 0..8 {
                let morton = (x & 1)
                    | ((y & 1) << 1)
                    | ((x & 2) << 1)
                    | ((y & 2) << 2)
                    | ((x & 4) << 2)
                    | ((y & 4) << 3);
                texture[morton * 4..morton * 4 + 4].copy_from_slice(&[
                    x as u8 * 20,
                    y as u8 * 20,
                    0,
                    255,
                ]);
            }
        }
        memory.write(VRAM_BASE, &texture).unwrap();

        let pipeline = pica.fragment_pipeline().unwrap();
        let varyings = PicaFragmentVaryings {
            texture_coordinates: [
                [0.5 / 8.0, 0.5 / 8.0],
                [1.5 / 8.0, 2.5 / 8.0],
                [5.5 / 8.0, 6.5 / 8.0],
            ],
            ..PicaFragmentVaryings::default()
        };
        let output = pipeline.shade_fragment(&memory, &varyings).unwrap();
        assert_eq!(output[0], 20.0 / 255.0);
        assert_eq!(output[1], 40.0 / 255.0);
    }

    #[test]
    fn applies_guest_configured_framebuffer_blending_after_tev() {
        let mut pica = Pica200::new();
        pica.write_register(REG_COLOR_OPERATION, 1 << 8).unwrap();
        pica.write_register(
            REG_BLEND_FUNC,
            0 | (0 << 8) | (6 << 16) | (7 << 20) | (1 << 24),
        )
        .unwrap();
        let pipeline = pica.fragment_pipeline().unwrap();

        assert_eq!(
            pipeline.apply_fragment_output(
                [1.0, 0.0, 0.0, 0.5],
                [0.0, 0.0, 1.0, 1.0],
            ),
            Ok([128, 0, 128, 128])
        );
    }

    #[test]
    fn rejects_enabled_procedural_unit_instead_of_using_zero_texels() {
        let mut pica = Pica200::new();
        pica.write_register(REG_TEXUNIT_CONFIG, 1 << 10).unwrap();
        assert_eq!(
            pica.fragment_pipeline(),
            Err(PicaFragmentPipelineError::ProceduralTextureUnitUnsupported)
        );
    }

    #[test]
    fn rejects_non_finite_fragment_colors() {
        let pica = Pica200::new();
        let pipeline = pica.fragment_pipeline().unwrap();
        let varyings = PicaFragmentVaryings {
            primary_color: [f32::NAN, 1.0, 1.0, 1.0],
            ..PicaFragmentVaryings::default()
        };
        assert_eq!(
            pipeline.shade_fragment(&GpuPhysicalMemory::new(), &varyings),
            Err(PicaFragmentPipelineError::NonFiniteColor {
                attribute: "primary color",
                channel: 0,
            })
        );
    }
}
