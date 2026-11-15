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
    GpuPhysicalMemory, Pica200, PicaDrawExecError, PicaDrawRequest, PicaOutputMapError,
    PicaShadedDraw, PicaVertexOutputMap, PicaVertexShaderProgram, SoftwareRasterizer,
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
}
