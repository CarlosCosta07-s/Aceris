//! Vector description of the Aceris icon: an acerola.
//!
//! The scene is the single source of truth. The same nodes are rasterised to
//! PNG by [`crate::raster`] and exported as SVG by [`Scene::to_svg`].

pub type Point = (f32, f32);

/// An sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn hex(value: u32) -> Self {
        Self((value >> 16) as u8, (value >> 8) as u8, value as u8)
    }

    fn css(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.0, self.1, self.2)
    }
}

/// One colour stop of a gradient.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub offset: f32,
    pub color: Rgb,
    pub opacity: f32,
}

const fn stop(offset: f32, color: u32, opacity: f32) -> Stop {
    Stop {
        offset,
        color: Rgb::hex(color),
        opacity,
    }
}

/// How a shape is filled. Gradients use scene coordinates.
#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    Solid(Rgb),
    Linear {
        from: Point,
        to: Point,
        stops: Vec<Stop>,
    },
    Radial {
        center: Point,
        radius: f32,
        stops: Vec<Stop>,
    },
}

/// A chain of cubic Bézier curves.
#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    pub start: Point,
    pub curves: Vec<[Point; 3]>,
    pub closed: bool,
}

impl Path {
    pub fn new(start: Point) -> Self {
        Self {
            start,
            curves: Vec::new(),
            closed: false,
        }
    }

    pub fn curve(mut self, control1: Point, control2: Point, end: Point) -> Self {
        self.curves.push([control1, control2, end]);
        self
    }

    pub fn close(mut self) -> Self {
        self.closed = true;
        self
    }

    /// Approximates the path with line segments.
    pub fn flatten(&self) -> Vec<Point> {
        const STEPS: usize = 32;

        let mut points = vec![self.start];
        let mut from = self.start;
        for &[c1, c2, to] in &self.curves {
            for step in 1..=STEPS {
                let t = step as f32 / STEPS as f32;
                let u = 1.0 - t;
                let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
                points.push((
                    w0 * from.0 + w1 * c1.0 + w2 * c2.0 + w3 * to.0,
                    w0 * from.1 + w1 * c1.1 + w2 * c2.1 + w3 * to.1,
                ));
            }
            from = to;
        }
        points
    }

    fn svg_data(&self) -> String {
        let mut data = format!("M{} {}", num(self.start.0), num(self.start.1));
        for [c1, c2, to] in &self.curves {
            data.push_str(&format!(
                " C{} {},{} {},{} {}",
                num(c1.0),
                num(c1.1),
                num(c2.0),
                num(c2.1),
                num(to.0),
                num(to.1)
            ));
        }
        if self.closed {
            data.push_str(" Z");
        }
        data
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// The whole canvas with rounded corners.
    RoundRect {
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
        /// Clockwise rotation in degrees (y axis points down).
        rotation: f32,
    },
    Path(Path),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Style {
    Fill(Paint),
    /// A round-capped stroke. Only paths are ever stroked.
    Stroke {
        color: Rgb,
        width: f32,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub shape: Shape,
    pub style: Style,
    pub opacity: f32,
}

impl Node {
    fn fill(shape: Shape, paint: Paint) -> Self {
        Self {
            shape,
            style: Style::Fill(paint),
            opacity: 1.0,
        }
    }

    fn solid(shape: Shape, color: u32) -> Self {
        Self::fill(shape, Paint::Solid(Rgb::hex(color)))
    }

    fn stroke(path: Path, color: u32, width: f32) -> Self {
        Self {
            shape: Shape::Path(path),
            style: Style::Stroke {
                color: Rgb::hex(color),
                width,
            },
            opacity: 1.0,
        }
    }

    fn opacity(mut self, opacity: f32) -> Self {
        self.opacity = opacity;
        self
    }
}

/// Which flavour of the icon to build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Full-colour rounded square: desktop, store listings.
    Rounded,
    /// Full-colour circle: legacy round Android launcher icon.
    Circle,
    /// Transparent foreground for Android adaptive icons.
    Foreground,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    /// Side of the square canvas, in scene units.
    pub size: f32,
    pub nodes: Vec<Node>,
}

/// Background colour used behind the adaptive-icon foreground.
pub const ADAPTIVE_BACKGROUND: Rgb = Rgb::hex(0x2A1240);

/// Canvas side used by every variant.
pub const CANVAS: f32 = 1024.0;

/// Builds the icon. Geometry is authored around the fruit's own centre and
/// then scaled and moved to the middle of the canvas.
pub fn icon(variant: Variant) -> Scene {
    const FRUIT_CENTER: Point = (520.0, 562.0);
    let scale = match variant {
        Variant::Rounded => 1.12,
        Variant::Circle => 1.0,
        // Adaptive icons keep important content in the central 66 %.
        Variant::Foreground => 0.82,
    };
    let middle = CANVAS / 2.0;
    let at = move |(x, y): Point| -> Point {
        (
            middle + (x - FRUIT_CENTER.0) * scale,
            middle + (y - FRUIT_CENTER.1) * scale,
        )
    };
    let len = move |value: f32| value * scale;
    let path = move |start: Point, curves: &[[Point; 3]]| -> Path {
        curves
            .iter()
            .fold(Path::new(at(start)), |path, &[c1, c2, to]| {
                path.curve(at(c1), at(c2), at(to))
            })
    };

    let mut nodes = Vec::new();

    if variant != Variant::Foreground {
        let background = match variant {
            Variant::Circle => Shape::Circle {
                center: (middle, middle),
                radius: middle,
            },
            _ => Shape::RoundRect { radius: 224.0 },
        };
        nodes.push(Node::fill(
            background,
            Paint::Linear {
                from: (0.0, 0.0),
                to: (CANVAS, CANVAS),
                stops: vec![stop(0.0, 0x4A1D5E, 1.0), stop(1.0, 0x1A0B2E, 1.0)],
            },
        ));
        nodes.push(Node::fill(
            Shape::Circle {
                center: (512.0, 560.0),
                radius: 430.0,
            },
            Paint::Radial {
                center: (512.0, 560.0),
                radius: 430.0,
                stops: vec![stop(0.0, 0xFF4D6D, 0.38), stop(1.0, 0xFF4D6D, 0.0)],
            },
        ));
    }

    // Soft shadow under the fruit.
    nodes.push(
        Node::solid(
            Shape::Ellipse {
                center: at((512.0, 905.0)),
                rx: len(215.0),
                ry: len(26.0),
                rotation: 0.0,
            },
            0x000000,
        )
        .opacity(0.28),
    );

    // The acerola's three lobes share one gradient so they read as one fruit.
    let skin = Paint::Radial {
        center: at((430.0, 500.0)),
        radius: len(470.0),
        stops: vec![
            stop(0.0, 0xFF6B6B, 1.0),
            stop(0.45, 0xE5173F, 1.0),
            stop(1.0, 0x8E0A2B, 1.0),
        ],
    };
    for center in [(445.0, 590.0), (580.0, 590.0), (512.0, 680.0)] {
        nodes.push(Node::fill(
            Shape::Circle {
                center: at(center),
                radius: len(215.0),
            },
            skin.clone(),
        ));
    }

    // Creases between the lobes.
    let creases = [
        (
            (512.0, 400.0),
            [(500.0, 490.0), (500.0, 570.0), (512.0, 640.0)],
        ),
        (
            (512.0, 640.0),
            [(440.0, 660.0), (370.0, 700.0), (312.0, 752.0)],
        ),
        (
            (512.0, 640.0),
            [(584.0, 660.0), (654.0, 700.0), (712.0, 752.0)],
        ),
    ];
    for (start, curve) in creases {
        nodes.push(Node::stroke(path(start, &[curve]), 0x7A0A26, len(10.0)).opacity(0.35));
    }

    // Highlights.
    nodes.push(
        Node::solid(
            Shape::Ellipse {
                center: at((385.0, 525.0)),
                rx: len(68.0),
                ry: len(112.0),
                rotation: 28.0,
            },
            0xFFFFFF,
        )
        .opacity(0.36),
    );
    nodes.push(
        Node::solid(
            Shape::Circle {
                center: at((468.0, 440.0)),
                radius: len(17.0),
            },
            0xFFFFFF,
        )
        .opacity(0.5),
    );

    // Stem.
    nodes.push(Node::stroke(
        path(
            (512.0, 398.0),
            &[[(512.0, 340.0), (530.0, 292.0), (575.0, 252.0)]],
        ),
        0x5E8B2F,
        len(22.0),
    ));

    // Leaf, shaded along its own bounding box.
    let leaf = path(
        (572.0, 262.0),
        &[
            [(600.0, 190.0), (700.0, 170.0), (800.0, 205.0)],
            [(785.0, 290.0), (680.0, 330.0), (572.0, 262.0)],
        ],
    )
    .close();
    let (mut min, mut max) = ((f32::MAX, f32::MAX), (f32::MIN, f32::MIN));
    for (x, y) in leaf.flatten() {
        min = (min.0.min(x), min.1.min(y));
        max = (max.0.max(x), max.1.max(y));
    }
    nodes.push(Node::fill(
        Shape::Path(leaf),
        Paint::Linear {
            from: (min.0, max.1),
            to: (max.0, min.1),
            stops: vec![stop(0.0, 0x2E8B3E, 1.0), stop(1.0, 0x86D95F, 1.0)],
        },
    ));
    nodes.push(
        Node::stroke(
            path(
                (582.0, 258.0),
                &[[(650.0, 240.0), (720.0, 226.0), (782.0, 210.0)]],
            ),
            0x1F5E2B,
            len(7.0),
        )
        .opacity(0.55),
    );

    // Calyx where the stem meets the fruit.
    for (center, rx, ry, rotation) in [
        ((512.0, 400.0), 46.0, 18.0, 0.0),
        ((480.0, 410.0), 30.0, 12.0, -20.0),
        ((544.0, 410.0), 30.0, 12.0, 20.0),
    ] {
        nodes.push(Node::solid(
            Shape::Ellipse {
                center: at(center),
                rx: len(rx),
                ry: len(ry),
                rotation,
            },
            0x3E8E3A,
        ));
    }

    Scene {
        size: CANVAS,
        nodes,
    }
}

/// Formats a number compactly for SVG output.
fn num(value: f32) -> String {
    let text = format!("{value:.2}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    match text {
        "" | "-0" => "0".to_string(),
        other => other.to_string(),
    }
}

impl Scene {
    /// Serialises the scene as a standalone SVG document.
    pub fn to_svg(&self) -> String {
        let mut gradients: Vec<&Paint> = Vec::new();
        let mut body = String::new();

        for node in &self.nodes {
            let mut element = match &node.shape {
                Shape::RoundRect { radius } => format!(
                    "<rect width=\"{0}\" height=\"{0}\" rx=\"{1}\"",
                    num(self.size),
                    num(*radius)
                ),
                Shape::Circle { center, radius } => format!(
                    "<circle cx=\"{}\" cy=\"{}\" r=\"{}\"",
                    num(center.0),
                    num(center.1),
                    num(*radius)
                ),
                Shape::Ellipse {
                    center,
                    rx,
                    ry,
                    rotation,
                } => {
                    let mut text = format!(
                        "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\"",
                        num(center.0),
                        num(center.1),
                        num(*rx),
                        num(*ry)
                    );
                    if *rotation != 0.0 {
                        text.push_str(&format!(
                            " transform=\"rotate({} {} {})\"",
                            num(*rotation),
                            num(center.0),
                            num(center.1)
                        ));
                    }
                    text
                }
                Shape::Path(path) => format!("<path d=\"{}\"", path.svg_data()),
            };

            match &node.style {
                Style::Fill(Paint::Solid(color)) => {
                    element.push_str(&format!(" fill=\"{}\"", color.css()));
                }
                Style::Fill(paint) => {
                    let index = gradients.iter().position(|known| *known == paint);
                    let index = index.unwrap_or_else(|| {
                        gradients.push(paint);
                        gradients.len() - 1
                    });
                    element.push_str(&format!(" fill=\"url(#g{index})\""));
                }
                Style::Stroke { color, width } => element.push_str(&format!(
                    " fill=\"none\" stroke=\"{}\" stroke-width=\"{}\" stroke-linecap=\"round\"",
                    color.css(),
                    num(*width)
                )),
            }
            if node.opacity < 1.0 {
                let attribute = match node.style {
                    Style::Fill(_) => "fill-opacity",
                    Style::Stroke { .. } => "stroke-opacity",
                };
                element.push_str(&format!(" {attribute}=\"{}\"", num(node.opacity)));
            }
            body.push_str(&format!("  {element}/>\n"));
        }

        let mut svg = format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {0} {0}\" width=\"{0}\" height=\"{0}\">\n  <title>Aceris</title>\n",
            num(self.size)
        );
        if !gradients.is_empty() {
            svg.push_str("  <defs>\n");
            for (index, paint) in gradients.iter().enumerate() {
                svg.push_str(&gradient_markup(index, paint));
            }
            svg.push_str("  </defs>\n");
        }
        svg.push_str(&body);
        svg.push_str("</svg>\n");
        svg
    }
}

fn gradient_markup(index: usize, paint: &Paint) -> String {
    let (open, close, stops) = match paint {
        Paint::Solid(_) => return String::new(),
        Paint::Linear { from, to, stops } => (
            format!(
                "<linearGradient id=\"g{index}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">",
                num(from.0),
                num(from.1),
                num(to.0),
                num(to.1)
            ),
            "</linearGradient>",
            stops,
        ),
        Paint::Radial {
            center,
            radius,
            stops,
        } => (
            format!(
                "<radialGradient id=\"g{index}\" gradientUnits=\"userSpaceOnUse\" cx=\"{}\" cy=\"{}\" r=\"{}\">",
                num(center.0),
                num(center.1),
                num(*radius)
            ),
            "</radialGradient>",
            stops,
        ),
    };

    let mut markup = format!("    {open}\n");
    for stop in stops {
        markup.push_str(&format!(
            "      <stop offset=\"{}\" stop-color=\"{}\" stop-opacity=\"{}\"/>\n",
            num(stop.offset),
            stop.color.css(),
            num(stop.opacity)
        ));
    }
    markup.push_str(&format!("    {close}\n"));
    markup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_formatted_compactly() {
        assert_eq!(num(512.0), "512");
        assert_eq!(num(0.28), "0.28");
        assert_eq!(num(-0.001), "0");
        assert_eq!(num(100.5), "100.5");
        assert_eq!(num(0.0), "0");
    }

    #[test]
    fn hex_colours_round_trip() {
        assert_eq!(Rgb::hex(0x4A1D5E), Rgb(0x4A, 0x1D, 0x5E));
        assert_eq!(Rgb::hex(0x4A1D5E).css(), "#4A1D5E");
    }

    #[test]
    fn foreground_has_no_background_or_glow() {
        let foreground = icon(Variant::Foreground);
        let rounded = icon(Variant::Rounded);
        assert_eq!(rounded.nodes.len(), foreground.nodes.len() + 2);
        assert!(matches!(rounded.nodes[0].shape, Shape::RoundRect { .. }));
        assert!(matches!(
            icon(Variant::Circle).nodes[0].shape,
            Shape::Circle { .. }
        ));
    }

    #[test]
    fn flattening_ends_at_the_last_control_point() {
        let path = Path::new((0.0, 0.0)).curve((0.0, 10.0), (10.0, 10.0), (10.0, 0.0));
        let points = path.flatten();
        assert_eq!(points.first(), Some(&(0.0, 0.0)));
        let (x, y) = *points.last().unwrap();
        assert!((x - 10.0).abs() < 1e-4 && y.abs() < 1e-4);
    }

    #[test]
    fn svg_shares_the_fruit_gradient_between_lobes() {
        let svg = icon(Variant::Rounded).to_svg();
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.trim_end().ends_with("</svg>"));
        assert!(svg.contains("<title>Aceris</title>"));
        // Background, glow, fruit skin (shared by three lobes) and leaf.
        assert_eq!(svg.matches("Gradient id=").count(), 4);
        assert_eq!(svg.matches("fill=\"url(#g2)\"").count(), 3);
    }

    #[test]
    fn foreground_svg_has_no_background_gradient() {
        let svg = icon(Variant::Foreground).to_svg();
        assert!(!svg.contains("<rect"));
        assert_eq!(svg.matches("Gradient id=").count(), 2);
    }
}
