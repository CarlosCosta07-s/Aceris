//! A small anti-aliased software rasteriser for [`Scene`]s.
//!
//! Every pixel is sampled on a regular grid and the samples are composited
//! with the "over" operator in sRGB space, the way an SVG renderer would.

use crate::scene::{Node, Paint, Point, Scene, Shape, Stop, Style};

/// Samples per pixel along each axis.
const SAMPLES: usize = 4;

/// An 8-bit, non-premultiplied RGBA image.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

enum Geometry {
    RoundRect {
        size: f32,
        radius: f32,
    },
    Circle {
        center: Point,
        radius: f32,
    },
    Ellipse {
        center: Point,
        rx: f32,
        ry: f32,
        cos: f32,
        sin: f32,
    },
    Outline {
        points: Vec<Point>,
        closed: bool,
    },
}

/// A node with its geometry flattened and its bounding box computed.
struct Prepared<'a> {
    node: &'a Node,
    geometry: Geometry,
    /// `[min_x, min_y, max_x, max_y]`.
    bounds: [f32; 4],
}

fn prepare(node: &Node, canvas: f32) -> Prepared<'_> {
    let half_stroke = match node.style {
        Style::Stroke { width, .. } => width / 2.0,
        Style::Fill(_) => 0.0,
    };

    let (geometry, bounds) = match &node.shape {
        Shape::RoundRect { radius } => (
            Geometry::RoundRect {
                size: canvas,
                radius: *radius,
            },
            [0.0, 0.0, canvas, canvas],
        ),
        Shape::Circle { center, radius } => (
            Geometry::Circle {
                center: *center,
                radius: *radius,
            },
            [
                center.0 - radius,
                center.1 - radius,
                center.0 + radius,
                center.1 + radius,
            ],
        ),
        Shape::Ellipse {
            center,
            rx,
            ry,
            rotation,
        } => {
            let (sin, cos) = rotation.to_radians().sin_cos();
            let reach = rx.max(*ry);
            (
                Geometry::Ellipse {
                    center: *center,
                    rx: *rx,
                    ry: *ry,
                    cos,
                    sin,
                },
                [
                    center.0 - reach,
                    center.1 - reach,
                    center.0 + reach,
                    center.1 + reach,
                ],
            )
        }
        Shape::Path(path) => {
            let points = path.flatten();
            let mut bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
            for &(x, y) in &points {
                bounds = [
                    bounds[0].min(x),
                    bounds[1].min(y),
                    bounds[2].max(x),
                    bounds[3].max(y),
                ];
            }
            let bounds = [
                bounds[0] - half_stroke,
                bounds[1] - half_stroke,
                bounds[2] + half_stroke,
                bounds[3] + half_stroke,
            ];
            (
                Geometry::Outline {
                    points,
                    closed: path.closed,
                },
                bounds,
            )
        }
    };

    Prepared {
        node,
        geometry,
        bounds,
    }
}

impl Prepared<'_> {
    fn covers(&self, x: f32, y: f32) -> bool {
        let [min_x, min_y, max_x, max_y] = self.bounds;
        if x < min_x || x > max_x || y < min_y || y > max_y {
            return false;
        }

        match &self.geometry {
            Geometry::RoundRect { size, radius } => {
                let half = size / 2.0;
                let dx = ((x - half).abs() - (half - radius)).max(0.0);
                let dy = ((y - half).abs() - (half - radius)).max(0.0);
                dx * dx + dy * dy <= radius * radius
            }
            Geometry::Circle { center, radius } => {
                let (dx, dy) = (x - center.0, y - center.1);
                dx * dx + dy * dy <= radius * radius
            }
            Geometry::Ellipse {
                center,
                rx,
                ry,
                cos,
                sin,
            } => {
                let (dx, dy) = (x - center.0, y - center.1);
                let local_x = (dx * cos + dy * sin) / rx;
                let local_y = (-dx * sin + dy * cos) / ry;
                local_x * local_x + local_y * local_y <= 1.0
            }
            Geometry::Outline { points, closed } => match self.node.style {
                Style::Stroke { width, .. } => near_polyline(points, *closed, (x, y), width / 2.0),
                Style::Fill(_) => inside_polygon(points, (x, y)),
            },
        }
    }

    /// Colour and alpha (all 0..=1, not premultiplied) at a point.
    fn color_at(&self, x: f32, y: f32) -> [f32; 4] {
        match &self.node.style {
            Style::Stroke { color, .. } => rgb(*color, 1.0),
            Style::Fill(Paint::Solid(color)) => rgb(*color, 1.0),
            Style::Fill(Paint::Linear { from, to, stops }) => {
                let (axis_x, axis_y) = (to.0 - from.0, to.1 - from.1);
                let length_squared = axis_x * axis_x + axis_y * axis_y;
                let t = ((x - from.0) * axis_x + (y - from.1) * axis_y) / length_squared;
                gradient_color(stops, t)
            }
            Style::Fill(Paint::Radial {
                center,
                radius,
                stops,
            }) => {
                let t = (x - center.0).hypot(y - center.1) / radius;
                gradient_color(stops, t)
            }
        }
    }
}

fn rgb(color: crate::scene::Rgb, alpha: f32) -> [f32; 4] {
    [
        f32::from(color.0) / 255.0,
        f32::from(color.1) / 255.0,
        f32::from(color.2) / 255.0,
        alpha,
    ]
}

fn gradient_color(stops: &[Stop], t: f32) -> [f32; 4] {
    let t = t.clamp(0.0, 1.0);
    let (Some(first), Some(last)) = (stops.first(), stops.last()) else {
        return [0.0; 4];
    };
    if t <= first.offset {
        return rgb(first.color, first.opacity);
    }
    for pair in stops.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t <= b.offset {
            let span = (b.offset - a.offset).max(f32::EPSILON);
            let mix = (t - a.offset) / span;
            let (ca, cb) = (rgb(a.color, a.opacity), rgb(b.color, b.opacity));
            return [
                ca[0] + (cb[0] - ca[0]) * mix,
                ca[1] + (cb[1] - ca[1]) * mix,
                ca[2] + (cb[2] - ca[2]) * mix,
                ca[3] + (cb[3] - ca[3]) * mix,
            ];
        }
    }
    rgb(last.color, last.opacity)
}

/// Even-odd point-in-polygon test; the polygon is implicitly closed.
fn inside_polygon(points: &[Point], (x, y): Point) -> bool {
    let mut inside = false;
    let mut previous = match points.last() {
        Some(&point) => point,
        None => return false,
    };
    for &current in points {
        let ((x1, y1), (x2, y2)) = (previous, current);
        if (y1 > y) != (y2 > y) && x < (x2 - x1) * (y - y1) / (y2 - y1) + x1 {
            inside = !inside;
        }
        previous = current;
    }
    inside
}

/// Whether `point` lies within `reach` of the polyline. Round caps follow
/// naturally from measuring the distance to the end points.
fn near_polyline(points: &[Point], closed: bool, point: Point, reach: f32) -> bool {
    let limit = reach * reach;
    let segment_hit = |a: Point, b: Point| distance_squared_to_segment(point, a, b) <= limit;

    if points.windows(2).any(|pair| segment_hit(pair[0], pair[1])) {
        return true;
    }
    match (closed, points.first(), points.last()) {
        (true, Some(&first), Some(&last)) => segment_hit(last, first),
        _ => false,
    }
}

fn distance_squared_to_segment((px, py): Point, (ax, ay): Point, (bx, by): Point) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let length_squared = dx * dx + dy * dy;
    let t = if length_squared == 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / length_squared).clamp(0.0, 1.0)
    };
    let (cx, cy) = (ax + t * dx - px, ay + t * dy - py);
    cx * cx + cy * cy
}

/// Composites every node at one sample point. The result is premultiplied.
fn sample(nodes: &[Prepared<'_>], x: f32, y: f32) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    for prepared in nodes {
        if !prepared.covers(x, y) {
            continue;
        }
        let [r, g, b, a] = prepared.color_at(x, y);
        let alpha = a * prepared.node.opacity;
        let keep = 1.0 - alpha;
        out = [
            r * alpha + out[0] * keep,
            g * alpha + out[1] * keep,
            b * alpha + out[2] * keep,
            alpha + out[3] * keep,
        ];
    }
    out
}

/// Renders `scene` into a square image of `size` pixels.
pub fn render(scene: &Scene, size: u32) -> Image {
    let nodes: Vec<Prepared<'_>> = scene
        .nodes
        .iter()
        .map(|node| prepare(node, scene.size))
        .collect();

    let step = scene.size / size as f32;
    let weight = (SAMPLES * SAMPLES) as f32;
    let mut rgba = Vec::with_capacity(size as usize * size as usize * 4);

    for py in 0..size {
        for px in 0..size {
            let mut total = [0.0f32; 4];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) * step;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) * step;
                    for (sum, value) in total.iter_mut().zip(sample(&nodes, x, y)) {
                        *sum += value;
                    }
                }
            }

            let alpha = total[3] / weight;
            if alpha <= 0.0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            for channel in &total[..3] {
                rgba.push(to_byte(channel / weight / alpha));
            }
            rgba.push(to_byte(alpha));
        }
    }

    Image {
        width: size,
        height: size,
        rgba,
    }
}

fn to_byte(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{Variant, icon};

    fn pixel(image: &Image, x: u32, y: u32) -> [u8; 4] {
        let at = ((y * image.width + x) * 4) as usize;
        image.rgba[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn rounded_icon_has_transparent_corners_and_a_red_centre() {
        let image = render(&icon(Variant::Rounded), 64);
        assert_eq!(image.rgba.len(), 64 * 64 * 4);
        assert_eq!(pixel(&image, 0, 0)[3], 0);

        let [r, g, b, a] = pixel(&image, 32, 32);
        assert_eq!(a, 255);
        assert!(
            r > 150 && g < 110 && b < 130,
            "centre should be red: {r},{g},{b}"
        );
    }

    #[test]
    fn rounded_icon_background_is_dark_purple_near_the_edge() {
        let image = render(&icon(Variant::Rounded), 64);
        let [r, g, b, a] = pixel(&image, 32, 2);
        assert_eq!(a, 255);
        assert!(b > r && b > g && r < 90, "expected purple, got {r},{g},{b}");
    }

    #[test]
    fn circle_icon_is_opaque_at_the_rim_and_clear_in_the_corner() {
        let image = render(&icon(Variant::Circle), 64);
        assert_eq!(pixel(&image, 32, 2)[3], 255);
        assert_eq!(pixel(&image, 0, 0)[3], 0);
        assert_eq!(pixel(&image, 63, 63)[3], 0);
    }

    #[test]
    fn foreground_is_transparent_outside_the_fruit() {
        let image = render(&icon(Variant::Foreground), 64);
        assert_eq!(pixel(&image, 0, 0)[3], 0);
        assert_eq!(pixel(&image, 63, 0)[3], 0);
        assert_eq!(pixel(&image, 32, 32)[3], 255);
    }

    #[test]
    fn polygon_and_segment_helpers() {
        let square = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        assert!(inside_polygon(&square, (5.0, 5.0)));
        assert!(!inside_polygon(&square, (15.0, 5.0)));
        assert!(!inside_polygon(&[], (0.0, 0.0)));

        assert!(near_polyline(
            &[(0.0, 0.0), (10.0, 0.0)],
            false,
            (5.0, 1.0),
            1.5
        ));
        assert!(!near_polyline(
            &[(0.0, 0.0), (10.0, 0.0)],
            false,
            (5.0, 3.0),
            1.5
        ));
        // Round cap beyond the end of the segment.
        assert!(near_polyline(
            &[(0.0, 0.0), (10.0, 0.0)],
            false,
            (11.0, 0.0),
            1.5
        ));
    }

    #[test]
    fn gradients_interpolate_and_clamp() {
        use crate::scene::Rgb;
        let stops = [
            Stop {
                offset: 0.0,
                color: Rgb(0, 0, 0),
                opacity: 1.0,
            },
            Stop {
                offset: 1.0,
                color: Rgb(255, 255, 255),
                opacity: 0.0,
            },
        ];
        assert_eq!(gradient_color(&stops, -1.0), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(gradient_color(&stops, 2.0), [1.0, 1.0, 1.0, 0.0]);
        let middle = gradient_color(&stops, 0.5);
        assert!((middle[0] - 0.5).abs() < 1e-6 && (middle[3] - 0.5).abs() < 1e-6);
        assert_eq!(gradient_color(&[], 0.5), [0.0; 4]);
    }
}
