//! Persistent PICA200 fragment-stage render session.
//!
//! This backend is selected explicitly by the System fragment-rendering
//! switch. It consumes the same guest draw triggers and physical vertex-base
//! resolver as the reference renderer, but routes each draw through PICA
//! clipping, perspective-correct varyings, texture fetch, TEV, alpha/depth
//! tests, blending and the existing tiled render-target codec.

use super::{
    DepthFunction, GpuPhysicalMemory, Matrix4, Pica200, PicaCommandListReport,
    PicaCommandListRunError, PicaDepthBufferConfig, PicaDepthBufferError, PicaDepthBufferFormat,
    PicaDepthBuffering, PicaDepthMap, PicaDepthMode, PicaDrawExecError, PicaDrawRequest,
    PicaFragmentPipelineError, PicaFragmentRasterError, PicaFragmentRasterReport,
    PicaFragmentRasterState, PicaFragmentRasterizer, PicaFragmentVertexDefaults,
    PicaOutputMapError, PicaRenderReport, PicaRenderSettings, PicaRenderTargetConfig,
    PicaRenderTargetError, REG_COLORBUFFER_WRITE, REG_DEPTH_COLOR_MASK, REG_DEPTHBUFFER_WRITE,
    Viewport,
};

/// Failure while drawing through the PICA fragment-stage backend.
#[derive(Clone, Debug, PartialEq)]
pub enum PicaFragmentRenderSessionError {
    Target(PicaRenderTargetError),
    Rasterizer(PicaFragmentRasterError),
    Draw(PicaDrawExecError),
    OutputMap(PicaOutputMapError),
    Pipeline(PicaFragmentPipelineError),
    DepthBuffer(PicaDepthBufferError),
    StencilBufferRequired,
    StencilReadDisabled,
    UnresolvedVertexBase,
}

impl std::fmt::Display for PicaFragmentRenderSessionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Target(error) => write!(formatter, "fragment render target failed: {error}"),
            Self::Rasterizer(error) => write!(formatter, "fragment rasterizer failed: {error}"),
            Self::Draw(error) => write!(formatter, "vertex draw failed: {error}"),
            Self::OutputMap(error) => write!(formatter, "vertex output map failed: {error}"),
            Self::Pipeline(error) => write!(formatter, "fragment pipeline failed: {error}"),
            Self::DepthBuffer(error) => write!(formatter, "physical depth buffer failed: {error}"),
            Self::StencilBufferRequired => {
                formatter.write_str("PICA stencil testing requires a physical D24S8 depth buffer")
            }
            Self::StencilReadDisabled => {
                formatter.write_str("PICA stencil testing requires stencil-buffer read access")
            }
            Self::UnresolvedVertexBase => {
                formatter.write_str("vertex base address was not resolved")
            }
        }
    }
}

impl std::error::Error for PicaFragmentRenderSessionError {}

/// Keeps PICA fragment color/depth state across guest GSP command lists.
#[derive(Debug)]
pub struct PicaFragmentRenderSession {
    settings: PicaRenderSettings,
    active: Option<(
        PicaRenderTargetConfig,
        Option<PicaDepthBufferConfig>,
        PicaFragmentRasterizer,
    )>,
    dirty: bool,
    reload_on_next_draw: bool,
    totals: PicaRenderReport,
    draws_rendered: u64,
}

impl PicaFragmentRenderSession {
    pub fn new(settings: PicaRenderSettings) -> Self {
        Self {
            settings,
            active: None,
            dirty: false,
            reload_on_next_draw: true,
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

    /// Executes one guest draw using the live PICA register state.
    pub fn draw(
        &mut self,
        pica: &Pica200,
        request: &PicaDrawRequest,
        memory: &mut GpuPhysicalMemory,
        vertex_base: Option<u32>,
    ) -> Result<PicaRenderReport, PicaFragmentRenderSessionError> {
        let vertex_base =
            vertex_base.ok_or(PicaFragmentRenderSessionError::UnresolvedVertexBase)?;
        let config = pica
            .render_target_config()
            .map_err(PicaFragmentRenderSessionError::Target)?;
        let depth_config = PicaDepthBufferConfig::from_pica(pica, config.width, config.height)
            .map_err(PicaFragmentRenderSessionError::DepthBuffer)?;
        let target_changed = self.select_target(config, depth_config, memory)?;
        if self.reload_on_next_draw {
            if !target_changed {
                let rasterizer = Self::load_target(config, depth_config, memory)?;
                self.active = Some((config, depth_config, rasterizer));
            }
            self.reload_on_next_draw = false;
        }

        let guest_viewport = pica.viewport_state();
        let viewport_width = guest_viewport.half_width * 2.0;
        let viewport_height = guest_viewport.half_height * 2.0;
        // The guest framebuffer bit mirrors the viewport vertically. The
        // software target is top-left, so convert the guest rectangle and
        // apply the optional second mirror before creating the viewport.
        let flip_viewport = config.vertical_flip_enabled;
        let viewport_top = pica_viewport_top(
            config.height as f32,
            guest_viewport.origin_y,
            viewport_height,
            flip_viewport,
        );
        let viewport = Viewport::new(
            guest_viewport.origin_x as f32,
            viewport_top,
            viewport_width,
            viewport_height,
        )
        .map_err(|_| {
            PicaFragmentRenderSessionError::Rasterizer(PicaFragmentRasterError::InvalidViewport)
        })?;

        let guest_depth = pica.depth_state();
        let depth_map = PicaDepthMap {
            scale: guest_depth.scale,
            offset: guest_depth.offset,
            mode: match guest_depth.buffering {
                PicaDepthBuffering::WBuffer => PicaDepthMode::WBuffer,
                PicaDepthBuffering::ZBuffer => PicaDepthMode::ZBuffer,
            },
        };
        let depth_test = pica.depth_test_state();
        let depth_function = if depth_test.enabled {
            depth_test.function
        } else {
            DepthFunction::Always
        };

        // Decode a snapshot at the draw trigger. Later command-list register
        // writes must not retroactively change this draw.
        let fragment_pipeline = pica
            .fragment_pipeline()
            .map_err(PicaFragmentRenderSessionError::Pipeline)?;
        let output_map = pica
            .vertex_output_map()
            .map_err(PicaFragmentRenderSessionError::OutputMap)?;
        let stencil_state = fragment_pipeline.stencil_state();
        if stencil_state.enabled {
            let depth = depth_config
                .filter(|depth| depth.format == PicaDepthBufferFormat::D24S8)
                .ok_or(PicaFragmentRenderSessionError::StencilBufferRequired)?;
            if !depth.stencil_read_enabled {
                return Err(PicaFragmentRenderSessionError::StencilReadDisabled);
            }
        }
        let vertex_program = pica.vertex_shader_program();
        let shaded = pica
            .shade_draw_at_registered_entrypoint(request, memory, vertex_base, &vertex_program)
            .map_err(PicaFragmentRenderSessionError::Draw)?;

        let defaults = PicaFragmentVertexDefaults {
            primary_color: self
                .settings
                .default_color
                .map(|channel| f32::from(channel) / 255.0),
            texture_coordinates: [[0.0; 2]; 3],
        };
        let depth_color_mask = pica.register(REG_DEPTH_COLOR_MASK).unwrap_or(0);
        let color_buffer_write = pica.register(REG_COLORBUFFER_WRITE).unwrap_or(0) & 0xF;
        let depth_buffer_write = pica.register(REG_DEPTHBUFFER_WRITE).unwrap_or(0);
        let color_write_mask = [
            depth_color_mask & (1 << 8) != 0 && color_buffer_write & (1 << 0) != 0,
            depth_color_mask & (1 << 9) != 0 && color_buffer_write & (1 << 1) != 0,
            depth_color_mask & (1 << 10) != 0 && color_buffer_write & (1 << 2) != 0,
            depth_color_mask & (1 << 11) != 0 && color_buffer_write & (1 << 3) != 0,
        ];
        let depth_write_enabled = depth_color_mask & (1 << 12) != 0
            && depth_buffer_write & (1 << 1) != 0
            && depth_config.is_some_and(|depth| depth.depth_write_enabled);

        let raster_state = PicaFragmentRasterState {
            transform: Matrix4::identity(),
            viewport,
            depth_function,
            color_write_mask,
            depth_write_enabled,
            stencil_write_enabled: depth_config.is_some_and(|depth| depth.stencil_write_enabled),
            depth_format: depth_config.map(|depth| depth.format),
            depth_map: Some(depth_map),
            minifying: [false; 3],
            cull_mode: pica.cull_mode(),
            flip_viewport,
            scissor: pica.scissor_state(),
        };

        let rasterizer = &mut self
            .active
            .as_mut()
            .expect("render target was selected above")
            .2;
        // A partial draw that later fails is still committed on flush.
        self.dirty = true;
        let fragment_report = rasterizer
            .draw_shaded(
                &shaded,
                &output_map,
                defaults,
                &fragment_pipeline,
                memory,
                raster_state,
            )
            .map_err(PicaFragmentRenderSessionError::Rasterizer)?;

        let report = render_report(fragment_report);
        self.draws_rendered = self.draws_rendered.saturating_add(1);
        self.totals.triangles_submitted = self
            .totals
            .triangles_submitted
            .saturating_add(report.triangles_submitted);
        self.totals.output_triangles = self
            .totals
            .output_triangles
            .saturating_add(report.output_triangles);
        self.totals.pixels_written = self
            .totals
            .pixels_written
            .saturating_add(report.pixels_written);
        self.totals.alpha_rejected_pixels = self
            .totals
            .alpha_rejected_pixels
            .saturating_add(report.alpha_rejected_pixels);
        self.totals.depth_rejected_pixels = self
            .totals
            .depth_rejected_pixels
            .saturating_add(report.depth_rejected_pixels);
        self.totals.stencil_rejected_pixels = self
            .totals
            .stencil_rejected_pixels
            .saturating_add(report.stencil_rejected_pixels);
        self.totals.executed_instructions = self
            .totals
            .executed_instructions
            .saturating_add(report.executed_instructions);
        Ok(report)
    }

    /// Executes a complete guest command list, rendering each draw at its trigger.
    pub fn execute_command_list(
        &mut self,
        pica: &mut Pica200,
        words: &[u32],
        memory: &mut GpuPhysicalMemory,
        mut resolve_vertex_base: impl FnMut(&Pica200, &PicaDrawRequest) -> Option<u32>,
    ) -> Result<
        (PicaCommandListReport, PicaRenderReport),
        PicaCommandListRunError<PicaFragmentRenderSessionError>,
    > {
        self.flush(memory).map_err(PicaCommandListRunError::Draw)?;
        self.reload_on_next_draw = true;
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
            alpha_rejected_pixels: after
                .alpha_rejected_pixels
                .saturating_sub(before.alpha_rejected_pixels),
            depth_rejected_pixels: after
                .depth_rejected_pixels
                .saturating_sub(before.depth_rejected_pixels),
            stencil_rejected_pixels: after
                .stencil_rejected_pixels
                .saturating_sub(before.stencil_rejected_pixels),
            executed_instructions: after
                .executed_instructions
                .saturating_sub(before.executed_instructions),
        };
        Ok((command_report, render_delta))
    }

    /// Publishes the current tiled color target to physical GPU memory.
    pub fn flush(
        &mut self,
        memory: &mut GpuPhysicalMemory,
    ) -> Result<(), PicaFragmentRenderSessionError> {
        if self.dirty {
            if let Some((config, depth_config, rasterizer)) = self.active.as_mut() {
                rasterizer
                    .write_render_target(*config, memory)
                    .map_err(PicaFragmentRenderSessionError::Rasterizer)?;
                if let Some(depth_config) = depth_config.as_ref() {
                    rasterizer
                        .write_depth_buffer(*depth_config, memory)
                        .map_err(PicaFragmentRenderSessionError::Rasterizer)?;
                }
            }
            self.dirty = false;
        }
        Ok(())
    }

    pub fn finish(
        mut self,
        memory: &mut GpuPhysicalMemory,
    ) -> Result<PicaRenderReport, PicaFragmentRenderSessionError> {
        self.flush(memory)?;
        Ok(self.totals)
    }

    fn select_target(
        &mut self,
        config: PicaRenderTargetConfig,
        depth_config: Option<PicaDepthBufferConfig>,
        memory: &mut GpuPhysicalMemory,
    ) -> Result<bool, PicaFragmentRenderSessionError> {
        if matches!(
            &self.active,
            Some((current, current_depth, _))
                if *current == config && *current_depth == depth_config
        ) {
            return Ok(false);
        }
        // Publish the previous target before switching to a new one.
        self.flush(memory)?;
        let rasterizer = Self::load_target(config, depth_config, memory)?;
        self.active = Some((config, depth_config, rasterizer));
        Ok(true)
    }

    fn load_target(
        config: PicaRenderTargetConfig,
        depth_config: Option<PicaDepthBufferConfig>,
        memory: &GpuPhysicalMemory,
    ) -> Result<PicaFragmentRasterizer, PicaFragmentRenderSessionError> {
        let mut rasterizer = PicaFragmentRasterizer::from_render_target(config, memory, 1.0)
            .map_err(PicaFragmentRenderSessionError::Rasterizer)?;
        if let Some(depth_config) = depth_config {
            rasterizer
                .load_depth_buffer(depth_config, memory, 1.0)
                .map_err(PicaFragmentRenderSessionError::Rasterizer)?;
        }
        Ok(rasterizer)
    }
}

fn pica_viewport_top(height: f32, origin_y: i32, viewport_height: f32, flip: bool) -> f32 {
    if flip {
        origin_y as f32
    } else {
        height - (origin_y as f32 + viewport_height)
    }
}

fn render_report(report: PicaFragmentRasterReport) -> PicaRenderReport {
    PicaRenderReport {
        triangles_submitted: report.input_triangles,
        output_triangles: report.clipped_triangles,
        pixels_written: report.shaded_pixels,
        alpha_rejected_pixels: report.alpha_rejected_pixels,
        depth_rejected_pixels: report.depth_rejected_pixels,
        stencil_rejected_pixels: report.stencil_rejected_pixels,
        executed_instructions: report.executed_instructions,
    }
}

#[cfg(test)]
mod orientation_tests {
    use super::pica_viewport_top;

    #[test]
    fn framebuffer_flip_mirrors_the_guest_viewport_rectangle() {
        // H=240, guest Y=30 and height=100. The ordinary bottom-left to
        // top-left conversion places it at 110; bit-24 flip mirrors it to 30.
        assert_eq!(pica_viewport_top(240.0, 30, 100.0, false), 110.0);
        assert_eq!(pica_viewport_top(240.0, 30, 100.0, true), 30.0);
    }

    #[test]
    fn framebuffer_flip_preserves_negative_guest_viewport_origins() {
        assert_eq!(pica_viewport_top(240.0, -12, 100.0, false), 152.0);
        assert_eq!(pica_viewport_top(240.0, -12, 100.0, true), -12.0);
    }
}

