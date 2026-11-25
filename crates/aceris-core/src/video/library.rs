//! Game library screen: the list shown after the intro, where a game is chosen.
//!
//! This module only holds state and draws RGBA frames; frontends feed it
//! keyboard, mouse or touch input and decide what "launch" means.

use super::font::{self, draw_text};
use aceris_common::brand;
use std::fs;
use std::path::{Path, PathBuf};

/// Canvas size the layout is designed for.
pub const WIDTH: usize = 400;
/// Canvas height the layout is designed for.
pub const HEIGHT: usize = 480;

const HEADER_HEIGHT: i32 = 56;
const ROW_HEIGHT: i32 = 28;
const LIST_TOP: i32 = HEADER_HEIGHT + 8;
const FOOTER_TOP: i32 = 452;
/// Rows visible at once.
pub const VISIBLE_ROWS: usize = ((FOOTER_TOP - 22 - LIST_TOP) / ROW_HEIGHT) as usize;
const TEXT_SCALE: usize = 2;
const MARGIN: i32 = 20;

/// One game in the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    pub path: PathBuf,
}


/// Which launcher page is currently visible.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LibraryPage {
    #[default]
    Games,
    Settings,
}

/// What a click did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickResult {
    /// The click hit nothing.
    Ignored,
    /// A row was selected.
    Selected,
    /// The already selected row was clicked again: launch it.
    Activated,
}

/// Library state.
#[derive(Clone, Debug)]
pub struct Library {
    entries: Vec<Entry>,
    selected: usize,
    first_visible: usize,
    directory_label: String,
    status: Option<String>,
    page: LibraryPage,
}

/// Lists supported executable images (`.3ds`, `.cci`, `.3dsx`) directly
/// inside `directory`, sorted by title. A missing or unreadable directory
/// yields an empty list.
pub fn scan_games(directory: &Path) -> Vec<Entry> {
    let Ok(read) = fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = read
        .filter_map(|item| item.ok())
        .map(|item| item.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("3ds")
                            || ext.eq_ignore_ascii_case("cci")
                            || ext.eq_ignore_ascii_case("3dsx")
                    })
        })
        .map(|path| Entry {
            title: path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path,
        })
        .collect();
    entries.sort_by_key(|entry| entry.title.to_lowercase());
    entries
}

impl Library {
    pub fn new(entries: Vec<Entry>, directory_label: String) -> Self {
        Self {
            entries,
            selected: 0,
            first_visible: 0,
            directory_label,
            status: None,
            page: LibraryPage::Games,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.entries.get(self.selected)
    }

    /// Shows a message (for example a load error) above the footer.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
    }

    /// Moves the selection by `delta` rows, clamped to the list.
    pub fn move_selection(&mut self, delta: i32) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as i64 - 1;
        self.selected = (self.selected as i64 + i64::from(delta)).clamp(0, last) as usize;
        self.status = None;
        if self.selected < self.first_visible {
            self.first_visible = self.selected;
        } else if self.selected >= self.first_visible + VISIBLE_ROWS {
            self.first_visible = self.selected + 1 - VISIBLE_ROWS;
        }
    }

    /// Handles a click or tap at canvas coordinates.
    pub fn click(&mut self, x: i32, y: i32) -> ClickResult {
        if x < MARGIN - 8 || x >= WIDTH as i32 - MARGIN + 8 || y < LIST_TOP {
            return ClickResult::Ignored;
        }
        let row = ((y - LIST_TOP) / ROW_HEIGHT) as usize;
        let index = self.first_visible + row;
        if row >= VISIBLE_ROWS || index >= self.entries.len() {
            return ClickResult::Ignored;
        }
        if index == self.selected {
            ClickResult::Activated
        } else {
            self.move_selection(index as i32 - self.selected as i32);
            ClickResult::Selected
        }
    }

    /// Current launcher page.
    pub fn page(&self) -> LibraryPage {
        self.page
    }

    /// Re-scans the user's game folder without restarting the application.
    pub fn reload(&mut self) {
        self.entries = scan_games(Path::new(&self.directory_label));
        self.selected = 0;
        self.first_visible = 0;
        self.status = Some(format!("BIBLIOTECA ATUALIZADA: {} JOGO(S)", self.entries.len()));
    }

    /// Moves selection using the number of rows actually visible on this surface.
    pub fn move_selection_responsive(&mut self, delta: i32, width: u32, height: u32) {
        let Some((portrait, scale, _, logical_height)) = responsive_metrics(width, height) else {
            return;
        };
        let (list_top, row_height, list_bottom) = if portrait {
            (250.0, 58.0, logical_height - 100.0)
        } else {
            (142.0, 76.0, logical_height - 54.0)
        };
        let visible = ((list_bottom - list_top).max(row_height) / row_height)
            .floor()
            .max(1.0) as usize;
        self.move_selection_with_visible(delta, visible);
        let _ = scale;
    }

    fn move_selection_with_visible(&mut self, delta: i32, visible_rows: usize) {
        if self.entries.is_empty() {
            return;
        }
        let last = self.entries.len() as i64 - 1;
        self.selected = (self.selected as i64 + i64::from(delta)).clamp(0, last) as usize;
        self.status = None;
        let visible_rows = visible_rows.max(1);
        if self.selected < self.first_visible {
            self.first_visible = self.selected;
        } else if self.selected >= self.first_visible + visible_rows {
            self.first_visible = self.selected + 1 - visible_rows;
        }
    }

    /// Handles navigation, refresh and game selection in the responsive launcher.
    /// Coordinates are supplied in physical surface pixels.
    pub fn click_responsive(&mut self, x: i32, y: i32, width: u32, height: u32) -> ClickResult {
        let Some((portrait, scale, _, logical_height)) = responsive_metrics(width, height) else {
            return ClickResult::Ignored;
        };
        let (x, y) = (x as f32 / scale, y as f32 / scale);
        if portrait {
            if (100.0..150.0).contains(&y) {
                self.page = if x < 200.0 { LibraryPage::Games } else { LibraryPage::Settings };
                self.status = None;
                return ClickResult::Ignored;
            }
            if self.page == LibraryPage::Settings {
                if (600.0..655.0).contains(&y) && (20.0..380.0).contains(&x) {
                    self.reload();
                }
                return ClickResult::Ignored;
            }
            if ((170.0..225.0).contains(&y) && x >= 300.0)
                || ((600.0..655.0).contains(&y) && (38.0..362.0).contains(&x))
            {
                self.reload();
                return ClickResult::Ignored;
            }
            if y < 250.0 {
                return ClickResult::Ignored;
            }
            let row_height = 58.0;
            let visible = ((logical_height - 350.0).max(row_height) / row_height)
                .floor()
                .max(1.0) as usize;
            let index = self.first_visible + ((y - 250.0) / row_height).floor().max(0.0) as usize;
            if index >= self.first_visible + visible || index >= self.entries.len() {
                return ClickResult::Ignored;
            }
            self.select_or_activate(index)
        } else {
            if x < 210.0 {
                if (104.0..158.0).contains(&y) {
                    self.page = LibraryPage::Games;
                    self.status = None;
                } else if (164.0..218.0).contains(&y) {
                    self.page = LibraryPage::Settings;
                    self.status = None;
                }
                return ClickResult::Ignored;
            }
            if self.page == LibraryPage::Settings {
                if ((30.0..86.0).contains(&y) && x >= 820.0)
                    || ((366.0..422.0).contains(&y) && (270.0..600.0).contains(&x))
                {
                    self.reload();
                }
                return ClickResult::Ignored;
            }
            if ((30.0..86.0).contains(&y) && x >= 820.0)
                || ((452.0..508.0).contains(&y) && (490.0..730.0).contains(&x))
            {
                self.reload();
                return ClickResult::Ignored;
            }
            if y < 142.0 {
                return ClickResult::Ignored;
            }
            let row_height = 76.0;
            let visible = ((logical_height - 196.0).max(row_height) / row_height)
                .floor()
                .max(1.0) as usize;
            let index = self.first_visible + ((y - 142.0) / row_height).floor().max(0.0) as usize;
            if index >= self.first_visible + visible || index >= self.entries.len() {
                return ClickResult::Ignored;
            }
            self.select_or_activate(index)
        }
    }

    fn select_or_activate(&mut self, index: usize) -> ClickResult {
        if index == self.selected {
            ClickResult::Activated
        } else {
            self.move_selection_with_visible(index as i32 - self.selected as i32, VISIBLE_ROWS);
            ClickResult::Selected
        }
    }

    /// Renders the launcher directly at the physical surface size.
    pub fn render_responsive(&self, width: u32, height: u32) -> Vec<u8> {
        let mut frame = Vec::new();
        self.render_responsive_into(width, height, &mut frame);
        frame
    }

    /// Renders into a caller-owned RGBA buffer, retaining its allocation.
    pub fn render_responsive_into(&self, width: u32, height: u32, frame: &mut Vec<u8>) -> bool {
        let Some((portrait, scale, logical_width, logical_height)) = responsive_metrics(width, height) else {
            frame.clear();
            return false;
        };
        let Some(byte_len) = (width as usize).checked_mul(height as usize).and_then(|n| n.checked_mul(4)) else {
            frame.clear();
            return false;
        };
        if frame.capacity() < byte_len {
            let additional = byte_len.saturating_sub(frame.len());
            if frame.try_reserve_exact(additional).is_err() {
                frame.clear();
                return false;
            }
        }
        frame.resize(byte_len, 255);
        responsive_fill(frame, width as usize, height as usize, 0.0, 0.0, logical_width, logical_height, scale, 0x100A1B);
        if portrait {
            self.render_portrait(frame, width, height, scale, logical_height);
        } else {
            self.render_landscape(frame, width, height, scale, logical_height);
        }
        true
    }

    fn render_portrait(&self, frame: &mut [u8], width: u32, height: u32, scale: f32, logical_height: f32) {
        let font_scale = (scale * 1.7).round().max(1.0) as usize;
        let small_scale = (font_scale as f32 * 0.67).round().max(1.0) as usize;
        responsive_fill(frame, width as usize, height as usize, 0.0, 0.0, 400.0, 84.0, scale, 0x160D22);
        responsive_fill(frame, width as usize, height as usize, 0.0, 82.0, 400.0, 3.0, scale, brand::ACCENT_RED);
        responsive_text(frame, width, height, 22.0, 14.0, font_scale, "ACERIS", brand::TEXT);
        responsive_text(frame, width, height, 24.0, 54.0, small_scale, "EMULADOR DE NINTENDO 3DS", 0xBBAFC7);

        let active = if self.page == LibraryPage::Games { 0 } else { 1 };
        responsive_panel(frame, width, height, 20.0, 101.0, 174.0, 44.0, scale, if active == 0 { brand::ACCENT_RED } else { 0x251731 });
        responsive_panel(frame, width, height, 206.0, 101.0, 174.0, 44.0, scale, if active == 1 { brand::ACCENT_RED } else { 0x251731 });
        responsive_text(frame, width, height, 45.0, 116.0, small_scale, "JOGOS", brand::TEXT);
        responsive_text(frame, width, height, 232.0, 116.0, small_scale, "CONFIGURACOES", brand::TEXT);

        if self.page == LibraryPage::Settings {
            responsive_text(frame, width, height, 22.0, 178.0, font_scale, "CONFIGURACOES", brand::TEXT);
            responsive_text(frame, width, height, 22.0, 210.0, small_scale, "ARMAZENAMENTO E COMPATIBILIDADE", 0xBBAFC7);
            responsive_panel(frame, width, height, 20.0, 252.0, 360.0, 310.0, scale, 0x21152F);
            responsive_text(frame, width, height, 38.0, 278.0, small_scale, "PASTA DE JOGOS", brand::ACCENT_GREEN_LIGHT);
            responsive_text(frame, width, height, 38.0, 308.0, small_scale, &tail(&self.directory_label, 46), brand::TEXT);
            responsive_text(frame, width, height, 38.0, 365.0, small_scale, "FORMATOS LISTADOS", brand::ACCENT_GREEN_LIGHT);
            responsive_text(frame, width, height, 38.0, 395.0, small_scale, ".3DS  .CCI  .3DSX", brand::TEXT);
            responsive_text(frame, width, height, 38.0, 452.0, small_scale, "GRAFICO: AUTO (PADRAO)", brand::TEXT);
            responsive_text(frame, width, height, 38.0, 482.0, small_scale, "VULKAN / OPENGL ES COM FALLBACK", 0xBBAFC7);
            responsive_panel(frame, width, height, 20.0, 600.0, 360.0, 52.0, scale, brand::ACCENT_RED);
            responsive_text(frame, width, height, 66.0, 618.0, small_scale, "RECARREGAR BIBLIOTECA", brand::TEXT);
        } else {
            responsive_text(frame, width, height, 22.0, 178.0, font_scale, "SUA BIBLIOTECA", brand::TEXT);
            responsive_text(frame, width, height, 22.0, 212.0, small_scale, &format!("{} JOGO(S) DISPONIVEL(IS)", self.entries.len()), 0xBBAFC7);
            responsive_panel(frame, width, height, 322.0, 174.0, 58.0, 44.0, scale, 0x2D1A3B);
            responsive_text(frame, width, height, 329.0, 188.0, small_scale, "RECAR.", brand::ACCENT_GREEN_LIGHT);
            if self.entries.is_empty() {
                let card_height = (logical_height - 320.0).clamp(340.0, 520.0);
                responsive_panel(frame, width, height, 20.0, 250.0, 360.0, card_height, scale, 0x21152F);
                responsive_folder_icon(frame, width, height, 166.0, 302.0, scale);
                responsive_text(frame, width, height, 45.0, 414.0, small_scale, "SUA COLECAO COMeca AQUI", brand::TEXT);
                responsive_text(frame, width, height, 42.0, 454.0, small_scale, "ADICIONE SEUS ARQUIVOS 3DS", 0xBBAFC7);
                responsive_text(frame, width, height, 42.0, 482.0, small_scale, "A PASTA DE JOGOS DO ACERIS.", 0xBBAFC7);
                responsive_text(frame, width, height, 42.0, 532.0, small_scale, &tail(&self.directory_label, 40), brand::ACCENT_GREEN_LIGHT);
                responsive_panel(frame, width, height, 38.0, 602.0, 324.0, 52.0, scale, brand::ACCENT_RED);
                responsive_text(frame, width, height, 74.0, 620.0, small_scale, "ATUALIZAR BIBLIOTECA", brand::TEXT);
                responsive_text(frame, width, height, 42.0, 682.0, small_scale, "FORMATOS: .3DS  .CCI  .3DSX", 0xBBAFC7);
            } else {
                let row_height = 58.0;
                let visible = ((logical_height - 350.0).max(row_height) / row_height).floor().max(1.0) as usize;
                for (row, entry) in self.entries.iter().enumerate().skip(self.first_visible).take(visible) {
                    let y = 250.0 + (row - self.first_visible) as f32 * row_height;
                    responsive_panel(frame, width, height, 20.0, y, 360.0, 50.0, scale, if row == self.selected { 0x422038 } else { 0x21152F });
                    responsive_text(frame, width, height, 36.0, y + 17.0, small_scale, &truncate(&entry.title, 38), brand::TEXT);
                    let ext = entry.path.extension().and_then(|v| v.to_str()).unwrap_or("3DS");
                    responsive_text(frame, width, height, 330.0, y + 18.0, small_scale, &ext.to_ascii_uppercase(), brand::ACCENT_GREEN_LIGHT);
                }
            }
        }
        if let Some(status) = &self.status {
            responsive_text(frame, width, height, 22.0, (logical_height - 62.0).max(720.0), small_scale, &truncate(status, 54), brand::ACCENT_RED_LIGHT);
        }
        responsive_fill(frame, width as usize, height as usize, 0.0, (logical_height - 36.0).max(0.0), 400.0, 36.0, scale, 0x160D22);
        responsive_text(frame, width, height, 22.0, (logical_height - 22.0).max(0.0), small_scale, "ACERIS  |  RUST  |  SEUS ARQUIVOS, SUAS REGRAS", 0xBBAFC7);
    }

    fn render_landscape(&self, frame: &mut [u8], width: u32, height: u32, scale: f32, logical_height: f32) {
        let font_scale = (scale * 1.8).round().max(1.0) as usize;
        let small_scale = (font_scale as f32 * 0.67).round().max(1.0) as usize;
        responsive_fill(frame, width as usize, height as usize, 0.0, 0.0, 205.0, logical_height, scale, 0x160D22);
        responsive_fill(frame, width as usize, height as usize, 202.0, 0.0, 3.0, logical_height, scale, brand::ACCENT_RED);
        responsive_text(frame, width, height, 24.0, 30.0, font_scale, "ACERIS", brand::TEXT);
        responsive_text(frame, width, height, 24.0, 68.0, small_scale, "NINTENDO 3DS", 0xBBAFC7);

        let active_games = self.page == LibraryPage::Games;
        responsive_panel(frame, width, height, 16.0, 118.0, 174.0, 48.0, scale, if active_games { brand::ACCENT_RED } else { 0x251731 });
        responsive_text(frame, width, height, 40.0, 134.0, small_scale, "BIBLIOTECA", brand::TEXT);
        responsive_panel(frame, width, height, 16.0, 176.0, 174.0, 48.0, scale, if !active_games { brand::ACCENT_RED } else { 0x251731 });
        responsive_text(frame, width, height, 40.0, 192.0, small_scale, "CONFIGURACOES", brand::TEXT);
        responsive_text(frame, width, height, 24.0, (logical_height - 62.0).max(160.0), small_scale, "PC  |  ANDROID", 0xBBAFC7);

        responsive_text(frame, width, height, 238.0, 32.0, font_scale, if active_games { "SUA BIBLIOTECA" } else { "CONFIGURACOES" }, brand::TEXT);
        responsive_text(frame, width, height, 238.0, 72.0, small_scale, "GERENCIE SEUS JOGOS E PREFERENCIAS", 0xBBAFC7);
        responsive_panel(frame, width, height, 826.0, 28.0, 154.0, 54.0, scale, 0x2D1A3B);
        responsive_text(frame, width, height, 850.0, 48.0, small_scale, if active_games { "ATUALIZAR" } else { "RECARREGAR" }, brand::ACCENT_GREEN_LIGHT);

        if !active_games {
            responsive_panel(frame, width, height, 238.0, 130.0, 742.0, 340.0, scale, 0x21152F);
            responsive_text(frame, width, height, 270.0, 166.0, small_scale, "PASTA DE JOGOS", brand::ACCENT_GREEN_LIGHT);
            responsive_text(frame, width, height, 270.0, 204.0, small_scale, &tail(&self.directory_label, 86), brand::TEXT);
            responsive_text(frame, width, height, 270.0, 266.0, small_scale, "FORMATOS LISTADOS: .3DS  .CCI  .3DSX", brand::TEXT);
            responsive_text(frame, width, height, 270.0, 310.0, small_scale, "GRAFICO: AUTO / VULKAN / OPENGL", 0xBBAFC7);
            responsive_panel(frame, width, height, 270.0, 366.0, 330.0, 54.0, scale, brand::ACCENT_RED);
            responsive_text(frame, width, height, 318.0, 385.0, small_scale, "RECARREGAR BIBLIOTECA", brand::TEXT);
        } else if self.entries.is_empty() {
            responsive_panel(frame, width, height, 238.0, 130.0, 742.0, (logical_height - 184.0).max(250.0), scale, 0x21152F);
            responsive_folder_icon(frame, width, height, 565.0, 214.0, scale);
            responsive_text(frame, width, height, 420.0, 318.0, font_scale, "SUA COLECAO COMeca AQUI", brand::TEXT);
            responsive_text(frame, width, height, 408.0, 360.0, small_scale, "COLOQUE .3DS, .CCI OU .3DSX NA PASTA DE JOGOS", 0xBBAFC7);
            responsive_text(frame, width, height, 290.0, 405.0, small_scale, &tail(&self.directory_label, 96), brand::ACCENT_GREEN_LIGHT);
            responsive_panel(frame, width, height, 490.0, 452.0, 240.0, 54.0, scale, brand::ACCENT_RED);
            responsive_text(frame, width, height, 520.0, 471.0, small_scale, "ATUALIZAR", brand::TEXT);
        } else {
            let row_height = 76.0;
            let visible = ((logical_height - 196.0).max(row_height) / row_height).floor().max(1.0) as usize;
            for (row, entry) in self.entries.iter().enumerate().skip(self.first_visible).take(visible) {
                let y = 142.0 + (row - self.first_visible) as f32 * row_height;
                responsive_panel(frame, width, height, 238.0, y, 742.0, 62.0, scale, if row == self.selected { 0x422038 } else { 0x21152F });
                responsive_text(frame, width, height, 264.0, y + 22.0, small_scale, &truncate(&entry.title, 86), brand::TEXT);
                let ext = entry.path.extension().and_then(|v| v.to_str()).unwrap_or("3DS");
                responsive_text(frame, width, height, 890.0, y + 24.0, small_scale, &ext.to_ascii_uppercase(), brand::ACCENT_GREEN_LIGHT);
            }
        }
        if let Some(status) = &self.status {
            responsive_text(frame, width, height, 238.0, (logical_height - 42.0).max(0.0), small_scale, &truncate(status, 110), brand::ACCENT_RED_LIGHT);
        }
    }

    /// Draws the library as a 400x480 RGBA frame.
    pub fn render(&self) -> Vec<u8> {
        self.render_animated(0, None)
    }

    /// Draws the library with a deterministic animated brand backdrop.
    /// A host may supply one tightly-packed 400x480 RGBA background frame;
    /// invalid-sized frames are ignored rather than risking an out-of-bounds read.
    pub fn render_animated(&self, frame_index: u64, background_frame: Option<&[u8]>) -> Vec<u8> {
        let (w, h) = (WIDTH, HEIGHT);
        let mut out = vec![255u8; w * h * 4];
        let top = rgb(brand::PURPLE_LIGHT);
        let bottom = rgb(brand::PURPLE_DARK);
        for y in 0..h {
            let t = y as f32 / h as f32;
            let color = [0, 1, 2].map(|i| (top[i] as f32 * (1.0 - t) + bottom[i] as f32 * t) as u8);
            for x in 0..w {
                let at = (y * w + x) * 4;
                out[at..at + 3].copy_from_slice(&color);
            }
        }
        if let Some(background) = background_frame.filter(|frame| frame.len() == w * h * 4) {
            for (destination, source) in out.chunks_exact_mut(4).zip(background.chunks_exact(4)) {
                let alpha = (source[3] as f32 / 255.0 * 0.38).clamp(0.0, 0.38);
                for channel in 0..3 {
                    destination[channel] = (source[channel] as f32 * alpha
                        + destination[channel] as f32 * (1.0 - alpha))
                        as u8;
                }
            }
        }

        // Slow, low-contrast glints give the home screen movement without
        // changing the Aceris palette or competing with selected game rows.
        let animation = (frame_index % 1_000_000) as usize;
        let sweep = animation as i32 % 520 - 80;
        for y in (HEADER_HEIGHT + 4)..FOOTER_TOP {
            let x = (sweep + y / 5).rem_euclid(w as i32);
            blend_pixel(&mut out, x, y, rgb(brand::ACCENT_RED_LIGHT), 0.055);
        }
        let ambient_height = (FOOTER_TOP - HEADER_HEIGHT - 16) as usize;
        for particle in 0..7usize {
            let x = ((animation * (particle + 1) * 3 + particle * 47) % w) as i32;
            let y = HEADER_HEIGHT
                + 8
                + ((particle * 53 + animation * (particle + 1) / 9) % ambient_height) as i32;
            blend_pixel(&mut out, x, y, rgb(brand::ACCENT_GREEN_LIGHT), 0.11);
            blend_pixel(&mut out, x + 1, y, rgb(brand::ACCENT_RED_LIGHT), 0.07);
        }

        let text = rgb(brand::TEXT);
        let red = rgb(brand::ACCENT_RED);

        fill(
            &mut out,
            0,
            0,
            w as i32,
            HEADER_HEIGHT,
            rgb(brand::PURPLE_DARK),
        );
        fill(&mut out, 0, HEADER_HEIGHT - 3, w as i32, 3, red);
        draw_text(&mut out, w, h, MARGIN, 14, 4, "ACERIS", text);
        let count = format!("{} JOGOS", self.entries.len());
        let count_x = w as i32 - MARGIN - font::text_width(count.chars().count(), 1) as i32;
        draw_text(
            &mut out,
            w,
            h,
            count_x,
            22,
            1,
            &count,
            rgb(brand::ACCENT_RED_LIGHT),
        );

        if self.entries.is_empty() {
            self.draw_empty(&mut out);
        }
        let max_chars = (w as i32 - 2 * MARGIN - 16) as usize / (font::ADVANCE * TEXT_SCALE);
        for (row, entry) in self
            .entries
            .iter()
            .enumerate()
            .skip(self.first_visible)
            .take(VISIBLE_ROWS)
        {
            let y = LIST_TOP + (row - self.first_visible) as i32 * ROW_HEIGHT;
            let selected = row == self.selected;
            if selected {
                fill(
                    &mut out,
                    MARGIN - 8,
                    y,
                    w as i32 - 2 * (MARGIN - 8),
                    ROW_HEIGHT - 4,
                    red,
                );
            }
            let title = truncate(&entry.title, max_chars);
            let color = if selected { [255, 255, 255] } else { text };
            draw_text(&mut out, w, h, MARGIN, y + 5, TEXT_SCALE, &title, color);
        }
        if self.entries.len() > VISIBLE_ROWS {
            let track = (FOOTER_TOP - 22 - LIST_TOP) as f32;
            let thumb = (track * VISIBLE_ROWS as f32 / self.entries.len() as f32).max(12.0);
            let offset = (track - thumb) * self.first_visible as f32
                / (self.entries.len() - VISIBLE_ROWS) as f32;
            fill(
                &mut out,
                w as i32 - 6,
                LIST_TOP + offset as i32,
                3,
                thumb as i32,
                rgb(brand::ACCENT_GREEN),
            );
        }

        if let Some(status) = &self.status {
            let message = truncate(status, (w - 2 * MARGIN as usize) / font::ADVANCE);
            draw_text(
                &mut out,
                w,
                h,
                MARGIN,
                FOOTER_TOP - 16,
                1,
                &message,
                rgb(brand::ACCENT_RED_LIGHT),
            );
        }
        fill(
            &mut out,
            0,
            FOOTER_TOP,
            w as i32,
            h as i32 - FOOTER_TOP,
            rgb(brand::PURPLE_DARK),
        );
        draw_text(
            &mut out,
            w,
            h,
            MARGIN,
            FOOTER_TOP + 14,
            1,
            "SETAS: ESCOLHER   ENTER: JOGAR   ESC: SAIR",
            text,
        );
        out
    }

    fn draw_empty(&self, out: &mut [u8]) {
        let text = rgb(brand::TEXT);
        let lines = [
            "NENHUM JOGO ENCONTRADO".to_string(),
            "COLOQUE .3DS, .CCI OU .3DSX EM:".to_string(),
        ];
        for (i, line) in lines.iter().enumerate() {
            draw_text(
                out,
                WIDTH,
                HEIGHT,
                MARGIN,
                LIST_TOP + 20 + i as i32 * 24,
                1,
                line,
                text,
            );
        }
        let path = tail(
            &self.directory_label,
            (WIDTH - 2 * MARGIN as usize) / font::ADVANCE,
        );
        draw_text(
            out,
            WIDTH,
            HEIGHT,
            MARGIN,
            LIST_TOP + 68,
            1,
            &path,
            rgb(brand::ACCENT_GREEN_LIGHT),
        );
    }
}


fn responsive_metrics(width: u32, height: u32) -> Option<(bool, f32, f32, f32)> {
    if width < 160 || height < 160 || width > 4096 || height > 4096 {
        return None;
    }
    let portrait = height >= width;
    let logical_width = if portrait { 400.0 } else { 1000.0 };
    let scale = width as f32 / logical_width;
    let logical_height = height as f32 / scale;
    (scale.is_finite() && scale > 0.0 && logical_height.is_finite())
        .then_some((portrait, scale, logical_width, logical_height))
}

fn responsive_fill(
    frame: &mut [u8],
    width: usize,
    height: usize,
    x: f32,
    y: f32,
    rect_width: f32,
    rect_height: f32,
    scale: f32,
    color: u32,
) {
    let left = (x * scale).floor().max(0.0) as usize;
    let top = (y * scale).floor().max(0.0) as usize;
    let right = ((x + rect_width) * scale).ceil().min(width as f32) as usize;
    let bottom = ((y + rect_height) * scale).ceil().min(height as f32) as usize;
    let pixel = [
        (color >> 16) as u8,
        (color >> 8) as u8,
        color as u8,
        255,
    ];
    if left >= right || top >= bottom || frame.len() != width.saturating_mul(height).saturating_mul(4) {
        return;
    }
    for row in top..bottom {
        let start = (row * width + left) * 4;
        let end = (row * width + right) * 4;
        for rgba in frame[start..end].chunks_exact_mut(4) {
            rgba.copy_from_slice(&pixel);
        }
    }
}

fn responsive_panel(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    rect_width: f32,
    rect_height: f32,
    scale: f32,
    color: u32,
) {
    responsive_fill(frame, width as usize, height as usize, x, y, rect_width, rect_height, scale, 0x563B67);
    responsive_fill(frame, width as usize, height as usize, x + 1.0, y + 1.0, rect_width - 2.0, rect_height - 2.0, scale, color);
}

fn responsive_text(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    text_scale: usize,
    text: &str,
    color: u32,
) {
    draw_text(
        frame,
        width as usize,
        height as usize,
        x.round() as i32,
        y.round() as i32,
        text_scale.max(1),
        text,
        [
            (color >> 16) as u8,
            (color >> 8) as u8,
            color as u8,
        ],
    );
}

fn responsive_folder_icon(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: f32,
    y: f32,
    scale: f32,
) {
    responsive_fill(frame, width as usize, height as usize, x - 36.0, y - 12.0, 72.0, 48.0, scale, brand::ACCENT_GREEN_LIGHT);
    responsive_fill(frame, width as usize, height as usize, x - 29.0, y - 21.0, 34.0, 14.0, scale, brand::ACCENT_GREEN_LIGHT);
    responsive_fill(frame, width as usize, height as usize, x - 31.0, y - 7.0, 62.0, 38.0, scale, 0x2D1A3B);
}

fn blend_pixel(canvas: &mut [u8], x: i32, y: i32, color: [u8; 3], alpha: f32) {
    if x < 0 || y < 0 || x >= WIDTH as i32 || y >= HEIGHT as i32 {
        return;
    }
    let at = (y as usize * WIDTH + x as usize) * 4;
    for channel in 0..3 {
        canvas[at + channel] =
            (color[channel] as f32 * alpha + canvas[at + channel] as f32 * (1.0 - alpha)) as u8;
    }
}

fn rgb(color: u32) -> [u8; 3] {
    [(color >> 16) as u8, (color >> 8) as u8, color as u8]
}

fn fill(canvas: &mut [u8], x: i32, y: i32, width: i32, height: i32, color: [u8; 3]) {
    for py in y.max(0)..(y + height).min(HEIGHT as i32) {
        for px in x.max(0)..(x + width).min(WIDTH as i32) {
            let at = (py as usize * WIDTH + px as usize) * 4;
            canvas[at..at + 3].copy_from_slice(&color);
        }
    }
}

/// Shortens `text` to `max` characters, ending in `...` when cut.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(3);
    text.chars().take(keep).collect::<String>() + "..."
}

/// Keeps the last `max` characters, starting with `...` when cut.
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(3);
    "...".to_string() + &text.chars().skip(count - keep).collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn library(count: usize) -> Library {
        let entries = (0..count)
            .map(|i| Entry {
                title: format!("Game {i}"),
                path: PathBuf::from(format!("g{i}.3ds")),
            })
            .collect();
        Library::new(entries, "/games".to_string())
    }


    #[test]
    fn responsive_launcher_fills_phone_and_desktop_surfaces() {
        let library = Library::new(Vec::new(), "/games".to_string());
        for (width, height) in [(689, 1536), (960, 600), (1920, 1080)] {
            let frame = library.render_responsive(width, height);
            assert_eq!(frame.len(), width as usize * height as usize * 4);
            assert!(frame.chunks_exact(4).all(|pixel| pixel[3] == 255));
        }
    }

    #[test]
    fn responsive_navigation_refreshes_and_selects_games() {
        let dir = std::env::temp_dir().join(format!("aceris-responsive-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("sample.3dsx"), b"fixture").unwrap();
        let mut library = Library::new(Vec::new(), dir.display().to_string());
        assert_eq!(library.click_responsive(400, 1_240, 800, 1200), ClickResult::Ignored);
        assert_eq!(library.entries().len(), 1);
        assert_eq!(library.click_responsive(700, 250, 800, 1200), ClickResult::Ignored);
        assert_eq!(library.page(), LibraryPage::Settings);
        library.reload();
        assert_eq!(library.entries().len(), 1);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn selection_clamps_and_scrolls_to_stay_visible() {
        let mut lib = library(30);
        lib.move_selection(-5);
        assert_eq!(lib.selected_index(), 0);
        lib.move_selection(VISIBLE_ROWS as i32 + 2);
        assert_eq!(lib.selected_index(), VISIBLE_ROWS + 2);
        assert!(lib.selected_index() < lib.first_visible + VISIBLE_ROWS);
        lib.move_selection(1000);
        assert_eq!(lib.selected_index(), 29);
        lib.move_selection(-1000);
        assert_eq!((lib.selected_index(), lib.first_visible), (0, 0));
        assert_eq!(library(0).selected_entry(), None);
    }

    #[test]
    fn click_selects_then_activates_and_ignores_empty_areas() {
        let mut lib = library(3);
        let row_y = |row: i32| LIST_TOP + row * ROW_HEIGHT + 4;
        assert_eq!(lib.click(100, row_y(2)), ClickResult::Selected);
        assert_eq!(lib.selected_index(), 2);
        assert_eq!(lib.click(100, row_y(2)), ClickResult::Activated);
        assert_eq!(lib.click(100, row_y(5)), ClickResult::Ignored);
        assert_eq!(lib.click(100, 10), ClickResult::Ignored);
    }

    #[test]
    fn render_has_the_canvas_size_and_marks_the_selected_row() {
        let mut lib = library(4);
        lib.move_selection(1);
        let frame = lib.render();
        assert_eq!(frame.len(), WIDTH * HEIGHT * 4);
        let at = ((LIST_TOP + ROW_HEIGHT + 20) as usize * WIDTH + 12) * 4;
        assert_eq!(&frame[at..at + 3], &rgb(brand::ACCENT_RED));
        // The empty and status screens must render without panicking.
        let mut empty = library(0);
        empty.set_status("COULD NOT LOAD");
        assert_eq!(empty.render().len(), WIDTH * HEIGHT * 4);
        assert_eq!(library(500).render().len(), WIDTH * HEIGHT * 4);
    }

    #[test]
    fn truncation_helpers_respect_the_limit() {
        assert_eq!(truncate("ABCDEFGHIJ", 6), "ABC...");
        assert_eq!(truncate("ABC", 6), "ABC");
        assert_eq!(tail("/a/long/path/here", 8), ".../here");
    }

    #[test]
    fn scan_lists_only_cartridge_images_sorted() {
        let dir = std::env::temp_dir().join(format!("aceris-lib-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for name in ["b.3ds", "A.CCI", "homebrew.3dsx", "notes.txt", "c.cia"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let titles: Vec<String> = scan_games(&dir).into_iter().map(|e| e.title).collect();
        assert_eq!(titles, ["A", "b", "homebrew"]);
        fs::remove_dir_all(&dir).unwrap();
        assert!(scan_games(&dir).is_empty());
    }
}
