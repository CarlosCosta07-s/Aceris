//! PICA200 procedural texture unit 3, with explicit support boundaries.
//!
//! This backend snapshots the guest LUTs at draw setup, transforms the
//! selected interpolated coordinate pair, evaluates the configured base-shape
//! functions, and samples the RGB and optional alpha maps. Mipmap filter modes
//! use the software-reference level-zero nearest/linear fallback; actual LOD
//! selection is not implemented. Noise uses the public Citra/Azahar reference
//! algorithm; exact equivalence to the physical PICA noise generator is unknown.

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
    InvalidNoiseParameters,
    FilterUnsupported(PicaProceduralTextureFilter),
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
            Self::InvalidNoiseParameters => {
                formatter.write_str("procedural noise parameters are invalid or out of range")
            }
            Self::FilterUnsupported(filter) => {
                write!(
                    formatter,
                    "procedural texture filter {filter:?} is reserved"
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

/// Binary16/fixed-point noise parameters and the draw-local noise LUT.
///
/// Decoding happens once when the PICA draw state is captured, not once per
/// fragment. This keeps the inner sampler free of repeated format conversion.
#[derive(Clone, Debug, PartialEq)]
struct ProceduralNoiseState {
    frequency: [f32; 2],
    phase: [f32; 2],
    amplitude: [f32; 2],
    lut: [PicaProceduralTextureCurveEntry; CURVE_LUT_SIZE],
}

impl ProceduralNoiseState {
    fn from_pica(
        pica: &Pica200,
        config: &PicaProceduralTextureConfig,
    ) -> Result<Self, PicaProceduralTextureError> {
        let frequency = [
            decode_float16(config.u_noise_frequency_float1_5_10),
            decode_float16(config.v_noise_frequency_float1_5_10),
        ];
        let phase = [
            decode_float16(config.u_noise_phase_float1_5_10),
            decode_float16(config.v_noise_phase_float1_5_10),
        ];
        if frequency
            .iter()
            .chain(phase.iter())
            .any(|value| !value.is_finite())
            || frequency.iter().any(|value| *value < 0.0)
        {
            return Err(PicaProceduralTextureError::InvalidNoiseParameters);
        }

        let amplitude = [
            f32::from(config.u_noise_amplitude_fixed1_3_12 as i16) / 4095.0,
            f32::from(config.v_noise_amplitude_fixed1_3_12 as i16) / 4095.0,
        ];
        let lut = std::array::from_fn(|index| {
            pica.procedural_texture_curve_entry(PicaProceduralTextureLut::Noise, index)
                .expect("every index in the procedural noise table is valid")
        });

        Ok(Self {
            frequency,
            phase,
            amplitude,
            lut,
        })
    }
}

/// Draw-local snapshot of the PICA procedural-texture registers and lookup
/// tables. The snapshot keeps later FIFO uploads from changing an in-flight
/// draw, matching the other fragment-stage state snapshots.
#[derive(Clone, Debug, PartialEq)]
pub struct PicaProceduralTextureSampler {
    config: PicaProceduralTextureConfig,
    noise: Option<ProceduralNoiseState>,
    rgb_mapping: [PicaProceduralTextureCurveEntry; CURVE_LUT_SIZE],
    alpha_mapping: [PicaProceduralTextureCurveEntry; CURVE_LUT_SIZE],
    colors: [[u8; 4]; COLOR_LUT_SIZE],
    color_differences: [[i8; 4]; COLOR_LUT_SIZE],
}

impl PicaProceduralTextureSampler {
    /// Captures a procedural texture setup using the level-zero filter fallback.
    ///
    /// Mipmap selectors retain their within-level nearest/linear behavior, but
    /// choosing mip levels and applying offsets 1–3 remain unimplemented.
    pub fn from_pica(pica: &Pica200) -> Result<Self, PicaProceduralTextureError> {
        let config = pica.procedural_texture_config();

        if matches!(
            config.minification_filter,
            PicaProceduralTextureFilter::Reserved(_)
        ) {
            return Err(PicaProceduralTextureError::FilterUnsupported(
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

        let noise = if config.noise_enabled {
            Some(ProceduralNoiseState::from_pica(pica, &config)?)
        } else {
            None
        };
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
            noise,
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

        if let Some(noise_state) = &self.noise {
            let noise = procedural_noise_coefficient(u, v, noise_state)?;
            u = (u + noise * noise_state.amplitude[0]).abs();
            v = (v + noise * noise_state.amplitude[1]).abs();
        }

        u += u_shift;
        v += v_shift;
        u = clamp_coordinate(u, self.config.u_clamp)?;
        v = clamp_coordinate(v, self.config.v_clamp)?;

        let rgb_shape = self
            .config
            .rgb_mapping_function
            .evaluate_for_lookup(u, v)
            .ok_or(PicaProceduralTextureError::MappingFunctionUnsupported(
                self.config.rgb_mapping_function,
            ))?;
        let rgb_lookup = lookup_curve(&self.rgb_mapping, rgb_shape)?;
        let color_position = f32::from(self.config.mip_offsets[0])
            + rgb_lookup * f32::from(self.config.texture_width.saturating_sub(1));
        let mut color = self.sample_color_table(color_position)?;

        if self.config.separate_alpha {
            let alpha_shape = self
                .config
                .alpha_mapping_function
                .evaluate_for_lookup(u, v)
                .ok_or(PicaProceduralTextureError::MappingFunctionUnsupported(
                    self.config.alpha_mapping_function,
                ))?;
            color[3] = lookup_curve(&self.alpha_mapping, alpha_shape)?;
        }

        Ok(color)
    }

    fn sample_color_table(&self, position: f32) -> Result<[f32; 4], PicaProceduralTextureError> {
        if !position.is_finite() || !(0.0..=(COLOR_LUT_SIZE - 1) as f32).contains(&position) {
            return Err(PicaProceduralTextureError::ColorLookupOutOfRange);
        }

        let (index, fraction) = match self.config.minification_filter {
            PicaProceduralTextureFilter::Nearest
            | PicaProceduralTextureFilter::NearestMipmapNearest
            | PicaProceduralTextureFilter::NearestMipmapLinear => (position.round() as usize, 0.0),
            PicaProceduralTextureFilter::Linear
            | PicaProceduralTextureFilter::LinearMipmapNearest
            | PicaProceduralTextureFilter::LinearMipmapLinear => {
                let lower = position.floor();
                (lower as usize, position - lower)
            }
            filter @ PicaProceduralTextureFilter::Reserved(_) => {
                return Err(PicaProceduralTextureError::FilterUnsupported(filter));
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

/// Decodes an IEEE-754 binary16 value from its guest register representation.
/// Subnormals are preserved, matching the inspected software reference.
fn decode_float16(raw: u16) -> f32 {
    let sign = if raw & 0x8000 != 0 { 0x8000_0000 } else { 0 };
    let exponent = (raw >> 10) & 0x1F;
    let mantissa = u32::from(raw & 0x03FF);

    match exponent {
        0 if mantissa == 0 => f32::from_bits(sign),
        0 => {
            let value = mantissa as f32 / 16_777_216.0;
            if sign == 0 { value } else { -value }
        }
        0x1F if mantissa == 0 => f32::from_bits(sign | 0x7F80_0000),
        0x1F => f32::from_bits(sign | 0x7FC0_0000 | (mantissa << 13)),
        _ => {
            let exponent32 = u32::from(exponent) + 112;
            f32::from_bits(sign | (exponent32 << 23) | (mantissa << 13))
        }
    }
}

/// Deterministic one-dimensional hash used by the public software-reference
/// model of PICA procedural noise. It is not claimed to reproduce the exact
/// internal hardware PRNG.
fn noise_rand_1d(value: u32) -> u32 {
    const TABLE: [u32; 16] = [0, 4, 10, 8, 4, 9, 7, 12, 5, 15, 13, 14, 11, 15, 2, 11];
    (((value % 9 + 2) * 3) & 0xF) ^ TABLE[((value / 9) & 0xF) as usize]
}

/// Two-dimensional signed pseudo-random gradient from precomputed 1D hashes.
fn noise_gradient_from_hashes(u: u32, mut v: u32) -> f32 {
    const TABLE: [u32; 16] = [10, 2, 15, 8, 0, 7, 4, 5, 5, 13, 2, 6, 13, 9, 3, 14];
    if (u & 3) == 1 {
        v += 4;
    }
    v ^= (u & 1) * 6;
    v += 10 + u;
    v &= 0xF;
    v ^= TABLE[u as usize];
    -1.0 + (v as f32 * 2.0 / 15.0)
}

/// Generates the deterministic reference noise coefficient at one U/V pair.
/// Register decoding and amplitude conversion happen once when the draw
/// state is captured; only coordinate-dependent work remains per fragment.
fn procedural_noise_coefficient(
    u: f32,
    v: f32,
    state: &ProceduralNoiseState,
) -> Result<f32, PicaProceduralTextureError> {
    let x = 9.0 * state.frequency[0] * (u + state.phase[0]).abs();
    let y = 9.0 * state.frequency[1] * (v + state.phase[1]).abs();
    // The reference converts these coordinates to signed 32-bit integers,
    // then hashes the integer lattice points and their +1 neighbours.
    if !x.is_finite()
        || !y.is_finite()
        || x < 0.0
        || y < 0.0
        || x >= (i32::MAX - 1) as f32
        || y >= (i32::MAX - 1) as f32
    {
        return Err(PicaProceduralTextureError::InvalidNoiseParameters);
    }

    let x_integer = x as u32;
    let y_integer = y as u32;
    let x_fraction = x - x_integer as f32;
    let y_fraction = y - y_integer as f32;

    let x_hashes = [noise_rand_1d(x_integer), noise_rand_1d(x_integer + 1)];
    let y_hashes = [noise_rand_1d(y_integer), noise_rand_1d(y_integer + 1)];

    let fraction_sum = x_fraction + y_fraction;
    let g00 = noise_gradient_from_hashes(x_hashes[0], y_hashes[0]) * fraction_sum;
    let g10 = noise_gradient_from_hashes(x_hashes[1], y_hashes[0]) * (fraction_sum - 1.0);
    let g01 = noise_gradient_from_hashes(x_hashes[0], y_hashes[1]) * (fraction_sum - 1.0);
    let g11 = noise_gradient_from_hashes(x_hashes[1], y_hashes[1]) * (fraction_sum - 2.0);

    let blend_x = lookup_curve(&state.lut, x_fraction)?;
    let blend_y = lookup_curve(&state.lut, y_fraction)?;
    let row0 = g00 + (g10 - g00) * blend_x;
    let row1 = g01 + (g11 - g01) * blend_x;
    let noise = row0 + (row1 - row0) * blend_y;
    if noise.is_finite() {
        Ok(noise)
    } else {
        Err(PicaProceduralTextureError::InvalidNoiseParameters)
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
    if matches!(function, PicaProceduralTextureMappingFunction::Reserved(_)) {
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
        REG_TEXUNIT3_PROCTEX1, REG_TEXUNIT3_PROCTEX2, REG_TEXUNIT3_PROCTEX3, REG_TEXUNIT3_PROCTEX4,
        REG_TEXUNIT3_PROCTEX5, TEXENV_STAGE_SOURCE_REGISTERS,
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
            PicaProceduralTextureFilter::NearestMipmapNearest => 2,
            PicaProceduralTextureFilter::LinearMipmapNearest => 3,
            PicaProceduralTextureFilter::NearestMipmapLinear => 4,
            PicaProceduralTextureFilter::LinearMipmapLinear => 5,
            PicaProceduralTextureFilter::Reserved(value) => u32::from(value),
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
            [0_u8, 0, 0, 255],
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
        let color = sampler.sample(&[[0.5, 0.25], [0.0; 2], [0.0; 2]]).unwrap();
        assert_color([color[0], color[1], color[2], 1.0], [100, 110, 120, 255]);
        assert!((color[3] - (1024.0 / 4095.0)).abs() < 1.0e-6);
    }

    #[test]
    fn rmax_mapping_flows_from_registers_through_the_sampler() {
        let mut pica = fixture(false, PicaProceduralTextureFilter::Nearest);
        let original = pica.register(REG_TEXUNIT3_PROCTEX0).unwrap();
        write(&mut pica, REG_TEXUNIT3_PROCTEX0, original | (9 << 6));

        let sampler = PicaProceduralTextureSampler::from_pica(&pica).unwrap();
        let color = sampler.sample(&[[0.25, 0.5], [0.0; 2], [0.0; 2]]).unwrap();

        assert_color(color, [100, 110, 120, 200]);
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
        let color = sampler.sample(&[[0.5, 0.25], [0.0; 2], [0.0; 2]]).unwrap();
        let fraction = 3.0 * (2048.0 / 4095.0) - 1.0;
        for (channel, base) in [10.0, 20.0, 30.0].into_iter().enumerate() {
            let expected = (base + 50.0 * fraction) / 255.0;
            assert!(
                (color[channel] - expected).abs() < 1.0e-6,
                "channel {channel}: expected {expected}, got {}",
                color[channel]
            );
        }
        assert_eq!(color[3], 1.0);
    }

    #[test]
    fn noise_parameters_decode_binary16_and_signed_fixed_amplitudes() {
        assert_eq!(decode_float16(0x3C00), 1.0);
        assert_eq!(decode_float16(0xB400), -0.25);
        assert_eq!(decode_float16(0x0001), 1.0 / 16_777_216.0);
        assert!(decode_float16(0x7C00).is_infinite());
        assert!(decode_float16(0x7E00).is_nan());

        let mut pica = fixture(false, PicaProceduralTextureFilter::Nearest);
        write(&mut pica, REG_TEXUNIT3_PROCTEX1, 0x3C00_1000);
        write(&mut pica, REG_TEXUNIT3_PROCTEX2, 0xB400_F000);
        write(&mut pica, REG_TEXUNIT3_PROCTEX3, 0x3C00_3800);
        let config = pica.procedural_texture_config();
        assert!(
            (f32::from(config.u_noise_amplitude_fixed1_3_12 as i16) / 4095.0 - 1.0).abs() < 0.001
        );
        assert_eq!(decode_float16(config.u_noise_phase_float1_5_10), 1.0);
        assert_eq!(decode_float16(config.v_noise_phase_float1_5_10), -0.25);
        assert_eq!(decode_float16(config.u_noise_frequency_float1_5_10), 0.5);
        assert_eq!(decode_float16(config.v_noise_frequency_float1_5_10), 1.0);
    }

    #[test]
    fn noise_axis_hash_cache_preserves_reference_gradient_values() {
        for x in 0_u32..24 {
            for y in 0_u32..24 {
                let expected = {
                    let u = noise_rand_1d(x);
                    let mut v = noise_rand_1d(y);
                    const TABLE: [u32; 16] = [10, 2, 15, 8, 0, 7, 4, 5, 5, 13, 2, 6, 13, 9, 3, 14];
                    if (u & 3) == 1 {
                        v += 4;
                    }
                    v ^= (u & 1) * 6;
                    v += 10 + u;
                    v &= 0xF;
                    v ^= TABLE[u as usize];
                    -1.0 + (v as f32 * 2.0 / 15.0)
                };
                let actual = noise_gradient_from_hashes(noise_rand_1d(x), noise_rand_1d(y));
                assert_eq!(actual, expected, "lattice coordinate ({x}, {y})");
            }
        }
    }

    #[test]
    fn noise_reference_generates_a_finite_deterministic_coefficient() {
        let mut pica = fixture(false, PicaProceduralTextureFilter::Nearest);
        write(&mut pica, REG_TEXUNIT3_PROCTEX1, 0x0000_0FFF);
        write(&mut pica, REG_TEXUNIT3_PROCTEX2, 0x0000_0FFF);
        write(&mut pica, REG_TEXUNIT3_PROCTEX3, 0x3C00_3C00);
        write(
            &mut pica,
            REG_PROCTEX_LUT,
            (PicaProceduralTextureLut::Noise as u32) << 8,
        );
        for _ in 0..CURVE_LUT_SIZE {
            write(&mut pica, REG_PROCTEX_LUT_DATA0, 0x0000_0800);
        }
        let config = pica.procedural_texture_config();
        let noise_state = ProceduralNoiseState::from_pica(&pica, &config).unwrap();
        let first = procedural_noise_coefficient(0.25, 0.5, &noise_state).unwrap();
        let second = procedural_noise_coefficient(0.25, 0.5, &noise_state).unwrap();
        assert_eq!(first, second);
        assert!((first - 0.2168).abs() < 0.001);
    }

    #[test]
    fn noise_enabled_sampler_rejects_non_finite_binary16_parameters_at_capture() {
        let mut invalid_frequency = fixture(false, PicaProceduralTextureFilter::Nearest);
        let proctex0 = invalid_frequency.register(REG_TEXUNIT3_PROCTEX0).unwrap();
        write(
            &mut invalid_frequency,
            REG_TEXUNIT3_PROCTEX0,
            proctex0 | (1 << 15),
        );
        write(&mut invalid_frequency, REG_TEXUNIT3_PROCTEX3, 0x7E00_3C00);
        assert_eq!(
            PicaProceduralTextureSampler::from_pica(&invalid_frequency),
            Err(PicaProceduralTextureError::InvalidNoiseParameters)
        );

        let mut invalid_phase = fixture(false, PicaProceduralTextureFilter::Nearest);
        let proctex0 = invalid_phase.register(REG_TEXUNIT3_PROCTEX0).unwrap();
        write(
            &mut invalid_phase,
            REG_TEXUNIT3_PROCTEX0,
            proctex0 | (1 << 15),
        );
        write(&mut invalid_phase, REG_TEXUNIT3_PROCTEX1, 0x7E00_0000);
        assert_eq!(
            PicaProceduralTextureSampler::from_pica(&invalid_phase),
            Err(PicaProceduralTextureError::InvalidNoiseParameters)
        );
    }

    #[test]
    fn noise_enabled_sampler_uses_the_noise_lut_before_repeat_and_mapping() {
        let mut pica = Pica200::new();
        write(&mut pica, REG_TEXUNIT_CONFIG, 1 << 10);
        write(&mut pica, REG_TEXUNIT3_PROCTEX0, 1 | (1 << 3) | (1 << 15));
        write(&mut pica, REG_TEXUNIT3_PROCTEX1, 0x0000_0FFF);
        write(&mut pica, REG_TEXUNIT3_PROCTEX2, 0x0000_0FFF);
        write(&mut pica, REG_TEXUNIT3_PROCTEX3, 0x3C00_3C00);
        write(&mut pica, REG_TEXUNIT3_PROCTEX4, 128 << 11);
        write(&mut pica, REG_TEXUNIT3_PROCTEX5, 0);

        write(
            &mut pica,
            REG_PROCTEX_LUT,
            (PicaProceduralTextureLut::Noise as u32) << 8,
        );
        for _ in 0..CURVE_LUT_SIZE {
            write(&mut pica, REG_PROCTEX_LUT_DATA0, 0x0000_0800);
        }

        write(
            &mut pica,
            REG_PROCTEX_LUT,
            (PicaProceduralTextureLut::RgbMapping as u32) << 8,
        );
        for index in 0..CURVE_LUT_SIZE {
            let value = ((index * 4095) / (CURVE_LUT_SIZE - 1)) as u32;
            let difference = if index + 1 < CURVE_LUT_SIZE {
                32_u32
            } else {
                0
            };
            write(&mut pica, REG_PROCTEX_LUT_DATA0, value | (difference << 12));
        }

        write(
            &mut pica,
            REG_PROCTEX_LUT,
            (PicaProceduralTextureLut::Color as u32) << 8,
        );
        for index in 0..128_u32 {
            write(
                &mut pica,
                REG_PROCTEX_LUT_DATA0,
                index | (index << 8) | (index << 16) | (255 << 24),
            );
        }

        let sampler = PicaProceduralTextureSampler::from_pica(&pica).unwrap();
        let color = sampler.sample(&[[0.25, 0.5], [0.0; 2], [0.0; 2]]).unwrap();
        assert!(color[0] > 58.0 / 255.0);
        assert!(color[0] < 62.0 / 255.0);
        assert_eq!(color[0], color[1]);
        assert_eq!(color[1], color[2]);
        assert_eq!(color[3], 1.0);
    }

    #[test]
    fn mipmap_filter_modes_use_the_software_reference_level_zero_fallback() {
        let samples = [
            (
                PicaProceduralTextureFilter::NearestMipmapNearest,
                [100, 110, 120, 200],
            ),
            (
                PicaProceduralTextureFilter::NearestMipmapLinear,
                [100, 110, 120, 200],
            ),
            (
                PicaProceduralTextureFilter::LinearMipmapNearest,
                [10, 20, 30, 255],
            ),
            (
                PicaProceduralTextureFilter::LinearMipmapLinear,
                [10, 20, 30, 255],
            ),
        ];

        for (filter, expected) in samples {
            let pica = fixture(false, filter);
            let sampler = PicaProceduralTextureSampler::from_pica(&pica).unwrap();
            let color = sampler.sample(&[[0.5, 0.25], [0.0; 2], [0.0; 2]]).unwrap();
            assert_color(color, expected);
        }
    }

    #[test]
    fn reserved_procedural_filter_values_fail_explicitly() {
        let pica = fixture(false, PicaProceduralTextureFilter::Reserved(6));
        assert_eq!(
            PicaProceduralTextureSampler::from_pica(&pica).unwrap_err(),
            PicaProceduralTextureError::FilterUnsupported(PicaProceduralTextureFilter::Reserved(6))
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
