//! Independent PICA200 software fragment rasterizer.
//!
//! This path consumes the guest vertex-shader draw and output-map results,
//! clips varyings in the PICA clip volume, rasterizes pixel centers with a
//! top-left rule, and shades perspective-correct color/texture varyings.
//! Covered samples pass through texture sampling, TEV, linear/LUT fog, alpha,
//! depth/stencil tests and operations, blending/logic ops, and channel masks;
//! color and depth/stencil results are written through the PICA surface codecs.
//!
//! It remains separate from the generic GeometryPipeline and SoftwareRasterizer.
//! Multisample coverage, exact hardware fixed-point/rounding behavior and
//! independent validation against a physical PICA200 remain outstanding.

use super::{
    DepthFunction, GpuPhysicalMemory, Matrix4, PicaColorBufferFormat, PicaCullMode,
    PicaDepthBufferConfig, PicaDepthBufferError, PicaDepthBufferFormat, PicaDepthMap,
    PicaDepthMode, PicaFragmentDraw, PicaFragmentDrawError, PicaFragmentPipeline,
    PicaFragmentSampleError, PicaFragmentVertex, PicaRenderTargetConfig, PicaRenderTargetError,
    PicaRenderTargetWriteReport, PicaScissorMode, PicaScissorState, PicaShadedDraw,
    PicaVertexOutputMap, SoftwareRasterizer, Viewport,
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
    /// Face culling by triangle winding in PICA window space (Y up).
    pub cull_mode: PicaCullMode,
    /// Scissor rectangle in PICA window coordinates (origin bottom-left).
    pub scissor: PicaScissorState,
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
            cull_mode: PicaCullMode::KeepAll,
            scissor: PicaScissorState::default(),
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
    /// Storage format of the render target; written pixels are quantized to it.
    format: PicaColorBufferFormat,
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
            format: target.format,
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
        // `area` uses top-left screen coordinates (Y down); PICA window space
        // has Y up, so the winding seen by the guest is the opposite sign.
        let culled = match state.cull_mode {
            PicaCullMode::KeepAll => false,
            PicaCullMode::KeepClockwise => area < 0.0,
            PicaCullMode::KeepCounterClockwise => area > 0.0,
        };
        if culled {
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
                if scissor_rejects(state.scissor, x, y, self.height) {
                    continue;
                }

                report.covered_pixels = report.covered_pixels.saturating_add(1);
                let weights = edges.map(|edge| edge / area);
                let interpolated_depth = (0..3)
                    .map(|index| weights[index] * f64::from(vertices[index].depth))
                    .sum::<f64>() as f32;
                let fog_depth = match state.depth_map {
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
                // Fog consumes the PICA depth-map result, before the physical
                // D16/D24 target applies its storage precision.
                let fragment_depth = state
                    .depth_format
                    .map(|format| format.quantize_depth(fog_depth))
                    .unwrap_or(fog_depth);

                let pixel_index = y * self.width + x;
                let varyings = [
                    vertices[0].varying,
                    vertices[1].varying,
                    vertices[2].varying,
                ];
                // Barycentrics one pixel to the right and down give the texture
                // coordinate derivatives that select the mip level.
                let step_weights = |dx: f32, dy: f32| {
                    let shifted = ScreenVertex {
                        x: point.x + dx,
                        y: point.y + dy,
                        ..point
                    };
                    [
                        orient(vertices[1], vertices[2], shifted) / area,
                        orient(vertices[2], vertices[0], shifted) / area,
                        orient(vertices[0], vertices[1], shifted) / area,
                    ]
                    .map(|weight| weight as f32)
                };
                let shade_fragment = || {
                    pipeline
                        .shade_triangle_sample_with_derivatives(
                            memory,
                            varyings,
                            weights.map(|weight| weight as f32),
                            step_weights(1.0, 0.0),
                            step_weights(0.0, 1.0),
                            state.minifying,
                        )
                        .map(|color| pipeline.apply_fog(color, fog_depth))
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
                // Masked channels keep the stored pixel; the merged pixel is then
                // rounded to the target format, as a later blend reads it back.
                let mut merged = [0u8; 4];
                for (channel, enabled) in state.color_write_mask.iter().copied().enumerate() {
                    merged[channel] = if enabled {
                        output[channel]
                    } else {
                        self.color[color_offset + channel]
                    };
                }
                self.color[color_offset..color_offset + 4]
                    .copy_from_slice(&self.format.quantize(merged));
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

/// Whether the scissor discards the pixel at top-left coordinates (`x`, `y`).
fn scissor_rejects(scissor: PicaScissorState, x: usize, y: usize, height: usize) -> bool {
    if scissor.mode == PicaScissorMode::Disabled {
        return false;
    }
    // Convert to PICA window coordinates (origin bottom-left).
    let window_y = (height - 1 - y) as u64;
    let window_x = x as u64;
    let inside = (u64::from(scissor.x1)..=u64::from(scissor.x2)).contains(&window_x)
        && (u64::from(scissor.y1)..=u64::from(scissor.y2)).contains(&window_y);
    match scissor.mode {
        PicaScissorMode::Include => !inside,
        PicaScissorMode::Exclude => inside,
        PicaScissorMode::Disabled => false,
    }
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
        PicaVertexOutputMap, REG_BLEND_FUNC, REG_COLOR_OPERATION, REG_FOG_COLOR,
        REG_FOG_LINEAR_OFFSET, REG_FOG_LINEAR_SLOPE, REG_FOG_LUT_DATA0, REG_FOG_LUT_INDEX,
        REG_FRAGOP_ALPHA_TEST, REG_LOGIC_OP, REG_STENCIL_OP, REG_STENCIL_TEST,
        REG_TEXENV_UPDATE_BUFFER, REG_TEXUNIT_CONFIG, TEXENV_STAGE_SOURCE_REGISTERS,
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
    fn linear_fog_is_applied_to_fragments_written_by_the_rasterizer() {
        let mut pica = super::super::Pica200::new();
        configure_tev(&mut pica);
        pica.write_register(REG_TEXENV_UPDATE_BUFFER, 1).unwrap();
        pica.write_register(REG_FOG_COLOR, 0x00FF_0000).unwrap();
        pica.write_register(REG_FOG_LINEAR_SLOPE, 0).unwrap();
        pica.write_register(REG_FOG_LINEAR_OFFSET, 0).unwrap();
        let pipeline = pica.fragment_pipeline().unwrap();

        let target =
            PicaRenderTargetConfig::from_registers(VRAM_BASE >> 3, 2, 0, 8 | (7 << 12) | (1 << 24))
                .unwrap();
        let memory = GpuPhysicalMemory::new();
        let mut rasterizer =
            PicaFragmentRasterizer::from_render_target(target, &memory, 1.0).unwrap();
        let shaded = draw([
            fragment_vertex([-1.0, -1.0, -0.5, 1.0], [1.0, 0.0, 0.0, 1.0]),
            fragment_vertex([1.0, -1.0, -0.5, 1.0], [1.0, 0.0, 0.0, 1.0]),
            fragment_vertex([-1.0, 1.0, -0.5, 1.0], [1.0, 0.0, 0.0, 1.0]),
        ]);
        let state = PicaFragmentRasterState::new(
            Matrix4::identity(),
            Viewport::new(0.0, 0.0, 8.0, 8.0).unwrap(),
        );

        let report = rasterizer.draw(&shaded, &pipeline, &memory, state).unwrap();
        assert!(report.shaded_pixels > 0);
        assert_eq!(rasterizer.color_at(1, 6), Some([0, 0, 255, 255]));
    }

    #[test]
    fn fog_lut_color_reaches_the_tiled_framebuffer_through_rasterization() {
        let mut pica = super::super::Pica200::new();
        configure_tev(&mut pica);
        pica.write_register(REG_FOG_COLOR, 0x0000_FF00).unwrap();
        pica.write_register(REG_FOG_LUT_INDEX, 64).unwrap();
        // The test triangle has normalized depth 0.5, so its fog index is 64.
        pica.write_register(REG_FOG_LUT_DATA0, 0).unwrap();
        pica.write_register(REG_TEXENV_UPDATE_BUFFER, 5).unwrap();
        let pipeline = pica.fragment_pipeline().unwrap();

        let target =
            PicaRenderTargetConfig::from_registers(VRAM_BASE >> 3, 2, 0, 8 | (7 << 12) | (1 << 24))
                .unwrap();
        let memory = GpuPhysicalMemory::new();
        let mut rasterizer =
            PicaFragmentRasterizer::from_render_target(target, &memory, 1.0).unwrap();
        let shaded = draw([
            fragment_vertex([-1.0, -1.0, -0.5, 1.0], [1.0, 0.0, 0.0, 1.0]),
            fragment_vertex([1.0, -1.0, -0.5, 1.0], [1.0, 0.0, 0.0, 1.0]),
            fragment_vertex([-1.0, 1.0, -0.5, 1.0], [1.0, 0.0, 0.0, 1.0]),
        ]);
        let state = PicaFragmentRasterState::new(
            Matrix4::identity(),
            Viewport::new(0.0, 0.0, 8.0, 8.0).unwrap(),
        );

        let report = rasterizer.draw(&shaded, &pipeline, &memory, state).unwrap();
        assert!(report.shaded_pixels > 0);
        assert_eq!(rasterizer.color_at(1, 6), Some([0, 255, 0, 255]));
    }

    /// Runs the reference lower-left triangle (counter-clockwise in PICA window
    /// space) through an 8x8 target with the given culling/scissor state.
    fn raster_with(
        order: [usize; 3],
        cull_mode: PicaCullMode,
        scissor: PicaScissorState,
    ) -> (PicaFragmentRasterReport, PicaFragmentRasterizer) {
        let mut pica = super::super::Pica200::new();
        configure_tev(&mut pica);
        let pipeline = pica.fragment_pipeline().unwrap();
        let target =
            PicaRenderTargetConfig::from_registers(VRAM_BASE >> 3, 2, 0, 8 | (7 << 12) | (1 << 24))
                .unwrap();
        let memory = GpuPhysicalMemory::new();
        let mut rasterizer =
            PicaFragmentRasterizer::from_render_target(target, &memory, 1.0).unwrap();
        let corners = [
            fragment_vertex([-1.0, -1.0, -0.5, 1.0], [1.0, 0.0, 0.0, 1.0]),
            fragment_vertex([1.0, -1.0, -0.5, 1.0], [0.0, 1.0, 0.0, 1.0]),
            fragment_vertex([-1.0, 1.0, -0.5, 1.0], [0.0, 0.0, 1.0, 1.0]),
        ];
        let draw = draw(order.map(|index| corners[index]));
        let mut state = PicaFragmentRasterState::new(
            Matrix4::identity(),
            Viewport::new(0.0, 0.0, 8.0, 8.0).unwrap(),
        );
        state.cull_mode = cull_mode;
        state.scissor = scissor;
        let report = rasterizer.draw(&draw, &pipeline, &memory, state).unwrap();
        (report, rasterizer)
    }

    #[test]
    fn face_culling_follows_the_winding_in_pica_window_space() {
        let none = PicaScissorState::default();
        // Counter-clockwise (Y up) triangle.
        let ccw = [0, 1, 2];
        let cw = [0, 2, 1];
        for (order, mode, expected) in [
            (ccw, PicaCullMode::KeepAll, 28),
            (ccw, PicaCullMode::KeepCounterClockwise, 28),
            (ccw, PicaCullMode::KeepClockwise, 0),
            (cw, PicaCullMode::KeepAll, 28),
            (cw, PicaCullMode::KeepClockwise, 28),
            (cw, PicaCullMode::KeepCounterClockwise, 0),
        ] {
            let (report, rasterizer) = raster_with(order, mode, none);
            assert_eq!(report.covered_pixels, expected, "{order:?} {mode:?}");
            if expected == 0 {
                assert_eq!(rasterizer.color_at(1, 6), Some([0, 0, 0, 0]));
            }
        }
    }

    #[test]
    fn scissor_include_and_exclude_partition_the_covered_pixels() {
        let rect = |mode| PicaScissorState {
            mode,
            x1: 0,
            y1: 0,
            x2: 3,
            y2: 3,
        };
        let (include, _) = raster_with(
            [0, 1, 2],
            PicaCullMode::KeepAll,
            rect(PicaScissorMode::Include),
        );
        let (exclude, _) = raster_with(
            [0, 1, 2],
            PicaCullMode::KeepAll,
            rect(PicaScissorMode::Exclude),
        );
        assert!(include.covered_pixels > 0 && exclude.covered_pixels > 0);
        assert_eq!(include.covered_pixels + exclude.covered_pixels, 28);

        let (disabled, _) = raster_with(
            [0, 1, 2],
            PicaCullMode::KeepAll,
            rect(PicaScissorMode::Disabled),
        );
        assert_eq!(disabled.covered_pixels, 28);
    }

    #[test]
    fn scissor_rows_use_the_pica_bottom_left_origin() {
        // Window row 0 is the bottom row, i.e. screen row 7 of the 8x8 target.
        let bottom_row = PicaScissorState {
            mode: PicaScissorMode::Include,
            x1: 0,
            y1: 0,
            x2: 7,
            y2: 0,
        };
        let (report, rasterizer) = raster_with([0, 1, 2], PicaCullMode::KeepAll, bottom_row);
        assert_eq!(report.covered_pixels, 7);
        assert_ne!(rasterizer.color_at(0, 7), Some([0, 0, 0, 0]));
        assert_eq!(rasterizer.color_at(0, 6), Some([0, 0, 0, 0]));
    }

    /// Draws the reference triangle with texture unit 0 (Intensity8) mapped
    /// over the whole target and returns the shaded red channel at pixel (1, 6).
    fn textured_red(texture_size: u16, max_level: u32) -> u8 {
        let mut pica = super::super::Pica200::new();
        configure_tev(&mut pica);
        pica.write_register(TEXENV_STAGE_SOURCE_REGISTERS[0], 3 | (3 << 16))
            .unwrap();
        let texture_address = VRAM_BASE + 0x1_0000;
        pica.write_register(REG_TEXUNIT_CONFIG, 1).unwrap();
        pica.write_register(
            0x082,
            (u32::from(texture_size) << 16) | u32::from(texture_size),
        )
        .unwrap();
        pica.write_register(0x083, 0).unwrap();
        pica.write_register(0x084, max_level << 16).unwrap();
        pica.write_register(0x085, texture_address >> 3).unwrap();
        pica.write_register(0x08E, 7).unwrap();
        let pipeline = pica.fragment_pipeline().unwrap();

        // Mip chain: each level is filled with a distinct intensity.
        let mut memory = GpuPhysicalMemory::new();
        let mut address = texture_address;
        let mut size = u32::from(texture_size);
        for level in 0..=max_level {
            let bytes = (size.div_ceil(8) * size.div_ceil(8) * 64) as usize;
            memory
                .write(address, &vec![10 + 80 * level as u8; bytes])
                .unwrap();
            address += bytes as u32;
            size = (size / 2).max(1);
        }

        let target =
            PicaRenderTargetConfig::from_registers(VRAM_BASE >> 3, 2, 0, 8 | (7 << 12) | (1 << 24))
                .unwrap();
        let mut rasterizer =
            PicaFragmentRasterizer::from_render_target(target, &memory, 1.0).unwrap();
        let mut corners = [
            fragment_vertex([-1.0, -1.0, -0.5, 1.0], [1.0; 4]),
            fragment_vertex([1.0, -1.0, -0.5, 1.0], [1.0; 4]),
            fragment_vertex([-1.0, 1.0, -0.5, 1.0], [1.0; 4]),
        ];
        corners[0].texture_coordinates[0] = [0.0, 0.0];
        corners[1].texture_coordinates[0] = [1.0, 0.0];
        corners[2].texture_coordinates[0] = [0.0, 1.0];
        let state = PicaFragmentRasterState::new(
            Matrix4::identity(),
            Viewport::new(0.0, 0.0, 8.0, 8.0).unwrap(),
        );
        rasterizer
            .draw(&draw(corners), &pipeline, &memory, state)
            .unwrap();
        rasterizer.color_at(1, 6).unwrap()[0]
    }

    #[test]
    fn minified_texture_samples_the_mip_level_chosen_by_screen_derivatives() {
        // 8x8 texture on 8 pixels: 1 texel per pixel, level 0 (value 10).
        assert_eq!(textured_red(8, 0), 10);
        // 64x64 on 8 pixels: 8 texels per pixel -> LOD 3 -> level 3 (value 250).
        assert_eq!(textured_red(64, 3), 250);
        // Same minification without stored mip levels stays on level 0.
        assert_eq!(textured_red(64, 0), 10);
        // Fewer stored levels clamp the selection to the last one (level 2).
        assert_eq!(textured_red(64, 2), 170);
    }

    /// Draws the reference triangle `draws` times onto an 8x8 target of the
    /// given color format and returns the pixel at (1, 6).
    fn draw_on_format(
        color_format: u32,
        configure: impl Fn(&mut super::super::Pica200),
        color: [f32; 4],
        mask: [bool; 4],
        draws: usize,
    ) -> [u8; 4] {
        let mut pica = super::super::Pica200::new();
        configure_tev(&mut pica);
        configure(&mut pica);
        let pipeline = pica.fragment_pipeline().unwrap();
        let target = PicaRenderTargetConfig::from_registers(
            VRAM_BASE >> 3,
            color_format,
            0,
            8 | (7 << 12) | (1 << 24),
        )
        .unwrap();
        let memory = GpuPhysicalMemory::new();
        let mut rasterizer =
            PicaFragmentRasterizer::from_render_target(target, &memory, 1.0).unwrap();
        let corners = [
            fragment_vertex([-1.0, -1.0, -0.5, 1.0], color),
            fragment_vertex([1.0, -1.0, -0.5, 1.0], color),
            fragment_vertex([-1.0, 1.0, -0.5, 1.0], color),
        ];
        let draw = draw(corners);
        let mut state = PicaFragmentRasterState::new(
            Matrix4::identity(),
            Viewport::new(0.0, 0.0, 8.0, 8.0).unwrap(),
        );
        state.depth_function = DepthFunction::Always;
        state.color_write_mask = mask;
        for _ in 0..draws {
            rasterizer.draw(&draw, &pipeline, &memory, state).unwrap();
        }
        rasterizer.color_at(1, 6).unwrap()
    }

    const FORMAT_RGBA8: u32 = 2;
    const FORMAT_RGB565: u32 = 3 << 16;

    #[test]
    fn written_pixels_are_quantized_to_the_target_format() {
        let color = [100.0 / 255.0, 150.0 / 255.0, 200.0 / 255.0, 0.3];
        let exact = draw_on_format(FORMAT_RGBA8, |_| {}, color, [true; 4], 1);
        assert_eq!(exact, [100, 150, 200, 77]);
        let packed = draw_on_format(FORMAT_RGB565, |_| {}, color, [true; 4], 1);
        assert_eq!(
            packed,
            PicaColorBufferFormat::Rgb565.quantize([100, 150, 200, 77])
        );
        assert_eq!(packed[3], 255);
        assert_ne!(packed[..3], exact[..3]);
    }

    #[test]
    fn repeated_blends_read_back_the_quantized_destination() {
        // Blend: source alpha / one minus source alpha for color and alpha.
        let configure = |pica: &mut super::super::Pica200| {
            pica.write_register(REG_COLOR_OPERATION, 1 << 8).unwrap();
            let factors = 6 | (7 << 4);
            pica.write_register(REG_BLEND_FUNC, (factors << 16) | (factors << 24))
                .unwrap();
        };
        let color = [1.0, 1.0, 1.0, 0.5];
        let format = PicaColorBufferFormat::Rgb565;
        // Reference chain: each draw reads the previous quantized pixel.
        let mut stored = format.quantize([0, 0, 0, 255]);
        for _ in 0..3 {
            let blended = 0.5 + (f32::from(stored[0]) / 255.0) * 0.5;
            stored = format.quantize([(blended * 255.0).round() as u8, 0, 0, 255]);
        }
        let result = draw_on_format(FORMAT_RGB565, configure, color, [true; 4], 3);
        assert_eq!(result[0], stored[0]);
    }

    #[test]
    fn color_write_mask_keeps_masked_channels_of_the_stored_pixel() {
        let color = [100.0 / 255.0, 150.0 / 255.0, 200.0 / 255.0, 1.0];
        let only_red_and_blue =
            draw_on_format(FORMAT_RGBA8, |_| {}, color, [true, false, true, false], 1);
        // Green and alpha keep the initial destination (all zero in fresh memory).
        assert_eq!(only_red_and_blue, [100, 0, 200, 0]);
        let none = draw_on_format(FORMAT_RGBA8, |_| {}, color, [false; 4], 1);
        assert_eq!(none, [0, 0, 0, 0]);
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
