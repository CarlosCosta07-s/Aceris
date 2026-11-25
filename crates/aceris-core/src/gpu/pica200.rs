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
use super::texture::REG_TEXUNIT_CONFIG;

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
/// GPUREG_FACECULLING_CONFIG: bits 0..1 select the culling mode.
pub const REG_FACECULLING_CONFIG: u16 = 0x040;
/// GPUREG_SCISSORTEST_MODE: bits 0..1 select disabled/exclude/include.
pub const REG_SCISSORTEST_MODE: u16 = 0x065;
/// GPUREG_SCISSORTEST_POS: x1 in bits 0..9, y1 in bits 16..25.
pub const REG_SCISSORTEST_POS: u16 = 0x066;
/// GPUREG_SCISSORTEST_DIM: x2 in bits 0..9, y2 in bits 16..25 (inclusive).
pub const REG_SCISSORTEST_DIM: u16 = 0x067;
/// GPUREG_DEPTH_COLOR_MASK: depth test/function, channel masks and depth write.
pub const REG_DEPTH_COLOR_MASK: u16 = 0x107;
/// GPUREG_COLORBUFFER_WRITE: per-channel color-buffer write access.
pub const REG_COLORBUFFER_WRITE: u16 = 0x113;
/// GPUREG_DEPTHBUFFER_READ: stencil/depth read access.
pub const REG_DEPTHBUFFER_READ: u16 = 0x114;
/// GPUREG_DEPTHBUFFER_WRITE: stencil/depth write access.
pub const REG_DEPTHBUFFER_WRITE: u16 = 0x115;
/// GPUREG_DEPTHBUFFER_FORMAT: D16, D24 or D24S8.
pub const REG_DEPTHBUFFER_FORMAT: u16 = 0x116;
/// GPUREG_DEPTHBUFFER_LOC: physical depth-buffer address >> 3.
pub const REG_DEPTHBUFFER_LOC: u16 = 0x11C;
/// GPUREG_FOG_LUT_INDEX: next entry written by the fog LUT FIFO.
pub const REG_FOG_LUT_INDEX: u16 = 0x0E6;
/// First of eight FIFO registers used to upload the fog LUT.
pub const REG_FOG_LUT_DATA0: u16 = 0x0E8;
/// Number of entries in the PICA200 fog lookup table.
pub const FOG_LUT_ENTRY_COUNT: usize = 128;
const FOG_LUT_DATA_REGISTER_COUNT: u16 = 8;
/// GPUREG_GAS_LUT_INDEX: next entry written by the gas LUT FIFO.
pub const REG_GAS_LUT_INDEX: u16 = 0x123;
/// GPUREG_GAS_LUT_DATA: single FIFO data port for the 16-entry gas color table.
pub const REG_GAS_LUT_DATA: u16 = 0x124;
/// Number of entries in the PICA200 gas color lookup table.
pub const GAS_LUT_ENTRY_COUNT: usize = 16;
/// GPUREG_TEXUNIT3_PROCTEX0..5: procedural texture configuration registers.
pub const REG_TEXUNIT3_PROCTEX0: u16 = 0x0A8;
pub const REG_TEXUNIT3_PROCTEX1: u16 = 0x0A9;
pub const REG_TEXUNIT3_PROCTEX2: u16 = 0x0AA;
pub const REG_TEXUNIT3_PROCTEX3: u16 = 0x0AB;
pub const REG_TEXUNIT3_PROCTEX4: u16 = 0x0AC;
pub const REG_TEXUNIT3_PROCTEX5: u16 = 0x0AD;
/// GPUREG_PROCTEX_LUT selects a procedural texture table and starting index.
pub const REG_PROCTEX_LUT: u16 = 0x0AF;
/// First of eight FIFO aliases used to upload procedural texture LUT entries.
pub const REG_PROCTEX_LUT_DATA0: u16 = 0x0B0;
/// Number of procedural texture LUT FIFO aliases.
pub const PROCTEX_LUT_DATA_REGISTER_COUNT: u16 = 8;

/// A PICA200 procedural-texture lookup table selected by GPUREG_PROCTEX_LUT.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum PicaProceduralTextureLut {
    Noise = 0,
    RgbMapping = 2,
    AlphaMapping = 3,
    Color = 4,
    ColorDifference = 5,
}

impl PicaProceduralTextureLut {
    const fn entry_count(self) -> usize {
        match self {
            Self::Noise | Self::RgbMapping | Self::AlphaMapping => 128,
            Self::Color | Self::ColorDifference => 256,
        }
    }

    const fn from_selector(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Noise),
            2 => Some(Self::RgbMapping),
            3 => Some(Self::AlphaMapping),
            4 => Some(Self::Color),
            5 => Some(Self::ColorDifference),
            _ => None,
        }
    }
}

/// Coordinate wrapping modes documented for the PICA200 procedural texture unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaProceduralTextureClamp {
    ClampToZero,
    ClampToEdge,
    SymmetricalRepeat,
    MirroredRepeat,
    Pulse,
    Reserved(u8),
}

/// Texture-coordinate set routed to procedural texture unit 3.
///
/// GPUREG_TEXUNIT_CONFIG values 0, 1 and 2 select texture coordinates 0, 1 and 2.
/// Reserved selector values are retained instead of being redirected silently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaProceduralTextureCoordinateSource {
    Texture0,
    Texture1,
    Texture2,
    Reserved(u8),
}

/// Coordinate-to-scalar functions selected independently for RGB and alpha.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaProceduralTextureMappingFunction {
    U,
    USquared,
    V,
    VSquared,
    Average,
    SquaredAverage,
    RadialDistance,
    Minimum,
    Maximum,
    Rmax,
    Reserved(u8),
}

impl PicaProceduralTextureMappingFunction {
    /// Evaluates the selected G(u, v) base-shape equation before the mapping LUT.
    ///
    /// This raw helper does not apply noise, wrapping, shifts, coordinate
    /// normalization, or the PICA unit-interval saturation on radial functions.
    pub fn evaluate_raw(self, u: f32, v: f32) -> Option<f32> {
        if !u.is_finite() || !v.is_finite() {
            return None;
        }

        let value = match self {
            Self::U => u,
            Self::USquared => u * u,
            Self::V => v,
            Self::VSquared => v * v,
            Self::Average => (u + v) * 0.5,
            Self::SquaredAverage => (u * u + v * v) * 0.5,
            Self::RadialDistance => (u * u + v * v).sqrt(),
            Self::Minimum => u.min(v),
            Self::Maximum => u.max(v),
            Self::Rmax => ((u + v) * 0.5 + (u * u + v * v).sqrt()) * 0.5,
            Self::Reserved(_) => return None,
        };

        // Finite inputs can still overflow during squaring, addition or sqrt.
        // Do not leak an infinite/NaN intermediate into later texture stages.
        value.is_finite().then_some(value)
    }

    /// Evaluates G(u, v) as it enters the procedural mapping lookup table.
    ///
    /// PICA saturates the radial and Rmax functions at 1.0; the other
    /// documented functions are returned without an extra output clamp.
    pub fn evaluate_for_lookup(self, u: f32, v: f32) -> Option<f32> {
        let value = self.evaluate_raw(u, v)?;
        Some(match self {
            Self::RadialDistance | Self::Rmax => value.min(1.0),
            _ => value,
        })
    }
}

/// Selectable scalar transform described for the PICA200 procedural texture stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaProceduralTextureF {
    /// Pass the base-shape value through unchanged.
    Identity,
    /// Fold the [0, 1] domain into one triangular lobe.
    ///
    /// This uses the conventional tent-wave interpretation:
    /// F(g) = 1 - |2g - 1|. Public documentation identifies a triangle
    /// function but does not publish its exact hardware equation, so this
    /// helper remains isolated until compared with real PICA200 output.
    Triangle,
}

impl PicaProceduralTextureF {
    /// Applies the selected F(g) transform to a base-shape value in [0, 1].
    ///
    /// Values outside the documented input domain and non-finite values are
    /// rejected rather than clamped, to avoid hiding upstream coordinate bugs.
    pub fn evaluate(self, g: f32) -> Option<f32> {
        if !g.is_finite() || !(0.0..=1.0).contains(&g) {
            return None;
        }

        Some(match self {
            Self::Identity => g,
            Self::Triangle => 1.0 - (2.0 * g - 1.0).abs(),
        })
    }
}

/// Odd/even coordinate shifts applied by the procedural texture unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaProceduralTextureShift {
    None,
    Odd,
    Even,
    Reserved(u8),
}

/// Minification and mip filtering modes accepted by PROCTEX4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaProceduralTextureFilter {
    Nearest,
    Linear,
    NearestMipmapNearest,
    LinearMipmapNearest,
    NearestMipmapLinear,
    LinearMipmapLinear,
    Reserved(u8),
}

/// Register snapshot for PICA's procedural texture unit.
///
/// Noise fields and texture bias remain in their documented raw register
/// representation. This type does not claim to implement noise generation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaProceduralTextureConfig {
    /// Which ordinary texture-coordinate pair supplies U/V to unit 3.
    pub coordinate_source: PicaProceduralTextureCoordinateSource,
    pub u_clamp: PicaProceduralTextureClamp,
    pub v_clamp: PicaProceduralTextureClamp,
    pub rgb_mapping_function: PicaProceduralTextureMappingFunction,
    pub alpha_mapping_function: PicaProceduralTextureMappingFunction,
    pub separate_alpha: bool,
    pub noise_enabled: bool,
    pub u_shift: PicaProceduralTextureShift,
    pub v_shift: PicaProceduralTextureShift,
    pub texture_bias_lower: u8,
    pub u_noise_amplitude_fixed1_3_12: u16,
    pub u_noise_phase_float1_5_10: u16,
    pub v_noise_amplitude_fixed1_3_12: u16,
    pub v_noise_phase_float1_5_10: u16,
    pub u_noise_frequency_float1_5_10: u16,
    pub v_noise_frequency_float1_5_10: u16,
    pub minification_filter: PicaProceduralTextureFilter,
    pub min_lod: u8,
    pub max_lod: u8,
    pub texture_width: u8,
    pub texture_bias_upper: u8,
    pub mip_offsets: [u8; 4],
}

impl PicaProceduralTextureConfig {
    /// Selects the U/V pair routed to procedural texture unit 3.
    ///
    /// The pair is returned unchanged: this only resolves the coordinate-set
    /// selector and does not apply shifts, noise, wrapping, normalization, or
    /// clamping. Reserved selector values return `None`.
    pub fn select_coordinates(&self, texture_coordinates: &[[f32; 2]; 3]) -> Option<[f32; 2]> {
        match self.coordinate_source {
            PicaProceduralTextureCoordinateSource::Texture0 => Some(texture_coordinates[0]),
            PicaProceduralTextureCoordinateSource::Texture1 => Some(texture_coordinates[1]),
            PicaProceduralTextureCoordinateSource::Texture2 => Some(texture_coordinates[2]),
            PicaProceduralTextureCoordinateSource::Reserved(_) => None,
        }
    }
}

/// One fixed-point entry from the procedural noise/RGB/alpha mapping tables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaProceduralTextureCurveEntry {
    pub value_fixed0_0_12: u16,
    pub difference_fixed0_0_12: i16,
}

impl PicaProceduralTextureCurveEntry {
    /// Linearly interpolates this entry toward the next LUT entry.
    ///
    /// `fraction` must be in [0, 1]. The value uses unsigned 0.12 scaling;
    /// the signed 12-bit difference uses a 1/2048 scale as documented by
    /// PICA's mapping of the upper half of the unsigned range to [-1, 0).
    pub fn sample_linear(self, fraction: f32) -> Option<f32> {
        if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return None;
        }

        let value = f32::from(self.value_fixed0_0_12) / 4096.0;
        let difference = f32::from(self.difference_fixed0_0_12) / 2048.0;
        Some(value + difference * fraction)
    }
}
/// GPUREG_GAS_ATTENUATION: float1.5.10 raw density attenuation.
pub const REG_GAS_ATTENUATION: u16 = 0x0E4;
/// GPUREG_GAS_ACCMAX: float1.5.10 raw maximum accumulated density.
pub const REG_GAS_ACCMAX: u16 = 0x0E5;
/// GPUREG_GAS_LIGHT_XY: planar min/max intensity and density attenuation.
pub const REG_GAS_LIGHT_XY: u16 = 0x120;
/// GPUREG_GAS_LIGHT_Z: view min/max intensity and density attenuation.
pub const REG_GAS_LIGHT_Z: u16 = 0x121;
/// GPUREG_GAS_LIGHT_Z_COLOR: view intensity and color-LUT input selector.
pub const REG_GAS_LIGHT_Z_COLOR: u16 = 0x122;
/// GPUREG_GAS_ACCMAX_FEEDBACK: initial density (low) and max density (high).
pub const REG_GAS_ACCMAX_FEEDBACK: u16 = 0x125;
/// GPUREG_GAS_DELTAZ_DEPTH: depth attenuation and comparison-family selector.
pub const REG_GAS_DELTAZ_DEPTH: u16 = 0x126;
const REG_COLOR_OPERATION_FOR_GAS: usize = 0x100;
const REG_TEXENV_UPDATE_BUFFER_FOR_GAS: usize = 0x0E0;

/// Gas depth-comparison family selected by GPUREG_GAS_DELTAZ_DEPTH.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaGasDepthFunction {
    Never,
    Always,
    GreaterThanOrGreaterEqual,
    LessThanLessEqualEqualOrNotEqual,
}

/// One set of PICA gas light intensity parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaGasLightState {
    pub minimum_intensity: u8,
    pub maximum_intensity: u8,
    pub density_attenuation: u8,
}

/// Source used to generate the gas density before attenuation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaGasDensitySource {
    Plain,
    Depth,
}

/// Input selected for the signed/unsigned gas color lookup-table halves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaGasColorLutInput {
    Density,
    LightFactor,
}

/// Raw gas-stage register snapshot.
///
/// Float1.5.10 and fixed0.16.8 fields remain raw to avoid applying an
/// unverified conversion/rounding rule. Gas rasterization is not implemented.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PicaGasState {
    pub attenuation_float1_5_10: u16,
    pub accumulation_max_float1_5_10: u16,
    pub initial_max_density_float1_5_10: u16,
    pub maximum_density_feedback_float1_5_10: u16,
    pub planar_light: PicaGasLightState,
    pub view_light: PicaGasLightState,
    pub view_shading_intensity: u8,
    pub density_source: PicaGasDensitySource,
    pub color_lut_input: PicaGasColorLutInput,
    pub depth_attenuation_fixed0_16_8: u32,
    pub depth_function: PicaGasDepthFunction,
}

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

/// Triangle face culling selected by the guest.
///
/// "Clockwise" and "counter-clockwise" refer to the triangle winding in PICA
/// window space, where Y points up. The register encodings (0 keep all,
/// 1 keep clockwise, 2 keep counter-clockwise) come from public 3DS
/// documentation and are not verified on hardware; value 3 is treated as keep all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PicaCullMode {
    #[default]
    KeepAll,
    KeepClockwise,
    KeepCounterClockwise,
}

/// Scissor behavior selected by the guest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PicaScissorMode {
    #[default]
    Disabled,
    /// Fragments inside the rectangle are discarded.
    Exclude,
    /// Only fragments inside the rectangle are kept.
    Include,
}

/// Decoded scissor rectangle in PICA window coordinates (origin bottom-left,
/// both corners inclusive). Encodings are not verified on hardware; the
/// reserved mode value 2 is treated as disabled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PicaScissorState {
    pub mode: PicaScissorMode,
    pub x1: u32,
    pub y1: u32,
    pub x2: u32,
    pub y2: u32,
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
/// Vertex shader boolean constants b0-b15.
pub const REG_VSH_BOOLUNIFORM: u16 = 0x2B0;
/// Vertex shader integer constant i0; i1-i3 occupy the next three registers.
pub const REG_VSH_INTUNIFORM_I0: u16 = 0x2B1;
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
    fog_lut: [u32; FOG_LUT_ENTRY_COUNT],
    /// Raw 24-bit RGB gas LUT entries; gas rendering is not implemented yet.
    gas_lut: [u32; GAS_LUT_ENTRY_COUNT],
    /// Uploaded procedural noise, RGB-map and alpha-map tables (128 entries each).
    procedural_texture_map_luts: [[u32; 128]; 3],
    /// Uploaded procedural color and color-difference tables (256 entries each).
    procedural_texture_color_lut: [u32; 256],
    procedural_texture_color_difference_lut: [u32; 256],
    gas_accmax_feedback_max: u16,
    /// High-half value exposed to the guest; refreshed only after a gas fragment.
    gas_accmax_feedback_readback: u16,
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
            fog_lut: [0; FOG_LUT_ENTRY_COUNT],
            gas_lut: [0; GAS_LUT_ENTRY_COUNT],
            procedural_texture_map_luts: [[0; 128]; 3],
            procedural_texture_color_lut: [0; 256],
            procedural_texture_color_difference_lut: [0; 256],
            gas_accmax_feedback_max: 0,
            gas_accmax_feedback_readback: 0,
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

    /// Reads one uploaded 24-bit fog LUT entry.
    pub fn fog_lut_entry(&self, index: usize) -> Option<u32> {
        self.fog_lut.get(index).copied()
    }

    /// Reads one uploaded raw RGB gas LUT entry.
    ///
    /// This exposes table storage only; fragment gas shading is not modeled yet.
    pub fn gas_lut_entry(&self, index: usize) -> Option<u32> {
        self.gas_lut.get(index).copied()
    }

    /// Reads one uploaded procedural-texture LUT word.
    ///
    /// The entry is returned in its raw guest register representation. The
    /// procedural sampling/evaluation stage is separate and is not implied by
    /// exposing this storage.
    pub fn procedural_texture_lut_entry(
        &self,
        table: PicaProceduralTextureLut,
        index: usize,
    ) -> Option<u32> {
        if index >= table.entry_count() {
            return None;
        }

        Some(match table {
            PicaProceduralTextureLut::Noise => self.procedural_texture_map_luts[0][index],
            PicaProceduralTextureLut::RgbMapping => self.procedural_texture_map_luts[1][index],
            PicaProceduralTextureLut::AlphaMapping => self.procedural_texture_map_luts[2][index],
            PicaProceduralTextureLut::Color => self.procedural_texture_color_lut[index],
            PicaProceduralTextureLut::ColorDifference => {
                self.procedural_texture_color_difference_lut[index]
            }
        })
    }

    /// Decodes one noise or coordinate-mapping LUT entry.
    ///
    /// The value is unsigned fixed0.0.12. The difference is a signed 12-bit
    /// two's-complement fixed0.0.12 value describing the delta to the next
    /// element. This returns integer representations without rounding.
    pub fn procedural_texture_curve_entry(
        &self,
        table: PicaProceduralTextureLut,
        index: usize,
    ) -> Option<PicaProceduralTextureCurveEntry> {
        if !matches!(
            table,
            PicaProceduralTextureLut::Noise
                | PicaProceduralTextureLut::RgbMapping
                | PicaProceduralTextureLut::AlphaMapping
        ) {
            return None;
        }

        let raw = self.procedural_texture_lut_entry(table, index)?;
        let difference = ((raw >> 12) & 0x0FFF) as i16;
        let difference = if difference & 0x0800 != 0 {
            difference - 0x1000
        } else {
            difference
        };

        Some(PicaProceduralTextureCurveEntry {
            value_fixed0_0_12: (raw & 0x0FFF) as u16,
            difference_fixed0_0_12: difference,
        })
    }

    /// Decodes a procedural color-table entry as RGBA8.
    pub fn procedural_texture_color_entry(&self, index: usize) -> Option<[u8; 4]> {
        let raw = self.procedural_texture_lut_entry(PicaProceduralTextureLut::Color, index)?;
        Some([
            raw as u8,
            (raw >> 8) as u8,
            (raw >> 16) as u8,
            (raw >> 24) as u8,
        ])
    }

    /// Decodes the signed per-channel half-difference to the next color entry.
    pub fn procedural_texture_color_difference_entry(&self, index: usize) -> Option<[i8; 4]> {
        let raw =
            self.procedural_texture_lut_entry(PicaProceduralTextureLut::ColorDifference, index)?;
        Some([
            raw as u8 as i8,
            (raw >> 8) as u8 as i8,
            (raw >> 16) as u8 as i8,
            (raw >> 24) as u8 as i8,
        ])
    }

    /// Interpolates an RGBA color entry toward the next procedural color entry.
    ///
    /// The color-difference table stores half of each signed channel delta, so
    /// the interpolated byte-domain channel is `color + 2 * difference * fraction`.
    /// The result remains floating point; hardware rounding/saturation is not
    /// applied here.
    pub fn interpolate_procedural_texture_color(
        &self,
        index: usize,
        fraction: f32,
    ) -> Option<[f32; 4]> {
        if !fraction.is_finite() || !(0.0..=1.0).contains(&fraction) {
            return None;
        }

        let color = self.procedural_texture_color_entry(index)?;
        let difference = self.procedural_texture_color_difference_entry(index)?;
        let factor = 2.0 * fraction;

        Some([
            f32::from(color[0]) + f32::from(difference[0]) * factor,
            f32::from(color[1]) + f32::from(difference[1]) * factor,
            f32::from(color[2]) + f32::from(difference[2]) * factor,
            f32::from(color[3]) + f32::from(difference[3]) * factor,
        ])
    }

    /// Returns a typed snapshot of the six procedural-texture configuration registers.
    ///
    /// Numeric noise/bias fields stay raw because their fixed/float conversion
    /// and hardware rounding have not been independently verified.
    pub fn procedural_texture_config(&self) -> PicaProceduralTextureConfig {
        let texture_config = self.register(REG_TEXUNIT_CONFIG).unwrap_or(0);
        let proctex0 = self.register(REG_TEXUNIT3_PROCTEX0).unwrap_or(0);
        let proctex1 = self.register(REG_TEXUNIT3_PROCTEX1).unwrap_or(0);
        let proctex2 = self.register(REG_TEXUNIT3_PROCTEX2).unwrap_or(0);
        let proctex3 = self.register(REG_TEXUNIT3_PROCTEX3).unwrap_or(0);
        let proctex4 = self.register(REG_TEXUNIT3_PROCTEX4).unwrap_or(0);
        let proctex5 = self.register(REG_TEXUNIT3_PROCTEX5).unwrap_or(0);

        let decode_coordinate_source = |value: u8| match value {
            0 => PicaProceduralTextureCoordinateSource::Texture0,
            1 => PicaProceduralTextureCoordinateSource::Texture1,
            2 => PicaProceduralTextureCoordinateSource::Texture2,
            other => PicaProceduralTextureCoordinateSource::Reserved(other),
        };
        let decode_clamp = |value: u8| match value {
            0 => PicaProceduralTextureClamp::ClampToZero,
            1 => PicaProceduralTextureClamp::ClampToEdge,
            2 => PicaProceduralTextureClamp::SymmetricalRepeat,
            3 => PicaProceduralTextureClamp::MirroredRepeat,
            4 => PicaProceduralTextureClamp::Pulse,
            other => PicaProceduralTextureClamp::Reserved(other),
        };
        let decode_mapping = |value: u8| match value {
            0 => PicaProceduralTextureMappingFunction::U,
            1 => PicaProceduralTextureMappingFunction::USquared,
            2 => PicaProceduralTextureMappingFunction::V,
            3 => PicaProceduralTextureMappingFunction::VSquared,
            4 => PicaProceduralTextureMappingFunction::Average,
            5 => PicaProceduralTextureMappingFunction::SquaredAverage,
            6 => PicaProceduralTextureMappingFunction::RadialDistance,
            7 => PicaProceduralTextureMappingFunction::Minimum,
            8 => PicaProceduralTextureMappingFunction::Maximum,
            9 => PicaProceduralTextureMappingFunction::Rmax,
            other => PicaProceduralTextureMappingFunction::Reserved(other),
        };
        let decode_shift = |value: u8| match value {
            0 => PicaProceduralTextureShift::None,
            1 => PicaProceduralTextureShift::Odd,
            2 => PicaProceduralTextureShift::Even,
            other => PicaProceduralTextureShift::Reserved(other),
        };
        let decode_filter = |value: u8| match value {
            0 => PicaProceduralTextureFilter::Nearest,
            1 => PicaProceduralTextureFilter::Linear,
            2 => PicaProceduralTextureFilter::NearestMipmapNearest,
            3 => PicaProceduralTextureFilter::LinearMipmapNearest,
            4 => PicaProceduralTextureFilter::NearestMipmapLinear,
            5 => PicaProceduralTextureFilter::LinearMipmapLinear,
            other => PicaProceduralTextureFilter::Reserved(other),
        };

        PicaProceduralTextureConfig {
            coordinate_source: decode_coordinate_source(((texture_config >> 8) & 0x3) as u8),
            u_clamp: decode_clamp((proctex0 & 0x7) as u8),
            v_clamp: decode_clamp(((proctex0 >> 3) & 0x7) as u8),
            rgb_mapping_function: decode_mapping(((proctex0 >> 6) & 0xF) as u8),
            alpha_mapping_function: decode_mapping(((proctex0 >> 10) & 0xF) as u8),
            separate_alpha: proctex0 & (1 << 14) != 0,
            noise_enabled: proctex0 & (1 << 15) != 0,
            u_shift: decode_shift(((proctex0 >> 16) & 0x3) as u8),
            v_shift: decode_shift(((proctex0 >> 18) & 0x3) as u8),
            texture_bias_lower: ((proctex0 >> 20) & 0xFF) as u8,
            u_noise_amplitude_fixed1_3_12: proctex1 as u16,
            u_noise_phase_float1_5_10: (proctex1 >> 16) as u16,
            v_noise_amplitude_fixed1_3_12: proctex2 as u16,
            v_noise_phase_float1_5_10: (proctex2 >> 16) as u16,
            u_noise_frequency_float1_5_10: proctex3 as u16,
            v_noise_frequency_float1_5_10: (proctex3 >> 16) as u16,
            minification_filter: decode_filter((proctex4 & 0x7) as u8),
            min_lod: ((proctex4 >> 3) & 0xF) as u8,
            max_lod: ((proctex4 >> 7) & 0xF) as u8,
            texture_width: ((proctex4 >> 11) & 0xFF) as u8,
            texture_bias_upper: ((proctex4 >> 19) & 0xFF) as u8,
            mip_offsets: [
                proctex5 as u8,
                (proctex5 >> 8) as u8,
                (proctex5 >> 16) as u8,
                (proctex5 >> 24) as u8,
            ],
        }
    }

    /// Decodes one gas LUT entry into signed or unsigned RGB channel values.
    ///
    /// Entries 0..7 contain signed 8-bit channels; entries 8..15 contain
    /// unsigned 8-bit channels. The result is widened to i16 without scaling.
    pub fn gas_lut_rgb_entry(&self, index: usize) -> Option<[i16; 3]> {
        let value = self.gas_lut_entry(index)?;
        let channels = [value as u8, (value >> 8) as u8, (value >> 16) as u8];

        Some(if index < GAS_LUT_ENTRY_COUNT / 2 {
            channels.map(|channel| i16::from(channel as i8))
        } else {
            channels.map(i16::from)
        })
    }

    /// Returns a typed snapshot of the gas-related fixed-function registers.
    ///
    /// Numeric fields retain their documented raw representations. The
    /// snapshot does not imply that the gas fragment operation is implemented.
    pub fn gas_state(&self) -> PicaGasState {
        let light_xy = self.register(REG_GAS_LIGHT_XY).unwrap_or(0);
        let light_z = self.register(REG_GAS_LIGHT_Z).unwrap_or(0);
        let light_z_color = self.register(REG_GAS_LIGHT_Z_COLOR).unwrap_or(0);
        let delta_z_depth = self.register(REG_GAS_DELTAZ_DEPTH).unwrap_or(0);
        let feedback = self.register(REG_GAS_ACCMAX_FEEDBACK).unwrap_or(0);
        let density_config = self
            .register(REG_TEXENV_UPDATE_BUFFER_FOR_GAS as u16)
            .unwrap_or(0);

        let decode_light = |value: u32| PicaGasLightState {
            minimum_intensity: value as u8,
            maximum_intensity: (value >> 8) as u8,
            density_attenuation: (value >> 16) as u8,
        };

        PicaGasState {
            attenuation_float1_5_10: self.register(REG_GAS_ATTENUATION).unwrap_or(0) as u16,
            accumulation_max_float1_5_10: self.register(REG_GAS_ACCMAX).unwrap_or(0) as u16,
            initial_max_density_float1_5_10: feedback as u16,
            maximum_density_feedback_float1_5_10: (feedback >> 16) as u16,
            planar_light: decode_light(light_xy),
            view_light: decode_light(light_z),
            view_shading_intensity: light_z_color as u8,
            density_source: if density_config & (1 << 3) == 0 {
                PicaGasDensitySource::Plain
            } else {
                PicaGasDensitySource::Depth
            },
            color_lut_input: if light_z_color & (1 << 8) == 0 {
                PicaGasColorLutInput::Density
            } else {
                PicaGasColorLutInput::LightFactor
            },
            depth_attenuation_fixed0_16_8: delta_z_depth & 0x00FF_FFFF,
            depth_function: match (delta_z_depth >> 24) & 3 {
                0 => PicaGasDepthFunction::Never,
                1 => PicaGasDepthFunction::Always,
                2 => PicaGasDepthFunction::GreaterThanOrGreaterEqual,
                _ => PicaGasDepthFunction::LessThanLessEqualEqualOrNotEqual,
            },
        }
    }

    /// Updates the read-only maximum-density feedback using the documented
    /// unsigned 16-bit comparison. Returns None unless gas fragment mode is
    /// selected in GPUREG_COLOR_OPERATION.
    pub fn update_gas_accmax_feedback(&mut self, density_raw: u16) -> Option<u16> {
        if self.registers[REG_COLOR_OPERATION_FOR_GAS] & 3 != 1 {
            return None;
        }

        self.gas_accmax_feedback_max = self.gas_accmax_feedback_max.max(density_raw);
        self.gas_accmax_feedback_readback = self.gas_accmax_feedback_max;
        let index = usize::from(REG_GAS_ACCMAX_FEEDBACK);
        let initial = self.registers[index] & 0xFFFF;
        self.registers[index] = initial | (u32::from(self.gas_accmax_feedback_readback) << 16);
        Some(self.gas_accmax_feedback_max)
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

    /// Decodes GPUREG_FACECULLING_CONFIG.
    pub fn cull_mode(&self) -> PicaCullMode {
        match self.register(REG_FACECULLING_CONFIG).unwrap_or(0) & 3 {
            1 => PicaCullMode::KeepClockwise,
            2 => PicaCullMode::KeepCounterClockwise,
            _ => PicaCullMode::KeepAll,
        }
    }

    /// Decodes the scissor registers.
    pub fn scissor_state(&self) -> PicaScissorState {
        let mode = match self.register(REG_SCISSORTEST_MODE).unwrap_or(0) & 3 {
            1 => PicaScissorMode::Exclude,
            3 => PicaScissorMode::Include,
            _ => PicaScissorMode::Disabled,
        };
        let pos = self.register(REG_SCISSORTEST_POS).unwrap_or(0);
        let dim = self.register(REG_SCISSORTEST_DIM).unwrap_or(0);
        PicaScissorState {
            mode,
            x1: pos & 0x3FF,
            y1: (pos >> 16) & 0x3FF,
            x2: dim & 0x3FF,
            y2: (dim >> 16) & 0x3FF,
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
        self.apply_lookup_register_write(id, 0xF);
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

    /// Applies the fog and gas lookup-table FIFO side effects.
    ///
    /// Each write to one of the eight fog data ports commits a 24-bit entry
    /// and advances the 128-entry pointer. The single gas data port commits
    /// one RGB entry and advances its 16-entry pointer. Gas table storage is
    /// modeled independently of the still-unimplemented gas fragment stage.
    fn apply_lookup_register_write(&mut self, id: u16, written_mask: u8) {
        if id == REG_GAS_ACCMAX_FEEDBACK {
            let index = usize::from(REG_GAS_ACCMAX_FEEDBACK);
            let initial = self.registers[index] & 0xFFFF;
            // Only writes touching the low 16 bits reset the internal maximum.
            // The high half is read-only and remains unchanged until a later
            // gas fragment updates the hardware feedback value.
            if written_mask & 0x3 != 0 && self.registers[REG_COLOR_OPERATION_FOR_GAS] & 3 == 1 {
                self.gas_accmax_feedback_max = initial as u16;
            }
            self.registers[index] = initial | (u32::from(self.gas_accmax_feedback_readback) << 16);
            return;
        }

        if id == REG_FOG_LUT_INDEX {
            self.registers[usize::from(REG_FOG_LUT_INDEX)] &= 0xFFFF;
            return;
        }

        let fog_data_end = REG_FOG_LUT_DATA0 + FOG_LUT_DATA_REGISTER_COUNT - 1;
        if (REG_FOG_LUT_DATA0..=fog_data_end).contains(&id) {
            let index_register = usize::from(REG_FOG_LUT_INDEX);
            let index = (self.registers[index_register] as usize) % FOG_LUT_ENTRY_COUNT;
            self.fog_lut[index] = self.registers[usize::from(id)] & 0x00FF_FFFF;
            self.registers[index_register] = ((index + 1) % FOG_LUT_ENTRY_COUNT) as u32;
            return;
        }

        if id == REG_GAS_LUT_INDEX {
            self.registers[usize::from(REG_GAS_LUT_INDEX)] &= 0xFFFF;
            return;
        }

        if id == REG_GAS_LUT_DATA {
            let index_register = usize::from(REG_GAS_LUT_INDEX);
            let index = (self.registers[index_register] as usize) % GAS_LUT_ENTRY_COUNT;
            self.gas_lut[index] = self.registers[usize::from(REG_GAS_LUT_DATA)] & 0x00FF_FFFF;
            self.registers[index_register] = ((index + 1) % GAS_LUT_ENTRY_COUNT) as u32;
            return;
        }

        if id == REG_PROCTEX_LUT {
            self.registers[usize::from(REG_PROCTEX_LUT)] &= 0x0FFF;
            return;
        }

        let proctex_data_end = REG_PROCTEX_LUT_DATA0 + PROCTEX_LUT_DATA_REGISTER_COUNT - 1;
        if (REG_PROCTEX_LUT_DATA0..=proctex_data_end).contains(&id) {
            let selector_index = usize::from(REG_PROCTEX_LUT);
            let selector = self.registers[selector_index] & 0x0FFF;
            let table_selector = (selector >> 8) & 0xF;
            let Some(table) = PicaProceduralTextureLut::from_selector(table_selector) else {
                return;
            };

            let index = (selector as usize & 0xFF) % table.entry_count();
            let value = self.registers[usize::from(id)];
            match table {
                PicaProceduralTextureLut::Noise => {
                    self.procedural_texture_map_luts[0][index] = value;
                }
                PicaProceduralTextureLut::RgbMapping => {
                    self.procedural_texture_map_luts[1][index] = value;
                }
                PicaProceduralTextureLut::AlphaMapping => {
                    self.procedural_texture_map_luts[2][index] = value;
                }
                PicaProceduralTextureLut::Color => {
                    self.procedural_texture_color_lut[index] = value;
                }
                PicaProceduralTextureLut::ColorDifference => {
                    self.procedural_texture_color_difference_lut[index] = value;
                }
            }

            let next_index = (index + 1) % table.entry_count();
            self.registers[selector_index] = (table_selector << 8) | next_index as u32;
        }
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
            let extra_parameters = ((header >> 20) & 0x7FF) as usize;
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
                self.apply_lookup_register_write(register as u16, byte_mask);
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
            // Commands are padded to a multiple of two words (8-byte alignment).
            cursor += (command_words + 1) & !1;
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
        self.fog_lut.fill(0);
        self.gas_lut.fill(0);
        self.procedural_texture_map_luts.fill([0; 128]);
        self.procedural_texture_color_lut.fill(0);
        self.procedural_texture_color_difference_lut.fill(0);
        self.gas_accmax_feedback_max = 0;
        self.gas_accmax_feedback_readback = 0;
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
        let extra_parameters = ((header >> 20) & 0x7FF) as usize;
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
        // Commands are padded to a multiple of two words (8-byte alignment).
        cursor += (command_words + 1) & !1;
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
    #[test]
    fn cull_and_scissor_registers_decode_from_guest_writes() {
        let mut pica = Pica200::new();
        assert_eq!(pica.cull_mode(), PicaCullMode::KeepAll);
        assert_eq!(pica.scissor_state(), PicaScissorState::default());

        for (raw, expected) in [
            (0, PicaCullMode::KeepAll),
            (1, PicaCullMode::KeepClockwise),
            (2, PicaCullMode::KeepCounterClockwise),
            (3, PicaCullMode::KeepAll),
        ] {
            pica.write_register(REG_FACECULLING_CONFIG, raw).unwrap();
            assert_eq!(pica.cull_mode(), expected);
        }

        pica.write_register(REG_SCISSORTEST_MODE, 3).unwrap();
        pica.write_register(REG_SCISSORTEST_POS, (20 << 16) | 10)
            .unwrap();
        pica.write_register(REG_SCISSORTEST_DIM, (239 << 16) | 399)
            .unwrap();
        assert_eq!(
            pica.scissor_state(),
            PicaScissorState {
                mode: PicaScissorMode::Include,
                x1: 10,
                y1: 20,
                x2: 399,
                y2: 239,
            }
        );
        pica.write_register(REG_SCISSORTEST_MODE, 1).unwrap();
        assert_eq!(pica.scissor_state().mode, PicaScissorMode::Exclude);
        pica.write_register(REG_SCISSORTEST_MODE, 2).unwrap();
        assert_eq!(pica.scissor_state().mode, PicaScissorMode::Disabled);
    }

    use super::*;
    use crate::gpu::REG_COLOR_OPERATION;

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
    fn fog_lut_fifo_uploads_eight_data_ports_and_wraps_at_128_entries() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_FOG_LUT_INDEX, 126).unwrap();
        gpu.execute_command_list(&[
            0x0011_2233,
            command(REG_FOG_LUT_DATA0, 0xF, true, 3),
            0x0044_5566,
            0x0077_8899,
            0x00AA_BBCC,
        ])
        .unwrap();

        assert_eq!(gpu.fog_lut_entry(126), Some(0x0011_2233));
        assert_eq!(gpu.fog_lut_entry(127), Some(0x0044_5566));
        assert_eq!(gpu.fog_lut_entry(0), Some(0x0077_8899));
        assert_eq!(gpu.fog_lut_entry(1), Some(0x00AA_BBCC));
        assert_eq!(gpu.register(REG_FOG_LUT_INDEX), Some(2));
    }

    #[test]
    fn procedural_texture_lut_fifo_routes_all_tables_and_wraps_indices() {
        let mut gpu = Pica200::new();

        gpu.write_register(
            REG_PROCTEX_LUT,
            (u32::from(PicaProceduralTextureLut::Noise as u8) << 8) | 127,
        )
        .unwrap();
        gpu.execute_command_list(&[
            0x1020_3040,
            command(REG_PROCTEX_LUT_DATA0, 0xF, true, 2),
            0x5060_7080,
            0x90A0_B0C0,
        ])
        .unwrap();
        assert_eq!(
            gpu.procedural_texture_lut_entry(PicaProceduralTextureLut::Noise, 127),
            Some(0x1020_3040)
        );
        assert_eq!(
            gpu.procedural_texture_lut_entry(PicaProceduralTextureLut::Noise, 0),
            Some(0x5060_7080)
        );
        assert_eq!(
            gpu.procedural_texture_lut_entry(PicaProceduralTextureLut::Noise, 1),
            Some(0x90A0_B0C0)
        );
        assert_eq!(gpu.register(REG_PROCTEX_LUT), Some(2));

        for (table, start, value, next) in [
            (
                PicaProceduralTextureLut::RgbMapping,
                127,
                0x1111_1111,
                0x0200,
            ),
            (
                PicaProceduralTextureLut::AlphaMapping,
                127,
                0x2222_2222,
                0x0300,
            ),
            (PicaProceduralTextureLut::Color, 255, 0x3333_3333, 0x0400),
            (
                PicaProceduralTextureLut::ColorDifference,
                255,
                0x4444_4444,
                0x0500,
            ),
        ] {
            gpu.write_register(REG_PROCTEX_LUT, ((table as u32) << 8) | start)
                .unwrap();
            gpu.write_register(REG_PROCTEX_LUT_DATA0, value).unwrap();
            assert_eq!(
                gpu.procedural_texture_lut_entry(table, start as usize),
                Some(value)
            );
            assert_eq!(gpu.register(REG_PROCTEX_LUT), Some(next));
        }

        assert_eq!(
            gpu.procedural_texture_lut_entry(PicaProceduralTextureLut::Noise, 128),
            None
        );
        assert_eq!(
            gpu.procedural_texture_lut_entry(PicaProceduralTextureLut::Color, 256),
            None
        );
    }

    #[test]
    fn procedural_texture_lut_ignores_reserved_table_ids_and_reset_clears_tables() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_PROCTEX_LUT, 0x0105).unwrap();
        gpu.write_register(REG_PROCTEX_LUT_DATA0, 0xDEAD_BEEF)
            .unwrap();
        assert_eq!(gpu.register(REG_PROCTEX_LUT), Some(0x0105));
        assert_eq!(
            gpu.procedural_texture_lut_entry(PicaProceduralTextureLut::Noise, 5),
            Some(0)
        );

        gpu.write_register(REG_PROCTEX_LUT, 0x04FF).unwrap();
        gpu.write_register(REG_PROCTEX_LUT_DATA0, 0xAABB_CCDD)
            .unwrap();
        assert_eq!(
            gpu.procedural_texture_lut_entry(PicaProceduralTextureLut::Color, 255),
            Some(0xAABB_CCDD)
        );
        gpu.reset();
        assert_eq!(gpu.register(REG_PROCTEX_LUT), Some(0));
        assert_eq!(
            gpu.procedural_texture_lut_entry(PicaProceduralTextureLut::Color, 255),
            Some(0)
        );
    }
    #[test]
    fn procedural_texture_registers_decode_to_typed_snapshot() {
        let mut gpu = Pica200::new();
        let proctex0 = 1
            | (3 << 3)
            | (9 << 6)
            | (4 << 10)
            | (1 << 14)
            | (1 << 15)
            | (1 << 16)
            | (2 << 18)
            | (0xA5 << 20);
        gpu.write_register(REG_TEXUNIT3_PROCTEX0, proctex0).unwrap();
        gpu.write_register(REG_TEXUNIT3_PROCTEX1, 0xFEDC_BA98)
            .unwrap();
        gpu.write_register(REG_TEXUNIT3_PROCTEX2, 0x7654_3210)
            .unwrap();
        gpu.write_register(REG_TEXUNIT3_PROCTEX3, 0x9ABC_DEF0)
            .unwrap();
        gpu.write_register(
            REG_TEXUNIT3_PROCTEX4,
            5 | (3 << 3) | (9 << 7) | (200 << 11) | (0x5A << 19),
        )
        .unwrap();
        gpu.write_register(REG_TEXUNIT3_PROCTEX5, 0xE0C0_8040)
            .unwrap();

        let config = gpu.procedural_texture_config();
        assert_eq!(config.u_clamp, PicaProceduralTextureClamp::ClampToEdge);
        assert_eq!(config.v_clamp, PicaProceduralTextureClamp::MirroredRepeat);
        assert_eq!(
            config.rgb_mapping_function,
            PicaProceduralTextureMappingFunction::Rmax
        );
        assert_eq!(
            config.alpha_mapping_function,
            PicaProceduralTextureMappingFunction::Average
        );
        assert!(config.separate_alpha);
        assert!(config.noise_enabled);
        assert_eq!(config.u_shift, PicaProceduralTextureShift::Odd);
        assert_eq!(config.v_shift, PicaProceduralTextureShift::Even);
        assert_eq!(config.texture_bias_lower, 0xA5);
        assert_eq!(config.u_noise_amplitude_fixed1_3_12, 0xBA98);
        assert_eq!(config.u_noise_phase_float1_5_10, 0xFEDC);
        assert_eq!(config.v_noise_amplitude_fixed1_3_12, 0x3210);
        assert_eq!(config.v_noise_phase_float1_5_10, 0x7654);
        assert_eq!(config.u_noise_frequency_float1_5_10, 0xDEF0);
        assert_eq!(config.v_noise_frequency_float1_5_10, 0x9ABC);
        assert_eq!(
            config.minification_filter,
            PicaProceduralTextureFilter::LinearMipmapLinear
        );
        assert_eq!(config.min_lod, 3);
        assert_eq!(config.max_lod, 9);
        assert_eq!(config.texture_width, 200);
        assert_eq!(config.texture_bias_upper, 0x5A);
        assert_eq!(config.mip_offsets, [0x40, 0x80, 0xC0, 0xE0]);
    }

    #[test]
    fn procedural_texture_config_decodes_all_rgb_and_alpha_mapping_selectors() {
        let mut gpu = Pica200::new();
        let decode_mapping = |selector: u8| match selector {
            0 => PicaProceduralTextureMappingFunction::U,
            1 => PicaProceduralTextureMappingFunction::USquared,
            2 => PicaProceduralTextureMappingFunction::V,
            3 => PicaProceduralTextureMappingFunction::VSquared,
            4 => PicaProceduralTextureMappingFunction::Average,
            5 => PicaProceduralTextureMappingFunction::SquaredAverage,
            6 => PicaProceduralTextureMappingFunction::RadialDistance,
            7 => PicaProceduralTextureMappingFunction::Minimum,
            8 => PicaProceduralTextureMappingFunction::Maximum,
            9 => PicaProceduralTextureMappingFunction::Rmax,
            other => PicaProceduralTextureMappingFunction::Reserved(other),
        };

        for rgb_selector in 0_u8..=15 {
            let alpha_selector = 15 - rgb_selector;
            let separate_alpha = rgb_selector & 1 != 0;
            let separate_alpha_bit = if separate_alpha { 1 << 14 } else { 0 };
            let proctex0 = (u32::from(rgb_selector) << 6)
                | (u32::from(alpha_selector) << 10)
                | separate_alpha_bit;

            gpu.write_register(REG_TEXUNIT3_PROCTEX0, proctex0).unwrap();

            let config = gpu.procedural_texture_config();
            assert_eq!(
                config.rgb_mapping_function,
                decode_mapping(rgb_selector),
                "RGB selector {rgb_selector}"
            );
            assert_eq!(
                config.alpha_mapping_function,
                decode_mapping(alpha_selector),
                "alpha selector {alpha_selector}"
            );
            assert_eq!(config.separate_alpha, separate_alpha);
        }
    }

    #[test]
    fn procedural_texture_config_decodes_coordinate_source_selector() {
        let mut gpu = Pica200::new();
        let selectors = [
            PicaProceduralTextureCoordinateSource::Texture0,
            PicaProceduralTextureCoordinateSource::Texture1,
            PicaProceduralTextureCoordinateSource::Texture2,
            PicaProceduralTextureCoordinateSource::Reserved(3),
        ];

        for (selector, expected) in selectors.into_iter().enumerate() {
            gpu.write_register(REG_TEXUNIT_CONFIG, (selector as u32) << 8)
                .unwrap();
            assert_eq!(gpu.procedural_texture_config().coordinate_source, expected);
        }
    }

    #[test]
    fn procedural_texture_config_selects_coordinate_pair_without_transforming_it() {
        let texture_coordinates = [[-0.25, 1.5], [3.0, -2.0], [0.125, 0.875]];
        let mut gpu = Pica200::new();
        let selectors = [(0, [-0.25, 1.5]), (1, [3.0, -2.0]), (2, [0.125, 0.875])];

        for (selector, expected) in selectors {
            gpu.write_register(REG_TEXUNIT_CONFIG, selector << 8)
                .unwrap();
            assert_eq!(
                gpu.procedural_texture_config()
                    .select_coordinates(&texture_coordinates),
                Some(expected)
            );
        }

        gpu.write_register(REG_TEXUNIT_CONFIG, 3 << 8).unwrap();
        assert_eq!(
            gpu.procedural_texture_config()
                .select_coordinates(&texture_coordinates),
            None
        );
    }

    #[test]
    fn procedural_texture_lut_entries_decode_fixed_point_and_rgba_channels() {
        let mut gpu = Pica200::new();

        gpu.write_register(REG_PROCTEX_LUT, 0x0007).unwrap();
        gpu.write_register(REG_PROCTEX_LUT_DATA0, 0x00F8_0123)
            .unwrap();
        assert_eq!(
            gpu.procedural_texture_curve_entry(PicaProceduralTextureLut::Noise, 7),
            Some(PicaProceduralTextureCurveEntry {
                value_fixed0_0_12: 0x123,
                difference_fixed0_0_12: -128,
            })
        );
        assert_eq!(
            gpu.procedural_texture_curve_entry(PicaProceduralTextureLut::Color, 7),
            None
        );

        gpu.write_register(REG_PROCTEX_LUT, 0x040B).unwrap();
        gpu.write_register(REG_PROCTEX_LUT_DATA0, 0x8040_2010)
            .unwrap();
        assert_eq!(
            gpu.procedural_texture_color_entry(11),
            Some([0x10, 0x20, 0x40, 0x80])
        );

        gpu.write_register(REG_PROCTEX_LUT, 0x050D).unwrap();
        gpu.write_register(REG_PROCTEX_LUT_DATA0, 0x807F_01FE)
            .unwrap();
        assert_eq!(
            gpu.procedural_texture_color_difference_entry(13),
            Some([-2, 1, 127, -128])
        );
        assert_eq!(gpu.procedural_texture_color_difference_entry(256), None);
    }

    #[test]
    fn procedural_texture_luts_interpolate_curve_and_rgba_entries() {
        let mut gpu = Pica200::new();

        gpu.write_register(REG_PROCTEX_LUT, 0x0007).unwrap();
        gpu.write_register(REG_PROCTEX_LUT_DATA0, 0x00F8_0123)
            .unwrap();
        let curve = gpu
            .procedural_texture_curve_entry(PicaProceduralTextureLut::Noise, 7)
            .unwrap();
        assert!((curve.sample_linear(0.0).unwrap() - (0x123 as f32 / 4096.0)).abs() < 0.000_001);
        assert!((curve.sample_linear(0.5).unwrap() - (0x0A3 as f32 / 4096.0)).abs() < 0.000_001);
        assert!((curve.sample_linear(1.0).unwrap() - (0x023 as f32 / 4096.0)).abs() < 0.000_001);
        assert_eq!(curve.sample_linear(1.01), None);
        assert_eq!(curve.sample_linear(f32::NAN), None);

        gpu.write_register(REG_PROCTEX_LUT, 0x040F).unwrap();
        gpu.write_register(REG_PROCTEX_LUT_DATA0, 0xA08C_7864)
            .unwrap();
        gpu.write_register(REG_PROCTEX_LUT, 0x050F).unwrap();
        gpu.write_register(REG_PROCTEX_LUT_DATA0, 0xEC14_F60A)
            .unwrap();
        assert_eq!(
            gpu.interpolate_procedural_texture_color(15, 0.0),
            Some([100.0, 120.0, 140.0, 160.0])
        );
        assert_eq!(
            gpu.interpolate_procedural_texture_color(15, 0.5),
            Some([110.0, 110.0, 160.0, 140.0])
        );
        assert_eq!(
            gpu.interpolate_procedural_texture_color(15, 1.0),
            Some([120.0, 100.0, 180.0, 120.0])
        );
        assert_eq!(gpu.interpolate_procedural_texture_color(15, -0.1), None);
        assert_eq!(gpu.interpolate_procedural_texture_color(256, 0.5), None);
    }
    #[test]
    fn procedural_texture_f_functions_evaluate_identity_and_triangle_domain() {
        let samples = [(0.0, 0.0), (0.25, 0.5), (0.5, 1.0), (0.75, 0.5), (1.0, 0.0)];

        for (g, expected_triangle) in samples {
            assert_eq!(PicaProceduralTextureF::Identity.evaluate(g), Some(g));
            let actual = PicaProceduralTextureF::Triangle.evaluate(g).unwrap();
            assert!(
                (actual - expected_triangle).abs() < 0.000_001,
                "triangle F({g}) expected {expected_triangle}, got {actual}"
            );
        }

        for invalid in [-0.001, 1.001, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(PicaProceduralTextureF::Identity.evaluate(invalid), None);
            assert_eq!(PicaProceduralTextureF::Triangle.evaluate(invalid), None);
        }
    }
    #[test]
    fn procedural_texture_mapping_functions_evaluate_documented_g_equations() {
        let u = 0.25_f32;
        let v = 0.5_f32;
        let cases = [
            (PicaProceduralTextureMappingFunction::U, 0.25),
            (PicaProceduralTextureMappingFunction::USquared, 0.0625),
            (PicaProceduralTextureMappingFunction::V, 0.5),
            (PicaProceduralTextureMappingFunction::VSquared, 0.25),
            (PicaProceduralTextureMappingFunction::Average, 0.375),
            (
                PicaProceduralTextureMappingFunction::SquaredAverage,
                0.15625,
            ),
            (
                PicaProceduralTextureMappingFunction::RadialDistance,
                0.3125_f32.sqrt(),
            ),
            (PicaProceduralTextureMappingFunction::Minimum, 0.25),
            (PicaProceduralTextureMappingFunction::Maximum, 0.5),
        ];

        for (function, expected) in cases {
            let actual = function.evaluate_raw(u, v).unwrap();
            assert!(
                (actual - expected).abs() < 0.000_001,
                "{function:?}: expected {expected}, got {actual}"
            );
        }

        let expected_rmax = ((u + v) * 0.5 + (u * u + v * v).sqrt()) * 0.5;
        let actual_rmax = PicaProceduralTextureMappingFunction::Rmax
            .evaluate_raw(u, v)
            .unwrap();
        assert!((actual_rmax - expected_rmax).abs() < 0.000_001);

        assert_eq!(
            PicaProceduralTextureMappingFunction::RadialDistance.evaluate_for_lookup(1.0, 1.0),
            Some(1.0)
        );
        assert_eq!(
            PicaProceduralTextureMappingFunction::Rmax.evaluate_for_lookup(1.0, 1.0),
            Some(1.0)
        );
        assert_eq!(
            PicaProceduralTextureMappingFunction::Reserved(10).evaluate_raw(u, v),
            None
        );
        assert_eq!(
            PicaProceduralTextureMappingFunction::U.evaluate_raw(f32::NAN, v),
            None
        );
        assert_eq!(
            PicaProceduralTextureMappingFunction::V.evaluate_raw(u, f32::INFINITY),
            None
        );

        // Reject non-finite results even when both supplied coordinates are finite.
        for function in [
            PicaProceduralTextureMappingFunction::USquared,
            PicaProceduralTextureMappingFunction::Average,
            PicaProceduralTextureMappingFunction::SquaredAverage,
            PicaProceduralTextureMappingFunction::RadialDistance,
            PicaProceduralTextureMappingFunction::Rmax,
        ] {
            assert_eq!(
                function.evaluate_raw(f32::MAX, f32::MAX),
                None,
                "{function:?} must reject arithmetic overflow"
            );
        }

        // Raw G evaluation does not clamp a finite result to the unit interval.
        assert_eq!(
            PicaProceduralTextureMappingFunction::U.evaluate_raw(f32::MAX, 0.0),
            Some(f32::MAX)
        );
    }
    #[test]
    fn gas_lut_fifo_uploads_repeated_data_port_and_wraps_at_16_entries() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_GAS_LUT_INDEX, 14).unwrap();
        gpu.execute_command_list(&[
            0xFF11_2233,
            command(REG_GAS_LUT_DATA, 0xF, false, 3),
            0xFF44_5566,
            0xFF77_8899,
            0xFFAA_BBCC,
        ])
        .unwrap();

        assert_eq!(gpu.gas_lut_entry(14), Some(0x0011_2233));
        assert_eq!(gpu.gas_lut_entry(15), Some(0x0044_5566));
        assert_eq!(gpu.gas_lut_entry(0), Some(0x0077_8899));
        assert_eq!(gpu.gas_lut_entry(1), Some(0x00AA_BBCC));
        assert_eq!(gpu.register(REG_GAS_LUT_INDEX), Some(2));
    }

    #[test]
    fn gas_lut_index_is_16_bit_and_direct_data_writes_advance_it() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_GAS_LUT_INDEX, 0x0010).unwrap();
        gpu.write_register(REG_GAS_LUT_DATA, 0xFF12_3456).unwrap();

        assert_eq!(gpu.gas_lut_entry(0), Some(0x0012_3456));
        assert_eq!(gpu.register(REG_GAS_LUT_INDEX), Some(1));
    }

    #[test]
    fn gas_lut_decodes_signed_and_unsigned_rgb_halves() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_GAS_LUT_INDEX, 0).unwrap();
        gpu.write_register(REG_GAS_LUT_DATA, 0x007F_80FF).unwrap();
        gpu.write_register(REG_GAS_LUT_INDEX, 8).unwrap();
        gpu.write_register(REG_GAS_LUT_DATA, 0x00FF_807F).unwrap();

        assert_eq!(gpu.gas_lut_rgb_entry(0), Some([-1, -128, 127]));
        assert_eq!(gpu.gas_lut_rgb_entry(8), Some([127, 128, 255]));
        assert_eq!(gpu.gas_lut_rgb_entry(16), None);
    }

    #[test]
    fn gas_state_decodes_raw_fixed_fields_and_light_selectors() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_TEXENV_UPDATE_BUFFER_FOR_GAS as u16, 7 | (1 << 3))
            .unwrap();
        gpu.write_register(REG_GAS_ATTENUATION, 0xA55A).unwrap();
        gpu.write_register(REG_GAS_ACCMAX, 0x1234).unwrap();
        gpu.write_register(REG_GAS_LIGHT_XY, 0x0033_2211).unwrap();
        gpu.write_register(REG_GAS_LIGHT_Z, 0x0066_5544).unwrap();
        gpu.write_register(REG_GAS_LIGHT_Z_COLOR, 0x100 | 0x77)
            .unwrap();
        gpu.write_register(REG_GAS_DELTAZ_DEPTH, (2 << 24) | 0x00AB_CDEF)
            .unwrap();

        assert_eq!(
            gpu.gas_state(),
            PicaGasState {
                attenuation_float1_5_10: 0xA55A,
                accumulation_max_float1_5_10: 0x1234,
                initial_max_density_float1_5_10: 0,
                maximum_density_feedback_float1_5_10: 0,
                planar_light: PicaGasLightState {
                    minimum_intensity: 0x11,
                    maximum_intensity: 0x22,
                    density_attenuation: 0x33,
                },
                view_light: PicaGasLightState {
                    minimum_intensity: 0x44,
                    maximum_intensity: 0x55,
                    density_attenuation: 0x66,
                },
                view_shading_intensity: 0x77,
                density_source: PicaGasDensitySource::Depth,
                color_lut_input: PicaGasColorLutInput::LightFactor,
                depth_attenuation_fixed0_16_8: 0x00AB_CDEF,
                depth_function: PicaGasDepthFunction::GreaterThanOrGreaterEqual,
            }
        );

        gpu.write_register(REG_GAS_LIGHT_Z_COLOR, 0x33).unwrap();
        assert_eq!(
            gpu.gas_state().color_lut_input,
            PicaGasColorLutInput::Density
        );

        gpu.write_register(REG_TEXENV_UPDATE_BUFFER_FOR_GAS as u16, 7)
            .unwrap();
        assert_eq!(gpu.gas_state().density_source, PicaGasDensitySource::Plain);
    }

    #[test]
    fn gas_accmax_feedback_resets_in_gas_mode_and_publishes_after_next_fragment() {
        let mut gpu = Pica200::new();

        gpu.write_register(REG_GAS_ACCMAX_FEEDBACK, 0xFFFF_1234)
            .unwrap();
        assert_eq!(gpu.register(REG_GAS_ACCMAX_FEEDBACK), Some(0x0000_1234));
        assert_eq!(gpu.update_gas_accmax_feedback(0x7000), None);

        gpu.write_register(REG_COLOR_OPERATION, 1).unwrap();
        gpu.write_register(REG_GAS_ACCMAX_FEEDBACK, 0xAAAA_8000)
            .unwrap();
        // The reset changes the internal maximum now, but readback remains
        // unchanged until the next fragment runs through gas mode.
        assert_eq!(gpu.register(REG_GAS_ACCMAX_FEEDBACK), Some(0x0000_8000));
        assert_eq!(gpu.update_gas_accmax_feedback(0x7FFF), Some(0x8000));
        assert_eq!(gpu.register(REG_GAS_ACCMAX_FEEDBACK), Some(0x8000_8000));
        assert_eq!(gpu.update_gas_accmax_feedback(0xFFFF), Some(0xFFFF));
        assert_eq!(gpu.update_gas_accmax_feedback(0x0001), Some(0xFFFF));
        assert_eq!(gpu.register(REG_GAS_ACCMAX_FEEDBACK), Some(0xFFFF_8000));

        gpu.write_register(REG_COLOR_OPERATION, 0).unwrap();
        gpu.write_register(REG_GAS_ACCMAX_FEEDBACK, 0xDEAD_4321)
            .unwrap();
        assert_eq!(gpu.register(REG_GAS_ACCMAX_FEEDBACK), Some(0xFFFF_4321));
    }

    #[test]
    fn gas_accmax_feedback_command_list_write_obeys_current_mode_and_readback_delay() {
        let mut gpu = Pica200::new();
        gpu.execute_command_list(&[
            1,
            command(REG_COLOR_OPERATION, 0xF, false, 0),
            0xBEEF_2222,
            command(REG_GAS_ACCMAX_FEEDBACK, 0xF, false, 0),
        ])
        .unwrap();
        assert_eq!(gpu.register(REG_GAS_ACCMAX_FEEDBACK), Some(0x0000_2222));
        assert_eq!(gpu.update_gas_accmax_feedback(0x1111), Some(0x2222));
        assert_eq!(gpu.register(REG_GAS_ACCMAX_FEEDBACK), Some(0x2222_2222));
    }

    #[test]
    fn gas_accmax_feedback_ignores_high_half_only_writes() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_COLOR_OPERATION, 1).unwrap();
        gpu.write_register(REG_GAS_ACCMAX_FEEDBACK, 0x0000_9000)
            .unwrap();
        assert_eq!(gpu.update_gas_accmax_feedback(0xA000), Some(0xA000));
        assert_eq!(gpu.register(REG_GAS_ACCMAX_FEEDBACK), Some(0xA000_9000));

        gpu.execute_command_list(&[
            0x0000_0000,
            command(REG_GAS_ACCMAX_FEEDBACK, 0b1100, false, 0),
        ])
        .unwrap();
        assert_eq!(gpu.register(REG_GAS_ACCMAX_FEEDBACK), Some(0xA000_9000));
        assert_eq!(gpu.update_gas_accmax_feedback(0x0100), Some(0xA000));
    }

    #[test]
    fn gas_lut_is_cleared_on_gpu_reset() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_GAS_LUT_INDEX, 7).unwrap();
        gpu.write_register(REG_GAS_LUT_DATA, 0x0012_3456).unwrap();
        gpu.reset();
        assert_eq!(gpu.gas_lut_entry(7), Some(0));
        assert_eq!(gpu.register(REG_GAS_LUT_INDEX), Some(0));
    }

    #[test]
    fn fog_lut_index_register_preserves_16_bit_value_and_data_is_24_bit() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_FOG_LUT_INDEX, 0x0080).unwrap();
        gpu.write_register(REG_FOG_LUT_DATA0, 0xFF12_3456).unwrap();

        assert_eq!(gpu.fog_lut_entry(0), Some(0x0012_3456));
        assert_eq!(gpu.register(REG_FOG_LUT_INDEX), Some(1));
    }

    #[test]
    fn fog_lut_is_cleared_on_gpu_reset() {
        let mut gpu = Pica200::new();
        gpu.write_register(REG_FOG_LUT_INDEX, 7).unwrap();
        gpu.write_register(REG_FOG_LUT_DATA0, 0x0012_3456).unwrap();
        gpu.reset();
        assert_eq!(gpu.fog_lut_entry(7), Some(0));
        assert_eq!(gpu.register(REG_FOG_LUT_INDEX), Some(0));
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
