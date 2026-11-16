//! Connects shaded PICA200 draws to the reference geometry stage and rasterizer.
//!
//! The vertex shader outputs a clip-space position, so the pipeline passed in
//! should normally use an identity transform; the caller picks the depth range
//! and viewport. This stage performs no face culling (the PICA200 culling
//! rules are not established here), so both windings are rasterized, and it
//! interpolates only the vertex color: texturing, lighting, blending, stencil
//! and the framebuffer formats of the real GPU are not modeled.

use super::{
    DepthFunction, GeometryError, GeometryPipeline, GeometryReport, GeometryVertex,
    GpuPhysicalMemory, Pica200, PicaCommandListReport, PicaCommandListRunError, PicaDrawExecError,
    PicaDrawRequest, PicaOutputMapError, PicaRenderTargetConfig, PicaRenderTargetError,
    PicaShadedDraw, PicaVertexOutputMap, PicaVertexShaderProgram, RasterizerError,
    SoftwareRasterizer, Viewport,
};

/// Totals for one rendered draw.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PicaRenderReport {
    /// Triangles assembled for the draw (before clipping).
    pub triangles_submitted: u64,
    /// Triangles left after clipping and fan triangulation.
    pub output_triangles: u64,
    pub pixels_written: u64,
    pub depth_rejected_pixels: u64,
    /// Shader instructions executed; zero when rendering an already shaded draw.
    pub executed_instructions: u64,
}

/// Failure while rendering a draw.
#[derive(Clone, Debug, PartialEq)]
pub enum PicaRenderError {
    Shade(PicaDrawExecError),
    OutputMap(PicaOutputMapError),
    Geometry(GeometryError),
}

impl std::fmt::Display for PicaRenderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shade(error) => write!(formatter, "vertex stage failed: {error}"),
            Self::OutputMap(error) => write!(formatter, "output map failed: {error}"),
            Self::Geometry(error) => write!(formatter, "geometry stage failed: {error}"),
        }
    }
}

impl std::error::Error for PicaRenderError {}

impl From<PicaDrawExecError> for PicaRenderError {
    fn from(error: PicaDrawExecError) -> Self {
        Self::Shade(error)
    }
}

impl From<PicaOutputMapError> for PicaRenderError {
    fn from(error: PicaOutputMapError) -> Self {
        Self::OutputMap(error)
    }
}

impl From<GeometryError> for PicaRenderError {
    fn from(error: GeometryError) -> Self {
        Self::Geometry(error)
    }
}

impl GeometryPipeline {
    /// Rasterizes a shaded draw.
    ///
    /// Every vertex is routed through `map` once. `default_color` is used for
    /// vertices whose map has no color. Stops at the first error; triangles
    /// drawn before it stay in the rasterizer.
    pub fn draw_shaded(
        &self,
        shaded: &PicaShadedDraw,
        map: &PicaVertexOutputMap,
        default_color: [u8; 4],
        rasterizer: &mut SoftwareRasterizer,
        depth_function: DepthFunction,
    ) -> Result<PicaRenderReport, PicaRenderError> {
        let vertices = shaded
            .vertices
            .iter()
            .map(|vertex| {
                vertex
                    .resolve(map)
                    .map(|resolved| resolved.to_geometry_vertex(default_color))
            })
            .collect::<Result<Vec<GeometryVertex>, _>>()?;

        let mut report = PicaRenderReport {
            triangles_submitted: shaded.triangles.len() as u64,
            executed_instructions: shaded.executed_instructions,
            ..PicaRenderReport::default()
        };
        for triangle in &shaded.triangles {
            let geometry: GeometryReport = self.draw_triangle(
                [
                    vertices[triangle[0]],
                    vertices[triangle[1]],
                    vertices[triangle[2]],
                ],
                rasterizer,
                depth_function,
            )?;
            report.output_triangles += geometry.output_triangles;
            report.pixels_written += geometry.pixels_written;
            report.depth_rejected_pixels += geometry.depth_rejected_pixels;
        }
        Ok(report)
    }
}

impl Pica200 {
    /// Shades and rasterizes one captured draw request.
    ///
    /// Starts the shader at GPUREG_VSH_ENTRYPOINT and reads the output map from
    /// the live registers, so call it before the command list changes them.
    /// `resolved_physical_byte_base` is the caller-translated vertex base.
    #[allow(clippy::too_many_arguments)]
    pub fn render_draw_request(
        &self,
        request: &PicaDrawRequest,
        memory: &GpuPhysicalMemory,
        resolved_physical_byte_base: u32,
        program: &PicaVertexShaderProgram,
        pipeline: &GeometryPipeline,
        default_color: [u8; 4],
        rasterizer: &mut SoftwareRasterizer,
        depth_function: DepthFunction,
    ) -> Result<PicaRenderReport, PicaRenderError> {
        let shaded = self.shade_draw_at_registered_entrypoint(
            request,
            memory,
            resolved_physical_byte_base,
            program,
        )?;
        let map = self.vertex_output_map()?;
        pipeline.draw_shaded(&shaded, &map, default_color, rasterizer, depth_function)
    }
}

/// Fixed (non-register) inputs of a render session.
#[derive(Clone, Copy, Debug)]
pub struct PicaRenderSettings {
    pub pipeline: GeometryPipeline,
    pub default_color: [u8; 4],
    pub depth_function: DepthFunction,
}

/// Failure while rendering into the live render target.
#[derive(Clone, Debug, PartialEq)]
pub enum PicaRenderSessionError {
    Target(PicaRenderTargetError),
    Rasterizer(RasterizerError),
    Render(PicaRenderError),
    /// The caller could not translate the draw's vertex base address.
    UnresolvedVertexBase,
}

impl std::fmt::Display for PicaRenderSessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Target(error) => write!(formatter, "render target failed: {error}"),
            Self::Rasterizer(error) => write!(formatter, "rasterizer failed: {error}"),
            Self::Render(error) => write!(formatter, "{error}"),
            Self::UnresolvedVertexBase => write!(formatter, "vertex base address not resolved"),
        }
    }
}

impl std::error::Error for PicaRenderSessionError {}

/// Renders the draws of a command list into the tiled render target.
///
/// The target registers are re-read for every draw. The color buffer is loaded
/// from physical memory when a target is first used (so earlier content is
/// kept) and written back by `flush`/`finish` and whenever the target changes.
/// Depth is not stored in memory by this model: it starts at 1.0 when a target
/// is loaded and is discarded when the target changes.
#[derive(Debug)]
pub struct PicaRenderSession {
    settings: PicaRenderSettings,
    active: Option<(PicaRenderTargetConfig, SoftwareRasterizer)>,
    dirty: bool,
    totals: PicaRenderReport,
    draws_rendered: u64,
}

impl PicaRenderSession {
    pub fn new(settings: PicaRenderSettings) -> Self {
        Self {
            settings,
            active: None,
            dirty: false,
            totals: PicaRenderReport::default(),
            draws_rendered: 0,
        }
    }

    pub fn totals(&self) -> PicaRenderReport {
        self.totals
    }

    pub fn draws_rendered(&self) -> u64 {
        self.draws_rendered
    }

    /// Renders one draw using the GPU's live state. `vertex_base` is the
    /// caller-translated physical byte base of the vertex data.
    pub fn draw(
        &mut self,
        pica: &Pica200,
        request: &PicaDrawRequest,
        memory: &mut GpuPhysicalMemory,
        vertex_base: Option<u32>,
    ) -> Result<PicaRenderReport, PicaRenderSessionError> {
        let vertex_base = vertex_base.ok_or(PicaRenderSessionError::UnresolvedVertexBase)?;
        let config = pica
            .render_target_config()
            .map_err(PicaRenderSessionError::Target)?;
        self.select_target(config, memory)?;
        let configured_viewport = self.settings.pipeline.viewport();
        let target_pipeline = GeometryPipeline::new(
            self.settings.pipeline.transform(),
            Viewport::new(
                configured_viewport.x,
                configured_viewport.y,
                config.width as f32,
                config.height as f32,
            )
            .map_err(|error| PicaRenderSessionError::Render(PicaRenderError::Geometry(error)))?,
            self.settings.pipeline.depth_range(),
        );
        let program = pica.vertex_shader_program();
        let rasterizer = &mut self.active.as_mut().expect("target selected above").1;
        // Mark dirty first: triangles drawn before an error stay in the buffer.
        self.dirty = true;
        let report = pica
            .render_draw_request(
                request,
                memory,
                vertex_base,
                &program,
                &target_pipeline,
                self.settings.default_color,
                rasterizer,
                self.settings.depth_function,
            )
            .map_err(PicaRenderSessionError::Render)?;
        self.draws_rendered += 1;
        self.totals.triangles_submitted += report.triangles_submitted;
        self.totals.output_triangles += report.output_triangles;
        self.totals.pixels_written += report.pixels_written;
        self.totals.depth_rejected_pixels += report.depth_rejected_pixels;
        self.totals.executed_instructions += report.executed_instructions;
        Ok(report)
    }

    /// Executes one command list through this persistent render session.
    ///
    /// Keeping the session alive across submissions preserves the active
    /// color/depth buffers instead of recreating the depth buffer for every
    /// GSP ProcessCommandList request. The returned render report contains only
    /// work performed by this command list.
    pub fn execute_command_list(
        &mut self,
        pica: &mut Pica200,
        words: &[u32],
        memory: &mut GpuPhysicalMemory,
        mut resolve_vertex_base: impl FnMut(&Pica200, &PicaDrawRequest) -> Option<u32>,
    ) -> Result<
        (PicaCommandListReport, PicaRenderReport),
        PicaCommandListRunError<PicaRenderSessionError>,
    > {
        let before = self.totals;
        let command_report = pica.execute_command_list_with_draws(words, |pica, request| {
            let vertex_base = resolve_vertex_base(pica, request);
            self.draw(pica, request, memory, vertex_base).map(|_| ())
        })?;
        let after = self.totals;
        let render_delta = PicaRenderReport {
            triangles_submitted: after
                .triangles_submitted
                .saturating_sub(before.triangles_submitted),
            output_triangles: after
                .output_triangles
                .saturating_sub(before.output_triangles),
            pixels_written: after.pixels_written.saturating_sub(before.pixels_written),
            depth_rejected_pixels: after
                .depth_rejected_pixels
                .saturating_sub(before.depth_rejected_pixels),
            executed_instructions: after
                .executed_instructions
                .saturating_sub(before.executed_instructions),
        };
        Ok((command_report, render_delta))
    }

    /// Writes pending pixels to the target memory; the target stays loaded.
    pub fn flush(&mut self, memory: &mut GpuPhysicalMemory) -> Result<(), PicaRenderSessionError> {
        if self.dirty {
            if let Some((config, rasterizer)) = &self.active {
                config
                    .write_rasterizer(rasterizer, memory)
                    .map_err(PicaRenderSessionError::Target)?;
            }
            self.dirty = false;
        }
        Ok(())
    }

    /// Flushes and returns the accumulated totals.
    pub fn finish(
        mut self,
        memory: &mut GpuPhysicalMemory,
    ) -> Result<PicaRenderReport, PicaRenderSessionError> {
        self.flush(memory)?;
        Ok(self.totals)
    }

    fn select_target(
        &mut self,
        config: PicaRenderTargetConfig,
        memory: &mut GpuPhysicalMemory,
    ) -> Result<(), PicaRenderSessionError> {
        if matches!(&self.active, Some((current, _)) if *current == config) {
            return Ok(());
        }
        self.flush(memory)?;
        let mut rasterizer = SoftwareRasterizer::new(config.width as usize, config.height as usize)
            .map_err(PicaRenderSessionError::Rasterizer)?;
        let pixels = config
            .read_rgba8(memory)
            .map_err(PicaRenderSessionError::Target)?;
        rasterizer
            .load_color_rgba8(&pixels)
            .map_err(PicaRenderSessionError::Rasterizer)?;
        self.active = Some((config, rasterizer));
        Ok(())
    }
}

/// Failure of `execute_command_list_rendering`.
#[derive(Clone, Debug, PartialEq)]
pub enum PicaRenderingRunError {
    Run(PicaCommandListRunError<PicaRenderSessionError>),
    /// The list ran but the final write-back failed.
    Flush(PicaRenderSessionError),
}

impl std::fmt::Display for PicaRenderingRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Run(error) => write!(formatter, "{error}"),
            Self::Flush(error) => write!(formatter, "render flush failed: {error}"),
        }
    }
}

impl std::error::Error for PicaRenderingRunError {}

impl Pica200 {
    /// Executes a command list and renders every draw at the moment it triggers.
    ///
    /// `resolve_vertex_base` maps the live GPU state and the draw to the
    /// physical byte base of the vertex data (the attribute-buffer address
    /// encoding is translated by the caller). Pixels drawn before an error are
    /// still written back; the original error is the one reported.
    pub fn execute_command_list_rendering(
        &mut self,
        words: &[u32],
        memory: &mut GpuPhysicalMemory,
        settings: PicaRenderSettings,
        mut resolve_vertex_base: impl FnMut(&Pica200, &PicaDrawRequest) -> Option<u32>,
    ) -> Result<(PicaCommandListReport, PicaRenderReport), PicaRenderingRunError> {
        let mut session = PicaRenderSession::new(settings);
        let result = session.execute_command_list(self, words, memory, &mut resolve_vertex_base);
        let flushed = session.flush(memory);
        match result {
            Err(error) => Err(PicaRenderingRunError::Run(error)),
            Ok(reports) => flushed
                .map(|()| reports)
                .map_err(PicaRenderingRunError::Flush),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{
        ClipDepthRange, FCRAM_BASE, Matrix4, PicaShadedVertex, REG_ATTRIBBUFFER0_OFFSET,
        REG_ATTRIBBUFFERS_FORMAT_HIGH, REG_ATTRIBBUFFERS_FORMAT_LOW, REG_ATTRIBBUFFERS_LOC,
        REG_DRAWARRAYS, REG_NUMVERTICES, REG_SH_OUTMAP_O0, REG_VSH_ATTRIBUTES_PERMUTATION_HIGH,
        REG_VSH_ATTRIBUTES_PERMUTATION_LOW, REG_VSH_CODETRANSFER_DATA0, REG_VSH_CODETRANSFER_INDEX,
        REG_VSH_INPUTBUFFER_CONFIG, REG_VSH_OPDESCS_DATA0, REG_VSH_OPDESCS_INDEX, Viewport,
    };

    const SIZE: usize = 8;
    const CLEAR: [u8; 4] = [0, 0, 0, 255];

    fn viewport() -> Viewport {
        Viewport::new(0.0, 0.0, SIZE as f32, SIZE as f32).unwrap()
    }

    fn cleared_rasterizer() -> SoftwareRasterizer {
        let mut rasterizer = SoftwareRasterizer::new(SIZE, SIZE).unwrap();
        rasterizer.clear(CLEAR, 1.0).unwrap();
        rasterizer
    }

    fn identity_pipeline() -> GeometryPipeline {
        GeometryPipeline::new(
            Matrix4::identity(),
            viewport(),
            ClipDepthRange::NegativeOneToOne,
        )
    }

    /// o0 = position, o1 = color; every other output unused.
    fn position_and_color_map() -> PicaVertexOutputMap {
        PicaVertexOutputMap::from_registers([
            0x0302_0100,
            0x0B0A_0908,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
        ])
        .unwrap()
    }

    fn shaded_vertex(index: u32, position: [f32; 4], color: [f32; 4]) -> PicaShadedVertex {
        let mut outputs = [[0.0; 4]; 16];
        outputs[0] = position;
        outputs[1] = color;
        PicaShadedVertex {
            source_index: index,
            outputs,
        }
    }

    fn lower_left_triangle(color: [f32; 4]) -> PicaShadedDraw {
        PicaShadedDraw {
            vertices: vec![
                shaded_vertex(0, [-1.0, -1.0, 0.0, 1.0], color),
                shaded_vertex(1, [1.0, -1.0, 0.0, 1.0], color),
                shaded_vertex(2, [-1.0, 1.0, 0.0, 1.0], color),
            ],
            triangles: vec![[0, 1, 2]],
            executed_instructions: 6,
        }
    }

    #[test]
    fn shaded_triangle_reaches_the_color_buffer() {
        let mut rasterizer = cleared_rasterizer();
        let report = identity_pipeline()
            .draw_shaded(
                &lower_left_triangle([1.0, 0.0, 0.0, 1.0]),
                &position_and_color_map(),
                [9, 9, 9, 9],
                &mut rasterizer,
                DepthFunction::Less,
            )
            .unwrap();

        assert_eq!(report.triangles_submitted, 1);
        assert_eq!(report.output_triangles, 1);
        assert_eq!(report.executed_instructions, 6);
        assert!(report.pixels_written >= 28, "{report:?}");
        // The triangle covers the lower-left half of the viewport (y grows down).
        assert_eq!(rasterizer.color_at(1, 6), Some([255, 0, 0, 255]));
        assert_eq!(rasterizer.color_at(6, 1), Some(CLEAR));
    }

    #[test]
    fn vertices_without_a_mapped_color_use_the_caller_default() {
        let map = PicaVertexOutputMap::from_registers([
            0x0302_0100,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
            0x1F1F_1F1F,
        ])
        .unwrap();
        let mut rasterizer = cleared_rasterizer();
        identity_pipeline()
            .draw_shaded(
                &lower_left_triangle([1.0, 0.0, 0.0, 1.0]),
                &map,
                [10, 20, 30, 255],
                &mut rasterizer,
                DepthFunction::Less,
            )
            .unwrap();
        assert_eq!(rasterizer.color_at(1, 6), Some([10, 20, 30, 255]));
    }

    #[test]
    fn a_triangle_entirely_outside_the_clip_volume_draws_nothing() {
        let mut shaded = lower_left_triangle([1.0, 0.0, 0.0, 1.0]);
        for vertex in &mut shaded.vertices {
            vertex.outputs[0][0] += 10.0;
        }
        let mut rasterizer = cleared_rasterizer();
        let report = identity_pipeline()
            .draw_shaded(
                &shaded,
                &position_and_color_map(),
                [0; 4],
                &mut rasterizer,
                DepthFunction::Less,
            )
            .unwrap();
        assert_eq!(report.triangles_submitted, 1);
        assert_eq!(report.output_triangles, 0);
        assert_eq!(report.pixels_written, 0);
    }

    #[test]
    fn output_map_errors_stop_before_any_pixel_is_written() {
        // No position mapped.
        let map = PicaVertexOutputMap::from_registers([0x1F1F_1F1F; 7]).unwrap();
        let mut rasterizer = cleared_rasterizer();
        assert_eq!(
            identity_pipeline().draw_shaded(
                &lower_left_triangle([1.0; 4]),
                &map,
                [0; 4],
                &mut rasterizer,
                DepthFunction::Less,
            ),
            Err(PicaRenderError::OutputMap(
                PicaOutputMapError::MissingPosition
            ))
        );
        assert_eq!(rasterizer.color_at(1, 6), Some(CLEAR));
    }

    #[test]
    fn geometry_errors_are_reported() {
        let mut shaded = lower_left_triangle([1.0; 4]);
        shaded.vertices[1].outputs[0][0] = f32::NAN;
        let mut rasterizer = cleared_rasterizer();
        assert_eq!(
            identity_pipeline().draw_shaded(
                &shaded,
                &position_and_color_map(),
                [0; 4],
                &mut rasterizer,
                DepthFunction::Less,
            ),
            Err(PicaRenderError::Geometry(GeometryError::NonFiniteVertex {
                index: 1
            }))
        );
    }

    /// Full path: registers and command-list state to pixels, no hand-built shading.
    #[test]
    fn draw_trigger_renders_pixels_through_the_whole_vertex_path() {
        const BASE: u32 = FCRAM_BASE + 0x1000;
        const MOV_O0_V0: u32 = 0x13 << 26;
        const END: u32 = 0x22 << 26;

        let mut gpu = Pica200::new();
        gpu.write_register(REG_ATTRIBBUFFERS_LOC, 0x0BAD_F000)
            .unwrap();
        // One attribute of unsigned 8-bit scalars. Its component count is the
        // number of array entries that reference it (two here: x and y), so w
        // is filled in as 1 by the shader-input stage.
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_LOW, 0x1)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFERS_FORMAT_HIGH, 0)
            .unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET, 0).unwrap();
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 1, 0).unwrap();
        // Stride 2 bytes; two component entries, both selecting attribute 0 (map 0x00).
        gpu.write_register(REG_ATTRIBBUFFER0_OFFSET + 2, (2 << 16) | (2 << 28))
            .unwrap();
        gpu.write_register(REG_VSH_INPUTBUFFER_CONFIG, 0).unwrap();
        gpu.write_register(REG_VSH_ATTRIBUTES_PERMUTATION_LOW, 0)
            .unwrap();
        gpu.write_register(REG_VSH_ATTRIBUTES_PERMUTATION_HIGH, 0)
            .unwrap();
        gpu.write_register(REG_VSH_OPDESCS_INDEX, 0).unwrap();
        gpu.write_register(
            REG_VSH_OPDESCS_DATA0,
            0b1111 | (0x1B << 5) | (0x1B << 14) | (0x1B << 23),
        )
        .unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_INDEX, 0).unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, MOV_O0_V0)
            .unwrap();
        gpu.write_register(REG_VSH_CODETRANSFER_DATA0, END).unwrap();
        for index in 0..7u16 {
            gpu.write_register(REG_SH_OUTMAP_O0 + index, 0x1F1F_1F1F)
                .unwrap();
        }
        gpu.write_register(REG_SH_OUTMAP_O0, 0x0302_0100).unwrap();

        gpu.write_register(REG_NUMVERTICES, 3).unwrap();
        gpu.write_register(REG_DRAWARRAYS, 1).unwrap();
        let request = gpu.take_draw_requests()[0];

        // Vertices (0,0), (8,0), (0,8) in a 0..8 space.
        let mut memory = GpuPhysicalMemory::new();
        memory.write(BASE, &[0, 0, 8, 0, 0, 8]).unwrap();

        // Scale x, y by 1/4 and shift by -1 so the 0..8 space fills clip space.
        let transform = Matrix4::from_column_major([
            0.25, 0.0, 0.0, 0.0, //
            0.0, 0.25, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            -1.0, -1.0, 0.0, 1.0,
        ])
        .unwrap();
        let pipeline =
            GeometryPipeline::new(transform, viewport(), ClipDepthRange::NegativeOneToOne);

        let program = gpu.vertex_shader_program();
        let mut rasterizer = cleared_rasterizer();
        let report = gpu
            .render_draw_request(
                &request,
                &memory,
                BASE,
                &program,
                &pipeline,
                [200, 100, 50, 255],
                &mut rasterizer,
                DepthFunction::Less,
            )
            .unwrap();

        assert_eq!(report.triangles_submitted, 1);
        assert_eq!(report.executed_instructions, 6);
        assert!(report.pixels_written >= 28, "{report:?}");
        assert_eq!(rasterizer.color_at(1, 6), Some([200, 100, 50, 255]));
        assert_eq!(rasterizer.color_at(6, 1), Some(CLEAR));
    }

    // ---- render session / command-list integration ----

    const VERTEX_BASE: u32 = FCRAM_BASE + 0x1000;
    const TARGET_A: u32 = FCRAM_BASE + 0x2000;
    const TARGET_B: u32 = FCRAM_BASE + 0x4000;

    fn session_settings() -> PicaRenderSettings {
        let transform = Matrix4::from_column_major([
            0.25, 0.0, 0.0, 0.0, //
            0.0, 0.25, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            -1.0, -1.0, 0.0, 1.0,
        ])
        .unwrap();
        PicaRenderSettings {
            pipeline: GeometryPipeline::new(
                transform,
                viewport(),
                ClipDepthRange::NegativeOneToOne,
            ),
            default_color: [200, 100, 50, 255],
            depth_function: DepthFunction::Less,
        }
    }

    fn configure_vertex_path(gpu: &mut Pica200) {
        const MOV_O0_V0: u32 = 0x13 << 26;
        const END: u32 = 0x22 << 26;
        for (register, value) in [
            (REG_ATTRIBBUFFERS_LOC, 0x0BAD_F000),
            (REG_ATTRIBBUFFERS_FORMAT_LOW, 0x1),
            (REG_ATTRIBBUFFERS_FORMAT_HIGH, 0),
            (REG_ATTRIBBUFFER0_OFFSET, 0),
            (REG_ATTRIBBUFFER0_OFFSET + 1, 0),
            (REG_ATTRIBBUFFER0_OFFSET + 2, (2 << 16) | (2 << 28)),
            (REG_VSH_INPUTBUFFER_CONFIG, 0),
            (REG_VSH_ATTRIBUTES_PERMUTATION_LOW, 0),
            (REG_VSH_ATTRIBUTES_PERMUTATION_HIGH, 0),
            (REG_VSH_OPDESCS_INDEX, 0),
            (
                REG_VSH_OPDESCS_DATA0,
                0b1111 | (0x1B << 5) | (0x1B << 14) | (0x1B << 23),
            ),
            (REG_VSH_CODETRANSFER_INDEX, 0),
            (REG_VSH_CODETRANSFER_DATA0, MOV_O0_V0),
            (REG_VSH_CODETRANSFER_DATA0, END),
        ] {
            gpu.write_register(register, value).unwrap();
        }
        for index in 0..7u16 {
            gpu.write_register(REG_SH_OUTMAP_O0 + index, 0x1F1F_1F1F)
                .unwrap();
        }
        gpu.write_register(REG_SH_OUTMAP_O0, 0x0302_0100).unwrap();
        gpu.write_register(REG_NUMVERTICES, 3).unwrap();
    }

    fn point_target(gpu: &mut Pica200, address: u32) {
        gpu.write_register(crate::gpu::REG_COLORBUFFER_LOC, address >> 3)
            .unwrap();
        gpu.write_register(crate::gpu::REG_COLORBUFFER_FORMAT, 2)
            .unwrap();
        gpu.write_register(crate::gpu::REG_FRAMEBUFFER_BLOCK32, 0)
            .unwrap();
        gpu.write_register(
            crate::gpu::REG_FRAMEBUFFER_DIM,
            (((SIZE as u32) - 1) << 12) | SIZE as u32,
        )
        .unwrap();
    }

    /// Command list: one param+header write of DRAWARRAYS (reg 0x22E, mask 0xF).
    const DRAW_LIST: [u32; 2] = [1, 0x000F_022E];

    fn target_pixel(memory: &GpuPhysicalMemory, address: u32, x: usize, y: usize) -> [u8; 4] {
        let config = PicaRenderTargetConfig::from_registers(
            address >> 3,
            2,
            0,
            (((SIZE as u32) - 1) << 12) | SIZE as u32,
        )
        .unwrap();
        let pixels = config.read_rgba8(memory).unwrap();
        let offset = (y * SIZE + x) * 4;
        [
            pixels[offset],
            pixels[offset + 1],
            pixels[offset + 2],
            pixels[offset + 3],
        ]
    }

    fn memory_with_triangle() -> GpuPhysicalMemory {
        let mut memory = GpuPhysicalMemory::new();
        memory.write(VERTEX_BASE, &[0, 0, 8, 0, 0, 8]).unwrap();
        memory
    }

    #[test]
    fn command_list_draw_lands_in_the_tiled_target() {
        let mut gpu = Pica200::new();
        configure_vertex_path(&mut gpu);
        point_target(&mut gpu, TARGET_A);
        let mut memory = memory_with_triangle();

        let (list, render) = gpu
            .execute_command_list_rendering(&DRAW_LIST, &mut memory, session_settings(), |_, _| {
                Some(VERTEX_BASE)
            })
            .unwrap();

        assert_eq!(list.commands, 1);
        assert_eq!(render.triangles_submitted, 1);
        assert!(render.pixels_written >= 28);
        assert_eq!(target_pixel(&memory, TARGET_A, 1, 6), [200, 100, 50, 255]);
        assert_eq!(target_pixel(&memory, TARGET_A, 6, 1), [0, 0, 0, 0]);
        // Handled draws are not also queued for later.
        assert!(gpu.take_draw_requests().is_empty());
        assert_eq!(gpu.draw_requests_triggered(), 1);
    }

    #[test]
    fn persistent_render_session_preserves_depth_between_command_lists() {
        let mut gpu = Pica200::new();
        configure_vertex_path(&mut gpu);
        point_target(&mut gpu, TARGET_A);
        let mut memory = memory_with_triangle();
        let mut session = PicaRenderSession::new(session_settings());

        let (first_list, first_render) = session
            .execute_command_list(&mut gpu, &DRAW_LIST, &mut memory, |_, _| {
                Some(VERTEX_BASE)
            })
            .unwrap();
        assert_eq!(first_list.commands, 1);
        assert_eq!(first_render.triangles_submitted, 1);
        assert!(first_render.pixels_written > 0);
        session.flush(&mut memory).unwrap();

        let (second_list, second_render) = session
            .execute_command_list(&mut gpu, &DRAW_LIST, &mut memory, |_, _| {
                Some(VERTEX_BASE)
            })
            .unwrap();
        assert_eq!(second_list.commands, 1);
        assert_eq!(second_render.triangles_submitted, 1);
        assert_eq!(second_render.pixels_written, 0);
        assert!(second_render.depth_rejected_pixels > 0);
        assert_eq!(target_pixel(&memory, TARGET_A, 1, 6), [200, 100, 50, 255]);
    }

    #[test]
    fn render_session_scales_viewport_to_the_active_target_dimensions() {
        const WIDE_TARGET: u32 = FCRAM_BASE + 0x60_0000;
        const WIDTH: usize = 16;
        const HEIGHT: usize = 8;

        let mut gpu = Pica200::new();
        configure_vertex_path(&mut gpu);
        point_target(&mut gpu, WIDE_TARGET);
        gpu.write_register(
            crate::gpu::REG_FRAMEBUFFER_DIM,
            (((HEIGHT as u32) - 1) << 12) | WIDTH as u32,
        )
        .unwrap();

        let mut memory = memory_with_triangle();
        let mut session = PicaRenderSession::new(session_settings());
        let (_, report) = session
            .execute_command_list(&mut gpu, &DRAW_LIST, &mut memory, |_, _| {
                Some(VERTEX_BASE)
            })
            .unwrap();
        assert!(report.pixels_written > 0);
        session.flush(&mut memory).unwrap();

        let target = PicaRenderTargetConfig::from_registers(
            WIDE_TARGET >> 3,
            2,
            0,
            (((HEIGHT as u32) - 1) << 12) | WIDTH as u32,
        )
        .unwrap();
        let pixels = target.read_rgba8(&memory).unwrap();
        let pixel = |x: usize, y: usize| {
            let offset = (y * WIDTH + x) * 4;
            [
                pixels[offset],
                pixels[offset + 1],
                pixels[offset + 2],
                pixels[offset + 3],
            ]
        };
        assert_eq!(pixel(9, 6), [200, 100, 50, 255]);
        assert_eq!(pixel(13, 1), [0, 0, 0, 255]);
    }

    #[test]
    fn existing_target_content_is_preserved_by_a_later_list() {
        let mut gpu = Pica200::new();
        configure_vertex_path(&mut gpu);
        point_target(&mut gpu, TARGET_A);
        let mut memory = memory_with_triangle();
        let settings = session_settings();
        gpu.execute_command_list_rendering(&DRAW_LIST, &mut memory, settings, |_, _| {
            Some(VERTEX_BASE)
        })
        .unwrap();
        let mut second = settings;
        second.default_color = [1, 2, 3, 255];
        // Mirror triangle on the upper right: only the first one's pixels exist so far.
        memory.write(VERTEX_BASE, &[8, 8, 0, 8, 8, 0]).unwrap();
        gpu.execute_command_list_rendering(&DRAW_LIST, &mut memory, second, |_, _| {
            Some(VERTEX_BASE)
        })
        .unwrap();
        assert_eq!(target_pixel(&memory, TARGET_A, 1, 6), [200, 100, 50, 255]);
        assert_eq!(target_pixel(&memory, TARGET_A, 6, 1), [1, 2, 3, 255]);
    }

    #[test]
    fn a_list_that_changes_the_target_between_draws_renders_each_draw_correctly() {
        let mut gpu = Pica200::new();
        configure_vertex_path(&mut gpu);
        point_target(&mut gpu, TARGET_A);
        let mut memory = memory_with_triangle();
        // draw, then COLORBUFFER_LOC := TARGET_B>>3, then draw again.
        let list = [
            1,
            0x000F_022E,
            TARGET_B >> 3,
            0x000F_0000 | u32::from(crate::gpu::REG_COLORBUFFER_LOC),
            1,
            0x000F_022E,
        ];
        let (report, render) = gpu
            .execute_command_list_rendering(&list, &mut memory, session_settings(), |_, _| {
                Some(VERTEX_BASE)
            })
            .unwrap();
        assert_eq!(report.commands, 3);
        assert_eq!(render.triangles_submitted, 2);
        assert_eq!(target_pixel(&memory, TARGET_A, 1, 6), [200, 100, 50, 255]);
        assert_eq!(target_pixel(&memory, TARGET_B, 1, 6), [200, 100, 50, 255]);
    }

    #[test]
    fn an_unresolved_vertex_base_aborts_without_counting_the_list() {
        let mut gpu = Pica200::new();
        configure_vertex_path(&mut gpu);
        point_target(&mut gpu, TARGET_A);
        let mut memory = memory_with_triangle();
        let before = gpu.command_lists_submitted();
        let error = gpu
            .execute_command_list_rendering(&DRAW_LIST, &mut memory, session_settings(), |_, _| {
                None
            })
            .unwrap_err();
        assert_eq!(
            error,
            PicaRenderingRunError::Run(PicaCommandListRunError::Draw(
                PicaRenderSessionError::UnresolvedVertexBase
            ))
        );
        assert_eq!(gpu.command_lists_submitted(), before);
    }

    #[test]
    fn a_malformed_list_reports_a_command_error() {
        let mut gpu = Pica200::new();
        let mut memory = GpuPhysicalMemory::new();
        let error = gpu
            .execute_command_list_rendering(&[1], &mut memory, session_settings(), |_, _| None)
            .unwrap_err();
        assert!(matches!(
            error,
            PicaRenderingRunError::Run(PicaCommandListRunError::Command(_))
        ));
    }

    #[test]
    fn plain_execute_command_list_still_queues_draws() {
        let mut gpu = Pica200::new();
        gpu.execute_command_list(&DRAW_LIST).unwrap();
        assert_eq!(gpu.take_draw_requests().len(), 1);
    }

    #[test]
    fn load_color_rgba8_rejects_a_wrong_length() {
        let mut rasterizer = cleared_rasterizer();
        assert!(rasterizer.load_color_rgba8(&[0; 3]).is_err());
        rasterizer
            .load_color_rgba8(&vec![7; SIZE * SIZE * 4])
            .unwrap();
        assert_eq!(rasterizer.color_at(0, 0), Some([7; 4]));
    }

    /// End-to-end output path: PICA200 draw -> tiled color target ->
    /// GSP DisplayTransfer -> linear PDC buffers -> all three LCD surfaces.
    #[test]
    fn pica_draw_reaches_top_stereo_and_bottom_lcd_surfaces() {
        use crate::gpu::{
            DisplayTransferRequest, REG_COLORBUFFER_FORMAT, REG_COLORBUFFER_LOC,
            REG_FRAMEBUFFER_BLOCK32, REG_FRAMEBUFFER_DIM, execute_display_transfer,
        };
        use crate::video::{
            BOTTOM_SCREEN_HEIGHT, BOTTOM_SCREEN_WIDTH, DualScreenDisplay, LcdController, LcdPanel,
            PDC_CONTROL, PDC_FRAMEBUFFER_A_FIRST, PDC_FRAMEBUFFER_B_FIRST, PDC_FRAMEBUFFER_FORMAT,
            PDC_FRAMEBUFFER_STRIDE, TOP_SCREEN_HEIGHT, TOP_SCREEN_WIDTH,
        };

        const TOP_TILED: u32 = FCRAM_BASE + 0x20_0000;
        const BOTTOM_TILED: u32 = FCRAM_BASE + 0x30_0000;
        const TOP_LINEAR: u32 = FCRAM_BASE + 0x40_0000;
        const BOTTOM_LINEAR: u32 = FCRAM_BASE + 0x50_0000;

        fn settings_for(width: usize, height: usize) -> PicaRenderSettings {
            let transform = Matrix4::from_column_major([
                0.25, 0.0, 0.0, 0.0, 0.0, 0.25, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0,
            ])
            .unwrap();
            PicaRenderSettings {
                pipeline: GeometryPipeline::new(
                    transform,
                    Viewport::new(0.0, 0.0, width as f32, height as f32).unwrap(),
                    ClipDepthRange::NegativeOneToOne,
                ),
                default_color: [200, 100, 50, 255],
                depth_function: DepthFunction::Less,
            }
        }

        fn configure_target(gpu: &mut Pica200, address: u32, width: usize, height: usize) {
            gpu.write_register(REG_COLORBUFFER_LOC, address >> 3)
                .unwrap();
            gpu.write_register(REG_COLORBUFFER_FORMAT, 2).unwrap();
            gpu.write_register(REG_FRAMEBUFFER_BLOCK32, 0).unwrap();
            gpu.write_register(
                REG_FRAMEBUFFER_DIM,
                (((height as u32) - 1) << 12) | width as u32,
            )
            .unwrap();
        }

        fn transfer(
            memory: &mut GpuPhysicalMemory,
            source: u32,
            destination: u32,
            width: usize,
            height: usize,
        ) {
            execute_display_transfer(
                memory,
                DisplayTransferRequest {
                    input_address: source,
                    output_address: destination,
                    input_width: width as u16,
                    input_height: height as u16,
                    output_width: width as u16,
                    output_height: height as u16,
                    flags: 0, // tiled RGBA8 to linear RGBA8
                },
            )
            .unwrap();
        }

        let mut gpu = Pica200::new();
        configure_vertex_path(&mut gpu);
        let mut memory = memory_with_triangle();

        configure_target(&mut gpu, TOP_TILED, TOP_SCREEN_WIDTH, TOP_SCREEN_HEIGHT);
        gpu.execute_command_list_rendering(
            &DRAW_LIST,
            &mut memory,
            settings_for(TOP_SCREEN_WIDTH, TOP_SCREEN_HEIGHT),
            |_, _| Some(VERTEX_BASE),
        )
        .unwrap();

        configure_target(
            &mut gpu,
            BOTTOM_TILED,
            BOTTOM_SCREEN_WIDTH,
            BOTTOM_SCREEN_HEIGHT,
        );
        gpu.execute_command_list_rendering(
            &DRAW_LIST,
            &mut memory,
            settings_for(BOTTOM_SCREEN_WIDTH, BOTTOM_SCREEN_HEIGHT),
            |_, _| Some(VERTEX_BASE),
        )
        .unwrap();

        transfer(
            &mut memory,
            TOP_TILED,
            TOP_LINEAR,
            TOP_SCREEN_WIDTH,
            TOP_SCREEN_HEIGHT,
        );
        transfer(
            &mut memory,
            BOTTOM_TILED,
            BOTTOM_LINEAR,
            BOTTOM_SCREEN_WIDTH,
            BOTTOM_SCREEN_HEIGHT,
        );

        let mut lcd = LcdController::new();
        for (register, value) in [
            (PDC_FRAMEBUFFER_A_FIRST, TOP_LINEAR),
            (PDC_FRAMEBUFFER_B_FIRST, TOP_LINEAR),
            (PDC_FRAMEBUFFER_FORMAT, 0),
            (PDC_FRAMEBUFFER_STRIDE, (TOP_SCREEN_WIDTH * 4) as u32),
            (PDC_CONTROL, 0x1_0001),
        ] {
            lcd.write_register(LcdPanel::Top, register, value).unwrap();
        }
        for (register, value) in [
            (PDC_FRAMEBUFFER_A_FIRST, BOTTOM_LINEAR),
            (PDC_FRAMEBUFFER_FORMAT, 0),
            (PDC_FRAMEBUFFER_STRIDE, (BOTTOM_SCREEN_WIDTH * 4) as u32),
            (PDC_CONTROL, 0x1_0001),
        ] {
            lcd.write_register(LcdPanel::Bottom, register, value)
                .unwrap();
        }

        let mut display = DualScreenDisplay::new().unwrap();
        let report = lcd.scanout(&memory, &mut display).unwrap();
        assert_eq!(report.screens_updated, 3);

        for (name, pixels) in [
            ("top-left", display.top_left().pixels()),
            ("top-right", display.top_right().pixels()),
            ("bottom", display.bottom().pixels()),
        ] {
            let width = if name == "bottom" {
                BOTTOM_SCREEN_WIDTH
            } else {
                TOP_SCREEN_WIDTH
            };
            let inside = (180 * width + 40) * 4;
            assert_eq!(&pixels[inside..inside + 4], &[200, 100, 50, 255], "{name}");
        }
    }
}
