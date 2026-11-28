//! Shared, host-independent rendering and hit testing for the in-game quick menu.

use super::font;
use crate::input_mapping::{GamepadAxisSettings, GamepadControl, GamepadMapping, PadButton};
use aceris_common::brand;

pub const ACTION_COUNT: usize = 13;
/// Stable index for the Android-only virtual control editor action.
pub const ACTION_EDIT_CONTROLS: usize = 8;
/// Stable index for the screenshot action shared by desktop and Android.
pub const ACTION_CAPTURE_SCREENSHOT: usize = 9;
/// Stable index for the Android diagnostics action.
pub const ACTION_DIAGNOSTICS: usize = 10;
/// Opens the physical gamepad mapping editor on desktop and Android.
pub const ACTION_EDIT_GAMEPAD_MAPPING: usize = 11;
/// Android turbo (alternate speed) action; its label shows the current speed.
pub const ACTION_TURBO: usize = 12;
/// Number of actions exposed by the desktop quick menu.
pub const DESKTOP_ACTION_COUNT: usize = 9;

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
pub fn render_menu_button(frame: &mut [u8], width: u32, height: u32, format: PixelFormat) -> bool {
    let Some((button, scale)) = menu_button_rect(width, height) else {
        return false;
    };
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(frame.len()) {
        return false;
    }
    blend_rounded_rect(
        frame,
        width as usize,
        button,
        brand::rgb(brand::SURFACE_BUTTON),
        232,
        7 * scale,
        format,
    );
    draw_border(frame, width as usize, button, scale.max(1), format);
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        button.x + (button.width.saturating_sub(23 * scale * 2)) / 2,
        button.y + (button.height.saturating_sub(7 * scale * 2)) / 2,
        scale * 2,
        "MENU",
        brand::rgb(brand::TEXT),
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
    blend_rounded_rect(
        frame,
        width as usize,
        panel,
        brand::rgb(brand::SURFACE_PANEL),
        238,
        9 * scale,
        format,
    );
    draw_border(frame, width as usize, panel, scale, format);
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        panel.x + 10 * scale,
        panel.y + 5 * scale,
        scale,
        "ARRASTE: MOVER",
        brand::rgb(brand::TEXT),
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
        brand::rgb(brand::TEXT),
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
        brand::rgb(brand::ACCENT_GREEN_LIGHT),
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
            brand::rgb(brand::ACCENT_RED_LIGHT),
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
            brand::rgb(brand::ACCENT_RED_LIGHT),
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
        brand::rgb(brand::TEXT),
        235,
        format,
    );
    true
}

/// Draws a compact developer overlay over a composed RGBA8 or BGRX8 gameplay frame.
///
/// FPS and timing data are explicitly supplied by the host monitor. The guest PC
/// and unhandled IPC count are read from the emulated system and are not inferred
/// from host presentation timing.
pub fn render_diagnostics_overlay(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    guest_pc: u32,
    unhandled_requests: usize,
    first_unhandled: Option<&str>,
    host_performance: Option<&str>,
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

    let scale = (width.min(height) / 480).clamp(1, 3) as usize;
    let margin = 8 * scale;
    let panel_width = (width as usize).saturating_sub(margin * 2).min(480 * scale);
    let max_chars = panel_width
        .saturating_sub(20 * scale)
        .checked_div(6 * scale)
        .unwrap_or(1)
        .max(1);
    let mut lines = Vec::with_capacity(4);
    lines.push(format!("ARM11 PC: 0x{guest_pc:08X}"));
    lines.push(format!("IPC PENDENTE: {unhandled_requests}"));
    if let Some(request) = first_unhandled {
        lines.push(format!("ULTIMO IPC: {}", truncate_text(request, max_chars)));
    }
    lines.push(format!(
        "HOST: {}",
        host_performance
            .map(|summary| truncate_text(summary, max_chars.saturating_sub(6)))
            .unwrap_or_else(|| "METRICAS DESLIGADAS".to_owned())
    ));

    let panel_height = (18 + lines.len() * 10) * scale;
    let panel = Rect {
        x: margin,
        y: margin,
        width: panel_width,
        height: panel_height.min(height as usize - margin),
    };
    blend_rounded_rect(
        frame,
        width as usize,
        panel,
        brand::rgb(brand::SURFACE_BASE),
        236,
        8 * scale,
        format,
    );
    draw_colored_border(
        frame,
        width as usize,
        panel,
        scale,
        brand::rgb(brand::BORDER),
        format,
    );
    let accent = Rect {
        height: (3 * scale).min(panel.height),
        ..panel
    };
    blend_rect(
        frame,
        width as usize,
        accent,
        brand::rgb(brand::ACCENT_RED),
        255,
        format,
    );
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        panel.x + 10 * scale,
        panel.y + 6 * scale,
        scale,
        "ACERIS | DIAGNOSTICO",
        brand::rgb(brand::TEXT),
        format,
    );
    for (index, line) in lines.iter().enumerate() {
        draw_text_for_format(
            frame,
            width as usize,
            height as usize,
            panel.x + 10 * scale,
            panel.y + (18 + index * 10) * scale,
            scale,
            line,
            brand::rgb(brand::ACCENT_GREEN_LIGHT),
            format,
        );
    }
    true
}

fn truncate_text(text: &str, maximum_chars: usize) -> String {
    text.chars().take(maximum_chars).collect()
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
    let title = match action {
        ConfirmationAction::Restart => "REINICIAR JOGO?",
        ConfirmationAction::BackToLibrary => "VOLTAR À BIBLIOTECA?",
    };
    render_prompt_overlay(
        frame,
        width,
        height,
        format,
        title,
        ["Progresso não salvo", "pode ser perdido."],
        ["CANCELAR", "CONFIRMAR"],
        selected,
    )
}

/// Draws a two-choice prompt with the confirmation panel's geometry, so
/// [`hit_test_confirmation`] also maps touches for it (row 0 is the first
/// label, row 1 the second).
#[allow(clippy::too_many_arguments)]
pub fn render_prompt_overlay(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    title: &str,
    lines: [&str; 2],
    labels: [&str; 2],
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
    blend_rect(
        frame,
        width as usize,
        canvas,
        brand::rgb(brand::SURFACE_BASE),
        176,
        format,
    );
    draw_panel_chrome(frame, width as usize, geometry.panel, scale, format);

    let text_x = geometry.panel.x + 18 * scale;
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        text_x,
        geometry.panel.y + 12 * scale,
        text_scale,
        title,
        brand::rgb(brand::TEXT),
        format,
    );
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        text_x,
        geometry.panel.y + 42 * scale,
        text_scale,
        lines[0],
        brand::rgb(brand::TEXT),
        format,
    );
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        text_x,
        geometry.panel.y + 58 * scale,
        text_scale,
        lines[1],
        brand::rgb(brand::TEXT),
        format,
    );

    for (index, row) in geometry.rows.iter().enumerate() {
        let is_selected = index == selected.min(1);
        blend_rounded_rect(
            frame,
            width as usize,
            *row,
            if is_selected {
                brand::rgb(brand::SURFACE_SELECTED)
            } else {
                brand::rgb(brand::SURFACE_CARD)
            },
            if is_selected { 250 } else { 230 },
            7 * scale,
            format,
        );
        if is_selected {
            blend_rect(
                frame,
                width as usize,
                Rect {
                    x: row.x,
                    y: row.y,
                    width: (3 * scale).min(row.width),
                    height: row.height,
                },
                brand::rgb(brand::ACCENT_RED_LIGHT),
                255,
                format,
            );
        }
        let label_width = font::measure_text(labels[index], text_scale);
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
            brand::rgb(brand::TEXT),
            format,
        );
    }
    true
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GamepadMappingEditorHit {
    Scope,
    Control(usize),
    Target(usize),
    Reset,
    Capture,
    Axes,
    Close,
}

#[derive(Clone, Copy)]
struct GamepadMappingEditorGeometry {
    panel: Rect,
    rows: [Rect; GamepadControl::COUNT],
    scope: Rect,
    reset: Rect,
    capture: Rect,
    axes: Rect,
    close: Rect,
    scale: usize,
}

fn gamepad_mapping_editor_geometry(
    width: u32,
    height: u32,
) -> Option<GamepadMappingEditorGeometry> {
    if width < 220 || height < 180 || width > 4096 || height > 4096 {
        return None;
    }
    let width = width as usize;
    let height = height as usize;
    let scale = (width.min(height) / 480).clamp(1, 3);
    let margin = 6 * scale;
    let panel = Rect {
        x: margin,
        y: margin,
        width: width.saturating_sub(margin * 2),
        height: height.saturating_sub(margin * 2),
    };
    let horizontal_padding = 10 * scale;
    let row_top = panel.y + 38 * scale;
    let footer_y = panel.y + panel.height.saturating_sub(24 * scale);
    let row_gap = scale;
    let available = footer_y.saturating_sub(row_top);
    let row_height = available
        .saturating_sub(row_gap * (GamepadControl::COUNT - 1))
        .checked_div(GamepadControl::COUNT)?
        .min(48 * scale);
    if row_height < 7 * scale {
        return None;
    }
    let row_width = panel.width.saturating_sub(horizontal_padding * 2);
    let rows = std::array::from_fn(|index| Rect {
        x: panel.x + horizontal_padding,
        y: row_top + index * (row_height + row_gap),
        width: row_width,
        height: row_height,
    });
    let header_button_width = (160 * scale).min(panel.width / 2);
    let scope = Rect {
        x: panel.x
            + panel
                .width
                .saturating_sub(header_button_width + horizontal_padding),
        y: panel.y + 7 * scale,
        width: header_button_width,
        height: 20 * scale,
    };
    let footer_gap = 3 * scale;
    let footer_available = panel
        .width
        .saturating_sub(horizontal_padding * 2 + footer_gap * 3);
    let footer_button_width = (66 * scale).min(footer_available / 4);
    let footer_button_height = (18 * scale).min(panel.height.saturating_sub(2 * scale));
    let reset = Rect {
        x: panel.x + horizontal_padding,
        y: footer_y + 2 * scale,
        width: footer_button_width,
        height: footer_button_height,
    };
    let capture = Rect {
        x: reset.x + footer_button_width + footer_gap,
        y: footer_y + 2 * scale,
        width: footer_button_width,
        height: footer_button_height,
    };
    let axes = Rect {
        x: capture.x + footer_button_width + footer_gap,
        y: footer_y + 2 * scale,
        width: footer_button_width,
        height: footer_button_height,
    };
    let close = Rect {
        x: panel.x
            + panel
                .width
                .saturating_sub(horizontal_padding + footer_button_width),
        y: footer_y + 2 * scale,
        width: footer_button_width,
        height: footer_button_height,
    };
    Some(GamepadMappingEditorGeometry {
        panel,
        rows,
        scope,
        reset,
        capture,
        axes,
        close,
        scale,
    })
}

/// Identifies the editor element at a point, using the same geometry as rendering.
pub fn hit_test_gamepad_mapping_editor(
    width: u32,
    height: u32,
    x: i32,
    y: i32,
) -> Option<GamepadMappingEditorHit> {
    let geometry = gamepad_mapping_editor_geometry(width, height)?;
    if x < 0 || y < 0 {
        return None;
    }
    let x = x as usize;
    let y = y as usize;
    let inside = |rect: Rect| {
        x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
    };
    if inside(geometry.scope) {
        return Some(GamepadMappingEditorHit::Scope);
    }
    if inside(geometry.reset) {
        return Some(GamepadMappingEditorHit::Reset);
    }
    if inside(geometry.capture) {
        return Some(GamepadMappingEditorHit::Capture);
    }
    if inside(geometry.axes) {
        return Some(GamepadMappingEditorHit::Axes);
    }
    if inside(geometry.close) {
        return Some(GamepadMappingEditorHit::Close);
    }
    geometry.rows.iter().enumerate().find_map(|(index, row)| {
        if !inside(*row) {
            return None;
        }
        let split = row.x + row.width * 52 / 100;
        Some(if x >= split {
            GamepadMappingEditorHit::Target(index)
        } else {
            GamepadMappingEditorHit::Control(index)
        })
    })
}

/// Draws a full-screen physical-gamepad mapping editor over a paused game frame.
pub fn render_gamepad_mapping_editor(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    mapping: &GamepadMapping,
    selected: usize,
    scope_label: &str,
    capturing: bool,
) -> bool {
    let Some(geometry) = gamepad_mapping_editor_geometry(width, height) else {
        return false;
    };
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(frame.len()) {
        return false;
    }

    let scale = geometry.scale;
    let text_scale = if height >= 520 { scale * 2 } else { scale };
    let canvas = Rect {
        x: 0,
        y: 0,
        width: width as usize,
        height: height as usize,
    };
    blend_rect(
        frame,
        width as usize,
        canvas,
        brand::rgb(brand::SURFACE_BASE),
        205,
        format,
    );
    draw_panel_chrome(frame, width as usize, geometry.panel, scale, format);
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        geometry.panel.x + 12 * scale,
        geometry.panel.y + 10 * scale,
        text_scale,
        if capturing {
            "AGUARDANDO BOTAO"
        } else {
            "MAPEAMENTO FISICO"
        },
        brand::rgb(brand::TEXT),
        format,
    );
    blend_rounded_rect(
        frame,
        width as usize,
        geometry.scope,
        brand::rgb(brand::SURFACE_RAISED),
        248,
        6 * scale,
        format,
    );
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        geometry.scope.x + 6 * scale,
        geometry.scope.y + (geometry.scope.height.saturating_sub(7 * scale)) / 2,
        scale,
        scope_label,
        brand::rgb(brand::ACCENT_GREEN_LIGHT),
        format,
    );

    for (index, row) in geometry.rows.iter().enumerate() {
        let is_selected = index == selected.min(GamepadControl::COUNT - 1);
        blend_rounded_rect(
            frame,
            width as usize,
            *row,
            if is_selected {
                brand::rgb(brand::SURFACE_SELECTED)
            } else {
                brand::rgb(brand::SURFACE_CARD)
            },
            if is_selected { 250 } else { 232 },
            6 * scale,
            format,
        );
        if is_selected {
            blend_rect(
                frame,
                width as usize,
                Rect {
                    x: row.x,
                    y: row.y,
                    width: (3 * scale).min(row.width),
                    height: row.height,
                },
                brand::rgb(brand::ACCENT_RED_LIGHT),
                255,
                format,
            );
        }
        let split = row.x + row.width * 52 / 100;
        blend_rect(
            frame,
            width as usize,
            Rect {
                x: split,
                y: row.y + 2 * scale,
                width: scale,
                height: row.height.saturating_sub(4 * scale),
            },
            brand::rgb(brand::BORDER),
            180,
            format,
        );
        let text_y = row.y + (row.height.saturating_sub(7 * text_scale)) / 2;
        draw_text_for_format(
            frame,
            width as usize,
            height as usize,
            row.x + 8 * scale,
            text_y,
            text_scale,
            GamepadControl::ALL[index].label(),
            brand::rgb(brand::TEXT),
            format,
        );
        let target_label = mapping
            .target(GamepadControl::ALL[index])
            .map(PadButton::label)
            .unwrap_or("DESLIGADO");
        draw_text_for_format(
            frame,
            width as usize,
            height as usize,
            split + 8 * scale,
            text_y,
            text_scale,
            target_label,
            brand::rgb(brand::ACCENT_GREEN_LIGHT),
            format,
        );
    }

    for (rect, label) in [
        (geometry.reset, "PADRAO"),
        (
            geometry.capture,
            if capturing { "CANCELAR" } else { "CAPTURAR" },
        ),
        (geometry.axes, "EIXOS"),
        (geometry.close, "VOLTAR"),
    ] {
        blend_rounded_rect(
            frame,
            width as usize,
            rect,
            brand::rgb(brand::SURFACE_RAISED),
            248,
            6 * scale,
            format,
        );
        let label_width = font::measure_text(label, scale);
        draw_text_for_format(
            frame,
            width as usize,
            height as usize,
            rect.x + rect.width.saturating_sub(label_width) / 2,
            rect.y + (rect.height.saturating_sub(7 * scale)) / 2,
            scale,
            label,
            brand::rgb(brand::TEXT),
            format,
        );
    }
    true
}

/// Interactive targets for the physical analog-stick settings screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GamepadAxisEditorHit {
    Scope,
    InvertX,
    InvertY,
    SensitivityDown,
    SensitivityUp,
    DeadzoneDown,
    DeadzoneUp,
    DeadzoneInherit,
    Reset,
    Back,
}

#[derive(Clone, Copy)]
struct GamepadAxisEditorGeometry {
    panel: Rect,
    rows: [Rect; 4],
    scope: Rect,
    reset: Rect,
    back: Rect,
    scale: usize,
}

fn gamepad_axis_editor_geometry(width: u32, height: u32) -> Option<GamepadAxisEditorGeometry> {
    if width < 220 || height < 180 || width > 4096 || height > 4096 {
        return None;
    }
    let width = width as usize;
    let height = height as usize;
    let scale = (width.min(height) / 480).clamp(1, 3);
    let margin = 6 * scale;
    let panel = Rect {
        x: margin,
        y: margin,
        width: width.saturating_sub(margin * 2),
        height: height.saturating_sub(margin * 2),
    };
    let horizontal_padding = 12 * scale;
    let row_top = panel.y + 46 * scale;
    let footer_y = panel.y + panel.height.saturating_sub(25 * scale);
    let row_gap = 4 * scale;
    let available = footer_y.saturating_sub(row_top);
    let row_height = available
        .saturating_sub(row_gap * 3)
        .checked_div(4)?
        .min(48 * scale);
    if row_height < 10 * scale {
        return None;
    }
    let rows = std::array::from_fn(|index| Rect {
        x: panel.x + horizontal_padding,
        y: row_top + index * (row_height + row_gap),
        width: panel.width.saturating_sub(horizontal_padding * 2),
        height: row_height,
    });
    let scope_width = (156 * scale).min(panel.width / 2);
    let scope = Rect {
        x: panel.x + panel.width.saturating_sub(scope_width + horizontal_padding),
        y: panel.y + 7 * scale,
        width: scope_width,
        height: 20 * scale,
    };
    let footer_gap = 8 * scale;
    let footer_button_width = (104 * scale).min(
        panel
            .width
            .saturating_sub(horizontal_padding * 2 + footer_gap)
            / 2,
    );
    let footer_button_height = (18 * scale).min(panel.height.saturating_sub(2 * scale));
    let reset = Rect {
        x: panel.x + horizontal_padding,
        y: footer_y + 2 * scale,
        width: footer_button_width,
        height: footer_button_height,
    };
    let back = Rect {
        x: panel.x
            + panel
                .width
                .saturating_sub(horizontal_padding + footer_button_width),
        y: footer_y + 2 * scale,
        width: footer_button_width,
        height: footer_button_height,
    };
    Some(GamepadAxisEditorGeometry {
        panel,
        rows,
        scope,
        reset,
        back,
        scale,
    })
}

/// Hit-tests the analog-settings view using the same geometry as its renderer.
pub fn hit_test_gamepad_axis_editor(
    width: u32,
    height: u32,
    x: i32,
    y: i32,
) -> Option<GamepadAxisEditorHit> {
    let geometry = gamepad_axis_editor_geometry(width, height)?;
    if x < 0 || y < 0 {
        return None;
    }
    let x = x as usize;
    let y = y as usize;
    let inside = |rect: Rect| {
        x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
    };
    if inside(geometry.scope) {
        return Some(GamepadAxisEditorHit::Scope);
    }
    if inside(geometry.reset) {
        return Some(GamepadAxisEditorHit::Reset);
    }
    if inside(geometry.back) {
        return Some(GamepadAxisEditorHit::Back);
    }
    geometry.rows.iter().enumerate().find_map(|(index, row)| {
        if !inside(*row) {
            return None;
        }
        match index {
            0 => Some(GamepadAxisEditorHit::InvertX),
            1 => Some(GamepadAxisEditorHit::InvertY),
            2 if x < row.x + row.width * 45 / 100 => None,
            2 if x < row.x + row.width * 65 / 100 => Some(GamepadAxisEditorHit::SensitivityDown),
            2 if x < row.x + row.width * 85 / 100 => None,
            2 => Some(GamepadAxisEditorHit::SensitivityUp),
            3 if x < row.x + row.width * 45 / 100 => None,
            3 if x < row.x + row.width * 65 / 100 => Some(GamepadAxisEditorHit::DeadzoneDown),
            3 if x < row.x + row.width * 85 / 100 => Some(GamepadAxisEditorHit::DeadzoneInherit),
            3 => Some(GamepadAxisEditorHit::DeadzoneUp),
            _ => None,
        }
    })
}

/// Renders inversion and sensitivity controls for RGBA8 or BGRX8 frames.
pub fn render_gamepad_axis_editor(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    settings: &GamepadAxisSettings,
    inherited_deadzone_percent: u8,
    scope_label: &str,
) -> bool {
    let Some(geometry) = gamepad_axis_editor_geometry(width, height) else {
        return false;
    };
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if expected != Some(frame.len()) {
        return false;
    }

    let scale = geometry.scale;
    let text_scale = if height >= 520 { scale * 2 } else { scale };
    let canvas = Rect {
        x: 0,
        y: 0,
        width: width as usize,
        height: height as usize,
    };
    blend_rect(
        frame,
        width as usize,
        canvas,
        brand::rgb(brand::SURFACE_BASE),
        205,
        format,
    );
    draw_panel_chrome(frame, width as usize, geometry.panel, scale, format);
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        geometry.panel.x + 12 * scale,
        geometry.panel.y + 10 * scale,
        text_scale,
        "ANALOGICO",
        brand::rgb(brand::TEXT),
        format,
    );
    blend_rounded_rect(
        frame,
        width as usize,
        geometry.scope,
        brand::rgb(brand::SURFACE_RAISED),
        248,
        6 * scale,
        format,
    );
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        geometry.scope.x + 6 * scale,
        geometry.scope.y + (geometry.scope.height.saturating_sub(7 * scale)) / 2,
        scale,
        scope_label,
        brand::rgb(brand::ACCENT_GREEN_LIGHT),
        format,
    );

    for (index, row) in geometry.rows.iter().enumerate() {
        blend_rounded_rect(
            frame,
            width as usize,
            *row,
            brand::rgb(brand::SURFACE_CARD),
            232,
            6 * scale,
            format,
        );
        let label = match index {
            0 => "INVERTER EIXO X",
            1 => "INVERTER EIXO Y",
            2 => "SENSIBILIDADE",
            _ => "ZONA MORTA",
        };
        draw_text_for_format(
            frame,
            width as usize,
            height as usize,
            row.x + 9 * scale,
            row.y + (row.height.saturating_sub(7 * text_scale)) / 2,
            text_scale,
            label,
            brand::rgb(brand::TEXT),
            format,
        );
        let value = match index {
            0 => {
                if settings.invert_x() {
                    "SIM"
                } else {
                    "NAO"
                }
            }
            1 => {
                if settings.invert_y() {
                    "SIM"
                } else {
                    "NAO"
                }
            }
            _ => "",
        };
        if index < 2 {
            draw_text_for_format(
                frame,
                width as usize,
                height as usize,
                row.x
                    + row
                        .width
                        .saturating_sub(font::measure_text(value, text_scale))
                    - 9 * scale,
                row.y + (row.height.saturating_sub(7 * text_scale)) / 2,
                text_scale,
                value,
                brand::rgb(brand::ACCENT_GREEN_LIGHT),
                format,
            );
        } else {
            draw_text_for_format(
                frame,
                width as usize,
                height as usize,
                row.x + row.width * 58 / 100 - font::measure_text("-", text_scale) / 2,
                row.y + (row.height.saturating_sub(7 * text_scale)) / 2,
                text_scale,
                "-",
                brand::rgb(brand::ACCENT_GREEN_LIGHT),
                format,
            );
            let center_value = if index == 2 {
                format!("{}%", settings.sensitivity_percent())
            } else {
                settings
                    .deadzone_override_percent()
                    .map(|value| format!("{value}%"))
                    .unwrap_or_else(|| format!("AUTO {inherited_deadzone_percent}%"))
            };
            let center_x =
                row.x + row.width * 77 / 100 - font::measure_text(&center_value, text_scale) / 2;
            draw_text_for_format(
                frame,
                width as usize,
                height as usize,
                center_x,
                row.y + (row.height.saturating_sub(7 * text_scale)) / 2,
                text_scale,
                &center_value,
                brand::rgb(brand::ACCENT_GREEN_LIGHT),
                format,
            );
            draw_text_for_format(
                frame,
                width as usize,
                height as usize,
                row.x + row.width * 94 / 100 - font::measure_text("+", text_scale) / 2,
                row.y + (row.height.saturating_sub(7 * text_scale)) / 2,
                text_scale,
                "+",
                brand::rgb(brand::ACCENT_GREEN_LIGHT),
                format,
            );
        }
    }

    for (rect, label) in [(geometry.reset, "PADRAO"), (geometry.back, "VOLTAR")] {
        blend_rounded_rect(
            frame,
            width as usize,
            rect,
            brand::rgb(brand::SURFACE_RAISED),
            248,
            6 * scale,
            format,
        );
        let label_width = font::measure_text(label, scale);
        draw_text_for_format(
            frame,
            width as usize,
            height as usize,
            rect.x + rect.width.saturating_sub(label_width) / 2,
            rect.y + (rect.height.saturating_sub(7 * scale)) / 2,
            scale,
            label,
            brand::rgb(brand::TEXT),
            format,
        );
    }
    true
}

/// Draws a dimmed, nine-action quick menu over an existing RGBA8 or BGRX8 frame.
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
        "CAPTURAR TELA",
        "DIAGNÓSTICO",
        "TURBO",
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
    render_overlay_with_control_profiles(
        frame,
        width,
        height,
        format,
        selected,
        profile_label,
        "RESTAURAR CONTROLES",
        "ESCOPO DOS CONTROLES",
    )
}

/// Draws the Android menu with scope-aware control reset and profile actions.
pub fn render_overlay_with_control_profiles(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    selected: usize,
    profile_label: &str,
    control_reset_label: &str,
    control_scope_label: &str,
) -> bool {
    render_overlay_with_control_profiles_and_turbo(
        frame,
        width,
        height,
        format,
        selected,
        profile_label,
        control_reset_label,
        control_scope_label,
        "TURBO",
    )
}

/// Android menu including the turbo row, whose label shows the current speed
/// (for example `TURBO: 200%`).
#[allow(clippy::too_many_arguments)]
pub fn render_overlay_with_control_profiles_and_turbo(
    frame: &mut [u8],
    width: u32,
    height: u32,
    format: PixelFormat,
    selected: usize,
    profile_label: &str,
    control_reset_label: &str,
    control_scope_label: &str,
    turbo_label: &str,
) -> bool {
    render_overlay_with_action_count(
        frame,
        width,
        height,
        format,
        selected,
        profile_label,
        ACTION_COUNT,
        control_reset_label,
        control_scope_label,
        turbo_label,
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
    control_reset_label: &str,
    control_scope_label: &str,
    turbo_label: &str,
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
    let title_text_scale = scale * 2;
    let labels = [
        "CONTINUAR",
        "REINICIAR",
        "VOLTAR À BIBLIOTECA",
        "ALTERAR LAYOUT",
        "TROCAR TELAS",
        "PADRÃO GLOBAL",
        "RESTAURAR CONTROLES",
        "ESCOPO DOS CONTROLES",
        "EDITAR CONTROLES",
        "CAPTURAR TELA",
        "DIAGNÓSTICO",
        "MAPEAR CONTROLE",
        turbo_label,
    ];
    let row_text_scale = menu_row_text_scale(
        &geometry,
        action_count,
        scale,
        &labels,
        control_reset_label,
        control_scope_label,
    );
    let canvas = Rect {
        x: 0,
        y: 0,
        width: width as usize,
        height: height as usize,
    };
    blend_rect(
        frame,
        width as usize,
        canvas,
        brand::rgb(brand::SURFACE_BASE),
        164,
        format,
    );
    draw_panel_chrome(frame, width as usize, geometry.panel, scale, format);

    let title_x = geometry.panel.x + 18 * scale;
    let title_y = geometry.panel.y + 12 * scale;
    draw_text_for_format(
        frame,
        width as usize,
        height as usize,
        title_x,
        title_y,
        title_text_scale,
        "MENU DO JOGO",
        brand::rgb(brand::TEXT),
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
        brand::rgb(brand::ACCENT_GREEN_LIGHT),
        format,
    );
    for (index, row) in geometry.rows.iter().take(geometry.action_count).enumerate() {
        let is_selected = index == selected.min(action_count - 1);
        blend_rounded_rect(
            frame,
            width as usize,
            *row,
            if is_selected {
                brand::rgb(brand::SURFACE_SELECTED)
            } else {
                brand::rgb(brand::SURFACE_CARD)
            },
            if is_selected { 250 } else { 230 },
            7 * scale,
            format,
        );
        if is_selected {
            blend_rect(
                frame,
                width as usize,
                Rect {
                    x: row.x,
                    y: row.y,
                    width: (3 * scale).min(row.width),
                    height: row.height,
                },
                brand::rgb(brand::ACCENT_RED_LIGHT),
                255,
                format,
            );
        }
        let text_y = row.y + (row.height.saturating_sub(7 * row_text_scale)) / 2;
        let label = match index {
            6 => control_reset_label,
            7 => control_scope_label,
            8 if action_count == DESKTOP_ACTION_COUNT => "MAPEAR CONTROLE",
            _ => labels[index],
        };
        draw_text_for_format(
            frame,
            width as usize,
            height as usize,
            row.x + 12 * scale,
            text_y,
            row_text_scale,
            label,
            brand::rgb(brand::TEXT),
            format,
        );
    }
    true
}

/// Keeps action labels inside their rows on compact or narrow host surfaces.
fn menu_row_text_scale(
    geometry: &Geometry,
    action_count: usize,
    base_scale: usize,
    labels: &[&str; ACTION_COUNT],
    control_reset_label: &str,
    control_scope_label: &str,
) -> usize {
    let widest_label = (0..action_count)
        .map(|index| match index {
            6 => control_reset_label,
            7 => control_scope_label,
            8 if action_count == DESKTOP_ACTION_COUNT => "MAPEAR CONTROLE",
            _ => labels[index],
        })
        .max_by_key(|label| font::measure_text_with_weight(label, 1, font::TextWeight::Semibold))
        .unwrap_or("MENU");

    let row_height = geometry.rows[0].height;
    let vertical_scale = (row_height / 7).max(1);
    let available_width = geometry.rows[0].width.saturating_sub(24 * base_scale);
    let glyph_width =
        font::measure_text_with_weight(widest_label, 1, font::TextWeight::Semibold).max(1);
    let horizontal_scale = (available_width / glyph_width).max(1);
    (base_scale * 2)
        .min(vertical_scale)
        .min(horizontal_scale)
        .max(1)
}

fn draw_panel_chrome(
    frame: &mut [u8],
    width: usize,
    panel: Rect,
    scale: usize,
    format: PixelFormat,
) {
    let shadow = Rect {
        x: panel.x.saturating_add(3 * scale),
        y: panel.y.saturating_add(4 * scale),
        ..panel
    };
    blend_rounded_rect(frame, width, shadow, [0, 0, 0], 112, 10 * scale, format);
    let near_shadow = Rect {
        x: panel.x.saturating_add(scale),
        y: panel.y.saturating_add(2 * scale),
        ..panel
    };
    blend_rounded_rect(
        frame,
        width,
        near_shadow,
        brand::rgb(brand::SURFACE_BASE),
        150,
        9 * scale,
        format,
    );
    blend_rounded_rect(
        frame,
        width,
        panel,
        brand::rgb(brand::BORDER),
        255,
        9 * scale,
        format,
    );
    let inner = Rect {
        x: panel.x.saturating_add(scale),
        y: panel.y.saturating_add(scale),
        width: panel.width.saturating_sub(2 * scale),
        height: panel.height.saturating_sub(2 * scale),
    };
    blend_rounded_rect(
        frame,
        width,
        inner,
        brand::rgb(brand::SURFACE_PANEL),
        250,
        8 * scale,
        format,
    );
    let accent = Rect {
        x: panel.x.saturating_add(9 * scale),
        y: panel.y.saturating_add(scale),
        width: panel.width.saturating_sub(18 * scale),
        height: (3 * scale).min(panel.height),
    };
    blend_rounded_rect(
        frame,
        width,
        accent,
        brand::rgb(brand::ACCENT_RED),
        255,
        2 * scale,
        format,
    );
}

/// Blends a rounded rectangle by drawing clipped row spans. The hit-test
/// geometry remains rectangular, so touch and pointer targets do not change.
fn blend_rounded_rect(
    frame: &mut [u8],
    width: usize,
    rect: Rect,
    color: [u8; 3],
    alpha: u8,
    radius: usize,
    format: PixelFormat,
) {
    if width == 0 || frame.len() % width.saturating_mul(4) != 0 {
        return;
    }
    let height = frame.len() / 4 / width;
    let left = rect.x.min(width);
    let right = rect.x.saturating_add(rect.width).min(width);
    let top = rect.y.min(height);
    let bottom = rect.y.saturating_add(rect.height).min(height);
    if left >= right || top >= bottom {
        return;
    }
    let corner = radius.min((right - left) / 2).min((bottom - top) / 2);
    let rect_height = (bottom - top) as f32;
    for row in top..bottom {
        let local_y = (row - top) as f32 + 0.5;
        let distance = if local_y < corner as f32 {
            corner as f32 - local_y
        } else if local_y > rect_height - corner as f32 {
            local_y - (rect_height - corner as f32)
        } else {
            0.0
        };
        let inset = if distance > 0.0 && corner > 0 {
            (corner as f32
                - (corner as f32 * corner as f32 - distance * distance)
                    .max(0.0)
                    .sqrt())
            .ceil() as usize
        } else {
            0
        };
        let row_rect = Rect {
            x: left.saturating_add(inset).min(right),
            y: row,
            width: (right - left).saturating_sub(inset.saturating_mul(2)),
            height: 1,
        };
        blend_rect(frame, width, row_rect, color, alpha, format);
    }
}

fn draw_colored_border(
    frame: &mut [u8],
    width: usize,
    rect: Rect,
    thickness: usize,
    color: [u8; 3],
    format: PixelFormat,
) {
    blend_rect(
        frame,
        width,
        Rect {
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
        Rect {
            width: thickness,
            ..rect
        },
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

fn draw_border(frame: &mut [u8], width: usize, rect: Rect, thickness: usize, format: PixelFormat) {
    draw_colored_border(
        frame,
        width,
        rect,
        thickness,
        brand::rgb(brand::BORDER),
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
                    ((u32::from(source[channel]) * u32::from(alpha) + destination * inverse + 127)
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
    let weight = if scale >= 2 {
        font::TextWeight::Semibold
    } else {
        font::TextWeight::Regular
    };
    font::draw_text_with_weight(
        frame, width, height, x as i32, y as i32, scale, value, color, weight,
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
    fn mapping_editor_hit_testing_matches_rendered_rows_and_footer_actions() {
        for (width, height) in [(736u32, 240u32), (400, 800), (240, 320)] {
            let geometry = gamepad_mapping_editor_geometry(width, height).unwrap();
            let mut frame = vec![0; width as usize * height as usize * 4];
            assert!(render_gamepad_mapping_editor(
                &mut frame,
                width,
                height,
                PixelFormat::Rgba8,
                &GamepadMapping::default(),
                0,
                "ESCOPO: GLOBAL",
                false,
            ));
            let first = geometry.rows[0];
            assert_eq!(
                hit_test_gamepad_mapping_editor(
                    width,
                    height,
                    (first.x + 2) as i32,
                    (first.y + first.height / 2) as i32,
                ),
                Some(GamepadMappingEditorHit::Control(0))
            );
            assert_eq!(
                hit_test_gamepad_mapping_editor(
                    width,
                    height,
                    (first.x + first.width * 3 / 4) as i32,
                    (first.y + first.height / 2) as i32,
                ),
                Some(GamepadMappingEditorHit::Target(0))
            );
            assert_eq!(
                hit_test_gamepad_mapping_editor(
                    width,
                    height,
                    (geometry.scope.x + 1) as i32,
                    (geometry.scope.y + 1) as i32,
                ),
                Some(GamepadMappingEditorHit::Scope)
            );
            assert_eq!(
                hit_test_gamepad_mapping_editor(
                    width,
                    height,
                    (geometry.reset.x + 1) as i32,
                    (geometry.reset.y + 1) as i32,
                ),
                Some(GamepadMappingEditorHit::Reset)
            );
            assert_eq!(
                hit_test_gamepad_mapping_editor(
                    width,
                    height,
                    (geometry.capture.x + 1) as i32,
                    (geometry.capture.y + 1) as i32,
                ),
                Some(GamepadMappingEditorHit::Capture)
            );
            assert_eq!(
                hit_test_gamepad_mapping_editor(
                    width,
                    height,
                    (geometry.axes.x + 1) as i32,
                    (geometry.axes.y + 1) as i32,
                ),
                Some(GamepadMappingEditorHit::Axes)
            );
            assert_eq!(
                hit_test_gamepad_mapping_editor(
                    width,
                    height,
                    (geometry.close.x + 1) as i32,
                    (geometry.close.y + 1) as i32,
                ),
                Some(GamepadMappingEditorHit::Close)
            );
        }
    }

    #[test]
    fn axis_editor_hit_testing_matches_rendered_controls_on_phone_and_desktop() {
        for (width, height) in [(736u32, 240u32), (400, 800), (320, 240)] {
            let geometry = gamepad_axis_editor_geometry(width, height).unwrap();
            let mut frame = vec![0u8; width as usize * height as usize * 4];
            for format in [PixelFormat::Rgba8, PixelFormat::Bgrx8] {
                assert!(render_gamepad_axis_editor(
                    &mut frame,
                    width,
                    height,
                    format,
                    &GamepadAxisSettings::default(),
                    16,
                    "ESCOPO: GLOBAL",
                ));
            }
            for (index, row) in geometry.rows.iter().enumerate() {
                let x = (row.x
                    + if index >= 2 {
                        row.width * 58 / 100
                    } else {
                        row.width / 4
                    }) as i32;
                let y = (row.y + row.height / 2) as i32;
                let expected = match index {
                    0 => GamepadAxisEditorHit::InvertX,
                    1 => GamepadAxisEditorHit::InvertY,
                    2 => GamepadAxisEditorHit::SensitivityDown,
                    _ => GamepadAxisEditorHit::DeadzoneDown,
                };
                assert_eq!(
                    hit_test_gamepad_axis_editor(width, height, x, y),
                    Some(expected)
                );
            }
            let sensitivity = geometry.rows[2];
            assert_eq!(
                hit_test_gamepad_axis_editor(
                    width,
                    height,
                    (sensitivity.x + sensitivity.width * 90 / 100) as i32,
                    (sensitivity.y + sensitivity.height / 2) as i32,
                ),
                Some(GamepadAxisEditorHit::SensitivityUp)
            );
            let deadzone = geometry.rows[3];
            assert_eq!(
                hit_test_gamepad_axis_editor(
                    width,
                    height,
                    (deadzone.x + deadzone.width * 90 / 100) as i32,
                    (deadzone.y + deadzone.height / 2) as i32,
                ),
                Some(GamepadAxisEditorHit::DeadzoneUp)
            );
            assert_eq!(
                hit_test_gamepad_axis_editor(
                    width,
                    height,
                    (deadzone.x + deadzone.width * 75 / 100) as i32,
                    (deadzone.y + deadzone.height / 2) as i32,
                ),
                Some(GamepadAxisEditorHit::DeadzoneInherit)
            );
            assert_eq!(
                hit_test_gamepad_axis_editor(
                    width,
                    height,
                    (geometry.scope.x + 1) as i32,
                    (geometry.scope.y + 1) as i32,
                ),
                Some(GamepadAxisEditorHit::Scope)
            );
            assert_eq!(
                hit_test_gamepad_axis_editor(
                    width,
                    height,
                    (geometry.reset.x + 1) as i32,
                    (geometry.reset.y + 1) as i32,
                ),
                Some(GamepadAxisEditorHit::Reset)
            );
            assert_eq!(
                hit_test_gamepad_axis_editor(
                    width,
                    height,
                    (geometry.back.x + 1) as i32,
                    (geometry.back.y + 1) as i32,
                ),
                Some(GamepadAxisEditorHit::Back)
            );
        }
    }

    #[test]
    fn axis_editor_rejects_invalid_frame_buffers() {
        let mut frame = vec![9u8; 16];
        let before = frame.clone();
        assert!(!render_gamepad_axis_editor(
            &mut frame,
            736,
            240,
            PixelFormat::Rgba8,
            &GamepadAxisSettings::default(),
            16,
            "ESCOPO: GLOBAL",
        ));
        assert_eq!(frame, before);
        assert_eq!(hit_test_gamepad_axis_editor(0, 0, 0, 0), None);
    }

    #[test]
    fn profile_label_renders_on_compact_phone_and_desktop_canvases() {
        for (width, height) in [(320u32, 240u32), (400, 800), (736, 240), (2400, 1080)] {
            for (label, format) in [
                ("PADRÃO GLOBAL", PixelFormat::Rgba8),
                ("PERFIL DESTE JOGO", PixelFormat::Bgrx8),
            ] {
                let mut frame = vec![0u8; width as usize * height as usize * 4];
                assert!(render_overlay_with_profile(
                    &mut frame, width, height, format, 0, label,
                ));
                assert_eq!(frame.len(), width as usize * height as usize * 4);
                assert!(frame.iter().any(|channel| *channel != 0));
            }
        }
    }

    #[test]
    fn ninth_action_renders_and_remains_reachable_on_compact_canvas() {
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
        for row in desktop_geometry.rows.iter().take(DESKTOP_ACTION_COUNT) {
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
            assert_ne!(
                &frame[guide_pixel..guide_pixel + 3],
                &original[guide_pixel..guide_pixel + 3]
            );
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
        assert!(render_menu_button(
            &mut frame,
            width,
            height,
            PixelFormat::Rgba8
        ));
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
            for (index, row) in geometry.rows.iter().enumerate() {
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
                    &mut frame, width, height, format, action, 1,
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
    fn quick_menu_uses_aceris_panel_chrome_and_clear_selection_marker() {
        let width = 400u32;
        let height = 800u32;
        let mut frame = vec![0u8; width as usize * height as usize * 4];
        assert!(render_overlay(
            &mut frame,
            width,
            height,
            PixelFormat::Rgba8,
            0
        ));

        let layout = geometry(width, height, ACTION_COUNT).unwrap();
        let accent_center = (((layout.panel.y + layout.scale) * width as usize)
            + layout.panel.x
            + layout.panel.width / 2)
            * 4;
        assert_eq!(
            &frame[accent_center..accent_center + 3],
            &brand::rgb(brand::ACCENT_RED)
        );

        let selected = layout.rows[0];
        let marker = ((selected.y + selected.height / 2) * width as usize + selected.x) * 4;
        assert_eq!(
            &frame[marker..marker + 3],
            &brand::rgb(brand::ACCENT_RED_LIGHT)
        );

        let border = ((layout.panel.y + 12 * layout.scale) * width as usize + layout.panel.x) * 4;
        assert_eq!(&frame[border..border + 3], &brand::rgb(brand::BORDER));
    }

    #[test]
    fn diagnostic_overlay_renders_for_both_host_pixel_formats() {
        for format in [PixelFormat::Rgba8, PixelFormat::Bgrx8] {
            let (width, height) = (400u32, 800u32);
            let mut frame = vec![0u8; width as usize * height as usize * 4];
            let original = frame.clone();
            assert!(render_diagnostics_overlay(
                &mut frame,
                width,
                height,
                format,
                0x0010_2030,
                2,
                Some("srv:GetServiceHandle(dsp::DSP)"),
                Some("presented_fps=58.5, frame_ms_p95=19.3"),
            ));
            assert_ne!(frame, original);
            let outside = ((height as usize - 4) * width as usize + width as usize - 4) * 4;
            assert_eq!(
                &frame[outside..outside + 3],
                &original[outside..outside + 3]
            );
        }

        let mut invalid = vec![7u8; 8];
        let before = invalid.clone();
        assert!(!render_diagnostics_overlay(
            &mut invalid,
            400,
            800,
            PixelFormat::Rgba8,
            0,
            0,
            None,
            None,
        ));
        assert_eq!(invalid, before);
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

    #[test]
    fn scope_and_reset_labels_render_on_compact_android_canvas() {
        let (width, height) = (320u32, 240u32);
        let mut frame = vec![0u8; width as usize * height as usize * 4];
        assert!(render_overlay_with_control_profiles(
            &mut frame,
            width,
            height,
            PixelFormat::Rgba8,
            ACTION_COUNT - 1,
            "TELAS: GLOBAL | CONTROLES: JOGO",
            "RESTAURAR PERFIL",
            "PADRÃO GLOBAL",
        ));
        let geometry = geometry(width, height, ACTION_COUNT).unwrap();
        let last = geometry.rows[ACTION_COUNT - 1];
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
    fn android_menu_labels_scale_down_on_short_and_narrow_surfaces() {
        for (width, height) in [(320u32, 180u32), (240u32, 320u32)] {
            let layout = geometry(width, height, ACTION_COUNT).unwrap();
            let labels = [
                "CONTINUAR",
                "REINICIAR",
                "VOLTAR À BIBLIOTECA",
                "ALTERAR LAYOUT",
                "TROCAR TELAS",
                "PADRÃO GLOBAL",
                "RESTAURAR CONTROLES",
                "ESCOPO DOS CONTROLES",
                "EDITAR CONTROLES",
                "CAPTURAR TELA",
                "DIAGNÓSTICO",
                "MAPEAR CONTROLE",
                "TURBO: DESLIGADO",
            ];
            assert_eq!(
                menu_row_text_scale(
                    &layout,
                    ACTION_COUNT,
                    layout.scale,
                    &labels,
                    "RESTAURAR PERFIL",
                    "PADRÃO GLOBAL",
                ),
                1,
                "labels must fit at one-pixel font scale on {width}x{height}",
            );

            let mut frame = vec![0u8; width as usize * height as usize * 4];
            assert!(render_overlay_with_control_profiles(
                &mut frame,
                width,
                height,
                PixelFormat::Rgba8,
                ACTION_COUNT - 1,
                "TELAS: GLOBAL | CONTROLES: JOGO",
                "RESTAURAR PERFIL",
                "PADRÃO GLOBAL",
            ));
            for (index, row) in layout.rows.iter().take(ACTION_COUNT).enumerate() {
                assert_eq!(
                    hit_test(
                        width,
                        height,
                        (row.x + row.width / 2) as i32,
                        (row.y + row.height / 2) as i32,
                    ),
                    Some(index),
                );
            }
        }
    }
}
