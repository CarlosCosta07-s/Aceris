//! Independent PICA200 fragment rasterizer.
//!
//! This sidecar consumes the existing guest-authored vertex-shader draw and
//! output-map results, clips their varyings in the PICA clip volume, rasterizes
//! pixel centers with a top-left rule, shades each covered sample through the
//! texture/TEV/blend stages, and writes the final color target using the
//! existing PICA Morton render-target codec.
//!
//! It deliberately does not modify or replace the existing generic
//! GeometryPipeline or SoftwareRasterizer. Depth is currently kept in a
//! software buffer for this render invocation; the PICA depth-buffer memory
//! path, alpha test, stencil, per-channel write masks, LOD derivatives and
//! multisample coverage remain separate work.

use super::{
    DepthFunction, GpuPhysicalMemory, Matrix4, PicaDepthBufferConfig, PicaDepthBufferError,
    PicaDepthBufferFormat, PicaDepthMap, PicaDepthMode, PicaFragmentDraw, PicaFragmentDrawError,
    PicaFragmentPipeline, PicaFragmentSampleError, PicaFragmentVertex, PicaRenderTargetConfig,
    PicaRenderTargetError, PicaRenderTargetWriteReport, PicaShadedDraw, PicaVertexOutputMap,
    SoftwareRasterizer, Viewport,
};

const CLIP_W_EPSILON: f32 = 1.0e-6;
const MAX_CLIPPED_VERTICES: usize = 10;

/// PICA200 homogeneous clip planes.
///
/// PICA200 uses X/Y in [-W, W] and Z in [-W, 0]. This differs from the
/// conventional OpenGL -W..W depth range used by the generic geometry helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PicaClipPlane {
    Left,
    Right,
    Bottom,
    Top,
    Near,
    Far,
    PositiveW,
}

impl PicaClipPlane {
    const ALL: [Self; 7] = [
        Self::Left,
        Self::Right,
        Self::Bottom,
        Self::Top,
        Self::Near,
        Self::Far,
        Self::PositiveW,
    ];

    fn distance(self, position: [f32; 4]) -> f32 {
        let [x, y, z, w] = position;
        match self {
            Self::Left => x + w,
            Self::Right => w - x,
            Self::Bottom => y + w,
            Self::Top => w - y,
            Self::Near => z + w,
            Self::Far => -z,
            Self::PositiveW => w - CLIP_W_EPSILON,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ClipVertex {
    varying: PicaFragmentVertex,
}

impl ClipVertex {
    fn interpolate(self, other: Self, amount: f32) -> Self {
        let interpolate4 = |a: [f32; 4], b: [f32; 4]| {
            std::array::from_fn(|lane| a[lane] + (b[lane] - a[lane]) * amount)
        };
        let interpolate2 = |a: [f32; 2], b: [f32; 2]| {
            std::array::from_fn(|lane| a[lane] + (b[lane] - a[lane]) * amount)
        };
        Self {
            varying: PicaFragmentVertex {
                clip_position: interpolate4(
                    self.varying.clip_position,
                    other.varying.clip_position,
                ),
                primary_color: interpolate4(
                    self.varying.primary_color,
                    other.varying.primary_color,
                ),
                fragment_primary_color: interpolate4(
                    self.varying.fragment_primary_color,
                    other.varying.fragment_primary_color,
                ),
                fragment_secondary_color: interpolate4(
                    self.varying.fragment_secondary_color,
                    other.varying.fragment_secondary_color,
                ),
                texture_coordinates: std::array::from_fn(|index| {
                    interpolate2(
                        self.varying.texture_coordinates[index],
                        other.varying.texture_coordinates[index],
                    )
                }),
            },
        }
    }
}

/// Screen-space vertex retaining clip-space W for perspective interpolation.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ScreenVertex {
    varying: PicaFragmentVertex,
    x: f32,
    y: f32,
    depth: f32,
    reciprocal_w: f32,
}

/// Default attributes used when the guest vertex output map omits an output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaFragmentVertexDefaults {
    pub primary_color: [f32; 4],
    pub texture_coordinates: [[f32; 2]; 3],
}

impl Default for PicaFragmentVertexDefaults {
    fn default() -> Self {
        Self {
            primary_color: [1.0; 4],
            texture_coordinates: [[0.0; 2]; 3],
        }
    }
}

/// Configuration for one independent PICA fragment-rasterizer invocation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PicaFragmentRasterState {
    /// Additional transform applied to vertex-shader clip positions.
    /// Use identity when the guest vertex shader already outputs final clip
    /// coordinates.
    pub transform: Matrix4,
    pub viewport: Viewport,
    pub depth_function: DepthFunction,
    /// Independent R, G, B and alpha write enables after fragment output.
    pub color_write_mask: [bool; 4],
    /// Depth writes require the guest's depth/color mask and depth-write access.
    pub depth_write_enabled: bool,
    /// Whether guest permission allows physical stencil updates.
    pub stencil_write_enabled: bool,
    /// Precision used by the physical depth target, if one is configured.
    pub depth_format: Option<PicaDepthBufferFormat>,
    /// Guest PICA depth-map scale/offset and Z/W-buffer selection.
    pub depth_map: Option<PicaDepthMap>,
    /// LOD classification supplied by the caller until screen derivatives are
    /// calculated by the rasterizer itself.
    pub minifying: [bool; 3],
}

impl PicaFragmentRasterState {
    pub fn new(transform: Matrix4, viewport: Viewport) -> Self {
        Self {
            transform,
            viewport,
            depth_function: DepthFunction::Less,
            color_write_mask: [true; 4],
            depth_write_enabled: true,
            stencil_write_enabled: false,
            depth_format: None,
            depth_map: None,
            minifying: [false; 3],
        }
    }
}

/// Summary of the independent PICA fragment rasterization path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PicaFragmentRasterReport {
    pub input_triangles: u64,
    pub clipped_triangles: u64,
    pub covered_pixels: u64,
    pub alpha_rejected_pixels: u64,
    pub depth_rejected_pixels: u64,
    pub stencil_rejected_pixels: u64,
    pub shaded_pixels: u64,
    pub executed_instructions: u64,
}

/// Independent RGBA8/depth software buffers for PICA fragment shading.
#[derive(Clone, Debug)]
pub struct PicaFragmentRasterizer {
    width: usize,
    height: usize,
    color: Vec<u8>,
    depth: Vec<f32>,
    stencil: Vec<u8>,
    stencil_dirty: bool,
}

impl PicaFragmentRasterizer {
    /// Starts from the guest render target's current color contents.
    ///
    /// This preserves destination pixels for blend and logic operations.
    pub fn from_render_target(
        target: PicaRenderTargetConfig,
        memory: &GpuPhysicalMemory,
        clear_depth: f32,
    ) -> Result<Self, PicaFragmentRasterError> {
        if !clear_depth.is_finite() || !(0.0..=1.0).contains(&clear_depth) {
            return Err(PicaFragmentRasterError::InvalidClearDepth { depth: clear_depth });
        }
        let width = usize::try_from(target.width)
            .map_err(|_| PicaFragmentRasterError::InvalidDimensions)?;
        let height = usize::try_from(target.height)
            .map_err(|_| PicaFragmentRasterError::InvalidDimensions)?;
        let pixel_count = width
            .checked_mul(height)
            .ok_or(PicaFragmentRasterError::InvalidDimensions)?;
        let color_len = pixel_count
            .checked_mul(4)
            .ok_or(PicaFragmentRasterError::InvalidDimensions)?;
        let color = target
            .read_rgba8(memory)
            .map_err(PicaFragmentRasterError::RenderTarget)?;
        if color.len() != color_len {
            return Err(PicaFragmentRasterError::InvalidDimensions);
        }
        let mut depth = Vec::new();
        depth
            .try_reserve_exact(pixel_count)
            .map_err(|_| PicaFragmentRasterError::AllocationFailed)?;
        depth.resize(pixel_count, clear_depth);
        let mut stencil = Vec::new();
        stencil
            .try_reserve_exact(pixel_count)
            .map_err(|_| PicaFragmentRasterError::AllocationFailed)?;
        stencil.resize(pixel_count, 0);

        Ok(Self {
            width,
            height,
            color,
            depth,
            stencil,
            stencil_dirty: false,
        })
    }

    /// Loads guest depth values from a separately addressed PICA depth surface.
    ///
    /// The viewport/rasterizer uses top-left screen coordinates; the depth
    /// plane follows the same logical row order as the PICA color target.
    pub fn load_depth_buffer(
        &mut self,
        config: PicaDepthBufferConfig,
        memory: &GpuPhysicalMemory,
        clear_depth: f32,
    ) -> Result<(), PicaFragmentRasterError> {
        if usize::try_from(config.width).ok() != Some(self.width)
            || usize::try_from(config.height).ok() != Some(self.height)
        {
            return Err(PicaFragmentRasterError::InvalidDimensions);
        }
        self.depth = config
            .read_depth_values(memory, clear_depth)
            .map_err(PicaFragmentRasterError::DepthBuffer)?;
        let stencil_count = self.width * self.height;
        self.stencil = config
            .read_stencil_values(memory)
            .map_err(PicaFragmentRasterError::DepthBuffer)?
            .unwrap_or_else(|| vec![0; stencil_count]);
        self.stencil_dirty = false;
        Ok(())
    }

    /// Publishes the current depth plane into the guest's physical depth target.
    pub fn write_depth_buffer(
        &mut self,
        config: PicaDepthBufferConfig,
        memory: &mut GpuPhysicalMemory,
    ) -> Result<u64, PicaFragmentRasterError> {
        if usize::try_from(config.width).ok() != Some(self.width)
            || usize::try_from(config.height).ok() != Some(self.height)
        {
            return Err(PicaFragmentRasterError::InvalidDimensions);
        }
        let depth_bytes = config
            .write_depth_values(&self.depth, memory)
            .map_err(PicaFragmentRasterError::DepthBuffer)?;
        let stencil_bytes = if config.format == PicaDepthBufferFormat::D24S8 && self.stencil_dirty {
            let bytes = config
                .write_stencil_values(&self.stencil, memory)
                .map_err(PicaFragmentRasterError::DepthBuffer)?;
            self.stencil_dirty = false;
            bytes
        } else {
            0
        };
        Ok(depth_bytes.saturating_add(stencil_bytes))
    }

    pub fn stencil_buffer(&self) -> &[u8] {
        &self.stencil
    }

    pub const fn width(&self) -> usize {
        self.width
    }

    pub const fn height(&self) -> usize {
        self.height
    }

    pub fn color_buffer(&self) -> &[u8] {
        &self.color
    }

    pub fn depth_buffer(&self) -> &[f32] {
        &self.depth
    }

    pub fn color_at(&self, x: usize, y: usize) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let offset = (y * self.width + x) * 4;
        Some([
            self.color[offset],
            self.color[offset + 1],
            self.color[offset + 2],
            self.color[offset + 3],
        ])
    }

    pub fn clear(&mut self, color: [u8; 4], depth: f32) -> Result<(), PicaFragmentRasterError> {
        if !depth.is_finite() || !(0.0..=1.0).contains(&depth) {
            return Err(PicaFragmentRasterError::InvalidClearDepth { depth });
        }
        for pixel in self.color.chunks_exact_mut(4) {
            pixel.copy_from_slice(&color);
        }
        self.depth.fill(depth);
        self.stencil.fill(0);
        self.stencil_dirty = true;
        Ok(())
    }

    /// Rasterizes all triangles produced by the existing vertex-shader draw.
    ///
    /// The input draw is already output-map-resolved. Each triangle is clipped
    /// with all interpolants attached, then each covered pixel is passed through
    /// texture fetch, the six TEV combiners and the configured blend/logic op.
    pub fn draw(
        &mut self,
        draw: &PicaFragmentDraw,
        pipeline: &PicaFragmentPipeline,
        memory: &GpuPhysicalMemory,
        state: PicaFragmentRasterState,
    ) -> Result<PicaFragmentRasterReport, PicaFragmentRasterError> {
        validate_state(state)?;
        let mut report = PicaFragmentRasterReport::default();

        for (triangle_index, triangle) in draw.triangles().iter().enumerate() {
            report.input_triangles = report.input_triangles.saturating_add(1);
            let vertices = draw.vertices();
            let indices = *triangle;
            let mut clip_vertices = [
                ClipVertex {
                    varying: *vertices.get(indices[0]).ok_or(
                        PicaFragmentRasterError::InvalidTriangleVertex {
                            triangle: triangle_index,
                            vertex: indices[0],
                            vertex_count: vertices.len(),
                        },
                    )?,
                },
                ClipVertex {
                    varying: *vertices.get(indices[1]).ok_or(
                        PicaFragmentRasterError::InvalidTriangleVertex {
                            triangle: triangle_index,
                            vertex: indices[1],
                            vertex_count: vertices.len(),
                        },
                    )?,
                },
                ClipVertex {
                    varying: *vertices.get(indices[2]).ok_or(
                        PicaFragmentRasterError::InvalidTriangleVertex {
                            triangle: triangle_index,
                            vertex: indices[2],
                            vertex_count: vertices.len(),
                        },
                    )?,
                },
            ];

            for (vertex_index, vertex) in clip_vertices.iter_mut().enumerate() {
                vertex.varying.clip_position =
                    transform_position(state.transform, vertex.varying.clip_position);
                validate_clip_vertex(vertex.varying, vertex_index)?;
            }

            let mut polygon = clip_vertices.to_vec();
            for plane in PicaClipPlane::ALL {
                polygon = clip_polygon(polygon, plane)?;
                if polygon.len() < 3 {
                    break;
                }
            }
            if polygon.len() < 3 {
                continue;
            }

            let first = polygon[0];
            for index in 1..polygon.len() - 1 {
                let triangle = [first, polygon[index], polygon[index + 1]];
                let screen = triangle
                    .map(|vertex| to_screen(vertex, state.viewport, state.depth_map))
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()?;
                report.clipped_triangles = report.clipped_triangles.saturating_add(1);
                self.rasterize_triangle(
                    [screen[0], screen[1], screen[2]],
                    pipeline,
                    memory,
                    state,
                    &mut report,
                )?;
            }
        }

        Ok(report)
    }

    /// Resolves a completed guest vertex-shader draw and rasterizes it.
    ///
    /// This convenience path consumes the existing DRAWARRAYS/DRAWELEMENTS
    /// output without changing its vertex-fetch or shader-execution modules.
    pub fn draw_shaded(
        &mut self,
        shaded: &PicaShadedDraw,
        output_map: &PicaVertexOutputMap,
        defaults: PicaFragmentVertexDefaults,
        pipeline: &PicaFragmentPipeline,
        memory: &GpuPhysicalMemory,
        state: PicaFragmentRasterState,
    ) -> Result<PicaFragmentRasterReport, PicaFragmentRasterError> {
        let draw = PicaFragmentDraw::from_shaded_draw(
            shaded,
            output_map,
            defaults.primary_color,
            defaults.texture_coordinates,
        )
        .map_err(PicaFragmentRasterError::Draw)?;
        let mut report = self.draw(&draw, pipeline, memory, state)?;
        report.executed_instructions = shaded.executed_instructions;
        Ok(report)
    }

    /// Encodes and writes the final color buffer through the existing PICA
    /// render-target Morton codec. Depth remains in this software instance.
    pub fn write_render_target(
        &self,
        target: PicaRenderTargetConfig,
        memory: &mut GpuPhysicalMemory,
    ) -> Result<PicaRenderTargetWriteReport, PicaFragmentRasterError> {
        if usize::try_from(target.width).ok() != Some(self.width)
            || usize::try_from(target.height).ok() != Some(self.height)
        {
            return Err(PicaFragmentRasterError::InvalidDimensions);
        }
        let mut rasterizer = SoftwareRasterizer::new(self.width, self.height)
            .map_err(PicaFragmentRasterError::Rasterizer)?;
        rasterizer
            .load_color_rgba8(&self.color)
            .map_err(PicaFragmentRasterError::Rasterizer)?;
        target
            .write_rasterizer(&rasterizer, memory)
            .map_err(PicaFragmentRasterError::RenderTarget)
    }

    fn rasterize_triangle(
        &mut self,
        mut vertices: [ScreenVertex; 3],
        pipeline: &PicaFragmentPipeline,
        memory: &GpuPhysicalMemory,
        state: PicaFragmentRasterState,
        report: &mut PicaFragmentRasterReport,
    ) -> Result<(), PicaFragmentRasterError> {
        let mut area = orient(vertices[0], vertices[1], vertices[2]);
        if !area.is_finite() {
            return Err(PicaFragmentRasterError::InvalidScreenTriangle);
        }
        if area == 0.0 {
            return Ok(());
        }
        if area < 0.0 {
            vertices.swap(1, 2);
            area = -area;
        }

        let min_x = vertices
            .iter()
            .map(|vertex| f64::from(vertex.x))
            .fold(f64::INFINITY, f64::min);
        let max_x = vertices
            .iter()
            .map(|vertex| f64::from(vertex.x))
            .fold(f64::NEG_INFINITY, f64::max);
        let min_y = vertices
            .iter()
            .map(|vertex| f64::from(vertex.y))
            .fold(f64::INFINITY, f64::min);
        let max_y = vertices
            .iter()
            .map(|vertex| f64::from(vertex.y))
            .fold(f64::NEG_INFINITY, f64::max);

        let first_x = (min_x - 0.5).ceil();
        let final_x = (max_x - 0.5).floor();
        let first_y = (min_y - 0.5).ceil();
        let final_y = (max_y - 0.5).floor();
        if final_x < 0.0
            || final_y < 0.0
            || first_x > self.width as f64 - 1.0
            || first_y > self.height as f64 - 1.0
        {
            return Ok(());
        }

        let x_start = first_x.max(0.0) as usize;
        let x_end = final_x.min(self.width as f64 - 1.0) as usize;
        let y_start = first_y.max(0.0) as usize;
        let y_end = final_y.min(self.height as f64 - 1.0) as usize;
        if x_start > x_end || y_start > y_end {
            return Ok(());
        }

        let edge_top_left = [
            is_top_left(vertices[1], vertices[2]),
            is_top_left(vertices[2], vertices[0]),
            is_top_left(vertices[0], vertices[1]),
        ];

        for y in y_start..=y_end {
            for x in x_start..=x_end {
                let point = ScreenVertex {
                    varying: PicaFragmentVertex {
                        clip_position: [0.0; 4],
                        primary_color: [0.0; 4],
                        fragment_primary_color: [0.0; 4],
                        fragment_secondary_color: [0.0; 4],
                        texture_coordinates: [[0.0; 2]; 3],
                    },
                    x: x as f32 + 0.5,
                    y: y as f32 + 0.5,
                    depth: 0.0,
                    reciprocal_w: 1.0,
                };
                let edges = [
                    orient(vertices[1], vertices[2], point),
                    orient(vertices[2], vertices[0], point),
                    orient(vertices[0], vertices[1], point),
                ];
                if (0..3).any(|index| !inside_edge(edges[index], edge_top_left[index])) {
                    continue;
                }

                report.covered_pixels = report.covered_pixels.saturating_add(1);
                let weights = edges.map(|edge| edge / area);
                let interpolated_depth = (0..3)
                    .map(|index| weights[index] * f64::from(vertices[index].depth))
                    .sum::<f64>() as f32;
                let fragment_depth = match state.depth_map {
                    Some(map) => {
                        let scaled = interpolated_depth.mul_add(map.scale, map.offset);
                        match map.mode {
                            PicaDepthMode::ZBuffer => scaled,
                            PicaDepthMode::WBuffer => {
                                let reciprocal_w = (0..3)
                                    .map(|index| {
                                        weights[index] * f64::from(vertices[index].reciprocal_w)
                                    })
                                    .sum::<f64>()
                                    as f32;
                                scaled / reciprocal_w
                            }
                        }
                        .clamp(0.0, 1.0)
                    }
                    None => interpolated_depth.clamp(0.0, 1.0),
                };
                let fragment_depth = state
                    .depth_format
                    .map(|format| format.quantize_depth(fragment_depth))
                    .unwrap_or(fragment_depth);

                let pixel_index = y * self.width + x;
                let varyings = [
                    vertices[0].varying,
                    vertices[1].varying,
                    vertices[2].varying,
                ];
                let shade_fragment = || {
                    pipeline
                        .shade_triangle_sample(
                            memory,
                            varyings,
                            weights.map(|weight| weight as f32),
                            state.minifying,
                        )
                        .map_err(PicaFragmentRasterError::Fragment)
                };

                // PICA alpha testing is evaluated after TEV and before depth.
                let source_before_depth = if pipeline.alpha_test_enabled() {
                    let source = shade_fragment()?;
                    if !pipeline
                        .alpha_test_passes(source)
                        .map_err(PicaFragmentRasterError::FragmentOutput)?
                    {
                        report.alpha_rejected_pixels =
                            report.alpha_rejected_pixels.saturating_add(1);
                        continue;
                    }
                    Some(source)
                } else {
                    None
                };

                let stencil_state = pipeline.stencil_state();
                let stored_stencil = self.stencil[pixel_index];
                if stencil_state.enabled && !stencil_state.test_passes(stored_stencil) {
                    if state.stencil_write_enabled {
                        let updated =
                            stencil_state.apply(stencil_state.stencil_fail, stored_stencil);
                        if updated != stored_stencil {
                            self.stencil[pixel_index] = updated;
                            self.stencil_dirty = true;
                        }
                    }
                    report.stencil_rejected_pixels =
                        report.stencil_rejected_pixels.saturating_add(1);
                    continue;
                }

                if !depth_passes(
                    state.depth_function,
                    fragment_depth,
                    self.depth[pixel_index],
                ) {
                    if stencil_state.enabled && state.stencil_write_enabled {
                        let updated = stencil_state.apply(stencil_state.depth_fail, stored_stencil);
                        if updated != stored_stencil {
                            self.stencil[pixel_index] = updated;
                            self.stencil_dirty = true;
                        }
                    }
                    report.depth_rejected_pixels = report.depth_rejected_pixels.saturating_add(1);
                    continue;
                }

                let source = match source_before_depth {
                    Some(source) => source,
                    None => shade_fragment()?,
                };
                let color_offset = pixel_index * 4;
                let destination = [
                    f32::from(self.color[color_offset]) / 255.0,
                    f32::from(self.color[color_offset + 1]) / 255.0,
                    f32::from(self.color[color_offset + 2]) / 255.0,
                    f32::from(self.color[color_offset + 3]) / 255.0,
                ];
                let output = pipeline
                    .apply_fragment_output(source, destination)
                    .map_err(PicaFragmentRasterError::FragmentOutput)?;
                if stencil_state.enabled && state.stencil_write_enabled {
                    let updated = stencil_state.apply(stencil_state.depth_pass, stored_stencil);
                    if updated != stored_stencil {
                        self.stencil[pixel_index] = updated;
                        self.stencil_dirty = true;
                    }
                }
                for (channel, enabled) in state.color_write_mask.iter().copied().enumerate() {
                    if enabled {
                        self.color[color_offset + channel] = output[channel];
                    }
                }
                if state.depth_write_enabled {
                    self.depth[pixel_index] = fragment_depth;
                }
                report.shaded_pixels = report.shaded_pixels.saturating_add(1);
            }
        }

        Ok(())
    }
}

fn validate_state(state: PicaFragmentRasterState) -> Result<(), PicaFragmentRasterError> {
    let elements = state.transform.elements();
    if elements.iter().any(|value| !value.is_finite()) {
        return Err(PicaFragmentRasterError::InvalidTransform);
    }
    let viewport = state.viewport;
    if !viewport.x.is_finite()
        || !viewport.y.is_finite()
        || !viewport.width.is_finite()
        || !viewport.height.is_finite()
        || viewport.width <= 0.0
        || viewport.height <= 0.0
        || !(viewport.x + viewport.width).is_finite()
        || !(viewport.y + viewport.height).is_finite()
    {
        return Err(PicaFragmentRasterError::InvalidViewport);
    }
    if let Some(map) = state.depth_map {
        if !map.scale.is_finite() || !map.offset.is_finite() {
            return Err(PicaFragmentRasterError::InvalidDepthMap);
        }
    }
    Ok(())
}

fn transform_position(transform: Matrix4, position: [f32; 4]) -> [f32; 4] {
    let matrix = transform.elements();
    std::array::from_fn(|row| {
        matrix[row] * position[0]
            + matrix[4 + row] * position[1]
            + matrix[8 + row] * position[2]
            + matrix[12 + row] * position[3]
    })
}

fn validate_clip_vertex(
    vertex: PicaFragmentVertex,
    index: usize,
) -> Result<(), PicaFragmentRasterError> {
    if vertex
        .clip_position
        .iter()
        .chain(vertex.primary_color.iter())
        .chain(vertex.fragment_primary_color.iter())
        .chain(vertex.fragment_secondary_color.iter())
        .chain(
            vertex
                .texture_coordinates
                .iter()
                .flat_map(|coordinate| coordinate.iter()),
        )
        .any(|value| !value.is_finite())
    {
        return Err(PicaFragmentRasterError::NonFiniteVertex { index });
    }
    Ok(())
}

fn clip_polygon(
    polygon: Vec<ClipVertex>,
    plane: PicaClipPlane,
) -> Result<Vec<ClipVertex>, PicaFragmentRasterError> {
    if polygon.is_empty() {
        return Ok(polygon);
    }

    let mut output = Vec::with_capacity(MAX_CLIPPED_VERTICES);
    let mut previous = *polygon.last().expect("non-empty polygon checked");
    let mut previous_distance = plane.distance(previous.varying.clip_position);
    let mut previous_inside = previous_distance >= 0.0;

    for current in polygon {
        let current_distance = plane.distance(current.varying.clip_position);
        let current_inside = current_distance >= 0.0;

        if current_inside != previous_inside {
            let denominator = previous_distance - current_distance;
            if denominator == 0.0 || !denominator.is_finite() {
                return Err(PicaFragmentRasterError::InvalidClipIntersection);
            }
            let amount = previous_distance / denominator;
            if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
                return Err(PicaFragmentRasterError::InvalidClipIntersection);
            }
            output.push(previous.interpolate(current, amount));
        }
        if current_inside {
            output.push(current);
        }
        if output.len() > MAX_CLIPPED_VERTICES {
            return Err(PicaFragmentRasterError::ClippedPolygonOverflow);
        }

        previous = current;
        previous_distance = current_distance;
        previous_inside = current_inside;
    }

    Ok(output)
}

fn to_screen(
    vertex: ClipVertex,
    viewport: Viewport,
    depth_map: Option<PicaDepthMap>,
) -> Result<ScreenVertex, PicaFragmentRasterError> {
    let [clip_x, clip_y, clip_z, clip_w] = vertex.varying.clip_position;
    if !clip_w.is_finite() || clip_w < CLIP_W_EPSILON {
        return Err(PicaFragmentRasterError::InvalidPerspectiveDivide);
    }
    let reciprocal_w = clip_w.recip();
    let ndc_x = clip_x * reciprocal_w;
    let ndc_y = clip_y * reciprocal_w;
    let ndc_z = clip_z * reciprocal_w;
    let x = viewport.x + (ndc_x + 1.0) * (viewport.width * 0.5);
    let y = viewport.y + (1.0 - ndc_y) * (viewport.height * 0.5);
    if !reciprocal_w.is_finite() || !ndc_x.is_finite() || !ndc_y.is_finite() || !ndc_z.is_finite() {
        return Err(PicaFragmentRasterError::InvalidPerspectiveDivide);
    }

    // The native PICA clip range is [-W, 0]. Without an explicit guest depth
    // map, map NDC Z [-1, 0] to normalized depth [0, 1].
    let depth = if depth_map.is_some() {
        ndc_z
    } else {
        (ndc_z + 1.0).clamp(0.0, 1.0)
    };
    if !x.is_finite() || !y.is_finite() || !depth.is_finite() {
        return Err(PicaFragmentRasterError::InvalidPerspectiveDivide);
    }
    Ok(ScreenVertex {
        varying: vertex.varying,
        x,
        y,
        depth,
        reciprocal_w,
    })
}

fn orient(a: ScreenVertex, b: ScreenVertex, point: ScreenVertex) -> f64 {
    let ax = f64::from(a.x);
    let ay = f64::from(a.y);
    let bx = f64::from(b.x);
    let by = f64::from(b.y);
    let px = f64::from(point.x);
    let py = f64::from(point.y);
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

fn is_top_left(a: ScreenVertex, b: ScreenVertex) -> bool {
    let dx = f64::from(b.x) - f64::from(a.x);
    let dy = f64::from(b.y) - f64::from(a.y);
    dy < 0.0 || (dy == 0.0 && dx > 0.0)
}

fn inside_edge(value: f64, top_left: bool) -> bool {
    value > 0.0 || (value == 0.0 && top_left)
}

fn depth_passes(function: DepthFunction, source: f32, destination: f32) -> bool {
    match function {
        DepthFunction::Never => false,
        DepthFunction::Less => source < destination,
        DepthFunction::LessEqual => source <= destination,
        DepthFunction::Always => true,
        DepthFunction::Equal => source == destination,
        DepthFunction::NotEqual => source != destination,
        DepthFunction::Greater => source > destination,
        DepthFunction::GreaterEqual => source >= destination,
    }
}

/// Errors from PICA-specific clipping, shading or render-target presentation.
#[derive(Clone, Debug, PartialEq)]
pub enum PicaFragmentRasterError {
    InvalidDimensions,
    AllocationFailed,
    InvalidClearDepth {
        depth: f32,
    },
    InvalidTransform,
    InvalidViewport,
    InvalidDepthMap,
    NonFiniteVertex {
        index: usize,
    },
    InvalidTriangleVertex {
        triangle: usize,
        vertex: usize,
        vertex_count: usize,
    },
    InvalidClipIntersection,
    ClippedPolygonOverflow,
    InvalidPerspectiveDivide,
    InvalidScreenTriangle,
    Draw(PicaFragmentDrawError),
    Fragment(PicaFragmentSampleError),
    FragmentOutput(super::PicaFragmentPipelineError),
    DepthBuffer(PicaDepthBufferError),
    Rasterizer(super::RasterizerError),
    RenderTarget(PicaRenderTargetError),
}

impl std::fmt::Display for PicaFragmentRasterError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDimensions => {
                formatter.write_str("PICA fragment target dimensions are invalid")
            }
            Self::AllocationFailed => {
                formatter.write_str("could not allocate PICA fragment buffers")
            }
            Self::InvalidClearDepth { depth } => {
                write!(formatter, "PICA fragment clear depth {depth} is invalid")
            }
            Self::InvalidTransform => formatter.write_str("PICA fragment transform is not finite"),
            Self::InvalidViewport => formatter.write_str("PICA fragment viewport is invalid"),
            Self::InvalidDepthMap => formatter.write_str("PICA fragment depth map is invalid"),
            Self::NonFiniteVertex { index } => write!(
                formatter,
                "PICA fragment vertex {index} contains a non-finite value"
            ),
            Self::InvalidTriangleVertex {
                triangle,
                vertex,
                vertex_count,
            } => write!(
                formatter,
                "PICA fragment triangle {triangle} references vertex {vertex}, but draw has {vertex_count} vertices"
            ),
            Self::InvalidClipIntersection => {
                formatter.write_str("PICA fragment clipping intersection is invalid")
            }
            Self::ClippedPolygonOverflow => {
                formatter.write_str("PICA fragment clipped polygon exceeded its vertex bound")
            }
            Self::InvalidPerspectiveDivide => {
                formatter.write_str("PICA fragment perspective divide is invalid")
            }
            Self::InvalidScreenTriangle => {
                formatter.write_str("PICA fragment screen-space triangle is invalid")
            }
            Self::Draw(error) => write!(formatter, "PICA shaded draw adaptation failed: {error}"),
            Self::Fragment(error) => write!(formatter, "PICA fragment shading failed: {error}"),
            Self::FragmentOutput(error) => {
                write!(formatter, "PICA fragment output failed: {error}")
            }
            Self::DepthBuffer(error) => {
                write!(formatter, "PICA physical depth buffer failed: {error}")
            }
            Self::Rasterizer(error) => {
                write!(formatter, "PICA fragment output rasterizer failed: {error}")
            }
            Self::RenderTarget(error) => {
                write!(formatter, "PICA fragment render target failed: {error}")
            }
        }
    }
}

impl std::error::Error for PicaFragmentRasterError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::physical_memory::VRAM_BASE;
    use crate::gpu::{
        PicaFragmentDraw, PicaFragmentVertex, PicaShadedDraw, PicaShadedVertex,
        PicaVertexOutputMap, REG_FRAGOP_ALPHA_TEST, REG_LOGIC_OP, REG_STENCIL_OP, REG_STENCIL_TEST,
        REG_TEXUNIT_CONFIG, TEXENV_STAGE_SOURCE_REGISTERS,
    };

    fn fragment_vertex(position: [f32; 4], color: [f32; 4]) -> PicaFragmentVertex {
        PicaFragmentVertex {
            clip_position: position,
            primary_color: color,
            fragment_primary_color: [1.0; 4],
            fragment_secondary_color: [0.0; 4],
            texture_coordinates: [[0.0; 2]; 3],
        }
    }

    fn draw(vertices: [PicaFragmentVertex; 3]) -> PicaFragmentDraw {
        let output_map = PicaVertexOutputMap::from_registers([
            0x0302_0100,
            0x1F1F_1F1F,
            0x0B0A_0908,
            0x1F1F_0D0C,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
        ])
        .unwrap();
        let shaded = PicaShadedDraw {
            vertices: vertices
                .into_iter()
                .enumerate()
                .map(|(index, vertex)| {
                    let mut outputs = [[0.0; 4]; super::super::SHADER_OUTPUT_REGISTERS];
                    outputs[0] = vertex.clip_position;
                    outputs[2] = vertex.primary_color;
                    outputs[3][0] = vertex.texture_coordinates[0][0];
                    outputs[3][1] = vertex.texture_coordinates[0][1];
                    PicaShadedVertex {
                        source_index: index as u32,
                        outputs,
                    }
                })
                .collect(),
            triangles: vec![[0, 1, 2]],
            executed_instructions: 0,
        };
        PicaFragmentDraw::from_shaded_draw(&shaded, &output_map, [1.0; 4], [[0.0; 2]; 3]).unwrap()
    }

    fn configure_tev(pica: &mut super::super::Pica200) {
        for (stage, &base) in TEXENV_STAGE_SOURCE_REGISTERS.iter().enumerate() {
            let source = if stage == 0 { 0 } else { 15 };
            pica.write_register(base, source | (source << 16)).unwrap();
            pica.write_register(base + 1, 0).unwrap();
            pica.write_register(base + 2, 0).unwrap();
            pica.write_register(base + 3, 0).unwrap();
            pica.write_register(base + 4, 0).unwrap();
        }
        pica.write_register(REG_TEXUNIT_CONFIG, 0).unwrap();
        pica.write_register(REG_LOGIC_OP, 3).unwrap();
    }

    #[test]
    fn pica_clip_volume_rejects_vertices_past_zero_z_and_keeps_negative_z() {
        let mut positive = fragment_vertex([0.0, 0.0, 0.1, 1.0], [1.0; 4]);
        let negative = fragment_vertex([0.0, 0.0, -0.5, 1.0], [0.5; 4]);
        assert!(PicaClipPlane::Far.distance(positive.clip_position) < 0.0);
        assert!(PicaClipPlane::Near.distance(negative.clip_position) >= 0.0);
        positive.clip_position[2] = -1.1;
        assert!(PicaClipPlane::Near.distance(positive.clip_position) < 0.0);
    }

    #[test]
    fn writes_shaded_triangle_through_pica_render_target_codec() {
        let mut pica = super::super::Pica200::new();
        configure_tev(&mut pica);
        let pipeline = pica.fragment_pipeline().unwrap();
        let target =
            PicaRenderTargetConfig::from_registers(VRAM_BASE >> 3, 2, 0, 8 | (7 << 12) | (1 << 24))
                .unwrap();
        let mut memory = GpuPhysicalMemory::new();
        let mut framebuffer = vec![0u8; 8 * 8 * 4];
        for alpha in framebuffer.iter_mut().skip(3).step_by(4) {
            *alpha = 255;
        }
        // Initialize the tiled target via its own existing codec.
        let mut initial = SoftwareRasterizer::new(8, 8).unwrap();
        initial.load_color_rgba8(&framebuffer).unwrap();
        target.write_rasterizer(&initial, &mut memory).unwrap();

        let mut rasterizer =
            PicaFragmentRasterizer::from_render_target(target, &memory, 1.0).unwrap();
        let draw = draw([
            fragment_vertex([-1.0, -1.0, -0.5, 1.0], [1.0, 0.0, 0.0, 1.0]),
            fragment_vertex([1.0, -1.0, -0.5, 1.0], [0.0, 1.0, 0.0, 1.0]),
            fragment_vertex([-1.0, 1.0, -0.5, 1.0], [0.0, 0.0, 1.0, 1.0]),
        ]);
        let state = PicaFragmentRasterState::new(
            Matrix4::identity(),
            Viewport::new(0.0, 0.0, 8.0, 8.0).unwrap(),
        );
        let report = rasterizer.draw(&draw, &pipeline, &memory, state).unwrap();
        assert_eq!(report.input_triangles, 1);
        assert_eq!(report.shaded_pixels, 28);
        assert_eq!(rasterizer.color_at(1, 6), Some([159, 48, 48, 255]));

        rasterizer.write_render_target(target, &mut memory).unwrap();
        let decoded = target.read_rgba8(&memory).unwrap();
        let offset = (6 * 8 + 1) * 4;
        assert_eq!(&decoded[offset..offset + 4], &[159, 48, 48, 255]);
    }

    #[test]
    fn alpha_test_discards_fragments_before_depth_and_color_writes() {
        let mut pica = super::super::Pica200::new();
        configure_tev(&mut pica);
        // Enable alpha test with NEVER: every covered fragment is discarded.
        pica.write_register(REG_FRAGOP_ALPHA_TEST, 1).unwrap();
        let pipeline = pica.fragment_pipeline().unwrap();
        let target =
            PicaRenderTargetConfig::from_registers(VRAM_BASE >> 3, 2, 0, 8 | (7 << 12) | (1 << 24))
                .unwrap();

        let mut memory = GpuPhysicalMemory::new();
        let mut framebuffer = vec![0u8; 8 * 8 * 4];
        for alpha in framebuffer.iter_mut().skip(3).step_by(4) {
            *alpha = 255;
        }
        let mut initial = SoftwareRasterizer::new(8, 8).unwrap();
        initial.load_color_rgba8(&framebuffer).unwrap();
        target.write_rasterizer(&initial, &mut memory).unwrap();

        let mut rasterizer =
            PicaFragmentRasterizer::from_render_target(target, &memory, 1.0).unwrap();
        let draw = draw([
            fragment_vertex([-1.0, -1.0, -0.5, 1.0], [1.0, 0.0, 0.0, 1.0]),
            fragment_vertex([1.0, -1.0, -0.5, 1.0], [0.0, 1.0, 0.0, 1.0]),
            fragment_vertex([-1.0, 1.0, -0.5, 1.0], [0.0, 0.0, 1.0, 1.0]),
        ]);
        let state = PicaFragmentRasterState::new(
            Matrix4::identity(),
            Viewport::new(0.0, 0.0, 8.0, 8.0).unwrap(),
        );
        let report = rasterizer.draw(&draw, &pipeline, &memory, state).unwrap();

        assert_eq!(report.covered_pixels, 28);
        assert_eq!(report.alpha_rejected_pixels, 28);
        assert_eq!(report.depth_rejected_pixels, 0);
        assert_eq!(report.shaded_pixels, 0);
        assert_eq!(rasterizer.color_at(1, 6), Some([0, 0, 0, 255]));
        assert_eq!(rasterizer.depth_buffer()[6 * 8 + 1], 1.0);
    }

    #[test]
    fn framebuffer_color_and_depth_write_masks_are_independent() {
        let mut pica = super::super::Pica200::new();
        configure_tev(&mut pica);
        let pipeline = pica.fragment_pipeline().unwrap();
        let target =
            PicaRenderTargetConfig::from_registers(VRAM_BASE >> 3, 2, 0, 8 | (7 << 12) | (1 << 24))
                .unwrap();

        let mut memory = GpuPhysicalMemory::new();
        let initial_color = [20, 30, 40, 50];
        let mut initial_pixels = vec![0u8; 8 * 8 * 4];
        for pixel in initial_pixels.chunks_exact_mut(4) {
            pixel.copy_from_slice(&initial_color);
        }
        let mut initial = SoftwareRasterizer::new(8, 8).unwrap();
        initial.load_color_rgba8(&initial_pixels).unwrap();
        target.write_rasterizer(&initial, &mut memory).unwrap();

        let mut rasterizer =
            PicaFragmentRasterizer::from_render_target(target, &memory, 1.0).unwrap();
        let source = [200.0 / 255.0, 100.0 / 255.0, 50.0 / 255.0, 1.0];
        let guest_draw = draw([
            fragment_vertex([-1.0, -1.0, -0.5, 1.0], source),
            fragment_vertex([1.0, -1.0, -0.5, 1.0], source),
            fragment_vertex([-1.0, 1.0, -0.5, 1.0], source),
        ]);
        let mut state = PicaFragmentRasterState::new(
            Matrix4::identity(),
            Viewport::new(0.0, 0.0, 8.0, 8.0).unwrap(),
        );
        state.depth_function = DepthFunction::Always;
        state.color_write_mask = [false, true, false, false];
        state.depth_write_enabled = false;

        let report = rasterizer
            .draw(&guest_draw, &pipeline, &memory, state)
            .unwrap();
        assert!(report.shaded_pixels > 0);
        assert_eq!(rasterizer.color_at(1, 6), Some([20, 100, 40, 50]));
        assert_eq!(rasterizer.depth_buffer()[6 * 8 + 1], 1.0);

        state.color_write_mask = [true; 4];
        state.depth_write_enabled = true;
        rasterizer
            .draw(&guest_draw, &pipeline, &memory, state)
            .unwrap();
        assert_eq!(rasterizer.color_at(1, 6), Some([200, 100, 50, 255]));
        assert!((rasterizer.depth_buffer()[6 * 8 + 1] - 0.5).abs() < 1.0e-6);
    }

    #[test]
    fn stencil_fail_replace_then_equal_increment_updates_d24s8_surface() {
        let mut pica = super::super::Pica200::new();
        configure_tev(&mut pica);
        pica.write_register(REG_LOGIC_OP, 3).unwrap();
        pica.write_register(REG_STENCIL_TEST, 1 | (0xFF << 8) | (9 << 16) | (0xFF << 24))
            .unwrap();
        pica.write_register(REG_STENCIL_OP, 2).unwrap(); // stencil fail: replace
        let mut pipeline = pica.fragment_pipeline().unwrap();

        let target =
            PicaRenderTargetConfig::from_registers(VRAM_BASE >> 3, 2, 0, 8 | (7 << 12) | (1 << 24))
                .unwrap();
        let depth_config = PicaDepthBufferConfig {
            address: VRAM_BASE + 0x1000,
            width: 8,
            height: 8,
            format: PicaDepthBufferFormat::D24S8,
            depth_read_enabled: true,
            stencil_read_enabled: true,
            depth_write_enabled: true,
            stencil_write_enabled: true,
        };

        let mut memory = GpuPhysicalMemory::new();
        let initial_pixels = vec![0u8; 8 * 8 * 4];
        let mut initial = SoftwareRasterizer::new(8, 8).unwrap();
        initial.load_color_rgba8(&initial_pixels).unwrap();
        target.write_rasterizer(&initial, &mut memory).unwrap();
        depth_config
            .write_depth_values(&vec![1.0; 64], &mut memory)
            .unwrap();
        depth_config
            .write_stencil_values(&[0; 64], &mut memory)
            .unwrap();

        let mut rasterizer =
            PicaFragmentRasterizer::from_render_target(target, &memory, 1.0).unwrap();
        rasterizer
            .load_depth_buffer(depth_config, &memory, 1.0)
            .unwrap();
        let draw = draw([
            fragment_vertex([-1.0, -1.0, -0.5, 1.0], [1.0, 0.0, 0.0, 1.0]),
            fragment_vertex([1.0, -1.0, -0.5, 1.0], [0.0, 1.0, 0.0, 1.0]),
            fragment_vertex([-1.0, 1.0, -0.5, 1.0], [0.0, 0.0, 1.0, 1.0]),
        ]);
        let mut state = PicaFragmentRasterState::new(
            Matrix4::identity(),
            Viewport::new(0.0, 0.0, 8.0, 8.0).unwrap(),
        );
        state.depth_function = DepthFunction::Always;
        state.depth_write_enabled = false;
        state.depth_format = Some(PicaDepthBufferFormat::D24S8);
        state.stencil_write_enabled = true;

        // First draw fails stencil, but REPLACE writes the reference value 9.
        let rejected = rasterizer.draw(&draw, &pipeline, &memory, state).unwrap();
        assert_eq!(rejected.stencil_rejected_pixels, 28);
        assert_eq!(rejected.shaded_pixels, 0);
        let pixel_index = 6 * 8 + 1;
        assert_eq!(rasterizer.stencil_buffer()[pixel_index], 9);
        assert_eq!(rasterizer.color_at(1, 6), Some([0, 0, 0, 0]));

        rasterizer
            .write_depth_buffer(depth_config, &mut memory)
            .unwrap();
        let stored = depth_config.read_stencil_values(&memory).unwrap().unwrap();
        assert_eq!(stored[pixel_index], 9);

        // Stencil now matches, but depth fails. The depth-fail action increments.
        depth_config
            .write_depth_values(&vec![0.25; 64], &mut memory)
            .unwrap();
        rasterizer
            .load_depth_buffer(depth_config, &memory, 1.0)
            .unwrap();
        pica.write_register(
            REG_STENCIL_TEST,
            1 | (2 << 4) | (0xFF << 8) | (9 << 16) | (0xFF << 24),
        )
        .unwrap();
        pica.write_register(REG_STENCIL_OP, 3 << 4).unwrap();
        pipeline = pica.fragment_pipeline().unwrap();
        state.depth_function = DepthFunction::Less;

        let depth_rejected = rasterizer.draw(&draw, &pipeline, &memory, state).unwrap();
        assert_eq!(depth_rejected.stencil_rejected_pixels, 0);
        assert!(depth_rejected.depth_rejected_pixels > 0);
        assert_eq!(depth_rejected.shaded_pixels, 0);
        assert_eq!(rasterizer.stencil_buffer()[pixel_index], 10);
        assert_eq!(rasterizer.color_at(1, 6), Some([0, 0, 0, 0]));

        rasterizer
            .write_depth_buffer(depth_config, &mut memory)
            .unwrap();
        let stored = depth_config.read_stencil_values(&memory).unwrap().unwrap();
        assert_eq!(stored[pixel_index], 10);

        // An always-passing depth test now runs the depth-pass wrap operation.
        pica.write_register(
            REG_STENCIL_TEST,
            1 | (2 << 4) | (0xFF << 8) | (10 << 16) | (0xFF << 24),
        )
        .unwrap();
        pica.write_register(REG_STENCIL_OP, 6 << 8).unwrap();
        pipeline = pica.fragment_pipeline().unwrap();
        state.depth_function = DepthFunction::Always;

        let accepted = rasterizer.draw(&draw, &pipeline, &memory, state).unwrap();
        assert_eq!(accepted.stencil_rejected_pixels, 0);
        assert_eq!(accepted.shaded_pixels, 28);
        assert_eq!(rasterizer.stencil_buffer()[pixel_index], 11);
        assert_ne!(rasterizer.color_at(1, 6), Some([0, 0, 0, 0]));

        rasterizer
            .write_depth_buffer(depth_config, &mut memory)
            .unwrap();
        let stored = depth_config.read_stencil_values(&memory).unwrap().unwrap();
        assert_eq!(stored[pixel_index], 11);
    }

    #[test]
    fn clipping_interpolates_texture_coordinates_with_vertex_outputs() {
        let first = ClipVertex {
            varying: PicaFragmentVertex {
                clip_position: [-2.0, 0.0, -0.5, 1.0],
                primary_color: [0.0; 4],
                fragment_primary_color: [1.0; 4],
                fragment_secondary_color: [0.0; 4],
                texture_coordinates: [[0.0, 0.0]; 3],
            },
        };
        let second = ClipVertex {
            varying: PicaFragmentVertex {
                clip_position: [0.0, 0.0, -0.5, 1.0],
                primary_color: [1.0; 4],
                fragment_primary_color: [1.0; 4],
                fragment_secondary_color: [0.0; 4],
                texture_coordinates: [[1.0, 0.0]; 3],
            },
        };
        let clipped = first.interpolate(second, 0.5);
        assert_eq!(clipped.varying.clip_position[0], -1.0);
        assert_eq!(clipped.varying.primary_color, [0.5; 4]);
        assert_eq!(clipped.varying.texture_coordinates[0], [0.5, 0.0]);
    }
}
