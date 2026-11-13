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
}

/// Lists the cartridge images (`.3ds`, `.cci`) directly inside `directory`,
/// sorted by title. A missing or unreadable directory yields an empty list.
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
                        ext.eq_ignore_ascii_case("3ds") || ext.eq_ignore_ascii_case("cci")
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

    /// Draws the library as a 400x480 RGBA frame.
    pub fn render(&self) -> Vec<u8> {
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
            "COLOQUE ARQUIVOS .3DS OU .CCI EM:".to_string(),
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
        for name in ["b.3ds", "A.CCI", "notes.txt", "c.cia"] {
            fs::write(dir.join(name), b"x").unwrap();
        }
        let titles: Vec<String> = scan_games(&dir).into_iter().map(|e| e.title).collect();
        assert_eq!(titles, ["A", "b"]);
        fs::remove_dir_all(&dir).unwrap();
        assert!(scan_games(&dir).is_empty());
    }
}
