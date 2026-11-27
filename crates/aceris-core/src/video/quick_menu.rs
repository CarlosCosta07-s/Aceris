//! Shared, host-independent rendering and hit testing for the in-game quick menu.

use super::font;

pub const ACTION_COUNT: usize = 8;
/// Desktop exposes the six actions that do not depend on Android touch controls.
pub const DESKTOP_ACTION_COUNT: usize = 6;

/// A destructive action that must be confirmed before leaving the game session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmationAction {
    Restart,
    BackToLibrary,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
    Bgrx8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

#[derive(Clone, Copy)]
struct Geometry {
    panel: Rect,
    rows: [Rect; ACTION_COUNT],
    action_count: usize,
    scale: usize,
}

fn geometry(width: u32, height: u32, action_count: usize) -> Option<Geometry> {
    if width < 120
        || height < 100
        || width > 4096
        || height > 4096
        || action_count == 0
        || action_count > ACTION_COUNT
    {
        return None;
    }
    let width = width as usize;
    let height = height as usize;
    let scale = (width.min(height) / 240).clamp(1, 8);
    let margin = 8 * scale;
    let panel_width = (420 * scale).min(width.saturating_sub(margin * 2));
    let panel_height = (224 * scale).min(height.saturating_sub(margin * 2));
    if panel_width < 180 * scale || panel_height < 150 * scale {
        return None;
    }

    let panel = Rect {
        x: (width - panel_width) / 2,
        y: (height - panel_height) / 2,
        width: panel_width,
        height: panel_height,
    };
    let horizontal_padding = 12 * scale;
    let row_top = panel.y + 42 * scale;
    let row_gap = 2 * scale;
    let row_space = panel.height.saturating_sub(52 * scale);
    let row_height = row_space
        .saturating_sub(row_gap * (action_count - 1))
        .checked_div(action_count)?
        .min(32 * scale);
    let row_width = panel.width.saturating_sub(horizontal_padding * 2);
    let rows = std::array::from_fn(|index| {
        if index >= action_count {
            return Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            };
        }
        Rect {
            x: panel.x + horizontal_padding,
            y: row_top + index * (row_height + row_gap),
            width: row_width,
            height: row_height,
        }
    });
    Some(Geometry {
        panel,
        rows,
        action_count,
        scale,
    })
}

#[derive(Clone, Copy)]
struct ConfirmationGeometry {
    panel: Rect,
    rows: [Rect; 2],
    scale: usize,
}

fn confirmation_geometry(width: u32, height: u32) -> Option<ConfirmationGeometry> {
    let base = geometry(width, height, ACTION_COUNT)?;
    let scale = base.scale;
    let horizontal_padding = 12 * scale;
    let row_height = 28 * scale;
    let row_gap = 4 * scale;
    let total_rows_height = row_height * 2 + row_gap;
    let bottom_padding = 10 * scale;
    if total_rows_height + bottom_padding >= base.panel.height {
        return None;
    }
    let row_top = base.panel.y + base.panel.height - total_rows_height - bottom_padding;
    let row_width = base.panel.width.saturating_sub(horizontal_padding * 2);
    let rows = std::array::from_fn(|index| Rect {
        x: base.panel.x + horizontal_padding,
        y: row_top + index * (row_height + row_gap),
        width: row_width,
        height: row_height,
    });
    Some(ConfirmationGeometry {
        panel: base.panel,
        rows,
        scale,
    })
}

fn menu_button_rect(width: u32, height: u32) -> Option<(Rect, usize)> {
    let layout = geometry(width, height, ACTION_COUNT)?;
    let scale = layout.scale;
    let button = Rect {
        x: (width as usize).saturating_sub(76 * scale),
        y: 8 * scale,
        width: 66 * scale,
        height: 26 * scale,
    };
    Some((button, scale))
}

/// Returns true when a point is inside the small Android in-game menu shortcut.
pub fn hit_test_menu_button(width: u32, height: u32, x: i32, y: i32) -> bool {
    let Some((button, _)) = menu_button_rect(width, height) else {
        return false;
    };
    x >= button.x as i32
        && x < (button.x + button.width) as i32
        && y >= button.y as i32
        && y < (button.y + button.height) as i32
}

/// Draws the small in-game menu shortcut used by touch-first frontends.
pub fn render_menu_button(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
) -> bool {
    let Some((button, scale)) = menu_button_rect(width, height) else {
        return false;
    };
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(frame.len()) {
        return false;
    }
    blend_rect(frame, width as usize, button, [45, 30, 62], 220, format);
    draw_border(frame, width as usize, button, scale.max(1), format);
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        button.x + (button.width.saturating_sub(23 * scale * 2)) / 2,
        button.y + (button.height.saturating_sub(7 * scale * 2)) / 2,
        scale * 2,
        "MENU",
        [255, 255, 255],
        format,
    );
    true
}

/// Returns the action row under a surface-space point.
pub fn hit_test(width: u32, height: u32, x: i32, y: i32) -> Option<usize> {
    hit_test_with_action_count(width, height, x, y, ACTION_COUNT)
}

/// Returns the action row under a point for the desktop-only menu.
pub fn hit_test_desktop(width: u32, height: u32, x: i32, y: i32) -> Option<usize> {
    hit_test_with_action_count(width, height, x, y, DESKTOP_ACTION_COUNT)
}

fn hit_test_with_action_count(
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    action_count: usize,
) -> Option<usize> {
    let geometry = geometry(width, height, action_count)?;
    if x < 0 || y < 0 {
        return None;
    }
    geometry.rows.iter().take(action_count).position(|row| {
        let x = x as usize;
        let y = y as usize;
        x >= row.x && x < row.x + row.width && y >= row.y && y < row.y + row.height
    })
}

/// Draws concise controls-editor instructions over the paused game frame.
pub fn render_control_editor_hint(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
) -> bool {
    if width < 160 || height < 100 || width > 4096 || height > 4096 {
        return false;
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(frame.len()) {
        return false;
    }
    let scale = (width.min(height) / 240).clamp(1, 4) as usize;
    let panel = Rect {
        x: 8 * scale,
        y: (height as usize * 42 / 100).saturating_sub(23 * scale),
        width: (width as usize - 116 * scale).min(230 * scale),
        height: 46 * scale,
    };
    blend_rect(frame, width as usize, panel, [31, 19, 48], 232, format);
    draw_border(frame, width as usize, panel, scale, format);
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        panel.x + 10 * scale,
        panel.y + 5 * scale,
        scale,
        "ARRASTE: MOVER",
        [245, 238, 248],
        format,
    );
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        panel.x + 10 * scale,
        panel.y + 18 * scale,
        scale,
        "PINCA: TAMANHO",
        [245, 238, 248],
        format,
    );
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        panel.x + 10 * scale,
        panel.y + 31 * scale,
        scale,
        "VOLTAR: SALVAR",
        [190, 225, 203],
        format,
    );
    true
}

/// Draws dashed vertical and horizontal guides through the selected control center.
pub fn render_control_editor_guides(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    center: (i32, i32),
) -> bool {
    if width == 0 || height == 0 || width > 4096 || height > 4096 {
        return false;
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(frame.len()) {
        return false;
    }

    let scale = (width.min(height) / 240).clamp(1, 4) as usize;
    let x = (center.0.clamp(0, width as i32 - 1)) as usize;
    let y = (center.1.clamp(0, height as i32 - 1)) as usize;
    let thickness = scale.max(1);
    let dash = 5 * scale;
    let period = dash * 2;

    for top in (0..height as usize).step_by(period) {
        blend_rect(
            frame,
            width as usize,
            Rect {
                x: x.saturating_sub(thickness / 2),
                y: top,
                width: thickness,
                height: dash.min(height as usize - top),
            },
            [76, 220, 224],
            184,
            format,
        );
    }
    for left in (0..width as usize).step_by(period) {
        blend_rect(
            frame,
            width as usize,
            Rect {
                x: left,
                y: y.saturating_sub(thickness / 2),
                width: dash.min(width as usize - left),
                height: thickness,
            },
            [76, 220, 224],
            184,
            format,
        );
    }

    let marker_size = (3 * scale).max(2);
    blend_rect(
        frame,
        width as usize,
        Rect {
            x: x.saturating_sub(marker_size / 2),
            y: y.saturating_sub(marker_size / 2),
            width: marker_size.min(width as usize),
            height: marker_size.min(height as usize),
        },
        [245, 238, 248],
        235,
        format,
    );
    true
}

/// Returns the confirmation button under a surface-space point.
pub fn hit_test_confirmation(width: u32, height: u32, x: i32, y: i32) -> Option<usize> {
    let geometry = confirmation_geometry(width, height)?;
    if x < 0 || y < 0 {
        return None;
    }
    geometry.rows.iter().position(|row| {
        let x = x as usize;
        let y = y as usize;
        x >= row.x && x < row.x + row.width && y >= row.y && y < row.y + row.height
    })
}

/// Draws a confirmation panel for Restart or Back to Library without allocating.
pub fn render_confirmation_overlay(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    action: ConfirmationAction,
    selected: usize,
) -> bool {
    let Some(geometry) = confirmation_geometry(width, height) else {
        return false;
    };
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(frame.len()) {
        return false;
    }

    let scale = geometry.scale;
    let text_scale = scale * 2;
    let canvas = Rect {
        x: 0,
        y: 0,
        width: width as usize,
        height: height as usize,
    };
    blend_rect(frame, width as usize, canvas, [7, 5, 16], 176, format);
    blend_rect(
        frame,
        width as usize,
        geometry.panel,
        [31, 19, 48],
        248,
        format,
    );
    draw_border(frame, width as usize, geometry.panel, scale, format);

    let title = match action {
        ConfirmationAction::Restart => "REINICIAR JOGO?",
        ConfirmationAction::BackToLibrary => "VOLTAR À BIBLIOTECA?",
    };
    let text_x = geometry.panel.x + 18 * scale;
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        text_x,
        geometry.panel.y + 12 * scale,
        text_scale,
        title,
        [245, 238, 248],
        format,
    );
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        text_x,
        geometry.panel.y + 42 * scale,
        text_scale,
        "Progresso não salvo",
        [230, 220, 236],
        format,
    );
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        text_x,
        geometry.panel.y + 58 * scale,
        text_scale,
        "pode ser perdido.",
        [230, 220, 236],
        format,
    );

    let labels = ["CANCELAR", "CONFIRMAR"];
    for (index, row) in geometry.rows.iter().take(geometry.action_count).enumerate() {
        let is_selected = index == selected.min(1);
        blend_rect(
            frame,
            width as usize,
            *row,
            if is_selected { [166, 29, 61] } else { [54, 36, 70] },
            if is_selected { 245 } else { 230 },
            format,
        );
        let label_width = font::text_width(labels[index].chars().count(), text_scale);
        let text_x = row.x + row.width.saturating_sub(label_width) / 2;
        let text_y = row.y + (row.height.saturating_sub(7 * text_scale)) / 2;
        draw_text_for_format(
            frame,
            width as usize,
            height as usize,
            text_x,
            text_y,
            text_scale,
            labels[index],
            [255, 255, 255],
            format,
        );
    }
    true
}

/// Draws a dimmed, seven-action quick menu over an existing RGBA8 or BGRX8 frame.
///
/// The function does not allocate and leaves the frame unchanged when its
/// dimensions or byte length are invalid.
pub fn render_overlay(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    selected: usize,
) -> bool {
    render_overlay_with_profile(frame, width, height, format, selected, "PADRÃO GLOBAL")
}

/// Draws the desktop quick menu without Android-only virtual-touch actions.
pub fn render_desktop_overlay_with_profile(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    selected: usize,
    profile_label: &str,
) -> bool {
    render_overlay_with_action_count(
        frame,
        width,
        height,
        format,
        selected,
        profile_label,
        DESKTOP_ACTION_COUNT,
    )
}

/// Draws the quick menu and identifies whether the active layout uses global or
/// title-specific preferences.
pub fn render_overlay_with_profile(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    selected: usize,
    profile_label: &str,
) -> bool {
    render_overlay_with_action_count(
        frame,
        width,
        height,
        format,
        selected,
        profile_label,
        ACTION_COUNT,
    )
}

fn render_overlay_with_action_count(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    selected: usize,
    profile_label: &str,
    action_count: usize,
) -> bool {
    let Some(geometry) = geometry(width, height, action_count) else {
        return false;
    };
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(frame.len()) {
        return false;
    }

    let scale = geometry.scale;
    let text_scale = scale * 2;
    let canvas = Rect {
        x: 0,
        y: 0,
        width: width as usize,
        height: height as usize,
    };
    blend_rect(frame, width as usize, canvas, [7, 5, 16], 164, format);
    blend_rect(
        frame,
        width as usize,
        geometry.panel,
        [31, 19, 48],
        246,
        format,
    );
    draw_border(frame, width as usize, geometry.panel, scale, format);

    let title_x = geometry.panel.x + 18 * scale;
    let title_y = geometry.panel.y + 12 * scale;
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        title_x,
        title_y,
        text_scale,
        "MENU DO JOGO",
        [245, 238, 248],
        format,
    );
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        title_x,
        geometry.panel.y + 28 * scale,
        scale,
        profile_label,
        [190, 225, 203],
        format,
    );

    let labels = [
        "CONTINUAR",
        "REINICIAR",
        "VOLTAR À BIBLIOTECA",
        "ALTERAR LAYOUT",
        "TROCAR TELAS",
        "PADRÃO GLOBAL",
        "RESTAURAR CONTROLES",
        "EDITAR CONTROLES",
    ];
    for (index, row) in geometry.rows.iter().take(geometry.action_count).enumerate() {
        let is_selected = index == selected.min(action_count - 1);
        blend_rect(
            frame,
            width as usize,
            *row,
            if is_selected { [166, 29, 61] } else { [54, 36, 70] },
            if is_selected { 245 } else { 230 },
            format,
        );
        let text_y = row.y + (row.height.saturating_sub(7 * text_scale)) / 2;
        draw_text_for_format(
            frame,
            width as usize,
            height as usize,
            row.x + 12 * scale,
            text_y,
            text_scale,
            labels[index],
            [255, 255, 255],
            format,
        );
    }
    true
}

fn draw_border(
    frame: &mut [u8],
    width: usize,
    rect: Rect,
    thickness: usize,
    format: PixelFormat,
) {
    let color = [211, 52, 85];
    blend_rect(
        frame,
        width,
        Rect { height: thickness, ..rect },
        color,
        255,
        format,
    );
    blend_rect(
        frame,
        width,
        Rect {
            y: rect.y + rect.height.saturating_sub(thickness),
            height: thickness,
            ..rect
        },
        color,
        255,
        format,
    );
    blend_rect(
        frame,
        width,
        Rect { width: thickness, ..rect },
        color,
        255,
        format,
    );
    blend_rect(
        frame,
        width,
        Rect {
            x: rect.x + rect.width.saturating_sub(thickness),
            width: thickness,
            ..rect
        },
        color,
        255,
        format,
    );
}

fn blend_rect(
    frame: &mut [u8],
    width: usize,
    rect: Rect,
    color: [u8; 3],
    alpha: u8,
    format: PixelFormat,
) {
    let height = frame.len() / 4 / width;
    let left = rect.x.min(width);
    let right = rect.x.saturating_add(rect.width).min(width);
    let top = rect.y.min(height);
    let bottom = rect.y.saturating_add(rect.height).min(height);
    let inverse = 255u32 - u32::from(alpha);
    let source = channel_order(color, format);
    for y in top..bottom {
        for x in left..right {
            let at = (y * width + x) * 4;
            for channel in 0..3 {
                let destination = u32::from(frame[at + channel]);
                frame[at + channel] =
                    ((u32::from(source[channel]) * u32::from(alpha)
                        + destination * inverse
                        + 127)
                        / 255) as u8;
            }
            if format == PixelFormat::Rgba8 {
                frame[at + 3] = 255;
            }
        }
    }
}

fn channel_order(color: [u8; 3], format: PixelFormat) -> [u8; 3] {
    match format {
        PixelFormat::Rgba8 => color,
        PixelFormat::Bgrx8 => [color[2], color[1], color[0]],
    }
}

fn draw_text_for_format(
    frame: &mut [u8],
    width: usize,
    height: usize,
    x: usize,
    y: usize,
    scale: usize,
    value: &str,
    color: [u8; 3],
    format: PixelFormat,
) {
    let color = channel_order(color, format);
    font::draw_text(
        frame,
        width,
        height,
        x as i32,
        y as i32,
        scale,
        value,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_testing_matches_the_drawn_action_rows_on_phone_and_desktop() {
        for (width, height) in [(736, 240), (2400, 1080), (800, 960), (400, 800), (320, 240)] {
            let geometry = geometry(width, height, ACTION_COUNT).unwrap();
            for (index, row) in geometry.rows.iter().take(geometry.action_count).enumerate() {
                let x = (row.x + row.width / 2) as i32;
                let y = (row.y + row.height / 2) as i32;
                assert_eq!(hit_test(width, height, x, y), Some(index));
                assert!(row.x + row.width <= width as usize);
                assert!(row.y + row.height <= height as usize);
            }
            assert_eq!(hit_test(width, height, 0, 0), None);
        }
    }

    #[test]
    fn profile_label_renders_on_compact_phone_and_desktop_canvases() {
        for (width, height) in [(320u32, 240u32), (400, 800), (736, 240), (2400, 1080)] {
            for (label, format) in [
                ("PADRÃO GLOBAL", PixelFormat::Rgba8),
                ("PERFIL DESTE JOGO", PixelFormat::Bgrx8),
            ] {
                let mut frame = vec![0u8; width as usize * height as usize * 4];
                assert!(render_overlay_with_profile(&mut frame, width, height, format, 0, label,));
                assert_eq!(frame.len(), width as usize * height as usize * 4);
                assert!(frame.iter().any(|channel| *channel != 0));
            }
        }
    }

    #[test]
    fn eighth_action_renders_and_remains_reachable_on_compact_canvas() {
        let (width, height) = (320u32, 240u32);
        let mut frame = vec![0u8; width as usize * height as usize * 4];
        assert!(render_overlay(
            &mut frame,
            width,
            height,
            PixelFormat::Rgba8,
            ACTION_COUNT - 1,
        ));
        let geometry = geometry(width, height, ACTION_COUNT).unwrap();
        let last = &geometry.rows[ACTION_COUNT - 1];
        assert!(last.y + last.height <= height as usize);
        assert_eq!(
            hit_test(
                width,
                height,
                (last.x + last.width / 2) as i32,
                (last.y + last.height / 2) as i32,
            ),
            Some(ACTION_COUNT - 1)
        );
    }

    #[test]
    fn desktop_menu_hides_android_touch_editor_actions() {
        let (width, height) = (400u32, 800u32);
        let mut frame = vec![0u8; width as usize * height as usize * 4];
        assert!(render_desktop_overlay_with_profile(
            &mut frame,
            width,
            height,
            PixelFormat::Rgba8,
            DESKTOP_ACTION_COUNT - 1,
            "PADRÃO GLOBAL",
        ));
        let desktop_geometry = geometry(width, height, DESKTOP_ACTION_COUNT).unwrap();
        assert_eq!(desktop_geometry.action_count, DESKTOP_ACTION_COUNT);
        let last = &desktop_geometry.rows[DESKTOP_ACTION_COUNT - 1];
        assert_eq!(
            hit_test_desktop(
                width,
                height,
                (last.x + last.width / 2) as i32,
                (last.y + last.height / 2) as i32,
            ),
            Some(DESKTOP_ACTION_COUNT - 1)
        );
        let android_geometry = geometry(width, height, ACTION_COUNT).unwrap();
        for index in DESKTOP_ACTION_COUNT..ACTION_COUNT {
            let row = android_geometry.rows[index];
            assert_eq!(
                hit_test(
                    width,
                    height,
                    (row.x + row.width / 2) as i32,
                    (row.y + row.height / 2) as i32,
                ),
                Some(index)
            );
        }
        for row in &desktop_geometry.rows {
            let action = hit_test_desktop(
                width,
                height,
                (row.x + row.width / 2) as i32,
                (row.y + row.height / 2) as i32,
            );
            assert!(action.is_some_and(|index| index < DESKTOP_ACTION_COUNT));
        }
    }

    #[test]
    fn control_editor_alignment_guides_follow_the_selected_center() {
        for format in [PixelFormat::Rgba8, PixelFormat::Bgrx8] {
            let (width, height) = (400u32, 800u32);
            let mut frame = vec![0u8; width as usize * height as usize * 4];
            let original = frame.clone();
            assert!(render_control_editor_guides(
                &mut frame,
                width,
                height,
                format,
                (120, 360),
            ));
            let guide_pixel = (360usize * width as usize + 120) * 4;
            let outside_pixel = (10usize * width as usize + 10) * 4;
            assert_ne!(&frame[guide_pixel..guide_pixel + 3], &original[guide_pixel..guide_pixel + 3]);
            assert_eq!(
                &frame[outside_pixel..outside_pixel + 3],
                &original[outside_pixel..outside_pixel + 3]
            );
        }

        let mut invalid = vec![0u8; 4];
        assert!(!render_control_editor_guides(
            &mut invalid,
            0,
            1,
            PixelFormat::Rgba8,
            (0, 0),
        ));
    }

    #[test]
    fn android_menu_shortcut_has_a_matching_render_and_hit_target() {
        let (width, height) = (2400u32, 1080u32);
        let mut frame = vec![0u8; width as usize * height as usize * 4];
        assert!(render_menu_button(&mut frame, width, height, PixelFormat::Rgba8));
        let mut target = None;
        for y in 0..height as i32 {
            for x in 0..width as i32 {
                if hit_test_menu_button(width, height, x, y) {
                    target = Some((x, y));
                    break;
                }
            }
            if target.is_some() {
                break;
            }
        }
        let (x, y) = target.expect("visible menu button has a hit target");
        assert!(hit_test_menu_button(width, height, x, y));
        assert!(!hit_test_menu_button(width, height, 0, 0));
    }

    #[test]
    fn confirmation_hit_testing_matches_cancel_and_confirm_rows() {
        for (width, height) in [(736, 240), (2400, 1080), (800, 960), (400, 800), (320, 240)] {
            let geometry = confirmation_geometry(width, height).unwrap();
            for (index, row) in geometry.rows.iter().take(geometry.action_count).enumerate() {
                let x = (row.x + row.width / 2) as i32;
                let y = (row.y + row.height / 2) as i32;
                assert_eq!(hit_test_confirmation(width, height, x, y), Some(index));
            }
            assert_eq!(hit_test_confirmation(width, height, 0, 0), None);
        }
    }

    #[test]
    fn confirmation_overlay_supports_both_pixel_formats_and_actions() {
        for format in [PixelFormat::Rgba8, PixelFormat::Bgrx8] {
            for action in [
                ConfirmationAction::Restart,
                ConfirmationAction::BackToLibrary,
            ] {
                let (width, height) = (736u32, 240u32);
                let mut frame = vec![40u8; width as usize * height as usize * 4];
                let length = frame.len();
                assert!(render_confirmation_overlay(
                    &mut frame,
                    width,
                    height,
                    format,
                    action,
                    1,
                ));
                assert_eq!(frame.len(), length);
                assert_ne!(frame, vec![40u8; length]);
                if format == PixelFormat::Rgba8 {
                    assert!(frame.chunks_exact(4).all(|pixel| pixel[3] == 255));
                }
            }
        }
    }

    #[test]
    fn overlay_supports_rgba_and_bgrx_without_allocating_or_changing_frame_size() {
        for format in [PixelFormat::Rgba8, PixelFormat::Bgrx8] {
            let (width, height) = (736u32, 240u32);
            let mut frame = vec![40u8; width as usize * height as usize * 4];
            let length = frame.len();
            assert!(render_overlay(&mut frame, width, height, format, 1));
            assert_eq!(frame.len(), length);
            assert_ne!(frame, vec![40u8; length]);
            if format == PixelFormat::Rgba8 {
                assert!(frame.chunks_exact(4).all(|pixel| pixel[3] == 255));
            }
        }
    }

    #[test]
    fn invalid_dimensions_or_buffer_lengths_leave_the_frame_unchanged() {
        let mut frame = vec![17u8; 16];
        let before = frame.clone();
        assert!(!render_overlay(&mut frame, 0, 0, PixelFormat::Rgba8, 0));
        assert!(!render_overlay(&mut frame, 736, 240, PixelFormat::Rgba8, 0));
        assert_eq!(frame, before);
        assert_eq!(hit_test(0, 0, 2, 2), None);
    }
}
