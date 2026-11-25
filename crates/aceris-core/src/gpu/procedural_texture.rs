//! PICA200 procedural texture unit 3, with explicit support boundaries.
//!
//! This first backend covers non-noise, non-mipmapped draws. It snapshots the
//! guest LUTs at draw setup, transforms the selected interpolated coordinate
//! pair, evaluates the configured base-shape functions, and samples the RGB
//! and optional alpha maps. Unknown hardware behavior is rejected rather than
//! replaced with a synthetic texture.

use std::fmt;

use super::{
    Pica200, PicaProceduralTextureClamp, PicaProceduralTextureConfig,
    PicaProceduralTextureCurveEntry, PicaProceduralTextureFilter, PicaProceduralTextureLut,
    PicaProceduralTextureMappingFunction, PicaProceduralTextureShift,
};

const CURVE_LUT_SIZE: usize = 128;
const COLOR_LUT_SIZE: usize = 256;

/// A PICA procedural-texture configuration or sampling mode not implemented
/// by this backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicaProceduralTextureError {
    NoiseGenerationUnsupported,
    MipmapFilterUnsupported(PicaProceduralTextureFilter),
    TextureBiasUnsupported,
    CoordinateSourceUnsupported,
    ClampModeUnsupported(PicaProceduralTextureClamp),
    ShiftModeUnsupported(PicaProceduralTextureShift),
    MappingFunctionUnsupported(PicaProceduralTextureMappingFunction),
    InvalidTextureWidth(u8),
    InvalidColorTableRange { offset: u8, width: u8 },
    InvalidCoordinates,
    MappingCoordinateOutOfRange,
    ColorLookupOutOfRange,
}

impl fmt::Display for PicaProceduralTextureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoiseGenerationUnsupported => {
                formatter.write_str("procedural noise generation is not implemented")
            }
            Self::MipmapFilterUnsupported(filter) => {
                write!(
                    formatter,
                    "procedural mipmap filter {filter:?} is not implemented"
                )
            }
            Self::TextureBiasUnsupported => {
                formatter.write_str("non-zero procedural texture bias is not implemented")
            }
            Self::CoordinateSourceUnsupported => {
                formatter.write_str("procedural texture coordinate selector is reserved")
            }
            Self::ClampModeUnsupported(mode) => {
                write!(
                    formatter,
                    "procedural coordinate clamp {mode:?} is reserved"
                )
            }
            Self::ShiftModeUnsupported(mode) => {
                write!(
                    formatter,
                    "procedural coordinate shift {mode:?} is reserved"
                )
            }
            Self::MappingFunctionUnsupported(function) => {
                write!(
                    formatter,
                    "procedural mapping function {function:?} is not implemented"
                )
            }
            Self::InvalidTextureWidth(width) => {
                write!(formatter, "procedural color table width {width} is invalid")
            }
            Self::InvalidColorTableRange { offset, width } => write!(
                formatter,
                "procedural color table range {offset}..{} exceeds the 256-entry LUT",
                u16::from(*offset) + u16::from(*width)
            ),
            Self::InvalidCoordinates => {
                formatter.write_str("procedural texture coordinates are non-finite or too large")
            }
            Self::MappingCoordinateOutOfRange => {
                formatter.write_str("procedural mapping LUT coordinate is outside [0, 1]")
            }
            Self::ColorLookupOutOfRange => {
                formatter.write_str("procedural color lookup produced an invalid LUT index")
            }
        }
    }
}

impl std::error::Error for PicaProceduralTextureError {}

/// Draw-local snapshot of the PICA procedural-texture registers and lookup
/// tables. The snapshot keeps later FIFO uploads from changing an in-flight
/// draw, matching the other fragment-stage state snapshots.
#[derive(Clone, Debug, PartialEq)]
pub struct PicaProceduralTextureSampler {
    config: PicaProceduralTextureConfig,
    rgb_mapping: [PicaProceduralTextureCurveEntry; CURVE_LUT_SIZE],
    alpha_mapping: [PicaProceduralTextureCurveEntry; CURVE_LUT_SIZE],
    colors: [[u8; 4]; COLOR_LUT_SIZE],
    color_differences: [[i8; 4]; COLOR_LUT_SIZE],
}

impl PicaProceduralTextureSampler {
    /// Captures a supported non-noise, level-zero procedural texture setup.
    pub fn from_pica(pica: &Pica200) -> Result<Self, PicaProceduralTextureError> {
        let config = pica.procedural_texture_config();

        if config.noise_enabled {
            return Err(PicaProceduralTextureError::NoiseGenerationUnsupported);
        }
        if !matches!(
            config.minification_filter,
            PicaProceduralTextureFilter::Nearest | PicaProceduralTextureFilter::Linear
        ) {
            return Err(PicaProceduralTextureError::MipmapFilterUnsupported(
                config.minification_filter,
            ));
        }
        if config.texture_bias_lower != 0 || config.texture_bias_upper != 0 {
            return Err(PicaProceduralTextureError::TextureBiasUnsupported);
        }
        if matches!(
            config.coordinate_source,
            super::PicaProceduralTextureCoordinateSource::Reserved(_)
        ) {
            return Err(PicaProceduralTextureError::CoordinateSourceUnsupported);
        }
        validate_clamp(config.u_clamp)?;
        validate_clamp(config.v_clamp)?;
        validate_shift(config.u_shift)?;
        validate_shift(config.v_shift)?;
        validate_mapping(config.rgb_mapping_function)?;
        if config.separate_alpha {
            validate_mapping(config.alpha_mapping_function)?;
        }

        let width = usize::from(config.texture_width);
        let offset = usize::from(config.mip_offsets[0]);
        if width == 0 {
            return Err(PicaProceduralTextureError::InvalidTextureWidth(
                config.texture_width,
            ));
        }
        if offset + width > COLOR_LUT_SIZE {
            return Err(PicaProceduralTextureError::InvalidColorTableRange {
                offset: config.mip_offsets[0],
                width: config.texture_width,
            });
        }

        let rgb_mapping = std::array::from_fn(|index| {
            pica.procedural_texture_curve_entry(PicaProceduralTextureLut::RgbMapping, index)
                .expect("every index in the RGB mapping table is valid")
        });
        let alpha_mapping = std::array::from_fn(|index| {
            pica.procedural_texture_curve_entry(PicaProceduralTextureLut::AlphaMapping, index)
                .expect("every index in the alpha mapping table is valid")
        });
        let colors = std::array::from_fn(|index| {
            pica.procedural_texture_color_entry(index)
                .expect("every index in the procedural color table is valid")
        });
        let color_differences = std::array::from_fn(|index| {
            pica.procedural_texture_color_difference_entry(index)
                .expect("every index in the procedural color-difference table is valid")
        });

        Ok(Self {
            config,
            rgb_mapping,
            alpha_mapping,
            colors,
            color_differences,
        })
    }

    /// Evaluates one procedural texel from the three perspective-correct UV
    /// pairs carried by the fragment pipeline.
    pub fn sample(
        &self,
        texture_coordinates: &[[f32; 2]; 3],
    ) -> Result<[f32; 4], PicaProceduralTextureError> {
        let [mut u, mut v] = self
            .config
            .select_coordinates(texture_coordinates)
            .ok_or(PicaProceduralTextureError::CoordinateSourceUnsupported)?;
        if !u.is_finite() || !v.is_finite() {
            return Err(PicaProceduralTextureError::InvalidCoordinates);
        }

        // PICA procedural coordinates are folded into the positive domain
        // before the row/column shifts and repeat modes are applied.
        u = u.abs();
        v = v.abs();

        let u_shift = shift_offset(self.config.u_shift, v, self.config.u_clamp)?;
        let v_shift = shift_offset(self.config.v_shift, u, self.config.v_clamp)?;
        u += u_shift;
        v += v_shift;
        u = clamp_coordinate(u, self.config.u_clamp)?;
        v = clamp_coordinate(v, self.config.v_clamp)?;

        let rgb_shape = self
            .config
            .rgb_mapping_function
            .evaluate_raw(u, v)
            .ok_or_else(|| {
                PicaProceduralTextureError::MappingFunctionUnsupported(
                    self.config.rgb_mapping_function,
                )
            })?;
        let rgb_lookup = lookup_curve(&self.rgb_mapping, rgb_shape)?;
        let color_position = f32::from(self.config.mip_offsets[0])
            + rgb_lookup * f32::from(self.config.texture_width.saturating_sub(1));
        let mut color = self.sample_color_table(color_position)?;

        if self.config.separate_alpha {
            let alpha_shape = self
                .config
                .alpha_mapping_function
                .evaluate_raw(u, v)
                .ok_or_else(|| {
                    PicaProceduralTextureError::MappingFunctionUnsupported(
                        self.config.alpha_mapping_function,
                    )
                })?;
            color[3] = lookup_curve(&self.alpha_mapping, alpha_shape)?;
        }

        Ok(color)
    }

    fn sample_color_table(&self, position: f32) -> Result<[f32; 4], PicaProceduralTextureError> {
        if !position.is_finite() || !(0.0..=(COLOR_LUT_SIZE - 1) as f32).contains(&position) {
            return Err(PicaProceduralTextureError::ColorLookupOutOfRange);
        }

        let (index, fraction) = match self.config.minification_filter {
            PicaProceduralTextureFilter::Nearest => (position.round() as usize, 0.0),
            PicaProceduralTextureFilter::Linear => {
                let lower = position.floor();
                (lower as usize, position - lower)
            }
            filter => {
                return Err(PicaProceduralTextureError::MipmapFilterUnsupported(filter));
            }
        };

        let base = self.colors[index];
        let difference = self.color_differences[index];
        Ok(std::array::from_fn(|channel| {
            (f32::from(base[channel]) + 2.0 * f32::from(difference[channel]) * fraction)
                .clamp(0.0, 255.0)
                / 255.0
        }))
    }
}

fn validate_clamp(mode: PicaProceduralTextureClamp) -> Result<(), PicaProceduralTextureError> {
    if matches!(mode, PicaProceduralTextureClamp::Reserved(_)) {
        Err(PicaProceduralTextureError::ClampModeUnsupported(mode))
    } else {
        Ok(())
    }
}

fn validate_shift(mode: PicaProceduralTextureShift) -> Result<(), PicaProceduralTextureError> {
    if matches!(mode, PicaProceduralTextureShift::Reserved(_)) {
        Err(PicaProceduralTextureError::ShiftModeUnsupported(mode))
    } else {
        Ok(())
    }
}

fn validate_mapping(
    function: PicaProceduralTextureMappingFunction,
) -> Result<(), PicaProceduralTextureError> {
    if matches!(
        function,
        PicaProceduralTextureMappingFunction::Rmax
            | PicaProceduralTextureMappingFunction::Reserved(_)
    ) {
        Err(PicaProceduralTextureError::MappingFunctionUnsupported(
            function,
        ))
    } else {
        Ok(())
    }
}

/// Samples a 128-entry fixed-point curve. Coordinate 1.0 addresses the final
/// entry at its end point rather than wrapping to entry zero.
fn lookup_curve(
    table: &[PicaProceduralTextureCurveEntry; CURVE_LUT_SIZE],
    coordinate: f32,
) -> Result<f32, PicaProceduralTextureError> {
    if !coordinate.is_finite() || !(0.0..=1.0).contains(&coordinate) {
        return Err(PicaProceduralTextureError::MappingCoordinateOutOfRange);
    }
    let scaled = coordinate * CURVE_LUT_SIZE as f32;
    let floor = scaled.floor();
    let index = (floor as usize).min(CURVE_LUT_SIZE - 1);
    let fraction = scaled - index as f32;
    let result = table[index]
        .sample_linear(fraction)
        .ok_or(PicaProceduralTextureError::MappingCoordinateOutOfRange)?;
    if !result.is_finite() || !(0.0..=1.0).contains(&result) {
        return Err(PicaProceduralTextureError::MappingCoordinateOutOfRange);
    }
    Ok(result)
}

fn shift_offset(
    shift: PicaProceduralTextureShift,
    other_coordinate: f32,
    clamp: PicaProceduralTextureClamp,
) -> Result<f32, PicaProceduralTextureError> {
    let step = if clamp == PicaProceduralTextureClamp::MirroredRepeat {
        1.0
    } else {
        0.5
    };
    if shift == PicaProceduralTextureShift::None {
        return Ok(0.0);
    }
    if !other_coordinate.is_finite() || other_coordinate > i32::MAX as f32 {
        return Err(PicaProceduralTextureError::InvalidCoordinates);
    }
    let row = other_coordinate.floor() as i64;
    let parity = match shift {
        PicaProceduralTextureShift::None => 0,
        PicaProceduralTextureShift::Odd => (row / 2).rem_euclid(2),
        PicaProceduralTextureShift::Even => ((row + 1) / 2).rem_euclid(2),
        PicaProceduralTextureShift::Reserved(_) => {
            return Err(PicaProceduralTextureError::ShiftModeUnsupported(shift));
        }
    };
    Ok(step * parity as f32)
}

fn clamp_coordinate(
    coordinate: f32,
    mode: PicaProceduralTextureClamp,
) -> Result<f32, PicaProceduralTextureError> {
    if !coordinate.is_finite() || coordinate < 0.0 {
        return Err(PicaProceduralTextureError::InvalidCoordinates);
    }
    Ok(match mode {
        PicaProceduralTextureClamp::ClampToZero => {
            if coordinate > 1.0 {
                0.0
            } else {
                coordinate
            }
        }
        PicaProceduralTextureClamp::ClampToEdge => coordinate.min(1.0),
        PicaProceduralTextureClamp::SymmetricalRepeat => coordinate - coordinate.floor(),
        PicaProceduralTextureClamp::MirroredRepeat => {
            if coordinate > i32::MAX as f32 {
                return Err(PicaProceduralTextureError::InvalidCoordinates);
            }
            let integral = coordinate.floor() as i64;
            let fraction = coordinate - integral as f32;
            if integral.rem_euclid(2) == 0 {
                fraction
            } else {
                1.0 - fraction
            }
        }
        PicaProceduralTextureClamp::Pulse => {
            if coordinate <= 0.5 {
                0.0
            } else {
                1.0
            }
        }
        PicaProceduralTextureClamp::Reserved(_) => {
            return Err(PicaProceduralTextureError::ClampModeUnsupported(mode));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{
        GpuPhysicalMemory, PicaFragmentPipeline, PicaFragmentVaryings, PicaProceduralTextureLut,
        REG_PROCTEX_LUT, REG_PROCTEX_LUT_DATA0, REG_TEXUNIT_CONFIG, REG_TEXUNIT3_PROCTEX0,
        REG_TEXUNIT3_PROCTEX4, REG_TEXUNIT3_PROCTEX5, TEXENV_STAGE_SOURCE_REGISTERS,
    };

    fn write(pica: &mut Pica200, register: u16, value: u32) {
        pica.write_register(register, value).unwrap();
    }

    fn fixture(separate_alpha: bool, filter: PicaProceduralTextureFilter) -> Pica200 {
        let mut pica = Pica200::new();
        write(&mut pica, REG_TEXUNIT_CONFIG, 1 << 10);
        let alpha_flag = if separate_alpha { 1 << 14 } else { 0 };
        write(&mut pica, REG_TEXUNIT3_PROCTEX0, 1 | (1 << 3) | alpha_flag);
        let filter_value = match filter {
            PicaProceduralTextureFilter::Nearest => 0,
            PicaProceduralTextureFilter::Linear => 1,
            _ => 0,
        };
        write(&mut pica, REG_TEXUNIT3_PROCTEX4, (4 << 11) | filter_value);
        write(&mut pica, REG_TEXUNIT3_PROCTEX5, 0);

        write(
            &mut pica,
            REG_PROCTEX_LUT,
            (PicaProceduralTextureLut::RgbMapping as u32) << 8,
        );
        for _ in 0..CURVE_LUT_SIZE {
            write(&mut pica, REG_PROCTEX_LUT_DATA0, 0x0000_0800);
        }
        if separate_alpha {
            write(
                &mut pica,
                REG_PROCTEX_LUT,
                (PicaProceduralTextureLut::AlphaMapping as u32) << 8,
            );
            for _ in 0..CURVE_LUT_SIZE {
                write(&mut pica, REG_PROCTEX_LUT_DATA0, 0x0000_0400);
            }
        }

        write(
            &mut pica,
            REG_PROCTEX_LUT,
            (PicaProceduralTextureLut::Color as u32) << 8,
        );
        for [r, g, b, a] in [
            [0, 0, 0, 255],
            [10, 20, 30, 255],
            [100, 110, 120, 200],
            [150, 160, 170, 128],
        ] {
            let packed =
                u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16) | (u32::from(a) << 24);
            write(&mut pica, REG_PROCTEX_LUT_DATA0, packed);
        }
        pica
    }

    fn assert_color(actual: [f32; 4], expected: [u8; 4]) {
        for channel in 0..4 {
            let expected = f32::from(expected[channel]) / 255.0;
            assert!((actual[channel] - expected).abs() < 1.0e-6);
        }
    }

    #[test]
    fn samples_rgb_mapping_and_color_luts_when_noise_is_disabled() {
        let pica = fixture(false, PicaProceduralTextureFilter::Nearest);
        let sampler = PicaProceduralTextureSampler::from_pica(&pica).unwrap();
        let color = sampler.sample(&[[0.5, 0.25], [0.0; 2], [0.0; 2]]).unwrap();
        assert_color(color, [100, 110, 120, 200]);
    }

    #[test]
    fn separate_alpha_uses_its_own_mapping_lut() {
        let pica = fixture(true, PicaProceduralTextureFilter::Nearest);
        let sampler = PicaProceduralTextureSampler::from_pica(&pica).unwrap();
        let color = sampler
            .sample(&[[0.5, 0.25], [0.0; 2], [0.0; 2]])
            .unwrap();
        assert_color([color[0], color[1], color[2], 1.0], [100, 110, 120, 255]);
        assert!((color[3] - 0.25).abs() < 1.0e-6);
    }

    #[test]
    fn linear_filter_interpolates_between_color_entries() {
        let mut pica = fixture(false, PicaProceduralTextureFilter::Linear);
        write(
            &mut pica,
            REG_PROCTEX_LUT,
            ((PicaProceduralTextureLut::ColorDifference as u32) << 8) | 1,
        );
        write(&mut pica, REG_PROCTEX_LUT_DATA0, 0x0019_1919);
        let sampler = PicaProceduralTextureSampler::from_pica(&pica).unwrap();
        let color = sampler
            .sample(&[[0.5, 0.25], [0.0; 2], [0.0; 2]])
            .unwrap();
        assert_color(color, [35, 45, 55, 255]);
    }

    #[test]
    fn noise_and_mipmap_modes_fail_closed() {
        let mut noisy = fixture(false, PicaProceduralTextureFilter::Nearest);
        let original = noisy.register(REG_TEXUNIT3_PROCTEX0).unwrap();
        write(&mut noisy, REG_TEXUNIT3_PROCTEX0, original | (1 << 15));
        assert_eq!(
            PicaProceduralTextureSampler::from_pica(&noisy),
            Err(PicaProceduralTextureError::NoiseGenerationUnsupported)
        );

        let mut mipmapped = fixture(false, PicaProceduralTextureFilter::Nearest);
        write(&mut mipmapped, REG_TEXUNIT3_PROCTEX4, (4 << 11) | 2);
        assert_eq!(
            PicaProceduralTextureSampler::from_pica(&mipmapped),
            Err(PicaProceduralTextureError::MipmapFilterUnsupported(
                PicaProceduralTextureFilter::NearestMipmapNearest
            ))
        );
    }

    #[test]
    fn procedural_texture_reaches_the_tev_texture3_input() {
        let mut pica = fixture(false, PicaProceduralTextureFilter::Nearest);
        for (stage, base) in TEXENV_STAGE_SOURCE_REGISTERS.iter().copied().enumerate() {
            let source = if stage == 0 {
                6 | (6 << 16)
            } else {
                15 | (15 << 16)
            };
            write(&mut pica, base, source);
            write(&mut pica, base + 1, 0);
            write(&mut pica, base + 2, 0);
        }
        let pipeline = PicaFragmentPipeline::from_pica(&pica).unwrap();
        let varyings = PicaFragmentVaryings {
            texture_coordinates: [[0.5, 0.25], [0.0; 2], [0.0; 2]],
            ..PicaFragmentVaryings::default()
        };
        let result = pipeline
            .shade_fragment(&GpuPhysicalMemory::new(), &varyings)
            .unwrap();
        assert_color(result, [100, 110, 120, 200]);
    }

    #[test]
    fn clamp_and_shift_modes_are_applied_before_mapping() {
        assert_eq!(
            clamp_coordinate(1.25, PicaProceduralTextureClamp::ClampToZero).unwrap(),
            0.0
        );
        assert_eq!(
            clamp_coordinate(1.25, PicaProceduralTextureClamp::ClampToEdge).unwrap(),
            1.0
        );
        assert_eq!(
            clamp_coordinate(2.25, PicaProceduralTextureClamp::SymmetricalRepeat).unwrap(),
            0.25
        );
        assert_eq!(
            clamp_coordinate(1.25, PicaProceduralTextureClamp::MirroredRepeat).unwrap(),
            0.75
        );
        assert_eq!(
            clamp_coordinate(0.5, PicaProceduralTextureClamp::Pulse).unwrap(),
            0.0
        );
        assert_eq!(
            shift_offset(
                PicaProceduralTextureShift::Odd,
                2.0,
                PicaProceduralTextureClamp::ClampToEdge,
            )
            .unwrap(),
            0.5
        );
        assert_eq!(
            shift_offset(
                PicaProceduralTextureShift::Even,
                1.0,
                PicaProceduralTextureClamp::MirroredRepeat,
            )
            .unwrap(),
            1.0
        );
    }
}
