//! PICA200 fragment lighting.
//!
//! The lighting unit turns per-fragment geometry (a normal quaternion and a
//! view vector produced by the vertex shader) plus the light environment
//! registers into the *fragment primary* and *fragment secondary* colors that
//! the texture environment (TEV) can use as sources.
//!
//! Register layout (word offsets from public hardware documentation and the
//! register description recorded in `docs/AI_HANDOFF.md`):
//! - lights 0..7 at `0x140 + 0x10 * n`: specular0, specular1, diffuse, ambient
//!   (10-bit channels, 255 == 1.0), position (float16 x/y/z), spot direction
//!   (fixed 1.1.11), config, distance attenuation bias/scale (float20);
//! - `0x1C0` scene ambient, `0x1C2` enabled-light count - 1, `0x1C3` config0,
//!   `0x1C4` config1 (per-sampler disables), `0x1C5` LUT index/selector,
//!   `0x1C6` disable, `0x1C8..=0x1CF` LUT data ports, `0x1D0..=0x1D2` LUT input
//!   absolute / selector / scale, `0x1D9` light permutation.
//!
//! The evaluation below is an independent implementation. Items whose exact
//! hardware behavior is not verified are listed in `docs/AI_HANDOFF.md`
//! (spotlight sign convention, tangent/CP input, geometric-factor input,
//! shadow attenuation which is intentionally not applied).

use super::pica200::Pica200;

const REG_LIGHT0: u16 = 0x140;
const LIGHT_STRIDE: u16 = 0x10;
const REG_LIGHTING_AMBIENT: u16 = 0x1C0;
const REG_LIGHTING_NUM_LIGHTS: u16 = 0x1C2;
const REG_LIGHTING_CONFIG0: u16 = 0x1C3;
const REG_LIGHTING_CONFIG1: u16 = 0x1C4;
const REG_LIGHTING_DISABLE: u16 = 0x1C6;
const REG_LIGHTING_LUT_ABSOLUTE: u16 = 0x1D0;
const REG_LIGHTING_LUT_SELECT: u16 = 0x1D1;
const REG_LIGHTING_LUT_SCALE: u16 = 0x1D2;
const REG_LIGHTING_PERMUTATION: u16 = 0x1D9;
/// GPUREG_LIGHTING_ENABLE0: bit 0 turns the lighting unit on.
pub const REG_LIGHTING_ENABLE: u16 = 0x08F;

const LUT_D0: usize = 0;
const LUT_D1: usize = 1;
const LUT_FR: usize = 3;
const LUT_RB: usize = 4;
const LUT_RG: usize = 5;
const LUT_RR: usize = 6;
const LUT_SP_BASE: usize = 8;
const LUT_DA_BASE: usize = 16;

/// Sampler slots in the order used by the input/scale/absolute registers.
const SLOT_D0: usize = 0;
const SLOT_D1: usize = 1;
const SLOT_SP: usize = 2;
const SLOT_FR: usize = 3;
const SLOT_RB: usize = 4;
const SLOT_RG: usize = 5;
const SLOT_RR: usize = 6;
const SLOT_COUNT: usize = 7;

/// Values accepted by GPUREG_LIGHTING_CONFIG0's lighting configuration field.
/// Configuration 7 is encoded as 8; selector 7 is reserved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PicaLightingConfiguration {
    ReflectD0Spot = 0,
    ReflectFresnelSpot = 1,
    ReflectD0D1 = 2,
    D0D1Fresnel = 3,
    RgbD0D1Spot = 4,
    RgbD0FresnelSpot = 5,
    ReflectD0D1FresnelSpot = 6,
    RgbD0D1FresnelSpot = 8,
}

impl PicaLightingConfiguration {
    pub const fn from_register(value: u32) -> Option<Self> {
        match value & 0xF {
            0 => Some(Self::ReflectD0Spot),
            1 => Some(Self::ReflectFresnelSpot),
            2 => Some(Self::ReflectD0D1),
            3 => Some(Self::D0D1Fresnel),
            4 => Some(Self::RgbD0D1Spot),
            5 => Some(Self::RgbD0FresnelSpot),
            6 => Some(Self::ReflectD0D1FresnelSpot),
            8 => Some(Self::RgbD0D1FresnelSpot),
            _ => None,
        }
    }

    /// Reports whether this fixed hardware configuration uses the given LUT sampler.
    pub const fn supports_sampler(self, sampler: PicaLightingSampler) -> bool {
        match sampler {
            PicaLightingSampler::Distribution0 => !matches!(self, Self::ReflectFresnelSpot),
            PicaLightingSampler::Distribution1 => !matches!(
                self,
                Self::ReflectD0Spot | Self::ReflectFresnelSpot | Self::RgbD0FresnelSpot
            ),
            PicaLightingSampler::SpotlightAttenuation => {
                !matches!(self, Self::ReflectD0D1 | Self::D0D1Fresnel)
            }
            PicaLightingSampler::Fresnel => !matches!(
                self,
                Self::ReflectD0Spot | Self::ReflectD0D1 | Self::RgbD0D1Spot
            ),
            PicaLightingSampler::ReflectRed => !matches!(self, Self::D0D1Fresnel),
            PicaLightingSampler::ReflectGreen | PicaLightingSampler::ReflectBlue => matches!(
                self,
                Self::RgbD0D1Spot | Self::RgbD0FresnelSpot | Self::RgbD0D1FresnelSpot
            ),
        }
    }
}

/// Non-contiguous lighting LUT selectors in the PICA register table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PicaLightingSampler {
    Distribution0 = 0,
    Distribution1 = 1,
    Fresnel = 3,
    ReflectBlue = 4,
    ReflectGreen = 5,
    ReflectRed = 6,
    SpotlightAttenuation = 8,
}

impl PicaLightingSampler {
    /// LUT index for one of the eight per-light spotlight attenuation curves.
    pub const fn spotlight_attenuation_table(light_index: usize) -> Option<usize> {
        if light_index < 8 { Some(8 + light_index) } else { None }
    }

    /// LUT index for one of the eight per-light distance attenuation curves.
    pub const fn distance_attenuation_table(light_index: usize) -> Option<usize> {
        if light_index < 8 { Some(16 + light_index) } else { None }
    }
}

/// Failure while decoding the light environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaLightingError {
    /// The LUT configuration value is not one of 0..=6 or 8.
    Configuration { value: u32 },
    /// A sampler input selector is outside NH, VH, NV, LN, SP, CP.
    LutInput { slot: usize, value: u32 },
    /// A sampler scale selector is one of the undefined values 4 or 5.
    LutScale { slot: usize, value: u32 },
    /// The bump-map texture selector is not texture unit 0, 1 or 2.
    BumpSelector { value: u32 },
}

impl std::fmt::Display for PicaLightingError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Configuration { value } => {
                write!(formatter, "PICA lighting configuration {value} is invalid")
            }
            Self::LutInput { slot, value } => write!(
                formatter,
                "PICA lighting LUT input selector {value} for sampler slot {slot} is invalid"
            ),
            Self::LutScale { slot, value } => write!(
                formatter,
                "PICA lighting LUT scale selector {value} for sampler slot {slot} is invalid"
            ),
            Self::BumpSelector { value } => {
                write!(formatter, "PICA lighting bump selector {value} is invalid")
            }
        }
    }
}

impl std::error::Error for PicaLightingError {}

/// Dot-product input chosen for a LUT sampler.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LutInput {
    NormalHalf,
    ViewHalf,
    NormalView,
    LightNormal,
    Spot,
    Tangent,
}

impl LutInput {
    fn decode(slot: usize, value: u32) -> Result<Self, PicaLightingError> {
        Ok(match value {
            0 => Self::NormalHalf,
            1 => Self::ViewHalf,
            2 => Self::NormalView,
            3 => Self::LightNormal,
            4 => Self::Spot,
            5 => Self::Tangent,
            _ => return Err(PicaLightingError::LutInput { slot, value }),
        })
    }
}

fn decode_scale(slot: usize, value: u32) -> Result<f32, PicaLightingError> {
    Ok(match value {
        0 => 1.0,
        1 => 2.0,
        2 => 4.0,
        3 => 8.0,
        6 => 0.25,
        7 => 0.5,
        _ => return Err(PicaLightingError::LutScale { slot, value }),
    })
}

/// Bump-mapping mode selected by config0 bits 28..29.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaLightingBumpMode {
    None,
    NormalMap,
    TangentMap,
}

/// One decoded light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaLight {
    pub specular0: [f32; 3],
    pub specular1: [f32; 3],
    pub diffuse: [f32; 3],
    pub ambient: [f32; 3],
    pub position: [f32; 3],
    pub spot_direction: [f32; 3],
    pub directional: bool,
    pub two_sided_diffuse: bool,
    pub geometric_factor: [bool; 2],
    pub distance_bias: f32,
    pub distance_scale: f32,
}

/// Lighting inputs interpolated for one fragment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaLightingInput {
    /// Normal quaternion (x, y, z, w) from the vertex shader.
    pub quaternion: [f32; 4],
    /// View vector from the vertex shader (not required to be normalized).
    pub view: [f32; 3],
    /// Texel of the bump-map unit, when bump mapping is on.
    pub bump_texel: Option<[f32; 4]>,
}

impl Default for PicaLightingInput {
    fn default() -> Self {
        Self {
            quaternion: [0.0, 0.0, 0.0, 1.0],
            view: [0.0, 0.0, 1.0],
            bump_texel: None,
        }
    }
}

/// Result of lighting one fragment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaLightingOutput {
    pub primary: [f32; 4],
    pub secondary: [f32; 4],
}

/// Decoded light environment captured at draw time.
#[derive(Clone, Debug, PartialEq)]
pub struct PicaLightingState {
    global_ambient: [f32; 3],
    /// Enabled lights in permutation order.
    lights: Vec<(usize, PicaLight)>,
    config: PicaLightingConfiguration,
    enable_primary_alpha: bool,
    enable_secondary_alpha: bool,
    clamp_highlights: bool,
    bump_mode: PicaLightingBumpMode,
    bump_selector: usize,
    renormalize_bump: bool,
    disable_spot: u8,
    disable_distance: u8,
    disabled_samplers: [bool; SLOT_COUNT],
    absolute_disabled: [bool; SLOT_COUNT],
    inputs: [LutInput; SLOT_COUNT],
    scales: [f32; SLOT_COUNT],
    luts: Vec<[u32; 256]>,
}

impl PicaLightingState {
    /// Decodes the light environment, or `None` when lighting is off.
    pub fn from_pica(pica: &Pica200) -> Result<Option<Self>, PicaLightingError> {
        let register = |id: u16| pica.register(id).unwrap_or(0);
        if register(REG_LIGHTING_ENABLE) & 1 == 0 || register(REG_LIGHTING_DISABLE) & 1 != 0 {
            return Ok(None);
        }

        let config0 = register(REG_LIGHTING_CONFIG0);
        let config1 = register(REG_LIGHTING_CONFIG1);
        let config_value = (config0 >> 4) & 0xF;
        let config = PicaLightingConfiguration::from_register(config_value)
            .ok_or(PicaLightingError::Configuration { value: config_value })?;
        let bump_selector = ((config0 >> 22) & 3) as usize;
        if bump_selector > 2 {
            return Err(PicaLightingError::BumpSelector {
                value: bump_selector as u32,
            });
        }
        let bump_mode = match (config0 >> 28) & 3 {
            1 => PicaLightingBumpMode::NormalMap,
            2 => PicaLightingBumpMode::TangentMap,
            _ => PicaLightingBumpMode::None,
        };

        let absolute = register(REG_LIGHTING_LUT_ABSOLUTE);
        let select = register(REG_LIGHTING_LUT_SELECT);
        let scale = register(REG_LIGHTING_LUT_SCALE);
        let mut inputs = [LutInput::NormalHalf; SLOT_COUNT];
        let mut scales = [1.0f32; SLOT_COUNT];
        let mut absolute_disabled = [false; SLOT_COUNT];
        for slot in 0..SLOT_COUNT {
            inputs[slot] = LutInput::decode(slot, (select >> (slot * 4)) & 7)?;
            scales[slot] = decode_scale(slot, (scale >> (slot * 4)) & 7)?;
            absolute_disabled[slot] = (absolute >> (slot * 4 + 1)) & 1 != 0;
        }

        let light_count = (register(REG_LIGHTING_NUM_LIGHTS) & 7) as usize + 1;
        let permutation = register(REG_LIGHTING_PERMUTATION);
        let mut lights = Vec::with_capacity(light_count);
        for slot in 0..light_count {
            let index = ((permutation >> (slot * 4)) & 7) as usize;
            lights.push((index, decode_light(pica, index)));
        }

        let mut luts = Vec::with_capacity(24);
        for table in 0..24 {
            let mut entries = [0u32; 256];
            for (index, entry) in entries.iter_mut().enumerate() {
                *entry = pica.lighting_lut_entry(table, index).unwrap_or(0);
            }
            luts.push(entries);
        }

        let disabled_samplers = {
            let mut flags = [false; SLOT_COUNT];
            flags[SLOT_D0] = config1 & (1 << 16) != 0;
            flags[SLOT_D1] = config1 & (1 << 17) != 0;
            flags[SLOT_FR] = config1 & (1 << 19) != 0;
            flags[SLOT_RR] = config1 & (1 << 20) != 0;
            flags[SLOT_RG] = config1 & (1 << 21) != 0;
            flags[SLOT_RB] = config1 & (1 << 22) != 0;
            flags
        };

        Ok(Some(Self {
            global_ambient: decode_color(register(REG_LIGHTING_AMBIENT)),
            lights,
            config,
            enable_primary_alpha: config0 & (1 << 2) != 0,
            enable_secondary_alpha: config0 & (1 << 3) != 0,
            clamp_highlights: config0 & (1 << 27) != 0,
            bump_mode,
            bump_selector,
            renormalize_bump: config0 & (1 << 30) == 0,
            disable_spot: ((config1 >> 8) & 0xFF) as u8,
            disable_distance: ((config1 >> 24) & 0xFF) as u8,
            disabled_samplers,
            absolute_disabled,
            inputs,
            scales,
            luts,
        }))
    }

    /// Texture unit sampled for bump mapping, when a bump mode is active.
    pub fn bump_texture_unit(&self) -> Option<usize> {
        (self.bump_mode != PicaLightingBumpMode::None).then_some(self.bump_selector)
    }

    /// Enabled lights in evaluation order.
    #[cfg(test)]
    fn lights(&self) -> impl Iterator<Item = &PicaLight> {
        self.lights.iter().map(|(_, light)| light)
    }

    /// Replaces one LUT entry; test helper.
    #[cfg(test)]
    fn set_lut_entry(&mut self, table: usize, index: usize, raw: u32) {
        if let Some(entries) = self.luts.get_mut(table) {
            entries[index & 0xFF] = raw & 0x00FF_FFFF;
        }
    }

    fn sampler_supported(&self, sampler: usize) -> bool {
        let selector = match sampler {
            LUT_D0 => PicaLightingSampler::Distribution0,
            LUT_D1 => PicaLightingSampler::Distribution1,
            LUT_FR => PicaLightingSampler::Fresnel,
            LUT_RR => PicaLightingSampler::ReflectRed,
            LUT_RG => PicaLightingSampler::ReflectGreen,
            LUT_RB => PicaLightingSampler::ReflectBlue,
            LUT_SP_BASE => PicaLightingSampler::SpotlightAttenuation,
            _ => return true,
        };
        self.config.supports_sampler(selector)
    }

    fn lookup(&self, table: usize, value: f32, absolute: bool) -> f32 {
        let value = if value.is_nan() { 0.0 } else { value };
        let (index, fraction) = if absolute {
            let scaled = (value.abs() * 256.0).clamp(0.0, 255.0);
            let index = scaled.floor();
            (index as usize, scaled - index)
        } else {
            let scaled = (value * 128.0).clamp(-128.0, 127.0);
            let index = scaled.floor();
            ((index as i32 & 0xFF) as usize, scaled - index)
        };
        let raw = self.luts[table][index];
        let base = (raw & 0xFFF) as f32 / 4096.0;
        let delta = (((raw >> 12) & 0xFFF) as i32) << 20 >> 20;
        base + delta as f32 / 2048.0 * fraction
    }

    /// Looks up a sampler through its input selector, scale and abs flag.
    fn sampler(&self, table: usize, slot: usize, dot: f32) -> f32 {
        self.lookup(table, dot, !self.absolute_disabled[slot]) * self.scales[slot]
    }

    /// Evaluates the fragment primary and secondary colors.
    pub fn evaluate(&self, input: &PicaLightingInput) -> PicaLightingOutput {
        let (normal, tangent) = self.surface_basis(input);
        let view = normalize(input.view);

        let mut diffuse_sum = [0.0f32; 3];
        let mut specular_sum = [0.0f32; 3];
        let mut fresnel = 1.0f32;
        let last = self.lights.len().saturating_sub(1);

        for (position, (light_index, light)) in self.lights.iter().enumerate() {
            let light_vector = if light.directional {
                normalize(light.position)
            } else {
                normalize(add(light.position, input.view))
            };
            let half_unnormalized = add(light_vector, view);
            let half_length_squared = dot(half_unnormalized, half_unnormalized);
            let half = normalize(half_unnormalized);

            let dot_for = |selected: LutInput| match selected {
                LutInput::NormalHalf => dot(normal, half),
                LutInput::ViewHalf => dot(view, half),
                LutInput::NormalView => dot(normal, view),
                LutInput::LightNormal => dot(light_vector, normal),
                LutInput::Spot => -dot(light_vector, light.spot_direction),
                LutInput::Tangent => {
                    let projected = sub(half, scale3(normal, dot(normal, half)));
                    dot(projected, tangent)
                }
            };

            let light_normal = dot(light_vector, normal);
            let diffuse_factor = if light.two_sided_diffuse {
                light_normal.abs()
            } else {
                light_normal.max(0.0)
            };

            let mut attenuation = 1.0f32;
            if self.disable_distance & (1 << light_index) == 0 {
                let distance = length(add(input.view, light.position));
                let x = light.distance_scale.mul_add(distance, light.distance_bias);
                attenuation *= self.lookup(LUT_DA_BASE + light_index, x.clamp(0.0, 1.0), true);
            }
            if self.disable_spot & (1 << light_index) == 0 && self.sampler_supported(LUT_SP_BASE) {
                attenuation *= self.sampler(
                    LUT_SP_BASE + light_index,
                    SLOT_SP,
                    dot_for(self.inputs[SLOT_SP]),
                );
            }

            let d0 = if self.sampler_supported(LUT_D0) && !self.disabled_samplers[SLOT_D0] {
                self.sampler(LUT_D0, SLOT_D0, dot_for(self.inputs[SLOT_D0]))
            } else {
                1.0
            };
            let d1 = if self.sampler_supported(LUT_D1) && !self.disabled_samplers[SLOT_D1] {
                self.sampler(LUT_D1, SLOT_D1, dot_for(self.inputs[SLOT_D1]))
            } else {
                1.0
            };

            let mut reflect = [1.0f32; 3];
            if self.sampler_supported(LUT_RR) && !self.disabled_samplers[SLOT_RR] {
                reflect[0] = self.sampler(LUT_RR, SLOT_RR, dot_for(self.inputs[SLOT_RR]));
                reflect[1] = reflect[0];
                reflect[2] = reflect[0];
            }
            if self.sampler_supported(LUT_RG) {
                if !self.disabled_samplers[SLOT_RG] {
                    reflect[1] = self.sampler(LUT_RG, SLOT_RG, dot_for(self.inputs[SLOT_RG]));
                }
                if !self.disabled_samplers[SLOT_RB] {
                    reflect[2] = self.sampler(LUT_RB, SLOT_RB, dot_for(self.inputs[SLOT_RB]));
                }
            }

            if position == last
                && self.sampler_supported(LUT_FR)
                && !self.disabled_samplers[SLOT_FR]
            {
                fresnel = self.sampler(LUT_FR, SLOT_FR, dot_for(self.inputs[SLOT_FR]));
            }

            let geometric = |enabled: bool| {
                if !enabled {
                    1.0
                } else if half_length_squared == 0.0 {
                    0.0
                } else {
                    (light_normal / half_length_squared).min(1.0)
                }
            };
            let highlight = if self.clamp_highlights && light_normal <= 0.0 {
                0.0
            } else {
                1.0
            };

            for channel in 0..3 {
                diffuse_sum[channel] += attenuation
                    * (light.ambient[channel] + light.diffuse[channel] * diffuse_factor);
                specular_sum[channel] += attenuation
                    * highlight
                    * (d0 * geometric(light.geometric_factor[0]) * light.specular0[channel]
                        + d1 * geometric(light.geometric_factor[1])
                            * reflect[channel]
                            * light.specular1[channel]);
            }
        }

        let clamp = |value: f32| {
            if value.is_nan() {
                0.0
            } else {
                value.clamp(0.0, 1.0)
            }
        };
        PicaLightingOutput {
            primary: [
                clamp(self.global_ambient[0] + diffuse_sum[0]),
                clamp(self.global_ambient[1] + diffuse_sum[1]),
                clamp(self.global_ambient[2] + diffuse_sum[2]),
                if self.enable_primary_alpha {
                    clamp(fresnel)
                } else {
                    1.0
                },
            ],
            secondary: [
                clamp(specular_sum[0]),
                clamp(specular_sum[1]),
                clamp(specular_sum[2]),
                if self.enable_secondary_alpha {
                    clamp(fresnel)
                } else {
                    1.0
                },
            ],
        }
    }

    /// Surface normal and tangent in view space after optional bump mapping.
    fn surface_basis(&self, input: &PicaLightingInput) -> ([f32; 3], [f32; 3]) {
        let mut surface_normal = [0.0, 0.0, 1.0];
        let mut surface_tangent = [1.0, 0.0, 0.0];
        if let Some(texel) = input.bump_texel {
            let mut vector = [
                texel[0] * 2.0 - 1.0,
                texel[1] * 2.0 - 1.0,
                texel[2] * 2.0 - 1.0,
            ];
            if self.renormalize_bump {
                vector[2] = (1.0 - vector[0] * vector[0] - vector[1] * vector[1])
                    .max(0.0)
                    .sqrt();
            }
            match self.bump_mode {
                PicaLightingBumpMode::NormalMap => surface_normal = vector,
                PicaLightingBumpMode::TangentMap => surface_tangent = vector,
                PicaLightingBumpMode::None => {}
            }
        }
        let [x, y, z, w] = input.quaternion;
        let rotate = |v: [f32; 3]| {
            // Rotation by the unit quaternion: v + 2w(q x v) + 2 q x (q x v).
            let q = [x, y, z];
            let t = scale3(cross(q, v), 2.0);
            add(add(v, scale3(t, w)), cross(q, t))
        };
        (
            normalize(rotate(surface_normal)),
            normalize(rotate(surface_tangent)),
        )
    }
}

fn decode_color(raw: u32) -> [f32; 3] {
    [
        ((raw >> 20) & 0x3FF) as f32 / 255.0,
        ((raw >> 10) & 0x3FF) as f32 / 255.0,
        (raw & 0x3FF) as f32 / 255.0,
    ]
}

fn decode_light(pica: &Pica200, index: usize) -> PicaLight {
    let base = REG_LIGHT0 + LIGHT_STRIDE * index as u16;
    let register = |offset: u16| pica.register(base + offset).unwrap_or(0);
    let xy = register(4);
    let z = register(5);
    let spot_xy = register(6);
    let spot_z = register(7);
    let config = register(9);
    let spot = |raw: u32| (((raw & 0x1FFF) as i32) << 19 >> 19) as f32 / 2047.0;
    PicaLight {
        specular0: decode_color(register(0)),
        specular1: decode_color(register(1)),
        diffuse: decode_color(register(2)),
        ambient: decode_color(register(3)),
        position: [
            decode_float(xy & 0xFFFF, 5, 10),
            decode_float(xy >> 16, 5, 10),
            decode_float(z & 0xFFFF, 5, 10),
        ],
        spot_direction: [spot(spot_xy), spot(spot_xy >> 16), spot(spot_z)],
        directional: config & 1 != 0,
        two_sided_diffuse: config & 2 != 0,
        geometric_factor: [config & 4 != 0, config & 8 != 0],
        distance_bias: decode_float(register(10) & 0xF_FFFF, 7, 12),
        distance_scale: decode_float(register(11) & 0xF_FFFF, 7, 12),
    }
}

/// Decodes a PICA sign/exponent/mantissa float; denormals flush to zero.
fn decode_float(raw: u32, exponent_bits: u32, mantissa_bits: u32) -> f32 {
    let mantissa = raw & ((1 << mantissa_bits) - 1);
    let exponent = (raw >> mantissa_bits) & ((1 << exponent_bits) - 1);
    let negative = (raw >> (mantissa_bits + exponent_bits)) & 1 != 0;
    if exponent == 0 {
        return 0.0;
    }
    let bias = (1i32 << (exponent_bits - 1)) - 1;
    let magnitude = (1.0 + mantissa as f32 / (1u32 << mantissa_bits) as f32)
        * 2.0f32.powi(exponent as i32 - bias);
    if negative { -magnitude } else { magnitude }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn scale3(a: [f32; 3], factor: f32) -> [f32; 3] {
    [a[0] * factor, a[1] * factor, a[2] * factor]
}

fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}

fn normalize(a: [f32; 3]) -> [f32; 3] {
    let len = length(a);
    if len > 0.0 && len.is_finite() {
        scale3(a, 1.0 / len)
    } else {
        [0.0; 3]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(pica: &mut Pica200, id: u16, value: u32) {
        pica.write_register(id, value).unwrap();
    }

    fn color(r: u32, g: u32, b: u32) -> u32 {
        (r << 20) | (g << 10) | b
    }

    /// One directional light along +Z (towards the viewer) on an upright surface.
    fn single_light_gpu() -> Pica200 {
        let mut pica = Pica200::new();
        write(&mut pica, REG_LIGHTING_ENABLE, 1);
        write(&mut pica, REG_LIGHTING_AMBIENT, color(51, 0, 0));
        write(&mut pica, REG_LIGHT0 + 2, color(255, 128, 0)); // diffuse
        write(&mut pica, REG_LIGHT0, color(255, 255, 255)); // specular0
        write(&mut pica, REG_LIGHT0 + 5, 0x3C00); // z = 1.0 (float16)
        write(&mut pica, REG_LIGHT0 + 9, 1); // directional
        write(&mut pica, REG_LIGHTING_CONFIG1, 0xFF00_FF00 | (1 << 19));
        write(&mut pica, REG_LIGHTING_NUM_LIGHTS, 0);
        pica
    }

    #[test]
    fn disabled_lighting_yields_no_state() {
        let pica = Pica200::new();
        assert_eq!(PicaLightingState::from_pica(&pica).unwrap(), None);
        let mut pica = single_light_gpu();
        write(&mut pica, REG_LIGHTING_DISABLE, 1);
        assert_eq!(PicaLightingState::from_pica(&pica).unwrap(), None);
    }

    #[test]
    fn lighting_configuration_and_lut_selector_tables_are_complete() {
        let sampler_order = [
            PicaLightingSampler::Distribution0,
            PicaLightingSampler::Distribution1,
            PicaLightingSampler::SpotlightAttenuation,
            PicaLightingSampler::Fresnel,
            PicaLightingSampler::ReflectRed,
            PicaLightingSampler::ReflectGreen,
            PicaLightingSampler::ReflectBlue,
        ];
        let configurations = [
            (
                0,
                PicaLightingConfiguration::ReflectD0Spot,
                [true, false, true, false, true, false, false],
            ),
            (
                1,
                PicaLightingConfiguration::ReflectFresnelSpot,
                [false, false, true, true, true, false, false],
            ),
            (
                2,
                PicaLightingConfiguration::ReflectD0D1,
                [true, true, false, false, true, false, false],
            ),
            (
                3,
                PicaLightingConfiguration::D0D1Fresnel,
                [true, true, false, true, false, false, false],
            ),
            (
                4,
                PicaLightingConfiguration::RgbD0D1Spot,
                [true, true, true, false, true, true, true],
            ),
            (
                5,
                PicaLightingConfiguration::RgbD0FresnelSpot,
                [true, false, true, true, true, true, true],
            ),
            (
                6,
                PicaLightingConfiguration::ReflectD0D1FresnelSpot,
                [true, true, true, true, true, false, false],
            ),
            (
                8,
                PicaLightingConfiguration::RgbD0D1FresnelSpot,
                [true, true, true, true, true, true, true],
            ),
        ];

        for (raw, configuration, expected) in configurations {
            assert_eq!(PicaLightingConfiguration::from_register(raw), Some(configuration));
            for (sampler, supported) in sampler_order.into_iter().zip(expected) {
                assert_eq!(
                    configuration.supports_sampler(sampler),
                    supported,
                    "config={raw} sampler={sampler:?}"
                );
            }
        }
        assert_eq!(PicaLightingConfiguration::from_register(7), None);
        assert_eq!(PicaLightingSampler::spotlight_attenuation_table(0), Some(8));
        assert_eq!(PicaLightingSampler::spotlight_attenuation_table(7), Some(15));
        assert_eq!(PicaLightingSampler::spotlight_attenuation_table(8), None);
        assert_eq!(PicaLightingSampler::distance_attenuation_table(0), Some(16));
        assert_eq!(PicaLightingSampler::distance_attenuation_table(7), Some(23));
        assert_eq!(PicaLightingSampler::distance_attenuation_table(8), None);
    }

    #[test]
    fn float_decoders_match_known_values() {
        assert_eq!(decode_float(0x3C00, 5, 10), 1.0);
        assert_eq!(decode_float(0xC000, 5, 10), -2.0);
        assert_eq!(decode_float(0, 5, 10), 0.0);
        // float20: bias 63, mantissa 12 bits; 1.0 == exponent 63.
        assert_eq!(decode_float(63 << 12, 7, 12), 1.0);
        assert_eq!(decode_float((1 << 19) | (64 << 12), 7, 12), -2.0);
    }

    #[test]
    fn decodes_light_registers() {
        let mut pica = single_light_gpu();
        write(&mut pica, REG_LIGHT0 + 6, 0x0400 | (0x0400 << 16)); // spot x/y = 1024/2047
        write(&mut pica, REG_LIGHT0 + 9, 0b1011);
        let state = PicaLightingState::from_pica(&pica).unwrap().unwrap();
        let light = state.lights().next().unwrap();
        assert_eq!(light.position, [0.0, 0.0, 1.0]);
        assert_eq!(light.diffuse, [1.0, 128.0 / 255.0, 0.0]);
        assert!(light.directional && light.two_sided_diffuse);
        assert_eq!(light.geometric_factor, [false, true]);
        assert!((light.spot_direction[0] - 1024.0 / 2047.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_invalid_configuration_and_scales() {
        let mut pica = single_light_gpu();
        write(&mut pica, REG_LIGHTING_CONFIG0, 7 << 4);
        assert_eq!(
            PicaLightingState::from_pica(&pica),
            Err(PicaLightingError::Configuration { value: 7 })
        );
        let mut pica = single_light_gpu();
        write(&mut pica, REG_LIGHTING_LUT_SCALE, 4);
        assert_eq!(
            PicaLightingState::from_pica(&pica),
            Err(PicaLightingError::LutScale { slot: 0, value: 4 })
        );
    }

    #[test]
    fn diffuse_light_facing_the_surface_adds_scene_ambient() {
        let pica = single_light_gpu();
        let mut state = PicaLightingState::from_pica(&pica).unwrap().unwrap();
        // Distance attenuation disabled for light 0 by config1 bit 24; make the
        // spot LUT constant 1.0 so the factor does not zero the light.
        for index in 0..256 {
            state.set_lut_entry(LUT_SP_BASE, index, 0xFFF);
        }
        let output = state.evaluate(&PicaLightingInput::default());
        assert!((output.primary[0] - 1.0).abs() < 1e-3);
        assert!((output.primary[1] - 128.0 / 255.0).abs() < 2e-3);
        assert!(output.primary[2].abs() < 1e-6);
        assert_eq!(output.primary[3], 1.0);
        assert_eq!(output.secondary[3], 1.0);
    }

    #[test]
    fn light_behind_the_surface_leaves_only_ambient() {
        let mut pica = single_light_gpu();
        write(&mut pica, REG_LIGHT0 + 5, 0xBC00); // z = -1.0
        let mut state = PicaLightingState::from_pica(&pica).unwrap().unwrap();
        for index in 0..256 {
            state.set_lut_entry(LUT_SP_BASE, index, 0xFFF);
        }
        let output = state.evaluate(&PicaLightingInput::default());
        assert!((output.primary[0] - 51.0 / 255.0).abs() < 1e-6);
        assert_eq!(output.primary[1], 0.0);
    }

    #[test]
    fn quaternion_rotates_the_surface_normal() {
        let pica = single_light_gpu();
        let mut state = PicaLightingState::from_pica(&pica).unwrap().unwrap();
        for index in 0..256 {
            state.set_lut_entry(LUT_SP_BASE, index, 0xFFF);
        }
        // 90 degrees about Y turns +Z into +X: the +Z light now grazes it.
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let output = state.evaluate(&PicaLightingInput {
            quaternion: [0.0, half, 0.0, half],
            ..PicaLightingInput::default()
        });
        assert!((output.primary[0] - 51.0 / 255.0).abs() < 1e-4);
    }

    #[test]
    fn lut_lookup_interpolates_with_signed_delta() {
        let pica = single_light_gpu();
        let mut state = PicaLightingState::from_pica(&pica).unwrap().unwrap();
        // value 0.25, delta +0.5 (0x400 / 2048).
        state.set_lut_entry(LUT_D0, 10, 0x400_000 | 0x400);
        let at_entry = state.lookup(LUT_D0, 10.0 / 256.0, true);
        assert!((at_entry - 0.25).abs() < 1e-6);
        let halfway = state.lookup(LUT_D0, 10.5 / 256.0, true);
        assert!((halfway - 0.5).abs() < 1e-3);
        // Negative inputs wrap to the upper half when abs is not applied.
        state.set_lut_entry(LUT_D0, 255, 0x800);
        assert!((state.lookup(LUT_D0, -1.0 / 128.0, false) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn lighting_lut_fifo_selects_table_and_advances_index() {
        let mut pica = Pica200::new();
        write(&mut pica, REG_LIGHTING_LUT_INDEX_FOR_TESTS, (6 << 8) | 254);
        write(&mut pica, 0x1C8, 0x00_0111);
        write(&mut pica, 0x1C9, 0x00_0222);
        write(&mut pica, 0x1CA, 0x00_0333);
        assert_eq!(pica.lighting_lut_entry(6, 254), Some(0x111));
        assert_eq!(pica.lighting_lut_entry(6, 255), Some(0x222));
        assert_eq!(pica.lighting_lut_entry(6, 0), Some(0x333));
        assert_eq!(pica.register(0x1C5), Some((6 << 8) | 1));
        pica.reset();
        assert_eq!(pica.lighting_lut_entry(6, 254), Some(0));
    }

    const REG_LIGHTING_LUT_INDEX_FOR_TESTS: u16 = 0x1C5;
}
