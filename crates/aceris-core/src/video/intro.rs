//! Animated start-up sequence ("abertura") shown before a game boots.
//!
//! The animation is original and fully procedural: the acerola from the icon
//! drops in, bounces with squash and stretch, glints, and the wordmark pops up
//! letter by letter before the picture fades out. Frames are produced by a
//! pure function of the frame number, so every frontend (desktop window,
//! Android surface, PNG dumps) renders exactly the same sequence.

use aceris_common::brand;

/// Frames per second the sequence is designed for.
pub const FPS: u32 = 60;
/// Total number of frames (2.5 seconds).
pub const FRAME_COUNT: u32 = 150;

/// Frame with the finished logo, held on screen while waiting for a game.
pub const LOGO_FRAME: u32 = 120;

const DROP_END: f32 = 34.0;
const BOUNCE_1_END: f32 = 52.0;
const BOUNCE_2_END: f32 = 64.0;
const SHINE_START: f32 = 70.0;
const SHINE_END: f32 = 96.0;
const WORD_START: f32 = 84.0;
const FADE_START: f32 = 134.0;

type V2 = (f32, f32);

fn rgb(color: u32) -> [f32; 3] {
    [
        ((color >> 16) & 0xFF) as f32,
        ((color >> 8) & 0xFF) as f32,
        (color & 0xFF) as f32,
    ]
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// Blends `color` over `pixel` with the given coverage/alpha.
fn over(pixel: &mut [f32; 3], color: [f32; 3], alpha: f32) {
    *pixel = mix(*pixel, color, alpha);
}

/// Anti-aliased coverage from a signed distance in pixels.
fn coverage(distance: f32) -> f32 {
    (0.5 - distance).clamp(0.0, 1.0)
}

fn ease_out_back(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0) - 1.0;
    1.0 + 2.70158 * t * t * t + 1.70158 * t * t
}

fn length(v: V2) -> f32 {
    (v.0 * v.0 + v.1 * v.1).sqrt()
}

fn circle(p: V2, center: V2, radius: f32) -> f32 {
    length((p.0 - center.0, p.1 - center.1)) - radius
}

/// Approximate signed distance to a rotated ellipse.
fn ellipse(p: V2, center: V2, radii: V2, angle: f32) -> f32 {
    let (s, c) = angle.sin_cos();
    let d = (p.0 - center.0, p.1 - center.1);
    let q = (d.0 * c + d.1 * s, -d.0 * s + d.1 * c);
    (length((q.0 / radii.0, q.1 / radii.1)) - 1.0) * radii.0.min(radii.1)
}

fn segment(p: V2, a: V2, b: V2, width: f32) -> f32 {
    let pa = (p.0 - a.0, p.1 - a.1);
    let ba = (b.0 - a.0, b.1 - a.1);
    let h = ((pa.0 * ba.0 + pa.1 * ba.1) / (ba.0 * ba.0 + ba.1 * ba.1)).clamp(0.0, 1.0);
    length((pa.0 - ba.0 * h, pa.1 - ba.1 * h)) - width / 2.0
}

/// 5x7 glyphs for the wordmark, one row per entry (bit 4 is the left column).
const GLYPHS: [[u8; 7]; 6] = [
    [
        0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
    ], // A
    [
        0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110,
    ], // C
    [
        0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
    ], // E
    [
        0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
    ], // R
    [
        0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b11111,
    ], // I
    [
        0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
    ], // S
];

/// Height of the fruit's vertical motion and squash for `frame`.
/// Returns (vertical offset in fruit units, squash amount 0..1).
fn motion(frame: f32) -> (f32, f32) {
    const DROP_HEIGHT: f32 = 1100.0;
    const BOUNCE_1: f32 = 230.0;
    const BOUNCE_2: f32 = 70.0;
    let arc = |t: f32, height: f32| -height * (1.0 - (2.0 * t - 1.0).powi(2));
    let offset = if frame < DROP_END {
        let t = frame / DROP_END;
        -DROP_HEIGHT * (1.0 - t * t)
    } else if frame < BOUNCE_1_END {
        arc((frame - DROP_END) / (BOUNCE_1_END - DROP_END), BOUNCE_1)
    } else if frame < BOUNCE_2_END {
        arc(
            (frame - BOUNCE_1_END) / (BOUNCE_2_END - BOUNCE_1_END),
            BOUNCE_2,
        )
    } else {
        0.0
    };
    let impact = |at: f32, span: f32| (1.0 - (frame - at).abs() / span).max(0.0);
    let squash = (0.20 * impact(DROP_END, 5.0))
        .max(0.10 * impact(BOUNCE_1_END, 4.0))
        .max(0.04 * impact(BOUNCE_2_END, 3.0));
    (offset, squash)
}

/// Renders one frame as tightly packed RGBA8 (alpha always 255).
///
/// `frame` is clamped to the last frame. Any size works; the layout is
/// proportional to `width`, and the picture is centred vertically.
pub fn render_frame(frame: u32, width: usize, height: usize) -> Vec<u8> {
    let f = frame.min(FRAME_COUNT - 1) as f32;
    let mut out = vec![255u8; width * height * 4];
    if width == 0 || height == 0 {
        return out;
    }
    let (w, h) = (width as f32, height as f32);
    let k = w * 0.40 / 630.0; // pixels per fruit unit
    let (cx, ground) = (w * 0.5, h * 0.50);
    let (offset, squash) = motion(f);
    let (sx, sy) = (1.0 + 0.55 * squash, 1.0 - squash);

    let bg_top = rgb(brand::PURPLE_LIGHT);
    let bg_bottom = rgb(brand::PURPLE_DARK);
    let red_light = rgb(brand::ACCENT_RED_LIGHT);
    let red = rgb(brand::ACCENT_RED);
    let red_dark = rgb(brand::ACCENT_RED_DARK);
    let green = rgb(brand::ACCENT_GREEN);
    let green_light = rgb(brand::ACCENT_GREEN_LIGHT);
    let text = rgb(brand::TEXT);

    let fade_in = (f / 12.0).clamp(0.0, 1.0);
    let fade_out =
        1.0 - ((f - FADE_START) / (FRAME_COUNT as f32 - 1.0 - FADE_START)).clamp(0.0, 1.0);
    let brightness = fade_in * fade_out;

    // Word-mark layout: 6 glyphs of 5 cells with one cell between them.
    let cell = w * 0.80 / 35.0;
    let word_left = (w - 35.0 * cell) / 2.0;
    let word_top = ground + 90.0 * k;

    // Light sweeping across the fruit.
    let shine_t = ((f - SHINE_START) / (SHINE_END - SHINE_START)).clamp(0.0, 1.0);
    let shine_active = (SHINE_START..=SHINE_END).contains(&f);
    let shine_pos = -420.0 + shine_t * 840.0;

    for y in 0..height {
        for x in 0..width {
            let p = (x as f32 + 0.5, y as f32 + 0.5);
            // Background gradient with a soft vignette.
            let gy = y as f32 / h;
            let mut px = mix(bg_top, bg_bottom, gy * 0.9 + 0.1 * (x as f32 / w));
            let vignette =
                1.0 - 0.35 * (length(((p.0 - cx) / w, (p.1 - h * 0.5) / h)) * 1.6).powi(2);
            px = mix([0.0; 3], px, vignette.clamp(0.0, 1.0));

            // Glow behind the fruit.
            let glow = (1.0 - length((p.0 - cx, p.1 - (ground - 300.0 * k))) / (w * 0.55)).max(0.0);
            over(&mut px, red, glow * glow * 0.22);

            // Ground shadow shrinks as the fruit rises.
            let lift = (-offset / 1100.0).clamp(0.0, 1.0);
            let shadow_scale = 1.0 - 0.6 * lift;
            let shadow = ellipse(
                p,
                (cx, ground + 6.0 * k),
                (240.0 * k * shadow_scale, 30.0 * k * shadow_scale),
                0.0,
            );
            over(
                &mut px,
                [0.0; 3],
                coverage(shadow) * 0.30 * (1.0 - 0.7 * lift),
            );

            // Fruit, in local units: origin at the fruit's centre, y down.
            let contact = ground;
            let fx = (p.0 - cx) / (k * sx);
            let fy = (p.1 - contact) / (k * sy) + 340.0 - offset;
            let lp = (fx, fy);
            let body = circle(lp, (-76.0, 0.0), 240.0)
                .min(circle(lp, (76.0, 0.0), 240.0))
                .min(circle(lp, (0.0, 100.0), 240.0));
            let sway = (f * 0.18).sin() * 0.06 * (f / 60.0).min(1.0);
            let stem = segment(lp, (0.0, -215.0), (70.0 + sway * 200.0, -380.0), 26.0);
            let leaf_c = (190.0 + sway * 300.0, -385.0);
            let leaf = ellipse(lp, leaf_c, (140.0, 48.0), -0.28 + sway);
            over(
                &mut px,
                mix(green, green_light, 0.2),
                coverage(stem * k * sx.min(sy)),
            );
            let leaf_shade = ((lp.0 - leaf_c.0) / 280.0 + 0.5).clamp(0.0, 1.0);
            over(
                &mut px,
                mix(green, green_light, leaf_shade),
                coverage(leaf * k * sx.min(sy)),
            );

            let body_cov = coverage(body * k * sx.min(sy));
            if body_cov > 0.0 {
                let d = length((lp.0 + 90.0, lp.1 + 110.0)) / 520.0;
                let mut c = if d < 0.45 {
                    mix(red_light, red, d / 0.45)
                } else {
                    mix(red, red_dark, (d - 0.45) / 0.55)
                };
                let gloss = ellipse(lp, (-140.0, -75.0), (76.0, 125.0), 0.49);
                c = mix(c, [255.0; 3], coverage(gloss * k) * 0.36);
                if shine_active {
                    let band = ((lp.0 + lp.1 * 0.6) - shine_pos).abs();
                    c = mix(c, [255.0; 3], (1.0 - band / 70.0).max(0.0) * 0.55);
                }
                over(&mut px, c, body_cov);
            }

            // Wordmark, letter by letter.
            let (lx, ly) = ((p.0 - word_left) / cell, (p.1 - word_top) / cell);
            if (0.0..35.0).contains(&lx) && (0.0..7.0).contains(&ly) {
                let slot = (lx / 6.0).floor() as usize;
                let col = (lx - slot as f32 * 6.0).floor() as usize;
                if slot < 6 && col < 5 {
                    let row = ly.floor() as usize;
                    if GLYPHS[slot][row] >> (4 - col) & 1 == 1 {
                        let t = ((f - WORD_START - slot as f32 * 4.0) / 14.0).clamp(0.0, 1.0);
                        if t > 0.0 {
                            let rise = (1.0 - ease_out_back(t)) * 2.5;
                            let inside = ((ly - rise) >= 0.0 && (ly - rise) < 7.0) as u8 as f32;
                            let a = inside * t;
                            let tint = mix(text, red_light, (1.0 - t) * 0.8);
                            over(&mut px, tint, a);
                        }
                    }
                }
            }

            let at = (y * width + x) * 4;
            for (i, channel) in px.iter().enumerate() {
                out[at + i] = (channel * brightness).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// Blits an RGBA frame into BGRX, enlarged by an integer `scale`.
pub fn rgba_to_scaled_bgrx(rgba: &[u8], width: usize, scale: usize) -> Vec<u8> {
    let scale = scale.max(1);
    let mut out = Vec::with_capacity(rgba.len() * scale * scale);
    for row in rgba.chunks_exact(width * 4) {
        let mut line = Vec::with_capacity(width * 4 * scale);
        for pixel in row.chunks_exact(4) {
            for _ in 0..scale {
                line.extend_from_slice(&[pixel[2], pixel[1], pixel[0], 0xFF]);
            }
        }
        for _ in 0..scale {
            out.extend_from_slice(&line);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: usize = 200;
    const H: usize = 240;

    fn at(frame: &[u8], x: usize, y: usize) -> [u8; 3] {
        let i = (y * W + x) * 4;
        [frame[i], frame[i + 1], frame[i + 2]]
    }

    fn lit(frame: &[u8]) -> usize {
        frame
            .chunks_exact(4)
            .filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30)
            .count()
    }

    #[test]
    fn frames_have_the_requested_size_and_are_opaque() {
        let frame = render_frame(60, W, H);
        assert_eq!(frame.len(), W * H * 4);
        assert!(frame.chunks_exact(4).all(|p| p[3] == 255));
        assert_eq!(render_frame(0, 0, 0).len(), 0);
    }

    #[test]
    fn rendering_is_deterministic_and_clamps_past_the_end() {
        assert_eq!(render_frame(77, W, H), render_frame(77, W, H));
        assert_eq!(
            render_frame(FRAME_COUNT + 50, W, H),
            render_frame(FRAME_COUNT - 1, W, H)
        );
    }

    #[test]
    fn sequence_starts_and_ends_dark() {
        assert_eq!(lit(&render_frame(0, W, H)), 0);
        assert_eq!(lit(&render_frame(FRAME_COUNT - 1, W, H)), 0);
    }

    #[test]
    fn fruit_is_off_screen_while_dropping_and_red_once_landed() {
        let early = render_frame(4, W, H);
        let landed = render_frame(110, W, H);
        let red = |p: [u8; 3]| p[0] > 150 && p[1] < 90;
        assert!(!(0..H).any(|y| red(at(&early, W / 2, y))));
        assert!((0..H).any(|y| red(at(&landed, W / 2, y))));
    }

    #[test]
    fn wordmark_appears_after_the_fruit_lands() {
        let before = render_frame(80, W, H);
        let after = render_frame(120, W, H);
        let bright = |frame: &[u8]| {
            frame
                .chunks_exact(4)
                .filter(|p| p[0] > 220 && p[1] > 210 && p[2] > 220)
                .count()
        };
        assert!(bright(&after) > bright(&before) + 50);
    }

    #[test]
    fn scaling_swaps_to_bgrx_and_repeats_pixels() {
        let rgba = [1, 2, 3, 255, 4, 5, 6, 255];
        let out = rgba_to_scaled_bgrx(&rgba, 2, 2);
        assert_eq!(out.len(), 2 * 2 * 2 * 4);
        assert_eq!(&out[0..8], &[3, 2, 1, 255, 3, 2, 1, 255]);
        assert_eq!(&out[16..20], &out[0..4]);
    }
}
