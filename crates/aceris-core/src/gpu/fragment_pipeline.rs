//! Connects PICA200 texture sampling, the six-stage TEV combiner and fragment
//! output state.
//!
//! The PICA fragment rasterizer supplies perspective-correct interpolated
//! colors/coordinates and derivative-based LOD. This pipeline evaluates texture
//! fetch and TEV, captures linear or LUT fog state per draw, and exposes
//! alpha/depth-independent output operations for the rasterizer to apply in
//! the intended fragment-stage order.

use super::{
    FOG_LUT_ENTRY_COUNT, GpuPhysicalMemory, Pica200, PicaFragmentInputs, PicaFragmentOutputError,
    PicaFragmentOutputState, PicaOutputMapError, PicaResolvedVertex, PicaShadedDraw, PicaTevError,
    PicaTextureEnvironment, PicaTextureError, PicaTextureUnit, PicaVertexOutputMap,
    REG_TEXUNIT_CONFIG,
};

/// GPUREG_TEXENV_UPDATE_BUFFER: fog mode and depth-flip control.
pub const REG_TEXENV_UPDATE_BUFFER: u16 = 0x0E0;
/// GPUREG_FOG_COLOR: packed RGB fog color.
pub const REG_FOG_COLOR: u16 = 0x0E1;
/// GPUREG_FOG_LINEAR_SLOPE: signed 1.4.11 linear-fog slope.
pub const REG_FOG_LINEAR_SLOPE: u16 = 0x0E2;
/// GPUREG_FOG_LINEAR_OFFSET: signed 1.4.11 linear-fog offset.
pub const REG_FOG_LINEAR_OFFSET: u16 = 0x0E3;

const FOG_MODE_MASK: u32 = 0x7;
const FOG_MODE_DISABLED: u32 = 0;
const FOG_MODE_LINEAR: u32 = 1;
const FOG_MODE_LUT: u32 = 5;
const FOG_Z_FLIP: u32 = 1 << 16;

/// Captured linear-fog state.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PicaLinearFog {
    slope: f32,
    offset: f32,
    color: [f32; 3],
    z_flip: bool,
}

/// Captured fog lookup table and its fragment depth transform.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PicaFogLut {
    entries: [u32; FOG_LUT_ENTRY_COUNT],
    color: [f32; 3],
    z_flip: bool,
}

fn decode_fog_state(
    pica: &Pica200,
) -> Result<(Option<PicaLinearFog>, Option<PicaFogLut>), PicaFragmentPipelineError> {
    let update = pica.register(REG_TEXENV_UPDATE_BUFFER).unwrap_or(0);
    let packed_color = pica.register(REG_FOG_COLOR).unwrap_or(0);
    let color = [
        (packed_color & 0xFF) as f32 / 255.0,
        ((packed_color >> 8) & 0xFF) as f32 / 255.0,
        ((packed_color >> 16) & 0xFF) as f32 / 255.0,
    ];
    let z_flip = update & FOG_Z_FLIP != 0;

    match update & FOG_MODE_MASK {
        FOG_MODE_DISABLED => Ok((None, None)),
        FOG_MODE_LINEAR => {
            let decode_fixed = |value: u32| (value as u16 as i16) as f32 / 2048.0;
            Ok((
                Some(PicaLinearFog {
                    slope: decode_fixed(pica.register(REG_FOG_LINEAR_SLOPE).unwrap_or(0)),
                    offset: decode_fixed(pica.register(REG_FOG_LINEAR_OFFSET).unwrap_or(0)),
                    color,
                    z_flip,
                }),
                None,
            ))
        }
        FOG_MODE_LUT => Ok((
            None,
            Some(PicaFogLut {
                entries: std::array::from_fn(|index| pica.fog_lut_entry(index).unwrap_or(0)),
                color,
                z_flip,
            }),
        )),
        mode => Err(PicaFragmentPipelineError::UnsupportedFogMode { mode: mode as u8 }),
    }
}

impl PicaLinearFog {
    /// Applies the documented linear RGB blend. Alpha is passed through.
    ///
    /// PICA's fog factor has approximately eight bits of precision. This
    /// software path currently evaluates the formula in f32 and does not claim
    /// bit-exact hardware quantization.
    fn apply(self, mut color: [f32; 4], depth: f32) -> [f32; 4] {
        let depth = if self.z_flip { 1.0 - depth } else { depth };
        let factor = self.slope.mul_add(depth, self.offset).clamp(0.0, 1.0);
        for (channel, fog_channel) in color[..3].iter_mut().zip(self.color) {
            *channel = fog_factor_blend(*channel, fog_channel, factor);
        }
        color
    }
}

impl PicaFogLut {
    fn apply(self, mut color: [f32; 4], depth: f32) -> [f32; 4] {
        let depth = if self.z_flip { 1.0 - depth } else { depth };
        let fog_index = depth * FOG_LUT_ENTRY_COUNT as f32;
        let base = fog_index
            .floor()
            .clamp(0.0, (FOG_LUT_ENTRY_COUNT - 1) as f32);
        let fraction = fog_index - base;
        let entry = self.entries[base as usize] & 0x00FF_FFFF;
        let value = ((entry >> 13) & 0x7FF) as f32;
        let raw_difference = (entry & 0x1FFF) as i32;
        let difference = (raw_difference << 19) >> 19;
        // The table value is fixed0.0.11, so its unit denominator is 2^11.
        let factor = ((value + difference as f32 * fraction) / 2048.0).clamp(0.0, 1.0);

        for (channel, fog_channel) in color[..3].iter_mut().zip(self.color) {
            *channel = fog_factor_blend(*channel, fog_channel, factor);
        }
        color
    }
}

fn fog_factor_blend(source: f32, fog: f32, factor: f32) -> f32 {
    factor.mul_add(source, (1.0 - factor) * fog)
}

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
    /// Screen-derivative LOD per texture unit; `Some` takes precedence over
    /// `minifying` and selects mip levels through `PicaTextureUnit::sample_lod`.
    pub texture_lod: [Option<f32>; 3],
}

impl Default for PicaFragmentVaryings {
    fn default() -> Self {
        Self {
            primary_color: [1.0; 4],
            fragment_primary_color: [1.0; 4],
            fragment_secondary_color: [0.0; 4],
            texture_coordinates: [[0.0; 2]; 3],
            minifying: [false; 3],
            texture_lod: [None; 3],
        }
    }
}

/// Vertex-shader varyings before perspective-correct interpolation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaFragmentVertex {
    pub clip_position: [f32; 4],
    pub primary_color: [f32; 4],
    pub fragment_primary_color: [f32; 4],
    pub fragment_secondary_color: [f32; 4],
    pub texture_coordinates: [[f32; 2]; 3],
}

impl PicaFragmentVertex {
    /// Converts output-map data into fragment varyings with explicit defaults.
    pub fn from_resolved(
        vertex: &PicaResolvedVertex,
        default_primary_color: [f32; 4],
        default_texture_coordinates: [[f32; 2]; 3],
    ) -> Self {
        Self {
            clip_position: vertex.position,
            primary_color: vertex.color.unwrap_or(default_primary_color),
            fragment_primary_color: [1.0; 4],
            fragment_secondary_color: [0.0; 4],
            texture_coordinates: std::array::from_fn(|index| {
                vertex.texcoords[index].unwrap_or(default_texture_coordinates[index])
            }),
        }
    }
}

/// One vertex-shaded draw after semantic output-map resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct PicaFragmentDraw {
    vertices: Vec<PicaFragmentVertex>,
    source_indices: Vec<u32>,
    triangles: Vec<[usize; 3]>,
    executed_instructions: u64,
}

impl PicaFragmentDraw {
    /// Adapts a completed guest vertex-shader draw without changing its producer.
    pub fn from_shaded_draw(
        shaded: &PicaShadedDraw,
        output_map: &PicaVertexOutputMap,
        default_primary_color: [f32; 4],
        default_texture_coordinates: [[f32; 2]; 3],
    ) -> Result<Self, PicaFragmentDrawError> {
        let mut vertices = Vec::with_capacity(shaded.vertices.len());
        let mut source_indices = Vec::with_capacity(shaded.vertices.len());

        for shaded_vertex in &shaded.vertices {
            let resolved = output_map
                .resolve(&shaded_vertex.outputs)
                .map_err(PicaFragmentDrawError::OutputMap)?;
            vertices.push(PicaFragmentVertex::from_resolved(
                &resolved,
                default_primary_color,
                default_texture_coordinates,
            ));
            source_indices.push(shaded_vertex.source_index);
        }

        for (triangle_index, triangle) in shaded.triangles.iter().enumerate() {
            for &vertex_index in triangle {
                if vertex_index >= vertices.len() {
                    return Err(PicaFragmentDrawError::InvalidTriangleVertex {
                        triangle: triangle_index,
                        vertex: vertex_index,
                        vertex_count: vertices.len(),
                    });
                }
            }
        }

        Ok(Self {
            vertices,
            source_indices,
            triangles: shaded.triangles.clone(),
            executed_instructions: shaded.executed_instructions,
        })
    }

    pub fn vertices(&self) -> &[PicaFragmentVertex] {
        &self.vertices
    }

    pub fn source_indices(&self) -> &[u32] {
        &self.source_indices
    }

    pub fn triangles(&self) -> &[[usize; 3]] {
        &self.triangles
    }

    pub fn executed_instructions(&self) -> u64 {
        self.executed_instructions
    }
}

/// Failure while adapting a shaded draw to fragment-stage vertices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicaFragmentDrawError {
    OutputMap(PicaOutputMapError),
    InvalidTriangleVertex {
        triangle: usize,
        vertex: usize,
        vertex_count: usize,
    },
}

impl std::fmt::Display for PicaFragmentDrawError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutputMap(error) => write!(formatter, "PICA output map failed: {error}"),
            Self::InvalidTriangleVertex {
                triangle,
                vertex,
                vertex_count,
            } => write!(
                formatter,
                "PICA triangle {triangle} references vertex {vertex}, but draw has {vertex_count} vertices"
            ),
        }
    }
}

impl std::error::Error for PicaFragmentDrawError {}

/// Failure while interpolating vertex outputs for one covered fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PicaFragmentInterpolationError {
    InvalidBarycentricCoordinates,
    InvalidClipW { vertex: usize },
    InvalidPerspectiveDenominator,
}

impl std::fmt::Display for PicaFragmentInterpolationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidBarycentricCoordinates => {
                formatter.write_str("PICA fragment barycentric coordinates are invalid")
            }
            Self::InvalidClipW { vertex } => {
                write!(
                    formatter,
                    "PICA fragment vertex {vertex} has invalid clip-space W"
                )
            }
            Self::InvalidPerspectiveDenominator => {
                formatter.write_str("PICA fragment perspective denominator is invalid")
            }
        }
    }
}

impl std::error::Error for PicaFragmentInterpolationError {}

/// Decoded fixed-function fragment state for one PICA draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaFragmentPipeline {
    texture_environment: PicaTextureEnvironment,
    texture_units: [PicaTextureUnit; 3],
    output_state: PicaFragmentOutputState,
    linear_fog: Option<PicaLinearFog>,
    fog_lut: Option<PicaFogLut>,
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

        let (linear_fog, fog_lut) = decode_fog_state(pica)?;

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
            linear_fog,
            fog_lut,
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
            inputs.textures[index] = match varyings.texture_lod[index] {
                Some(lod) => unit.sample_lod(memory, s, t, lod),
                None => unit.sample(memory, s, t, varyings.minifying[index]),
            }
            .map_err(PicaFragmentPipelineError::Texture)?;
        }

        self.texture_environment
            .evaluate(&inputs)
            .map_err(PicaFragmentPipelineError::Tev)
    }

    /// Interpolates one covered triangle sample and evaluates textures/TEV.
    ///
    /// Barycentric coordinates are measured in screen space after clipping and
    /// viewport conversion. Varyings use reciprocal clip W; depth and coverage
    /// remain the responsibility of the rasterizer.
    pub fn shade_triangle_sample(
        &self,
        memory: &GpuPhysicalMemory,
        vertices: [PicaFragmentVertex; 3],
        barycentric: [f32; 3],
        minifying: [bool; 3],
    ) -> Result<[f32; 4], PicaFragmentSampleError> {
        let varyings = interpolate_triangle_varyings(vertices, barycentric, minifying)?;
        self.shade_fragment(memory, &varyings)
            .map_err(PicaFragmentSampleError::Pipeline)
    }

    /// Like [`Self::shade_triangle_sample`], but derives each texture unit's
    /// level of detail from screen-space derivatives.
    ///
    /// `barycentric_dx` and `barycentric_dy` are the barycentric coordinates of
    /// the pixels one step to the right and one step down. The LOD is the log2
    /// of the larger of the two coordinate steps measured in texels. A unit
    /// whose neighbouring samples cannot be interpolated falls back to the
    /// `minifying` flag. Derivative noise below `LOD_EPSILON` counts as zero.
    pub fn shade_triangle_sample_with_derivatives(
        &self,
        memory: &GpuPhysicalMemory,
        vertices: [PicaFragmentVertex; 3],
        barycentric: [f32; 3],
        barycentric_dx: [f32; 3],
        barycentric_dy: [f32; 3],
        minifying: [bool; 3],
    ) -> Result<[f32; 4], PicaFragmentSampleError> {
        let mut varyings = interpolate_triangle_varyings(vertices, barycentric, minifying)?;
        let neighbours = (
            interpolate_triangle_varyings(vertices, barycentric_dx, minifying),
            interpolate_triangle_varyings(vertices, barycentric_dy, minifying),
        );
        if let (Ok(dx), Ok(dy)) = neighbours {
            for (index, unit) in self.texture_units.iter().enumerate() {
                if !unit.enabled {
                    continue;
                }
                let coordinate = if index == 2 {
                    self.texture2_coordinate_index
                } else {
                    index
                };
                let [s, t] = varyings.texture_coordinates[coordinate];
                let step = |other: [f32; 2]| {
                    let ds = (other[0] - s) * f32::from(unit.width);
                    let dt = (other[1] - t) * f32::from(unit.height);
                    ds.hypot(dt)
                };
                let longest = step(dx.texture_coordinates[coordinate])
                    .max(step(dy.texture_coordinates[coordinate]));
                if longest.is_finite() {
                    let lod = longest.max(f32::MIN_POSITIVE).log2();
                    varyings.texture_lod[index] = Some(if lod < LOD_EPSILON { 0.0 } else { lod });
                }
            }
        }
        self.shade_fragment(memory, &varyings)
            .map_err(PicaFragmentSampleError::Pipeline)
    }

    /// Shades one triangle from an already resolved vertex-shader draw.
    pub fn shade_draw_triangle_sample(
        &self,
        memory: &GpuPhysicalMemory,
        draw: &PicaFragmentDraw,
        triangle_index: usize,
        barycentric: [f32; 3],
        minifying: [bool; 3],
    ) -> Result<[f32; 4], PicaFragmentSampleError> {
        let triangle =
            draw.triangles
                .get(triangle_index)
                .ok_or(PicaFragmentSampleError::InvalidTriangle {
                    triangle: triangle_index,
                })?;
        let vertices = [
            draw.vertices[triangle[0]],
            draw.vertices[triangle[1]],
            draw.vertices[triangle[2]],
        ];
        self.shade_triangle_sample(memory, vertices, barycentric, minifying)
    }

    /// Resolves output-map vertices directly into a perspective-correct fragment.
    ///
    /// This adapter keeps the existing vertex output-map and rasterizer modules
    /// untouched. The caller supplies screen-space barycentrics from its
    /// coverage stage and explicit defaults for unmapped color/UV semantics.
    pub fn shade_resolved_triangle_sample(
        &self,
        memory: &GpuPhysicalMemory,
        vertices: [PicaResolvedVertex; 3],
        barycentric: [f32; 3],
        default_primary_color: [f32; 4],
        default_texture_coordinates: [[f32; 2]; 3],
        minifying: [bool; 3],
    ) -> Result<[f32; 4], PicaFragmentSampleError> {
        let vertices = vertices.map(|vertex| {
            PicaFragmentVertex::from_resolved(
                &vertex,
                default_primary_color,
                default_texture_coordinates,
            )
        });
        self.shade_triangle_sample(memory, vertices, barycentric, minifying)
    }

    /// Applies the draw's captured fog state after TEV and before fragment output.
    pub(super) fn apply_fog(&self, color: [f32; 4], depth: f32) -> [f32; 4] {
        if let Some(fog) = self.linear_fog {
            fog.apply(color, depth)
        } else if let Some(fog) = self.fog_lut {
            fog.apply(color, depth)
        } else {
            color
        }
    }

    /// Returns whether this draw has alpha testing enabled.
    pub const fn alpha_test_enabled(&self) -> bool {
        self.output_state.alpha_test_enabled
    }

    /// Returns the guest's stencil-test and stencil-operation snapshot.
    pub const fn stencil_state(&self) -> super::PicaStencilState {
        self.output_state.stencil
    }

    /// Evaluates the guest-configured alpha test for a shaded fragment.
    pub fn alpha_test_passes(&self, source: [f32; 4]) -> Result<bool, PicaFragmentPipelineError> {
        self.output_state
            .alpha_test_passes(source[3])
            .map_err(PicaFragmentPipelineError::Output)
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

/// Error while interpolating or shading a fragment sample.
#[derive(Clone, Debug, PartialEq)]
pub enum PicaFragmentSampleError {
    Interpolation(PicaFragmentInterpolationError),
    Pipeline(PicaFragmentPipelineError),
    InvalidTriangle { triangle: usize },
}

impl std::fmt::Display for PicaFragmentSampleError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Interpolation(error) => {
                write!(formatter, "PICA fragment interpolation failed: {error}")
            }
            Self::Pipeline(error) => write!(formatter, "{error}"),
            Self::InvalidTriangle { triangle } => {
                write!(formatter, "PICA fragment draw has no triangle {triangle}")
            }
        }
    }
}

impl std::error::Error for PicaFragmentSampleError {}

/// Failures while preparing or evaluating the fixed-function fragment stage.
#[derive(Clone, Debug, PartialEq)]
pub enum PicaFragmentPipelineError {
    Texture(PicaTextureError),
    Tev(PicaTevError),
    Output(PicaFragmentOutputError),
    ProceduralTextureUnitUnsupported,
    UnsupportedFogMode {
        mode: u8,
    },
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
            Self::UnsupportedFogMode { mode } => write!(
                formatter,
                "PICA fog mode {mode} is not implemented (gas and reserved modes are unsupported)"
            ),
            Self::NonFiniteColor { attribute, channel } => write!(
                formatter,
                "PICA fragment {attribute} channel {channel} is not finite"
            ),
        }
    }
}

impl std::error::Error for PicaFragmentPipelineError {}

/// Derivative noise below this LOD is treated as exact 1:1 sampling.
const LOD_EPSILON: f32 = 1.0e-3;

/// Perspective-correct interpolation of one triangle sample.
fn interpolate_triangle_varyings(
    vertices: [PicaFragmentVertex; 3],
    barycentric: [f32; 3],
    minifying: [bool; 3],
) -> Result<PicaFragmentVaryings, PicaFragmentSampleError> {
    let weight_sum = barycentric.iter().sum::<f32>();
    if barycentric.iter().any(|value| !value.is_finite())
        || !weight_sum.is_finite()
        || weight_sum.abs() <= f32::EPSILON
    {
        return Err(PicaFragmentSampleError::Interpolation(
            PicaFragmentInterpolationError::InvalidBarycentricCoordinates,
        ));
    }

    let weights = barycentric.map(|value| value / weight_sum);
    let mut reciprocal_w = [0.0; 3];
    for (index, vertex) in vertices.iter().enumerate() {
        let w = vertex.clip_position[3];
        if !w.is_finite() || w <= 0.0 {
            return Err(PicaFragmentSampleError::Interpolation(
                PicaFragmentInterpolationError::InvalidClipW { vertex: index },
            ));
        }
        reciprocal_w[index] = 1.0 / w;
    }

    let denominator = (0..3)
        .map(|index| weights[index] * reciprocal_w[index])
        .sum::<f32>();
    if !denominator.is_finite() || denominator.abs() <= f32::EPSILON {
        return Err(PicaFragmentSampleError::Interpolation(
            PicaFragmentInterpolationError::InvalidPerspectiveDenominator,
        ));
    }

    let interpolate4 = |select: fn(&PicaFragmentVertex) -> [f32; 4]| {
        std::array::from_fn(|lane| {
            (0..3)
                .map(|index| weights[index] * reciprocal_w[index] * select(&vertices[index])[lane])
                .sum::<f32>()
                / denominator
        })
    };
    let interpolate2 = |coordinate: usize| {
        std::array::from_fn(|lane| {
            (0..3)
                .map(|index| {
                    weights[index]
                        * reciprocal_w[index]
                        * vertices[index].texture_coordinates[coordinate][lane]
                })
                .sum::<f32>()
                / denominator
        })
    };

    let varyings = PicaFragmentVaryings {
        primary_color: interpolate4(|vertex| vertex.primary_color),
        fragment_primary_color: interpolate4(|vertex| vertex.fragment_primary_color),
        fragment_secondary_color: interpolate4(|vertex| vertex.fragment_secondary_color),
        texture_coordinates: [interpolate2(0), interpolate2(1), interpolate2(2)],
        minifying,
        texture_lod: [None; 3],
    };
    Ok(varyings)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::PicaShadedVertex;
    use crate::gpu::physical_memory::VRAM_BASE;
    use crate::gpu::{
        PicaTevSource, REG_BLEND_FUNC, REG_COLOR_OPERATION, REG_FOG_COLOR, REG_FOG_LINEAR_OFFSET,
        REG_FOG_LINEAR_SLOPE, REG_FOG_LUT_DATA0, REG_FOG_LUT_INDEX, REG_TEXENV_UPDATE_BUFFER,
        TEXENV_STAGE_SOURCE_REGISTERS,
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
            texture.extend_from_slice(&[40, 30, 20, 10]);
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
                    255,
                    0,
                    y as u8 * 20,
                    x as u8 * 20,
                ]);
            }
        }
        memory.write(VRAM_BASE, &texture).unwrap();

        let pipeline = pica.fragment_pipeline().unwrap();
        let varyings = PicaFragmentVaryings {
            // t is inverted by the sampler (rows are stored bottom to top),
            // so 1 - t addresses the stored row used to fill the texture.
            texture_coordinates: [
                [0.5 / 8.0, 1.0 - 0.5 / 8.0],
                [1.5 / 8.0, 1.0 - 2.5 / 8.0],
                [5.5 / 8.0, 1.0 - 6.5 / 8.0],
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
        pica.write_register(REG_BLEND_FUNC, (6 << 16) | (7 << 20) | (1 << 24))
            .unwrap();
        let pipeline = pica.fragment_pipeline().unwrap();

        assert_eq!(
            pipeline.apply_fragment_output([1.0, 0.0, 0.0, 0.5], [0.0, 0.0, 1.0, 1.0],),
            Ok([128, 0, 128, 128])
        );
    }

    #[test]
    fn linear_fog_uses_signed_fixed_point_depth_blend_and_preserves_alpha() {
        let mut pica = Pica200::new();
        pica.write_register(REG_TEXENV_UPDATE_BUFFER, 1).unwrap();
        pica.write_register(REG_FOG_COLOR, 0x00FF_8040).unwrap();
        pica.write_register(REG_FOG_LINEAR_SLOPE, 2048).unwrap();
        pica.write_register(REG_FOG_LINEAR_OFFSET, 0).unwrap();

        let pipeline = pica.fragment_pipeline().unwrap();
        let color = pipeline.apply_fog([1.0, 0.5, 0.0, 0.25], 0.25);
        assert!((color[0] - (0.25 + (64.0 / 255.0) * 0.75)).abs() < 1.0e-6);
        assert!((color[1] - (0.5 * 0.25 + (128.0 / 255.0) * 0.75)).abs() < 1.0e-6);
        assert!((color[2] - 0.75).abs() < 1.0e-6);
        assert_eq!(color[3], 0.25);
    }

    #[test]
    fn linear_fog_applies_z_flip_and_clamps_its_factor() {
        let mut pica = Pica200::new();
        pica.write_register(REG_TEXENV_UPDATE_BUFFER, 1 | (1 << 16))
            .unwrap();
        pica.write_register(REG_FOG_COLOR, 0).unwrap();
        pica.write_register(REG_FOG_LINEAR_SLOPE, 2048).unwrap();
        pica.write_register(REG_FOG_LINEAR_OFFSET, 0).unwrap();

        let pipeline = pica.fragment_pipeline().unwrap();
        let flipped = pipeline.apply_fog([1.0, 0.5, 0.25, 1.0], 0.25);
        assert!((flipped[0] - 0.75).abs() < 1.0e-6);
        assert!((flipped[1] - 0.375).abs() < 1.0e-6);
        assert!((flipped[2] - 0.1875).abs() < 1.0e-6);

        pica.write_register(REG_FOG_LINEAR_OFFSET, 0xD000).unwrap();
        let below_range = pica.fragment_pipeline().unwrap().apply_fog([1.0; 4], 0.0);
        assert_eq!(below_range, [0.0, 0.0, 0.0, 1.0]);

        pica.write_register(REG_FOG_LINEAR_OFFSET, 0x4000).unwrap();
        let above_range = pica.fragment_pipeline().unwrap().apply_fog([1.0; 4], 0.0);
        assert_eq!(above_range, [1.0; 4]);
    }

    #[test]
    fn fog_lut_interpolates_the_signed_difference_and_preserves_alpha() {
        let mut pica = Pica200::new();
        pica.write_register(REG_FOG_COLOR, 0x0000_00FF).unwrap();
        pica.write_register(REG_FOG_LUT_INDEX, 0).unwrap();
        // Entry 0 has a value of 512/2048 and a +1024 interpolation delta.
        pica.write_register(REG_FOG_LUT_DATA0, (512 << 13) | 1024)
            .unwrap();
        pica.write_register(REG_FOG_LUT_DATA0 + 1, 1535 << 13)
            .unwrap();
        pica.write_register(REG_TEXENV_UPDATE_BUFFER, 5).unwrap();

        let pipeline = pica.fragment_pipeline().unwrap();
        // Depth 1/256 selects half-way between LUT entries 0 and 1.
        let color = pipeline.apply_fog([0.0, 0.0, 1.0, 0.375], 1.0 / 256.0);
        let expected_factor = 1024.0 / 2048.0;
        assert!((color[0] - (1.0 - expected_factor)).abs() < 1.0e-3);
        assert!((color[2] - expected_factor).abs() < 1.0e-3);
        assert_eq!(color[3], 0.375);
    }

    #[test]
    fn fog_lut_clamps_factor_and_respects_z_flip() {
        let mut pica = Pica200::new();
        pica.write_register(REG_FOG_COLOR, 0).unwrap();
        pica.write_register(REG_FOG_LUT_INDEX, 127).unwrap();
        // A negative table delta clamps the resulting fog factor at zero.
        pica.write_register(REG_FOG_LUT_DATA0, 0x1C00).unwrap();
        pica.write_register(REG_TEXENV_UPDATE_BUFFER, 5 | (1 << 16))
            .unwrap();
        let pipeline = pica.fragment_pipeline().unwrap();
        let color = pipeline.apply_fog([1.0; 4], 0.0);
        assert_eq!(color, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn rejects_unimplemented_gas_and_reserved_fog_modes_explicitly() {
        for mode in [2, 3, 4, 6, 7] {
            let mut pica = Pica200::new();
            pica.write_register(REG_TEXENV_UPDATE_BUFFER, mode).unwrap();
            assert_eq!(
                pica.fragment_pipeline(),
                Err(PicaFragmentPipelineError::UnsupportedFogMode { mode: mode as u8 })
            );
        }
    }

    #[test]
    fn perspective_correctly_interpolates_output_map_color_and_uvs() {
        let mut pica = Pica200::new();
        configure_tev(&mut pica, PicaTevSource::PrimaryColor);
        let pipeline = pica.fragment_pipeline().unwrap();

        let vertices = [
            PicaFragmentVertex {
                clip_position: [-1.0, -1.0, 0.0, 1.0],
                primary_color: [1.0, 0.0, 0.0, 1.0],
                fragment_primary_color: [1.0; 4],
                fragment_secondary_color: [0.0; 4],
                texture_coordinates: [[0.0, 0.0]; 3],
            },
            PicaFragmentVertex {
                clip_position: [2.0, -2.0, 0.0, 2.0],
                primary_color: [0.0, 1.0, 0.0, 1.0],
                fragment_primary_color: [1.0; 4],
                fragment_secondary_color: [0.0; 4],
                texture_coordinates: [[1.0, 0.0]; 3],
            },
            PicaFragmentVertex {
                clip_position: [0.0, 4.0, 0.0, 4.0],
                primary_color: [0.0, 0.0, 1.0, 1.0],
                fragment_primary_color: [1.0; 4],
                fragment_secondary_color: [0.0; 4],
                texture_coordinates: [[0.0, 1.0]; 3],
            },
        ];

        let output = pipeline
            .shade_triangle_sample(
                &GpuPhysicalMemory::new(),
                vertices,
                [0.25, 0.5, 0.25],
                [false; 3],
            )
            .unwrap();
        assert!((output[0] - 4.0 / 9.0).abs() < 1.0e-6);
        assert!((output[1] - 4.0 / 9.0).abs() < 1.0e-6);
        assert!((output[2] - 1.0 / 9.0).abs() < 1.0e-6);
    }

    #[test]
    fn resolves_vertex_output_map_values_before_fragment_interpolation() {
        let mut pica = Pica200::new();
        configure_tev(&mut pica, PicaTevSource::PrimaryColor);
        let pipeline = pica.fragment_pipeline().unwrap();
        let vertices = [
            PicaResolvedVertex {
                position: [-1.0, -1.0, 0.0, 1.0],
                color: Some([1.0, 0.0, 0.0, 1.0]),
                texcoords: [Some([0.0, 0.0]), None, None],
            },
            PicaResolvedVertex {
                position: [2.0, -2.0, 0.0, 2.0],
                color: Some([0.0, 1.0, 0.0, 1.0]),
                texcoords: [Some([1.0, 0.0]), None, None],
            },
            PicaResolvedVertex {
                position: [0.0, 4.0, 0.0, 4.0],
                color: Some([0.0, 0.0, 1.0, 1.0]),
                texcoords: [Some([0.0, 1.0]), None, None],
            },
        ];
        let output = pipeline
            .shade_resolved_triangle_sample(
                &GpuPhysicalMemory::new(),
                vertices,
                [0.25, 0.5, 0.25],
                [1.0; 4],
                [[0.0; 2]; 3],
                [false; 3],
            )
            .unwrap();
        assert!((output[0] - 4.0 / 9.0).abs() < 1.0e-6);
        assert!((output[1] - 4.0 / 9.0).abs() < 1.0e-6);
        assert!((output[2] - 1.0 / 9.0).abs() < 1.0e-6);
    }

    #[test]
    fn shaded_draw_output_map_flows_into_perspective_fragment_interpolation() {
        let output_map = PicaVertexOutputMap::from_registers([
            0x0302_0100,
            0x1F1F_1F1F,
            0x0B0A_0908,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
        ])
        .unwrap();
        let make_vertex = |source_index: u32, position: [f32; 4], color: [f32; 4], uv: [f32; 2]| {
            let mut outputs = [[0.0; 4]; super::super::SHADER_OUTPUT_REGISTERS];
            outputs[0] = position;
            outputs[2] = color;
            outputs[3][0] = uv[0];
            outputs[3][1] = uv[1];
            PicaShadedVertex {
                source_index,
                outputs,
            }
        };
        let shaded = PicaShadedDraw {
            vertices: vec![
                make_vertex(4, [-1.0, -1.0, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0], [0.0, 0.0]),
                make_vertex(7, [2.0, -2.0, 0.0, 2.0], [0.0, 1.0, 0.0, 1.0], [1.0, 0.0]),
                make_vertex(9, [0.0, 4.0, 0.0, 4.0], [0.0, 0.0, 1.0, 1.0], [0.0, 1.0]),
            ],
            triangles: vec![[0, 1, 2]],
            executed_instructions: 42,
        };
        let draw =
            PicaFragmentDraw::from_shaded_draw(&shaded, &output_map, [1.0; 4], [[0.0; 2]; 3])
                .unwrap();
        assert_eq!(draw.source_indices(), &[4, 7, 9]);
        assert_eq!(draw.executed_instructions(), 42);

        let mut pica = Pica200::new();
        configure_tev(&mut pica, PicaTevSource::PrimaryColor);
        let pipeline = pica.fragment_pipeline().unwrap();
        let output = pipeline
            .shade_draw_triangle_sample(
                &GpuPhysicalMemory::new(),
                &draw,
                0,
                [0.25, 0.5, 0.25],
                [false; 3],
            )
            .unwrap();
        assert!((output[0] - 4.0 / 9.0).abs() < 1.0e-6);
        assert!((output[1] - 4.0 / 9.0).abs() < 1.0e-6);
        assert!((output[2] - 1.0 / 9.0).abs() < 1.0e-6);
    }

    #[test]
    fn rejects_invalid_perspective_coordinates_before_texture_fetch() {
        let pipeline = Pica200::new().fragment_pipeline().unwrap();
        let vertex = PicaFragmentVertex {
            clip_position: [0.0, 0.0, 0.0, 0.0],
            primary_color: [1.0; 4],
            fragment_primary_color: [1.0; 4],
            fragment_secondary_color: [0.0; 4],
            texture_coordinates: [[0.0; 2]; 3],
        };
        assert_eq!(
            pipeline.shade_triangle_sample(
                &GpuPhysicalMemory::new(),
                [vertex; 3],
                [1.0, 0.0, 0.0],
                [false; 3],
            ),
            Err(PicaFragmentSampleError::Interpolation(
                PicaFragmentInterpolationError::InvalidClipW { vertex: 0 }
            ))
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
