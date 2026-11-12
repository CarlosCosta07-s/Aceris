//! Deterministic software triangle rasterizer for graphics-pipeline bring-up.
//!
//! This module accepts vertices that have already passed vertex processing and
//! clipping. It is a reusable reference stage for the future PICA200 pipeline;
//! it does not claim to emulate PICA200 shaders, texture units, blending, or
//! hardware-specific rasterization rules.

/// A screen-space vertex after vertex transformation and clipping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RasterVertex {
    /// Horizontal position in pixel coordinates.
    pub x: f32,
    /// Vertical position in pixel coordinates.
    pub y: f32,
    /// Normalized depth in the inclusive range 0.0..=1.0.
    pub depth: f32,
    /// Interpolated RGBA8 vertex color.
    pub color: [u8; 4],
    /// Reciprocal clip-space W used for perspective-correct varyings.
    /// The direct rasterizer constructor defaults this to 1.0.
    pub reciprocal_w: f32,
}

impl RasterVertex {
    pub const fn new(x: f32, y: f32, depth: f32, color: [u8; 4]) -> Self {
        Self {
            x,
            y,
            depth,
            color,
            reciprocal_w: 1.0,
        }
    }

    /// Sets reciprocal clip-space W for perspective-correct color interpolation.
    pub const fn with_reciprocal_w(mut self, reciprocal_w: f32) -> Self {
        self.reciprocal_w = reciprocal_w;
        self
    }
}

/// Depth comparison used before committing a covered fragment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DepthFunction {
    Never,
    #[default]
    Less,
    LessEqual,
    Always,
}

/// Counts the fragments considered while drawing one triangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RasterizationReport {
    /// Pixel centers inside the triangle after the top-left edge rule.
    pub covered_pixels: u64,
    /// Covered pixels rejected by the depth comparison.
    pub depth_rejected_pixels: u64,
    /// Fragments written to the color and depth buffers.
    pub pixels_written: u64,
}

/// Failure while allocating a target or validating a rasterization request.
#[derive(Clone, Debug, PartialEq)]
pub enum RasterizerError {
    InvalidDimensions { width: usize, height: usize },
    AllocationFailed,
    InvalidClearDepth { depth: f32 },
    InvalidVertex { index: usize },
    InvalidVertexDepth { index: usize, depth: f32 },
    InvalidReciprocalW { index: usize, value: f32 },
}

impl std::fmt::Display for RasterizerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidDimensions { width, height } => {
                write!(
                    formatter,
                    "invalid rasterizer target dimensions {width}x{height}"
                )
            }
            Self::AllocationFailed => formatter.write_str("could not allocate rasterizer buffers"),
            Self::InvalidClearDepth { depth } => {
                write!(formatter, "invalid depth clear value {depth}")
            }
            Self::InvalidVertex { index } => {
                write!(
                    formatter,
                    "raster vertex {index} contains a non-finite coordinate"
                )
            }
            Self::InvalidVertexDepth { index, depth } => {
                write!(
                    formatter,
                    "raster vertex {index} has depth {depth} outside 0.0..=1.0"
                )
            }
            Self::InvalidReciprocalW { index, value } => {
                write!(
                    formatter,
                    "raster vertex {index} has invalid reciprocal W {value}"
                )
            }
        }
    }
}

impl std::error::Error for RasterizerError {}

/// A small deterministic RGBA8 + depth target for transformed triangles.
#[derive(Clone, Debug)]
pub struct SoftwareRasterizer {
    width: usize,
    height: usize,
    color: Vec<u8>,
    depth: Vec<f32>,
}

impl SoftwareRasterizer {
    /// Creates an opaque-black target with its depth buffer cleared to 1.0.
    pub fn new(width: usize, height: usize) -> Result<Self, RasterizerError> {
        if width == 0 || height == 0 {
            return Err(RasterizerError::InvalidDimensions { width, height });
        }

        let pixel_count = width
            .checked_mul(height)
            .ok_or(RasterizerError::InvalidDimensions { width, height })?;
        let color_len = pixel_count
            .checked_mul(4)
            .ok_or(RasterizerError::InvalidDimensions { width, height })?;

        let mut color = Vec::new();
        color
            .try_reserve_exact(color_len)
            .map_err(|_| RasterizerError::AllocationFailed)?;
        color.resize(color_len, 0);
        for alpha in color.iter_mut().skip(3).step_by(4) {
            *alpha = 0xFF;
        }

        let mut depth = Vec::new();
        depth
            .try_reserve_exact(pixel_count)
            .map_err(|_| RasterizerError::AllocationFailed)?;
        depth.resize(pixel_count, 1.0);

        Ok(Self {
            width,
            height,
            color,
            depth,
        })
    }

    pub const fn width(&self) -> usize {
        self.width
    }

    pub const fn height(&self) -> usize {
        self.height
    }

    /// Returns the tightly packed RGBA8 color buffer.
    pub fn color_buffer(&self) -> &[u8] {
        &self.color
    }

    /// Returns the normalized depth buffer.
    pub fn depth_buffer(&self) -> &[f32] {
        &self.depth
    }

    /// Reads one RGBA pixel, returning None outside the target.
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

    /// Reads one depth value, returning None outside the target.
    pub fn depth_at(&self, x: usize, y: usize) -> Option<f32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.depth.get(y * self.width + x).copied()
    }

    /// Clears color and depth after validating the depth value.
    pub fn clear(&mut self, color: [u8; 4], depth: f32) -> Result<(), RasterizerError> {
        if !depth.is_finite() || !(0.0..=1.0).contains(&depth) {
            return Err(RasterizerError::InvalidClearDepth { depth });
        }
        for pixel in self.color.chunks_exact_mut(4) {
            pixel.copy_from_slice(&color);
        }
        self.depth.fill(depth);
        Ok(())
    }

    /// Rasterizes one already-transformed triangle using pixel-center sampling.
    ///
    /// The top-left edge rule makes adjacent triangles share an edge without
    /// leaving cracks or shading the shared edge twice. Triangles with zero
    /// area are valid no-ops. Invalid input is rejected before the target is
    /// modified.
    pub fn draw_triangle(
        &mut self,
        mut vertices: [RasterVertex; 3],
        depth_function: DepthFunction,
    ) -> Result<RasterizationReport, RasterizerError> {
        for (index, vertex) in vertices.iter().enumerate() {
            if !vertex.x.is_finite() || !vertex.y.is_finite() || !vertex.depth.is_finite() {
                return Err(RasterizerError::InvalidVertex { index });
            }
            if !(0.0..=1.0).contains(&vertex.depth) {
                return Err(RasterizerError::InvalidVertexDepth {
                    index,
                    depth: vertex.depth,
                });
            }
            if !vertex.reciprocal_w.is_finite() || vertex.reciprocal_w <= 0.0 {
                return Err(RasterizerError::InvalidReciprocalW {
                    index,
                    value: vertex.reciprocal_w,
                });
            }
        }

        let mut area = orient(vertices[0], vertices[1], vertices[2]);
        if area == 0.0 {
            return Ok(RasterizationReport::default());
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

        let last_x = self.width as f64 - 1.0;
        let last_y = self.height as f64 - 1.0;
        let first_x = (min_x - 0.5).ceil();
        let final_x = (max_x - 0.5).floor();
        let first_y = (min_y - 0.5).ceil();
        let final_y = (max_y - 0.5).floor();

        if final_x < 0.0 || final_y < 0.0 || first_x > last_x || first_y > last_y {
            return Ok(RasterizationReport::default());
        }

        let x_start = first_x.max(0.0) as usize;
        let x_end = final_x.min(last_x) as usize;
        let y_start = first_y.max(0.0) as usize;
        let y_end = final_y.min(last_y) as usize;
        if x_start > x_end || y_start > y_end {
            return Ok(RasterizationReport::default());
        }

        let top_left_12 = is_top_left(vertices[1], vertices[2]);
        let top_left_20 = is_top_left(vertices[2], vertices[0]);
        let top_left_01 = is_top_left(vertices[0], vertices[1]);
        let mut report = RasterizationReport::default();

        for y in y_start..=y_end {
            for x in x_start..=x_end {
                let point_x = x as f64 + 0.5;
                let point_y = y as f64 + 0.5;
                let point = RasterVertex::new(point_x as f32, point_y as f32, 0.0, [0; 4]);

                let edge_0 = orient(vertices[1], vertices[2], point);
                let edge_1 = orient(vertices[2], vertices[0], point);
                let edge_2 = orient(vertices[0], vertices[1], point);
                if !inside_edge(edge_0, top_left_12)
                    || !inside_edge(edge_1, top_left_20)
                    || !inside_edge(edge_2, top_left_01)
                {
                    continue;
                }

                report.covered_pixels = report.covered_pixels.saturating_add(1);
                let weight_0 = edge_0 / area;
                let weight_1 = edge_1 / area;
                let weight_2 = edge_2 / area;
                let fragment_depth = (weight_0 * f64::from(vertices[0].depth)
                    + weight_1 * f64::from(vertices[1].depth)
                    + weight_2 * f64::from(vertices[2].depth))
                    as f32;
                let index = y * self.width + x;

                if !depth_passes(depth_function, fragment_depth, self.depth[index]) {
                    report.depth_rejected_pixels = report.depth_rejected_pixels.saturating_add(1);
                    continue;
                }

                let color_offset = index * 4;
                let perspective_denominator = weight_0 * f64::from(vertices[0].reciprocal_w)
                    + weight_1 * f64::from(vertices[1].reciprocal_w)
                    + weight_2 * f64::from(vertices[2].reciprocal_w);
                for channel in 0..4 {
                    let numerator = weight_0
                        * f64::from(vertices[0].color[channel])
                        * f64::from(vertices[0].reciprocal_w)
                        + weight_1
                            * f64::from(vertices[1].color[channel])
                            * f64::from(vertices[1].reciprocal_w)
                        + weight_2
                            * f64::from(vertices[2].color[channel])
                            * f64::from(vertices[2].reciprocal_w);
                    let interpolated = numerator / perspective_denominator;
                    self.color[color_offset + channel] =
                        interpolated.round().clamp(0.0, 255.0) as u8;
                }
                self.depth[index] = fragment_depth;
                report.pixels_written = report.pixels_written.saturating_add(1);
            }
        }

        Ok(report)
    }
}

fn orient(a: RasterVertex, b: RasterVertex, point: RasterVertex) -> f64 {
    let ax = f64::from(a.x);
    let ay = f64::from(a.y);
    let bx = f64::from(b.x);
    let by = f64::from(b.y);
    let px = f64::from(point.x);
    let py = f64::from(point.y);
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

fn is_top_left(a: RasterVertex, b: RasterVertex) -> bool {
    let dx = f64::from(b.x) - f64::from(a.x);
    let dy = f64::from(b.y) - f64::from(a.y);
    dy < 0.0 || (dy == 0.0 && dx > 0.0)
}

fn inside_edge(value: f64, top_left: bool) -> bool {
    value > 0.0 || (value == 0.0 && top_left)
}

fn depth_passes(function: DepthFunction, incoming: f32, stored: f32) -> bool {
    match function {
        DepthFunction::Never => false,
        DepthFunction::Less => incoming < stored,
        DepthFunction::LessEqual => incoming <= stored,
        DepthFunction::Always => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertex(x: f32, y: f32, depth: f32, color: [u8; 4]) -> RasterVertex {
        RasterVertex::new(x, y, depth, color)
    }

    #[test]
    fn rasterizes_a_solid_triangle_into_the_color_and_depth_buffers() {
        let mut rasterizer = SoftwareRasterizer::new(4, 4).unwrap();
        let red = [255, 0, 0, 255];
        let report = rasterizer
            .draw_triangle(
                [
                    vertex(0.0, 0.0, 0.25, red),
                    vertex(4.0, 0.0, 0.25, red),
                    vertex(0.0, 4.0, 0.25, red),
                ],
                DepthFunction::Less,
            )
            .unwrap();

        assert!(report.pixels_written > 0);
        assert_eq!(rasterizer.color_at(0, 0), Some(red));
        assert_eq!(rasterizer.depth_at(0, 0), Some(0.25));
        assert_eq!(rasterizer.color_at(3, 3), Some([0, 0, 0, 255]));
    }

    #[test]
    fn winding_order_does_not_change_triangle_coverage_or_output() {
        let red = [240, 80, 20, 255];
        let triangle = [
            vertex(0.5, 0.5, 0.4, red),
            vertex(5.5, 1.0, 0.5, red),
            vertex(2.0, 5.5, 0.6, red),
        ];
        let mut clockwise = SoftwareRasterizer::new(6, 6).unwrap();
        let mut counter_clockwise = SoftwareRasterizer::new(6, 6).unwrap();

        clockwise
            .draw_triangle(triangle, DepthFunction::Less)
            .unwrap();
        counter_clockwise
            .draw_triangle([triangle[0], triangle[2], triangle[1]], DepthFunction::Less)
            .unwrap();

        assert_eq!(clockwise.color_buffer(), counter_clockwise.color_buffer());
        assert_eq!(clockwise.depth_buffer(), counter_clockwise.depth_buffer());
    }

    #[test]
    fn top_left_rule_tiles_a_rectangle_without_cracks_or_double_coverage() {
        let mut rasterizer = SoftwareRasterizer::new(4, 4).unwrap();
        let color = [10, 20, 30, 255];
        let a = vertex(1.0, 1.0, 0.5, color);
        let b = vertex(3.0, 1.0, 0.5, color);
        let c = vertex(3.0, 3.0, 0.5, color);
        let d = vertex(1.0, 3.0, 0.5, color);

        let first = rasterizer
            .draw_triangle([a, b, c], DepthFunction::LessEqual)
            .unwrap();
        let second = rasterizer
            .draw_triangle([a, c, d], DepthFunction::LessEqual)
            .unwrap();

        assert_eq!(first.pixels_written + second.pixels_written, 4);
        for y in 1..3 {
            for x in 1..3 {
                assert_eq!(rasterizer.color_at(x, y), Some(color));
            }
        }
    }

    #[test]
    fn nearer_fragments_replace_farther_fragments() {
        let mut rasterizer = SoftwareRasterizer::new(4, 4).unwrap();
        let triangle = |depth, color| {
            [
                vertex(0.0, 0.0, depth, color),
                vertex(4.0, 0.0, depth, color),
                vertex(0.0, 4.0, depth, color),
            ]
        };

        rasterizer
            .draw_triangle(triangle(0.8, [255, 0, 0, 255]), DepthFunction::Less)
            .unwrap();
        let nearer = rasterizer
            .draw_triangle(triangle(0.2, [0, 255, 0, 255]), DepthFunction::Less)
            .unwrap();
        let farther = rasterizer
            .draw_triangle(triangle(0.9, [0, 0, 255, 255]), DepthFunction::Less)
            .unwrap();

        assert!(nearer.pixels_written > 0);
        assert_eq!(farther.pixels_written, 0);
        assert!(farther.depth_rejected_pixels > 0);
        assert_eq!(rasterizer.color_at(0, 0), Some([0, 255, 0, 255]));
        assert_eq!(rasterizer.depth_at(0, 0), Some(0.2));
    }

    #[test]
    fn color_interpolation_is_perspective_correct_when_reciprocal_w_differs() {
        let mut rasterizer = SoftwareRasterizer::new(4, 4).unwrap();
        let triangle = [
            vertex(0.0, 0.0, 0.5, [0, 0, 0, 255]).with_reciprocal_w(1.0),
            vertex(4.0, 0.0, 0.5, [255, 0, 0, 255]).with_reciprocal_w(0.5),
            vertex(0.0, 4.0, 0.5, [0, 0, 0, 255]).with_reciprocal_w(0.5),
        ];

        rasterizer
            .draw_triangle(triangle, DepthFunction::Less)
            .unwrap();

        // Pixel center (0.5, 0.5) has screen barycentrics (0.75, 0.125, 0.125).
        // Perspective correction gives round((0.125 * 255 * 0.5) / 0.875) = 18.
        assert_eq!(rasterizer.color_at(0, 0), Some([18, 0, 0, 255]));
    }

    #[test]
    fn invalid_reciprocal_w_is_rejected_before_target_changes() {
        let mut rasterizer = SoftwareRasterizer::new(2, 2).unwrap();
        let before = rasterizer.color_buffer().to_vec();
        let result = rasterizer.draw_triangle(
            [
                vertex(0.0, 0.0, 0.5, [255; 4]),
                vertex(2.0, 0.0, 0.5, [255; 4]).with_reciprocal_w(0.0),
                vertex(0.0, 2.0, 0.5, [255; 4]),
            ],
            DepthFunction::Less,
        );
        assert_eq!(
            result,
            Err(RasterizerError::InvalidReciprocalW {
                index: 1,
                value: 0.0
            })
        );
        assert_eq!(rasterizer.color_buffer(), before);
    }

    #[test]
    fn invalid_vertices_are_rejected_without_changing_the_target() {
        let mut rasterizer = SoftwareRasterizer::new(2, 2).unwrap();
        let before_color = rasterizer.color_buffer().to_vec();
        let before_depth = rasterizer.depth_buffer().to_vec();

        let result = rasterizer.draw_triangle(
            [
                vertex(0.0, 0.0, 0.5, [255; 4]),
                vertex(f32::NAN, 1.0, 0.5, [255; 4]),
                vertex(1.0, 0.0, 0.5, [255; 4]),
            ],
            DepthFunction::Less,
        );

        assert_eq!(result, Err(RasterizerError::InvalidVertex { index: 1 }));
        assert_eq!(rasterizer.color_buffer(), before_color);
        assert_eq!(rasterizer.depth_buffer(), before_depth);
    }

    #[test]
    fn rejects_invalid_clear_depth_and_zero_sized_targets() {
        assert!(matches!(
            SoftwareRasterizer::new(0, 10),
            Err(RasterizerError::InvalidDimensions { .. })
        ));

        let mut rasterizer = SoftwareRasterizer::new(2, 2).unwrap();
        assert!(matches!(
            rasterizer.clear([0; 4], f32::INFINITY),
            Err(RasterizerError::InvalidClearDepth { .. })
        ));
    }
}
