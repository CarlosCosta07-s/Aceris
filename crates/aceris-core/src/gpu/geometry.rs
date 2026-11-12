//! Reference vertex transformation, homogeneous clipping, and viewport mapping.
//!
//! This is a generic bring-up stage, not a claim of PICA200 shader compatibility.
//! Matrices use column-major storage and multiply column vectors. The clipping
//! convention is explicit because the exact PICA200 clip-space rules still need
//! to be established from independent hardware documentation and tests.

use super::{
    DepthFunction, RasterizationReport, RasterVertex, RasterizerError, SoftwareRasterizer,
};

const CLIP_W_EPSILON: f32 = 1.0e-6;
const MAX_CLIPPED_POLYGON_VERTICES: usize = 10;

/// A 4x4 matrix stored in column-major order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix4 {
    elements: [f32; 16],
}

impl Matrix4 {
    pub const fn identity() -> Self {
        Self {
            elements: [
                1.0, 0.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0, //
                0.0, 0.0, 0.0, 1.0,
            ],
        }
    }

    /// Creates a matrix from 16 column-major elements.
    pub fn from_column_major(elements: [f32; 16]) -> Result<Self, GeometryError> {
        if elements.iter().any(|element| !element.is_finite()) {
            return Err(GeometryError::NonFiniteTransform);
        }
        Ok(Self { elements })
    }

    pub const fn elements(&self) -> &[f32; 16] {
        &self.elements
    }

    fn transform(&self, position: [f32; 4]) -> [f32; 4] {
        let mut output = [0.0; 4];
        for row in 0..4 {
            output[row] = self.elements[row] * position[0]
                + self.elements[4 + row] * position[1]
                + self.elements[8 + row] * position[2]
                + self.elements[12 + row] * position[3];
        }
        output
    }
}

impl Default for Matrix4 {
    fn default() -> Self {
        Self::identity()
    }
}

/// Clip-space convention for the near plane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ClipDepthRange {
    /// Clip Z to -W..=W, then map NDC depth to 0..=1.
    #[default]
    NegativeOneToOne,
    /// Clip Z to 0..=W; NDC depth is already in 0..=1.
    ZeroToOne,
}

/// A vertex before clipping, carrying homogeneous position and RGBA8 color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryVertex {
    pub position: [f32; 4],
    pub color: [u8; 4],
}

impl GeometryVertex {
    pub const fn new(position: [f32; 4], color: [u8; 4]) -> Self {
        Self { position, color }
    }
}

/// Pixel-space viewport. Y increases downwards, matching the display surfaces.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Viewport {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Viewport {
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Result<Self, GeometryError> {
        let viewport = Self {
            x,
            y,
            width,
            height,
        };
        if !x.is_finite()
            || !y.is_finite()
            || !width.is_finite()
            || !height.is_finite()
            || width <= 0.0
            || height <= 0.0
            || !(x + width).is_finite()
            || !(y + height).is_finite()
        {
            return Err(GeometryError::InvalidViewport);
        }
        Ok(viewport)
    }
}

/// Errors from the reference geometry stage.
#[derive(Clone, Debug, PartialEq)]
pub enum GeometryError {
    NonFiniteTransform,
    InvalidViewport,
    NonFiniteVertex { index: usize },
    ClippedPolygonOverflow,
    InvalidPerspectiveDivide,
    Rasterizer(RasterizerError),
}

impl std::fmt::Display for GeometryError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFiniteTransform => {
                formatter.write_str("geometry transform contains a non-finite value")
            }
            Self::InvalidViewport => formatter.write_str("geometry viewport is invalid"),
            Self::NonFiniteVertex { index } => {
                write!(formatter, "geometry vertex {index} contains a non-finite position")
            }
            Self::ClippedPolygonOverflow => {
                formatter.write_str("clipped triangle exceeded the supported vertex bound")
            }
            Self::InvalidPerspectiveDivide => {
                formatter.write_str("geometry vertex cannot be safely perspective-divided")
            }
            Self::Rasterizer(error) => write!(formatter, "rasterizer failed: {error}"),
        }
    }
}

impl std::error::Error for GeometryError {}

impl From<RasterizerError> for GeometryError {
    fn from(error: RasterizerError) -> Self {
        Self::Rasterizer(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ClipVertex {
    position: [f32; 4],
    color: [f32; 4],
}

impl ClipVertex {
    fn interpolate(self, other: Self, amount: f32) -> Self {
        let mut position = [0.0; 4];
        let mut color = [0.0; 4];
        for index in 0..4 {
            position[index] =
                self.position[index] + (other.position[index] - self.position[index]) * amount;
            color[index] = self.color[index] + (other.color[index] - self.color[index]) * amount;
        }
        Self { position, color }
    }
}

#[derive(Clone, Copy)]
enum ClipPlane {
    Left,
    Right,
    Bottom,
    Top,
    Near,
    Far,
    PositiveW,
}

impl ClipPlane {
    const ALL: [Self; 7] = [
        Self::Left,
        Self::Right,
        Self::Bottom,
        Self::Top,
        Self::Near,
        Self::Far,
        Self::PositiveW,
    ];

    fn distance(self, vertex: ClipVertex, depth_range: ClipDepthRange) -> f32 {
        let [x, y, z, w] = vertex.position;
        match self {
            Self::Left => x + w,
            Self::Right => w - x,
            Self::Bottom => y + w,
            Self::Top => w - y,
            Self::Near => match depth_range {
                ClipDepthRange::NegativeOneToOne => z + w,
                ClipDepthRange::ZeroToOne => z,
            },
            Self::Far => w - z,
            // A tiny positive W guard prevents division by zero at the eye apex.
            Self::PositiveW => w - CLIP_W_EPSILON,
        }
    }
}

/// Transforms, clips, and maps one triangle into the rasterizer's pixel space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GeometryPipeline {
    transform: Matrix4,
    viewport: Viewport,
    depth_range: ClipDepthRange,
}

/// Results from processing one input triangle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GeometryReport {
    /// Number of output triangles after clipping and fan triangulation.
    pub output_triangles: u64,
    /// Number of fragments written by the rasterizer.
    pub pixels_written: u64,
    /// Number of covered fragments rejected by depth testing.
    pub depth_rejected_pixels: u64,
}

impl GeometryPipeline {
    pub fn new(
        transform: Matrix4,
        viewport: Viewport,
        depth_range: ClipDepthRange,
    ) -> Self {
        Self {
            transform,
            viewport,
            depth_range,
        }
    }

    pub const fn transform(&self) -> Matrix4 {
        self.transform
    }

    pub const fn viewport(&self) -> Viewport {
        self.viewport
    }

    pub const fn depth_range(&self) -> ClipDepthRange {
        self.depth_range
    }

    /// Processes one triangle into zero or more pixel-space triangles.
    ///
    /// The triangle is transformed, clipped against the six canonical clip
    /// planes plus a small positive-W guard, triangulated as a fan, perspective
    /// divided, and mapped to the configured viewport. Color remains a vertex
    /// varying; the rasterizer performs perspective-correct interpolation.
    pub fn process_triangle(
        &self,
        vertices: [GeometryVertex; 3],
    ) -> Result<Vec<[RasterVertex; 3]>, GeometryError> {
        let mut polygon = Vec::with_capacity(MAX_CLIPPED_POLYGON_VERTICES);
        for (index, vertex) in vertices.iter().enumerate() {
            if vertex.position.iter().any(|component| !component.is_finite()) {
                return Err(GeometryError::NonFiniteVertex { index });
            }
            let position = self.transform.transform(vertex.position);
            if position.iter().any(|component| !component.is_finite()) {
                return Err(GeometryError::NonFiniteVertex { index });
            }
            polygon.push(ClipVertex {
                position,
                color: vertex.color.map(|channel| f32::from(channel)),
            });
        }

        for plane in ClipPlane::ALL {
            polygon = clip_polygon(polygon, plane, self.depth_range)?;
            if polygon.len() < 3 {
                return Ok(Vec::new());
            }
        }

        let first = self.to_raster_vertex(polygon[0])?;
        let mut output = Vec::with_capacity(polygon.len().saturating_sub(2));
        for index in 1..polygon.len() - 1 {
            output.push([
                first,
                self.to_raster_vertex(polygon[index])?,
                self.to_raster_vertex(polygon[index + 1])?,
            ]);
        }
        Ok(output)
    }

    /// Processes and immediately rasterizes one triangle.
    pub fn draw_triangle(
        &self,
        vertices: [GeometryVertex; 3],
        rasterizer: &mut SoftwareRasterizer,
        depth_function: DepthFunction,
    ) -> Result<GeometryReport, GeometryError> {
        let triangles = self.process_triangle(vertices)?;
        let mut report = GeometryReport {
            output_triangles: triangles.len() as u64,
            ..GeometryReport::default()
        };
        for triangle in triangles {
            let raster_report: RasterizationReport =
                rasterizer.draw_triangle(triangle, depth_function)?;
            report.pixels_written = report
                .pixels_written
                .saturating_add(raster_report.pixels_written);
            report.depth_rejected_pixels = report
                .depth_rejected_pixels
                .saturating_add(raster_report.depth_rejected_pixels);
        }
        Ok(report)
    }

    fn to_raster_vertex(self, vertex: ClipVertex) -> Result<RasterVertex, GeometryError> {
        let w = vertex.position[3];
        if !w.is_finite() || w < CLIP_W_EPSILON {
            return Err(GeometryError::InvalidPerspectiveDivide);
        }

        let inverse_w = 1.0 / w;
        let ndc_x = vertex.position[0] * inverse_w;
        let ndc_y = vertex.position[1] * inverse_w;
        let ndc_z = vertex.position[2] * inverse_w;
        if !inverse_w.is_finite()
            || !ndc_x.is_finite()
            || !ndc_y.is_finite()
            || !ndc_z.is_finite()
        {
            return Err(GeometryError::InvalidPerspectiveDivide);
        }

        let depth = match self.depth_range {
            ClipDepthRange::NegativeOneToOne => ndc_z.mul_add(0.5, 0.5),
            ClipDepthRange::ZeroToOne => ndc_z,
        }
        .clamp(0.0, 1.0);
        let x = self.viewport.x + (ndc_x + 1.0) * (self.viewport.width * 0.5);
        let y = self.viewport.y + (1.0 - ndc_y) * (self.viewport.height * 0.5);
        if !x.is_finite() || !y.is_finite() {
            return Err(GeometryError::InvalidPerspectiveDivide);
        }

        let color = vertex
            .color
            .map(|channel| channel.round().clamp(0.0, 255.0) as u8);
        Ok(RasterVertex::new(x, y, depth, color).with_reciprocal_w(inverse_w))
    }
}

fn clip_polygon(
    polygon: Vec<ClipVertex>,
    plane: ClipPlane,
    depth_range: ClipDepthRange,
) -> Result<Vec<ClipVertex>, GeometryError> {
    if polygon.is_empty() {
        return Ok(polygon);
    }

    let mut output = Vec::with_capacity(MAX_CLIPPED_POLYGON_VERTICES);
    let mut previous = *polygon.last().expect("non-empty polygon was checked");
    let mut previous_distance = plane.distance(previous, depth_range);
    let mut previous_inside = previous_distance >= 0.0;

    for current in polygon {
        let current_distance = plane.distance(current, depth_range);
        let current_inside = current_distance >= 0.0;

        if current_inside != previous_inside {
            let denominator = previous_distance - current_distance;
            if denominator == 0.0 || !denominator.is_finite() {
                return Err(GeometryError::InvalidPerspectiveDivide);
            }
            let amount = (previous_distance / denominator).clamp(0.0, 1.0);
            output.push(previous.interpolate(current, amount));
        }

        if current_inside {
            output.push(current);
        }

        if output.len() > MAX_CLIPPED_POLYGON_VERTICES {
            return Err(GeometryError::ClippedPolygonOverflow);
        }

        previous = current;
        previous_distance = current_distance;
        previous_inside = current_inside;
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vertex(x: f32, y: f32, z: f32, color: [u8; 4]) -> GeometryVertex {
        GeometryVertex::new([x, y, z, 1.0], color)
    }

    fn identity_pipeline(width: f32, height: f32) -> GeometryPipeline {
        GeometryPipeline::new(
            Matrix4::identity(),
            Viewport::new(0.0, 0.0, width, height).unwrap(),
            ClipDepthRange::NegativeOneToOne,
        )
    }

    #[test]
    fn identity_transform_maps_ndc_coordinates_into_a_downward_y_viewport() {
        let pipeline = identity_pipeline(400.0, 240.0);
        let output = pipeline
            .process_triangle([
                vertex(-1.0, 1.0, -1.0, [255, 0, 0, 255]),
                vertex(1.0, 1.0, 1.0, [0, 255, 0, 255]),
                vertex(-1.0, -1.0, 0.0, [0, 0, 255, 255]),
            ])
            .unwrap();

        assert_eq!(output.len(), 1);
        assert_eq!(output[0][0].x, 0.0);
        assert_eq!(output[0][0].y, 0.0);
        assert_eq!(output[0][0].depth, 0.0);
        assert_eq!(output[0][1].x, 400.0);
        assert_eq!(output[0][1].y, 0.0);
        assert_eq!(output[0][1].depth, 1.0);
        assert_eq!(output[0][2].x, 0.0);
        assert_eq!(output[0][2].y, 240.0);
        assert_eq!(output[0][2].depth, 0.5);
    }

    #[test]
    fn clips_a_triangle_crossing_the_left_plane_into_two_triangles() {
        let pipeline = identity_pipeline(100.0, 100.0);
        let output = pipeline
            .process_triangle([
                vertex(-2.0, 0.0, 0.0, [255, 0, 0, 255]),
                vertex(0.5, 0.8, 0.0, [0, 255, 0, 255]),
                vertex(0.5, -0.8, 0.0, [0, 0, 255, 255]),
            ])
            .unwrap();

        assert_eq!(output.len(), 2);
        for triangle in output {
            for vertex in triangle {
                assert!(vertex.x >= -0.001 && vertex.x <= 100.001);
                assert!(vertex.y >= -0.001 && vertex.y <= 100.001);
                assert!((0.0..=1.0).contains(&vertex.depth));
                assert!(vertex.reciprocal_w.is_finite() && vertex.reciprocal_w > 0.0);
            }
        }
    }

    #[test]
    fn fully_outside_triangle_is_clipped_away() {
        let pipeline = identity_pipeline(32.0, 32.0);
        let output = pipeline
            .process_triangle([
                vertex(-3.0, 0.0, 0.0, [255; 4]),
                vertex(-2.0, 0.5, 0.0, [255; 4]),
                vertex(-2.0, -0.5, 0.0, [255; 4]),
            ])
            .unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn zero_to_one_depth_range_clips_negative_z() {
        let pipeline = GeometryPipeline::new(
            Matrix4::identity(),
            Viewport::new(0.0, 0.0, 20.0, 20.0).unwrap(),
            ClipDepthRange::ZeroToOne,
        );
        let output = pipeline
            .process_triangle([
                vertex(-0.5, -0.5, -0.5, [255; 4]),
                vertex(0.5, -0.5, 0.5, [255; 4]),
                vertex(0.0, 0.5, 0.5, [255; 4]),
            ])
            .unwrap();
        assert_eq!(output.len(), 2);
        assert!(output.iter().flatten().all(|vertex| vertex.depth >= 0.0));
    }

    #[test]
    fn transformed_non_finite_input_is_rejected_before_rasterization() {
        let pipeline = identity_pipeline(16.0, 16.0);
        let error = pipeline
            .process_triangle([
                vertex(0.0, 0.0, 0.0, [0; 4]),
                GeometryVertex::new([f32::NAN, 0.0, 0.0, 1.0], [0; 4]),
                vertex(0.0, 1.0, 0.0, [0; 4]),
            ])
            .unwrap_err();
        assert_eq!(error, GeometryError::NonFiniteVertex { index: 1 });
    }

    #[test]
    fn invalid_viewport_and_matrix_are_rejected() {
        assert_eq!(
            Viewport::new(0.0, 0.0, 0.0, 10.0),
            Err(GeometryError::InvalidViewport)
        );
        assert_eq!(
            Matrix4::from_column_major([f32::INFINITY; 16]),
            Err(GeometryError::NonFiniteTransform)
        );
    }

    #[test]
    fn transformed_triangle_can_be_rasterized_into_a_display_target() {
        let pipeline = identity_pipeline(8.0, 8.0);
        let mut rasterizer = SoftwareRasterizer::new(8, 8).unwrap();
        let report = pipeline
            .draw_triangle(
                [
                    vertex(-1.0, 1.0, 0.0, [255, 0, 0, 255]),
                    vertex(1.0, 1.0, 0.0, [255, 0, 0, 255]),
                    vertex(-1.0, -1.0, 0.0, [255, 0, 0, 255]),
                ],
                &mut rasterizer,
                DepthFunction::Less,
            )
            .unwrap();

        assert_eq!(report.output_triangles, 1);
        assert!(report.pixels_written > 0);
        assert_eq!(rasterizer.color_at(0, 0), Some([255, 0, 0, 255]));
    }
}
