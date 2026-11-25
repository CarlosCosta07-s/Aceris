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
    Home,
    Games,
    Favorites,
    Recent,
    Homebrew,
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
    favorites: std::collections::HashSet<String>,
    recent: Vec<String>,
    sort_descending: bool,
    external_media_playing: bool,
    external_media_detection_available: bool,
}

/// Reads a persistent list of simple file names from the Aceris settings folder.
/// Invalid path components are ignored so the collection can never escape its game folder.
fn load_library_ids(path: &Path) -> Vec<String> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };
    contents
        .lines()
        .map(str::trim)
        .filter(|name| {
            !name.is_empty()
                && Path::new(name).file_name().and_then(|value| value.to_str()) == Some(*name)
        })
        .map(str::to_string)
        .collect()
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
        let directory = PathBuf::from(&directory_label);
        let favorites = load_library_ids(&directory.join(".aceris").join("favorites.txt"))
            .into_iter()
            .collect();
        let recent = load_library_ids(&directory.join(".aceris").join("recent.txt"));
        Self {
            entries,
            selected: 0,
            first_visible: 0,
            directory_label,
            status: None,
            page: LibraryPage::Home,
            favorites,
            recent,
            sort_descending: false,
            external_media_playing: false,
            external_media_detection_available: false,
        }
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        if self.page == LibraryPage::Home {
            return self.recent.iter().find_map(|name| {
                self.entries.iter().find(|entry| {
                    entry.path.file_name().and_then(|value| value.to_str()) == Some(name.as_str())
                })
            });
        }
        self.filtered_entries().get(self.selected).copied()
    }

    /// Number of entries shown by the current library section.
    pub fn visible_entry_count(&self) -> usize {
        self.filtered_entries().len()
    }

    /// True when an entry is in the user's persistent favorites collection.
    pub fn is_favorite(&self, entry: &Entry) -> bool {
        entry
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| self.favorites.contains(name))
    }

    /// Adds or removes the currently selected item from Favorites.
    pub fn toggle_selected_favorite(&mut self) -> bool {
        let Some(entry) = self.selected_entry() else {
            return false;
        };
        let Some(name) = entry.path.file_name().and_then(|value| value.to_str()) else {
            return false;
        };
        let name = name.to_string();
        if !self.favorites.remove(&name) {
            self.favorites.insert(name);
        }
        self.persist_library_ids("favorites.txt", self.favorites.iter().cloned());
        self.status = Some("Favoritos atualizados".to_string());
        true
    }

    /// Records a successfully launched game at the front of the persistent Recent list.
    pub fn mark_launched(&mut self, path: &Path) {
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            return;
        };
        let name = name.to_string();
        self.recent.retain(|item| item != &name);
        self.recent.insert(0, name);
        self.recent.truncate(24);
        self.persist_library_ids("recent.txt", self.recent.iter().cloned());
        self.status = None;
    }

    /// Number of existing game entries currently saved in Favorites.
    pub fn favorite_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| self.is_favorite(entry))
            .count()
    }

    /// Number of valid game entries kept in Recent.
    pub fn recent_count(&self) -> usize {
        self.recent
            .iter()
            .filter(|name| {
                self.entries.iter().any(|entry| {
                    entry.path.file_name().and_then(|value| value.to_str()) == Some(name.as_str())
                })
            })
            .count()
    }

    /// Switches between A–Z and Z–A; the Recent section keeps its time order.
    pub fn toggle_sort_order(&mut self) {
        self.sort_descending = !self.sort_descending;
        self.selected = 0;
        self.first_visible = 0;
        self.status = Some(
            if self.sort_descending {
                "Ordem: Z–A"
            } else {
                "Ordem: A–Z"
            }
            .to_string(),
        );
    }

    /// Updates the external-media indicator shown on the home screen.
    pub fn set_external_media_playing(&mut self, playing: bool) {
        self.external_media_playing = playing;
    }

    pub fn external_media_playing(&self) -> bool {
        self.external_media_playing
    }

    /// Indicates whether the Android host can reliably detect external media.
    pub fn set_external_media_detection_available(&mut self, available: bool) {
        self.external_media_detection_available = available;
        if !available {
            self.external_media_playing = false;
        }
    }

    pub fn external_media_detection_available(&self) -> bool {
        self.external_media_detection_available
    }

    fn persist_library_ids(&self, file_name: &str, values: impl IntoIterator<Item = String>) {
        let directory = Path::new(&self.directory_label).join(".aceris");
        if fs::create_dir_all(&directory).is_err() {
            return;
        }
        let content = values.into_iter().collect::<Vec<_>>().join("\n");
        let _ = fs::write(directory.join(file_name), content);
    }

    /// Changes the library section and resets its independent selection.
    pub fn select_page(&mut self, page: LibraryPage) {
        if self.page != page {
            self.page = page;
            self.selected = 0;
            self.first_visible = 0;
            self.status = None;
        }
    }

    fn entry_matches_page(&self, entry: &Entry) -> bool {
        let is_homebrew = entry
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("3dsx"));
        match self.page {
            LibraryPage::Home => false,
            LibraryPage::Games => !is_homebrew,
            LibraryPage::Favorites => self.is_favorite(entry),
            LibraryPage::Recent => self.recent.iter().any(|name| {
                entry.path.file_name().and_then(|value| value.to_str()) == Some(name.as_str())
            }),
            LibraryPage::Homebrew => is_homebrew,
            LibraryPage::Settings => false,
        }
    }

    fn filtered_entries(&self) -> Vec<&Entry> {
        if self.page == LibraryPage::Recent {
            return self
                .recent
                .iter()
                .filter_map(|name| {
                    self.entries.iter().find(|entry| {
                        entry.path.file_name().and_then(|value| value.to_str())
                            == Some(name.as_str())
                    })
                })
                .collect();
        }
        let mut entries: Vec<&Entry> = self
            .entries
            .iter()
            .filter(|entry| self.entry_matches_page(entry))
            .collect();
        if self.sort_descending {
            entries.reverse();
        }
        entries
    }

    /// Shows a message (for example a load error) above the footer.
    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
    }

    /// Current user-visible library status or loading diagnostic, if any.
    pub fn status_message(&self) -> Option<&str> {
        self.status.as_deref()
    }

    /// Moves the selection by `delta` rows, clamped to the list.
    pub fn move_selection(&mut self, delta: i32) {
        let count = self.visible_entry_count();
        if count == 0 {
            return;
        }
        let last = count as i64 - 1;
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
        self.status = Some(format!(
            "BIBLIOTECA ATUALIZADA: {} JOGO(S)",
            self.entries.len()
        ));
    }

    /// Moves selection using the number of rows actually visible on this surface.
    pub fn move_selection_responsive(&mut self, delta: i32, width: u32, height: u32) {
        let Some((portrait, _scale, _, logical_height)) = responsive_metrics(width, height) else {
            return;
        };
        if portrait {
            let visible = ((logical_height - 350.0).max(58.0) / 58.0).floor().max(1.0) as usize;
            self.move_selection_with_visible(delta, visible);
        } else {
            let visible_rows = ((logical_height - 166.0) / 94.0).floor().max(1.0) as usize;
            let visible_items = visible_rows * 3;
            let delta = if delta.abs() == 1 { delta * 3 } else { delta };
            self.move_grid_selection(delta, visible_items);
        }
    }

    fn move_grid_selection(&mut self, delta: i32, visible_items: usize) {
        let count = self.visible_entry_count();
        if count == 0 {
            return;
        }
        self.selected =
            (self.selected as i64 + i64::from(delta)).clamp(0, count as i64 - 1) as usize;
        self.status = None;
        let visible_items = visible_items.max(3);
        if self.selected < self.first_visible {
            self.first_visible = (self.selected / 3) * 3;
        } else if self.selected >= self.first_visible + visible_items {
            let needed = self.selected + 1 - visible_items;
            self.first_visible = needed.div_ceil(3) * 3;
        }
    }

    fn move_selection_with_visible(&mut self, delta: i32, visible_rows: usize) {
        let count = self.visible_entry_count();
        if count == 0 {
            return;
        }
        let last = count as i64 - 1;
        self.selected = (self.selected as i64 + i64::from(delta)).clamp(0, last) as usize;
        self.status = None;
        let visible_rows = visible_rows.max(1);
        if self.selected < self.first_visible {
            self.first_visible = self.selected;
        } else if self.selected >= self.first_visible + visible_rows {
            self.first_visible = self.selected + 1 - visible_rows;
        }
    }

    /// Moves one card horizontally in the landscape collection grid.
    pub fn move_selection_horizontal_responsive(&mut self, delta: i32, width: u32, height: u32) {
        let Some((portrait, _, _, logical_height)) = responsive_metrics(width, height) else {
            return;
        };
        if portrait {
            let visible = ((logical_height - 350.0).max(58.0) / 58.0).floor().max(1.0) as usize;
            self.move_selection_with_visible(delta, visible);
        } else {
            let visible_rows = ((logical_height - 166.0) / 94.0).floor().max(1.0) as usize;
            self.move_grid_selection(delta, visible_rows * 3);
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
            // Portrait rendering remains a fallback for non-Android hosts.
            if (100.0..150.0).contains(&y) {
                let page = if x < 134.0 {
                    LibraryPage::Games
                } else if x < 266.0 {
                    LibraryPage::Homebrew
                } else {
                    LibraryPage::Settings
                };
                self.select_page(page);
                return ClickResult::Ignored;
            }
            if self.page == LibraryPage::Settings {
                if (500.0..552.0).contains(&y) && (38.0..362.0).contains(&x) {
                    self.reload();
                }
                return ClickResult::Ignored;
            }
            if ((168.0..222.0).contains(&y) && x >= 304.0)
                || ((490.0..542.0).contains(&y) && (38.0..362.0).contains(&x))
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
            if index >= self.first_visible + visible || index >= self.visible_entry_count() {
                return ClickResult::Ignored;
            }
            return self.select_or_activate(index);
        }

        // Permanent horizontal launcher: the rail remains in the same place
        // and every target is a generous, predictable touch area.
        if x < 104.0 {
            let page = match y as i32 {
                96..=151 => Some(LibraryPage::Home),
                152..=207 => Some(LibraryPage::Games),
                208..=263 => Some(LibraryPage::Favorites),
                264..=319 => Some(LibraryPage::Recent),
                320..=375 => Some(LibraryPage::Homebrew),
                376..=432 => Some(LibraryPage::Settings),
                _ => None,
            };
            if let Some(page) = page {
                self.select_page(page);
            }
            return ClickResult::Ignored;
        }

        if (18.0..68.0).contains(&y) && (700.0..830.0).contains(&x) {
            self.toggle_sort_order();
            return ClickResult::Ignored;
        }
        if (18.0..68.0).contains(&y) && x >= 842.0 {
            self.reload();
            return ClickResult::Ignored;
        }

        match self.page {
            LibraryPage::Home => {
                if (112.0..340.0).contains(&y) && (126.0..650.0).contains(&x) {
                    if self.recent_count() > 0 {
                        self.selected = 0;
                        return ClickResult::Activated;
                    }
                    self.select_page(LibraryPage::Games);
                } else if (110.0..225.0).contains(&y) && x >= 664.0 {
                    self.select_page(LibraryPage::Favorites);
                } else if (232.0..348.0).contains(&y) && x >= 664.0 {
                    self.select_page(LibraryPage::Homebrew);
                } else if (350.0..logical_height - 24.0).contains(&y) && x < 650.0 {
                    self.select_page(LibraryPage::Games);
                }
                ClickResult::Ignored
            }
            LibraryPage::Settings => {
                // Match the visible refresh button in the landscape settings card.
                if (298.0..350.0).contains(&y) && (158.0..398.0).contains(&x) {
                    self.reload();
                }
                ClickResult::Ignored
            }
            LibraryPage::Games
            | LibraryPage::Favorites
            | LibraryPage::Recent
            | LibraryPage::Homebrew => {
                let columns = 3usize;
                let card_top = 124.0;
                let card_height = 82.0;
                let row_step = 94.0;
                let card_width = 270.0;
                let card_gap = 14.0;
                if y < card_top {
                    return ClickResult::Ignored;
                }
                let row = ((y - card_top) / row_step).floor().max(0.0) as usize;
                let column = ((x - 128.0) / (card_width + card_gap)).floor().max(0.0) as usize;
                if column >= columns {
                    return ClickResult::Ignored;
                }
                let card_x = 128.0 + column as f32 * (card_width + card_gap);
                let card_y = card_top + row as f32 * row_step;
                if x < card_x || x >= card_x + card_width || y >= card_y + card_height {
                    return ClickResult::Ignored;
                }
                let visible_rows = ((logical_height - card_top - 42.0) / row_step)
                    .floor()
                    .max(1.0) as usize;
                let index = self.first_visible + row * columns + column;
                if row >= visible_rows || index >= self.visible_entry_count() {
                    return ClickResult::Ignored;
                }
                self.selected = index;
                // The small favorite control is independent of the card's launch action.
                if x >= card_x + card_width - 58.0 && y <= card_y + 40.0 {
                    self.toggle_selected_favorite();
                    return ClickResult::Ignored;
                }
                self.select_or_activate(index)
            }
        }
    }

    fn select_or_activate(&mut self, index: usize) -> ClickResult {
        if index == self.selected {
            ClickResult::Activated
        } else {
            self.selected = index;
            self.status = None;
            ClickResult::Selected
        }
    }

    /// Renders the launcher directly at the physical surface size.
    pub fn render_responsive(&self, width: u32, height: u32) -> Vec<u8> {
        self.render_responsive_with_background(width, height, None)
    }

    /// Renders the responsive launcher with an optional user-provided background frame.
    pub fn render_responsive_with_background(
        &self,
        width: u32,
        height: u32,
        background: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut frame = Vec::new();
        self.render_responsive_into_with_background(width, height, &mut frame, background);
        frame
    }

    /// Renders into a caller-owned RGBA buffer, retaining its allocation.
    pub fn render_responsive_into(&self, width: u32, height: u32, frame: &mut Vec<u8>) -> bool {
        self.render_responsive_into_with_background(width, height, frame, None)
    }

    /// Renders into a reusable frame and optionally composites a 400x480 user background.
    pub fn render_responsive_into_with_background(
        &self,
        width: u32,
        height: u32,
        frame: &mut Vec<u8>,
        background: Option<&[u8]>,
    ) -> bool {
        let Some((portrait, scale, logical_width, logical_height)) =
            responsive_metrics(width, height)
        else {
            frame.clear();
            return false;
        };
        let Some(byte_len) = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(4))
        else {
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
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            0.0,
            logical_width,
            logical_height,
            scale,
            brand::PURPLE_DARK,
        );
        if let Some(background) = background {
            responsive_blend_background(frame, width, height, background);
        }
        if portrait {
            self.render_portrait(frame, width, height, scale, logical_height);
        } else {
            self.render_landscape(frame, width, height, scale, logical_height);
        }
        true
    }

    fn render_portrait(
        &self,
        frame: &mut [u8],
        width: u32,
        height: u32,
        scale: f32,
        logical_height: f32,
    ) {
        let font_scale = (scale * 1.7).round().max(2.0) as usize;
        let small_scale = (scale * 1.15).round().max(1.0) as usize;

        // Brand header.
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            0.0,
            400.0,
            84.0,
            scale,
            0x160D22,
        );
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            82.0,
            400.0,
            3.0,
            scale,
            brand::ACCENT_RED,
        );
        responsive_text(
            frame,
            width,
            height,
            22.0,
            14.0,
            font_scale,
            "ACERIS",
            brand::TEXT,
        );
        responsive_text(
            frame,
            width,
            height,
            24.0,
            54.0,
            small_scale,
            "Emulador de Nintendo 3DS",
            0xBBAFC7,
        );

        // Three clear sections: installed games, homebrew, and settings.
        let tabs = [
            (16.0, 116.0, LibraryPage::Games, "Jogos", 56.0),
            (142.0, 116.0, LibraryPage::Homebrew, "Homebrew", 152.0),
            (268.0, 116.0, LibraryPage::Settings, "Config.", 300.0),
        ];
        for (x, tab_width, page, label, text_x) in tabs {
            responsive_panel(
                frame,
                width,
                height,
                x,
                101.0,
                tab_width,
                44.0,
                scale,
                if self.page == page {
                    brand::ACCENT_RED
                } else {
                    0x251731
                },
            );
            responsive_text(
                frame,
                width,
                height,
                text_x,
                116.0,
                small_scale,
                label,
                brand::TEXT,
            );
        }

        let page_title = match self.page {
            LibraryPage::Home => "Início",
            LibraryPage::Games => "Biblioteca",
            LibraryPage::Favorites => "Favoritos",
            LibraryPage::Recent => "Recentes",
            LibraryPage::Homebrew => "Homebrew e demos",
            LibraryPage::Settings => "Configurações",
        };
        responsive_text(
            frame,
            width,
            height,
            22.0,
            174.0,
            font_scale,
            page_title,
            brand::TEXT,
        );

        if self.page == LibraryPage::Settings {
            responsive_text(
                frame,
                width,
                height,
                22.0,
                212.0,
                small_scale,
                "Pastas e formatos compatíveis",
                0xBBAFC7,
            );
            responsive_panel(
                frame, width, height, 20.0, 250.0, 360.0, 300.0, scale, 0x21152F,
            );
            responsive_text(
                frame,
                width,
                height,
                38.0,
                278.0,
                small_scale,
                "Pasta de jogos",
                brand::ACCENT_GREEN_LIGHT,
            );
            responsive_text(
                frame,
                width,
                height,
                38.0,
                310.0,
                small_scale,
                &tail(&self.directory_label, 44),
                brand::TEXT,
            );
            responsive_text(
                frame,
                width,
                height,
                38.0,
                370.0,
                small_scale,
                "Formatos reconhecidos",
                brand::ACCENT_GREEN_LIGHT,
            );
            responsive_text(
                frame,
                width,
                height,
                38.0,
                402.0,
                small_scale,
                ".3ds  /  .cci  /  .3dsx",
                brand::TEXT,
            );
            responsive_text(
                frame,
                width,
                height,
                38.0,
                458.0,
                small_scale,
                "Os arquivos permanecem no seu dispositivo.",
                0xBBAFC7,
            );
            responsive_panel(
                frame,
                width,
                height,
                38.0,
                500.0,
                324.0,
                52.0,
                scale,
                brand::ACCENT_RED,
            );
            responsive_text_centered(
                frame,
                width,
                height,
                200.0,
                518.0,
                small_scale,
                "Atualizar biblioteca",
                brand::TEXT,
            );
        } else {
            let count = self.visible_entry_count();
            let count_text = match (self.page, count) {
                (LibraryPage::Homebrew, 1) => "1 homebrew disponível".to_string(),
                (LibraryPage::Homebrew, _) => format!("{count} homebrews disponíveis"),
                (_, 1) => "1 jogo disponível".to_string(),
                _ => format!("{count} jogos disponíveis"),
            };
            responsive_text(
                frame,
                width,
                height,
                22.0,
                212.0,
                small_scale,
                &count_text,
                0xBBAFC7,
            );
            responsive_panel(
                frame, width, height, 306.0, 170.0, 78.0, 44.0, scale, 0x2D1A3B,
            );
            responsive_text(
                frame,
                width,
                height,
                311.0,
                188.0,
                small_scale,
                "Atualizar",
                brand::ACCENT_GREEN_LIGHT,
            );

            if count == 0 {
                let compact = logical_height < 720.0;
                let card_top = 250.0;
                let card_height = (logical_height - 298.0).clamp(290.0, 430.0);
                responsive_panel(
                    frame,
                    width,
                    height,
                    20.0,
                    card_top,
                    360.0,
                    card_height,
                    scale,
                    0x21152F,
                );
                let (icon_y, title_y, line_one_y, line_two_y, path_y, button_y, formats_y) =
                    if compact {
                        (
                            card_top + 42.0,
                            card_top + 105.0,
                            card_top + 138.0,
                            card_top + 160.0,
                            card_top + 186.0,
                            card_top + 240.0,
                            card_top + 294.0,
                        )
                    } else {
                        (
                            card_top + 58.0,
                            card_top + 130.0,
                            card_top + 168.0,
                            card_top + 192.0,
                            card_top + 226.0,
                            card_top + 285.0,
                            card_top + 350.0,
                        )
                    };
                responsive_folder_icon(frame, width, height, 200.0, icon_y, scale);
                let (empty_title, first_line, second_line, formats) =
                    if self.page == LibraryPage::Homebrew {
                        (
                            "Nenhum homebrew encontrado",
                            "Adicione arquivos .3dsx à pasta",
                            "de jogos configurada no Aceris.",
                            "Formato: .3dsx",
                        )
                    } else {
                        (
                            "Sua biblioteca está vazia",
                            "Adicione seus arquivos de jogos à pasta",
                            "do Aceris para comecar a jogar.",
                            "Formatos: .3ds  /  .cci  /  .3dsx",
                        )
                    };
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    title_y,
                    small_scale,
                    empty_title,
                    brand::TEXT,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    line_one_y,
                    small_scale,
                    first_line,
                    0xBBAFC7,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    line_two_y,
                    small_scale,
                    second_line,
                    0xBBAFC7,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    38.0,
                    path_y - 22.0,
                    small_scale,
                    "Pasta de jogos",
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_panel(
                    frame, width, height, 38.0, path_y, 324.0, 48.0, scale, 0x2D1A3B,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    48.0,
                    path_y + 16.0,
                    small_scale,
                    &tail(&self.directory_label, 42),
                    brand::TEXT,
                );
                responsive_panel(
                    frame,
                    width,
                    height,
                    38.0,
                    button_y,
                    324.0,
                    52.0,
                    scale,
                    brand::ACCENT_RED,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    button_y + 18.0,
                    small_scale,
                    "Atualizar biblioteca",
                    brand::TEXT,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    200.0,
                    formats_y,
                    small_scale,
                    formats,
                    0xBBAFC7,
                );
            } else {
                let row_height = 58.0;
                let visible = ((logical_height - 350.0).max(row_height) / row_height)
                    .floor()
                    .max(1.0) as usize;
                for (row, entry) in self
                    .filtered_entries()
                    .into_iter()
                    .enumerate()
                    .skip(self.first_visible)
                    .take(visible)
                {
                    let y = 250.0 + (row - self.first_visible) as f32 * row_height;
                    responsive_panel(
                        frame,
                        width,
                        height,
                        20.0,
                        y,
                        360.0,
                        50.0,
                        scale,
                        if row == self.selected {
                            0x422038
                        } else {
                            0x21152F
                        },
                    );
                    responsive_panel(
                        frame,
                        width,
                        height,
                        30.0,
                        y + 7.0,
                        46.0,
                        36.0,
                        scale,
                        0x2D1A3B,
                    );
                    responsive_text(
                        frame,
                        width,
                        height,
                        36.0,
                        y + 19.0,
                        small_scale,
                        "3DS",
                        brand::ACCENT_GREEN_LIGHT,
                    );
                    responsive_text(
                        frame,
                        width,
                        height,
                        88.0,
                        y + 17.0,
                        small_scale,
                        &truncate(&entry.title, 30),
                        brand::TEXT,
                    );
                    let extension = entry
                        .path
                        .extension()
                        .and_then(|value| value.to_str())
                        .unwrap_or("3ds");
                    responsive_text(
                        frame,
                        width,
                        height,
                        330.0,
                        y + 18.0,
                        small_scale,
                        &extension.to_ascii_lowercase(),
                        brand::ACCENT_GREEN_LIGHT,
                    );
                }
            }
        }

        if let Some(status) = &self.status {
            responsive_text(
                frame,
                width,
                height,
                22.0,
                (logical_height - 62.0).max(0.0),
                small_scale,
                &truncate(status, 52),
                brand::ACCENT_RED_LIGHT,
            );
        }
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            (logical_height - 36.0).max(0.0),
            400.0,
            36.0,
            scale,
            0x160D22,
        );
        responsive_text(
            frame,
            width,
            height,
            22.0,
            (logical_height - 22.0).max(0.0),
            small_scale,
            "Aceris  |  Rust  |  Seus arquivos, suas regras",
            0xBBAFC7,
        );
    }

    fn render_landscape(
        &self,
        frame: &mut [u8],
        width: u32,
        height: u32,
        scale: f32,
        logical_height: f32,
    ) {
        let title_scale = (scale * 1.8).round().max(2.0) as usize;
        let text_scale = (scale * 1.12).round().max(1.0) as usize;
        let bottom = (logical_height - 24.0).max(400.0);
        let homebrew_count = self
            .entries
            .iter()
            .filter(|entry| {
                entry
                    .path
                    .extension()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case("3dsx"))
            })
            .count();
        let game_count = self.entries.len().saturating_sub(homebrew_count);

        // Fixed navigation rail, sized for a one-handed landscape launcher.
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            0.0,
            0.0,
            104.0,
            logical_height,
            scale,
            0x160D22,
        );
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            102.0,
            0.0,
            2.0,
            logical_height,
            scale,
            0x563B67,
        );
        responsive_panel(
            frame,
            width,
            height,
            17.0,
            17.0,
            68.0,
            48.0,
            scale,
            brand::ACCENT_RED,
        );
        responsive_text_centered(
            frame,
            width,
            height,
            51.0,
            32.0,
            title_scale,
            "A",
            brand::TEXT,
        );

        let nav = [
            (96.0, LibraryPage::Home, "INÍCIO", "H"),
            (152.0, LibraryPage::Games, "JOGOS", "J"),
            (208.0, LibraryPage::Favorites, "FAVORITOS", "F"),
            (264.0, LibraryPage::Recent, "RECENTES", "R"),
            (320.0, LibraryPage::Homebrew, "HOMEBREW", "D"),
            (376.0, LibraryPage::Settings, "AJUSTES", "C"),
        ];
        for (y, page, label, mark) in nav {
            if self.page == page {
                responsive_fill(
                    frame,
                    width as usize,
                    height as usize,
                    0.0,
                    y,
                    4.0,
                    48.0,
                    scale,
                    brand::ACCENT_RED,
                );
            }
            responsive_panel(
                frame,
                width,
                height,
                13.0,
                y + 3.0,
                78.0,
                42.0,
                scale,
                if self.page == page {
                    0x422038
                } else {
                    0x21152F
                },
            );
            responsive_text_centered(
                frame,
                width,
                height,
                52.0,
                y + 7.0,
                text_scale + 1,
                mark,
                brand::ACCENT_GREEN_LIGHT,
            );
            responsive_text_centered(
                frame,
                width,
                height,
                52.0,
                y + 29.0,
                (text_scale as f32 * 0.72).round().max(1.0) as usize,
                label,
                brand::TEXT,
            );
        }
        responsive_text_centered(
            frame,
            width,
            height,
            52.0,
            (logical_height - 24.0).max(0.0),
            text_scale,
            "3DS",
            0xBBAFC7,
        );

        // Consistent header and two useful toolbar actions on every page.
        responsive_fill(
            frame,
            width as usize,
            height as usize,
            104.0,
            0.0,
            896.0,
            84.0,
            scale,
            0x1D1029,
        );
        let (page_title, subtitle) = match self.page {
            LibraryPage::Home => ("Seu espaço de jogos", "Tudo pronto para jogar"),
            LibraryPage::Games => ("Biblioteca", "Seus jogos instalados"),
            LibraryPage::Favorites => ("Favoritos", "Os jogos que você marcou"),
            LibraryPage::Recent => ("Recentes", "Continue de onde parou"),
            LibraryPage::Homebrew => ("Homebrew & demos", "Aplicativos e projetos caseiros"),
            LibraryPage::Settings => ("Configurações", "Personalize sua experiência"),
        };
        responsive_text(
            frame,
            width,
            height,
            130.0,
            17.0,
            title_scale,
            page_title,
            brand::TEXT,
        );
        responsive_text(
            frame, width, height, 132.0, 56.0, text_scale, subtitle, 0xBBAFC7,
        );
        responsive_panel(
            frame, width, height, 700.0, 18.0, 130.0, 50.0, scale, 0x2D1A3B,
        );
        responsive_text_centered(
            frame,
            width,
            height,
            765.0,
            35.0,
            text_scale,
            if self.sort_descending {
                "ORDEM Z–A"
            } else {
                "ORDEM A–Z"
            },
            brand::ACCENT_GREEN_LIGHT,
        );
        responsive_panel(
            frame,
            width,
            height,
            842.0,
            18.0,
            140.0,
            50.0,
            scale,
            brand::ACCENT_RED,
        );
        responsive_text_centered(
            frame,
            width,
            height,
            912.0,
            35.0,
            text_scale,
            "ATUALIZAR",
            brand::TEXT,
        );

        match self.page {
            LibraryPage::Home => {
                responsive_panel(
                    frame, width, height, 128.0, 112.0, 520.0, 228.0, scale, 0x21152F,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    154.0,
                    137.0,
                    text_scale,
                    "CONTINUAR JOGANDO",
                    brand::ACCENT_GREEN_LIGHT,
                );
                if let Some(entry) = self.selected_entry() {
                    responsive_panel(
                        frame, width, height, 154.0, 166.0, 132.0, 150.0, scale, 0x2D1A3B,
                    );
                    responsive_text_centered(
                        frame,
                        width,
                        height,
                        220.0,
                        202.0,
                        title_scale + 1,
                        "3DS",
                        brand::TEXT,
                    );
                    let extension = entry
                        .path
                        .extension()
                        .and_then(|v| v.to_str())
                        .unwrap_or("3ds");
                    responsive_text_centered(
                        frame,
                        width,
                        height,
                        220.0,
                        273.0,
                        text_scale,
                        &extension.to_ascii_uppercase(),
                        brand::ACCENT_GREEN_LIGHT,
                    );
                    responsive_text(
                        frame,
                        width,
                        height,
                        312.0,
                        189.0,
                        title_scale,
                        &truncate(&entry.title, 34),
                        brand::TEXT,
                    );
                    responsive_text(
                        frame,
                        width,
                        height,
                        314.0,
                        226.0,
                        text_scale,
                        "Seu último jogo",
                        0xBBAFC7,
                    );
                    responsive_panel(
                        frame,
                        width,
                        height,
                        312.0,
                        263.0,
                        270.0,
                        48.0,
                        scale,
                        brand::ACCENT_RED,
                    );
                    responsive_text_centered(
                        frame,
                        width,
                        height,
                        447.0,
                        279.0,
                        text_scale,
                        "CONTINUAR",
                        brand::TEXT,
                    );
                } else {
                    responsive_folder_icon(frame, width, height, 220.0, 235.0, scale);
                    responsive_text(
                        frame,
                        width,
                        height,
                        312.0,
                        190.0,
                        title_scale,
                        "Sua coleção começa aqui",
                        brand::TEXT,
                    );
                    responsive_text(
                        frame,
                        width,
                        height,
                        314.0,
                        231.0,
                        text_scale,
                        "Adicione jogos para criar sua biblioteca.",
                        0xBBAFC7,
                    );
                    responsive_panel(
                        frame,
                        width,
                        height,
                        312.0,
                        268.0,
                        270.0,
                        44.0,
                        scale,
                        brand::ACCENT_RED,
                    );
                    responsive_text_centered(
                        frame,
                        width,
                        height,
                        447.0,
                        282.0,
                        text_scale,
                        "VER JOGOS",
                        brand::TEXT,
                    );
                }
                responsive_panel(
                    frame, width, height, 666.0, 112.0, 304.0, 104.0, scale, 0x21152F,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    690.0,
                    137.0,
                    text_scale,
                    "FAVORITOS",
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    690.0,
                    169.0,
                    title_scale,
                    &self.favorite_count().to_string(),
                    brand::TEXT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    788.0,
                    176.0,
                    text_scale,
                    "VER COLEÇÃO",
                    0xBBAFC7,
                );
                responsive_panel(
                    frame, width, height, 666.0, 232.0, 304.0, 108.0, scale, 0x21152F,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    690.0,
                    257.0,
                    text_scale,
                    "HOMEBREW & DEMOS",
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    690.0,
                    290.0,
                    title_scale,
                    &homebrew_count.to_string(),
                    brand::TEXT,
                );
                responsive_text(
                    frame, width, height, 788.0, 298.0, text_scale, "EXPLORAR", 0xBBAFC7,
                );
                responsive_panel(
                    frame, width, height, 128.0, 352.0, 520.0, 72.0, scale, 0x21152F,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    154.0,
                    373.0,
                    text_scale,
                    "SUA BIBLIOTECA",
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    154.0,
                    399.0,
                    text_scale,
                    &format!("{} jogo(s) disponíveis", game_count),
                    brand::TEXT,
                );
                responsive_panel(
                    frame, width, height, 666.0, 352.0, 304.0, 72.0, scale, 0x21152F,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    690.0,
                    373.0,
                    text_scale,
                    "MÚSICA EXTERNA",
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    690.0,
                    399.0,
                    (text_scale as f32 * 0.82).round().max(1.0) as usize,
                    if !self.external_media_detection_available {
                        "Detecção indisponível neste Android"
                    } else if self.external_media_playing {
                        "Tocando — áudio do jogo mudo"
                    } else {
                        "Nenhuma mídia detectada"
                    },
                    if self.external_media_playing {
                        brand::ACCENT_RED_LIGHT
                    } else {
                        0xBBAFC7
                    },
                );
            }
            LibraryPage::Settings => {
                responsive_panel(
                    frame, width, height, 128.0, 112.0, 842.0, 318.0, scale, 0x21152F,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    158.0,
                    143.0,
                    text_scale,
                    "PASTA DE JOGOS",
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    158.0,
                    176.0,
                    text_scale,
                    &tail(&self.directory_label, 78),
                    brand::TEXT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    158.0,
                    222.0,
                    text_scale,
                    "FORMATOS RECONHECIDOS",
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    158.0,
                    254.0,
                    text_scale,
                    ".3ds  /  .cci  /  .3dsx",
                    brand::TEXT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    590.0,
                    143.0,
                    text_scale,
                    "ÁUDIO EM SEGUNDO PLANO",
                    brand::ACCENT_GREEN_LIGHT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    590.0,
                    176.0,
                    text_scale,
                    if self.external_media_detection_available {
                        "Detecção de mídia externa: automática"
                    } else {
                        "Detecção indisponível neste Android"
                    },
                    brand::TEXT,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    590.0,
                    205.0,
                    text_scale,
                    "Jogos são silenciados durante música externa.",
                    0xBBAFC7,
                );
                responsive_text(
                    frame,
                    width,
                    height,
                    590.0,
                    254.0,
                    text_scale,
                    if self.external_media_playing {
                        "Status: mídia tocando"
                    } else {
                        "Status: sem mídia ativa"
                    },
                    if self.external_media_playing {
                        brand::ACCENT_RED_LIGHT
                    } else {
                        0xBBAFC7
                    },
                );
                responsive_panel(
                    frame,
                    width,
                    height,
                    158.0,
                    298.0,
                    240.0,
                    52.0,
                    scale,
                    brand::ACCENT_RED,
                );
                responsive_text_centered(
                    frame,
                    width,
                    height,
                    278.0,
                    315.0,
                    text_scale,
                    "ATUALIZAR BIBLIOTECA",
                    brand::TEXT,
                );
            }
            LibraryPage::Games
            | LibraryPage::Favorites
            | LibraryPage::Recent
            | LibraryPage::Homebrew => {
                let entries = self.filtered_entries();
                if entries.is_empty() {
                    responsive_panel(
                        frame,
                        width,
                        height,
                        128.0,
                        112.0,
                        842.0,
                        (bottom - 112.0).max(240.0),
                        scale,
                        0x21152F,
                    );
                    responsive_folder_icon(frame, width, height, 550.0, 218.0, scale);
                    let empty = match self.page {
                        LibraryPage::Favorites => "Nenhum favorito ainda",
                        LibraryPage::Recent => "Nenhum jogo recente",
                        LibraryPage::Homebrew => "Nenhum homebrew encontrado",
                        _ => "Sua biblioteca está vazia",
                    };
                    responsive_text_centered(
                        frame,
                        width,
                        height,
                        550.0,
                        290.0,
                        title_scale,
                        empty,
                        brand::TEXT,
                    );
                    responsive_text_centered(
                        frame,
                        width,
                        height,
                        550.0,
                        326.0,
                        text_scale,
                        "Adicione jogos à pasta do Aceris e atualize.",
                        0xBBAFC7,
                    );
                    responsive_panel(
                        frame,
                        width,
                        height,
                        438.0,
                        366.0,
                        224.0,
                        46.0,
                        scale,
                        brand::ACCENT_RED,
                    );
                    responsive_text_centered(
                        frame,
                        width,
                        height,
                        550.0,
                        381.0,
                        text_scale,
                        "ATUALIZAR",
                        brand::TEXT,
                    );
                } else {
                    let row_step = 94.0;
                    let card_top = 124.0;
                    let visible_rows = ((bottom - card_top) / row_step).floor().max(1.0) as usize;
                    let visible_items = visible_rows * 3;
                    for (index, entry) in entries
                        .into_iter()
                        .enumerate()
                        .skip(self.first_visible)
                        .take(visible_items)
                    {
                        let relative = index - self.first_visible;
                        let row = relative / 3;
                        let column = relative % 3;
                        let x = 128.0 + column as f32 * 284.0;
                        let y = card_top + row as f32 * row_step;
                        responsive_panel(
                            frame,
                            width,
                            height,
                            x,
                            y,
                            270.0,
                            82.0,
                            scale,
                            if index == self.selected {
                                0x422038
                            } else {
                                0x21152F
                            },
                        );
                        responsive_panel(
                            frame,
                            width,
                            height,
                            x + 8.0,
                            y + 8.0,
                            56.0,
                            66.0,
                            scale,
                            0x2D1A3B,
                        );
                        responsive_text_centered(
                            frame,
                            width,
                            height,
                            x + 36.0,
                            y + 24.0,
                            text_scale + 1,
                            "3DS",
                            brand::ACCENT_GREEN_LIGHT,
                        );
                        responsive_text(
                            frame,
                            width,
                            height,
                            x + 74.0,
                            y + 19.0,
                            text_scale,
                            &truncate(&entry.title, 20),
                            brand::TEXT,
                        );
                        let extension = entry
                            .path
                            .extension()
                            .and_then(|v| v.to_str())
                            .unwrap_or("3ds");
                        responsive_text(
                            frame,
                            width,
                            height,
                            x + 74.0,
                            y + 49.0,
                            (text_scale as f32 * 0.84).round().max(1.0) as usize,
                            &extension.to_ascii_uppercase(),
                            0xBBAFC7,
                        );
                        responsive_panel(
                            frame,
                            width,
                            height,
                            x + 218.0,
                            y + 8.0,
                            42.0,
                            25.0,
                            scale,
                            if self.is_favorite(entry) {
                                brand::ACCENT_RED
                            } else {
                                0x33223F
                            },
                        );
                        responsive_text_centered(
                            frame,
                            width,
                            height,
                            x + 239.0,
                            y + 14.0,
                            text_scale,
                            if self.is_favorite(entry) { "F" } else { "+" },
                            brand::TEXT,
                        );
                    }
                }
            }
        }
        if let Some(status) = &self.status {
            responsive_text(
                frame,
                width,
                height,
                128.0,
                (logical_height - 44.0).max(0.0),
                text_scale,
                &truncate(status, 96),
                brand::ACCENT_RED_LIGHT,
            );
        }
        responsive_text(
            frame,
            width,
            height,
            128.0,
            (logical_height - 18.0).max(0.0),
            1,
            "ACERIS  •  RUST  •  SEUS ARQUIVOS, SUAS REGRAS",
            0xBBAFC7,
        );
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
    (scale.is_finite() && scale > 0.0 && logical_height.is_finite()).then_some((
        portrait,
        scale,
        logical_width,
        logical_height,
    ))
}

/// Cover-crops a user 400x480 RGBA background and blends it below launcher controls.
fn responsive_blend_background(frame: &mut [u8], width: u32, height: u32, background: &[u8]) {
    const SOURCE_WIDTH: u32 = WIDTH as u32;
    const SOURCE_HEIGHT: u32 = HEIGHT as u32;
    if width == 0
        || height == 0
        || background.len() != SOURCE_WIDTH as usize * SOURCE_HEIGHT as usize * 4
        || frame.len() != width as usize * height as usize * 4
    {
        return;
    }

    let target_w = width as f64;
    let target_h = height as f64;
    let source_w = SOURCE_WIDTH as f64;
    let source_h = SOURCE_HEIGHT as f64;
    let scale = (target_w / source_w).max(target_h / source_h);
    let scaled_w = source_w * scale;
    let scaled_h = source_h * scale;
    let crop_x = (scaled_w - target_w) * 0.5;
    let crop_y = (scaled_h - target_h) * 0.5;

    for y in 0..height {
        let source_y =
            (((y as f64 + 0.5 + crop_y) / scale).floor() as u32).min(SOURCE_HEIGHT - 1) as usize;
        for x in 0..width {
            let source_x =
                (((x as f64 + 0.5 + crop_x) / scale).floor() as u32).min(SOURCE_WIDTH - 1) as usize;
            let source_at = (source_y * SOURCE_WIDTH as usize + source_x) * 4;
            let target_at = (y as usize * width as usize + x as usize) * 4;
            let alpha = u32::from(background[source_at + 3]) * 38 / 100;
            let inverse = 255 - alpha;
            for channel in 0..3 {
                frame[target_at + channel] = ((u32::from(background[source_at + channel]) * alpha
                    + u32::from(frame[target_at + channel]) * inverse)
                    / 255) as u8;
            }
        }
    }
}

// These low-level draw primitives intentionally accept a full rectangle and target buffer.
#[allow(clippy::too_many_arguments)]
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
    let pixel = [(color >> 16) as u8, (color >> 8) as u8, color as u8, 255];
    if left >= right
        || top >= bottom
        || frame.len() != width.saturating_mul(height).saturating_mul(4)
    {
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

// Panels share the target geometry directly to avoid transient layout allocations.
#[allow(clippy::too_many_arguments)]
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
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x,
        y,
        rect_width,
        rect_height,
        scale,
        0x563B67,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x + 1.0,
        y + 1.0,
        rect_width - 2.0,
        rect_height - 2.0,
        scale,
        color,
    );
}

// Text coordinates and scale are explicit so all frontend layouts use the same renderer.
#[allow(clippy::too_many_arguments)]
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
    let Some((_, ui_scale, _, _)) = responsive_metrics(width, height) else {
        return;
    };
    draw_text(
        frame,
        width as usize,
        height as usize,
        (x * ui_scale).round() as i32,
        (y * ui_scale).round() as i32,
        text_scale.max(1),
        text,
        [(color >> 16) as u8, (color >> 8) as u8, color as u8],
    );
}

/// Draws text centred on a logical X coordinate, accounting for the UI scale.
#[allow(clippy::too_many_arguments)]
fn responsive_text_centered(
    frame: &mut [u8],
    width: u32,
    height: u32,
    center_x: f32,
    y: f32,
    text_scale: usize,
    text: &str,
    color: u32,
) {
    let Some((_, ui_scale, logical_width, _)) = responsive_metrics(width, height) else {
        return;
    };
    let glyph_width = font::text_width(text.chars().count(), text_scale.max(1)) as f32;
    let logical_text_width = glyph_width / ui_scale;
    let x = (center_x - logical_text_width * 0.5).clamp(0.0, logical_width);
    responsive_text(frame, width, height, x, y, text_scale, text, color);
}

fn responsive_folder_icon(frame: &mut [u8], width: u32, height: u32, x: f32, y: f32, scale: f32) {
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x - 36.0,
        y - 12.0,
        72.0,
        48.0,
        scale,
        brand::ACCENT_GREEN_LIGHT,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x - 29.0,
        y - 21.0,
        34.0,
        14.0,
        scale,
        brand::ACCENT_GREEN_LIGHT,
    );
    responsive_fill(
        frame,
        width as usize,
        height as usize,
        x - 31.0,
        y - 7.0,
        62.0,
        38.0,
        scale,
        0x2D1A3B,
    );
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
        let mut library = Library::new(entries, "/games".to_string());
        library.select_page(LibraryPage::Games);
        library
    }

    #[test]
    fn responsive_text_coordinates_follow_the_surface_scale() {
        let (width, height) = (800, 1200);
        let mut frame = vec![0u8; width as usize * height as usize * 4];
        responsive_text(&mut frame, width, height, 10.0, 10.0, 2, "A", 0xE5173F);

        // Portrait UI scale is 2: the first lit pixel in the 5x7 A glyph
        // must appear at logical (11, 10) transformed to physical (22, 20).
        let scaled = (20usize * width as usize + 22) * 4;
        assert_eq!(&frame[scaled..scaled + 3], &[0xE5, 0x17, 0x3F]);

        let unscaled = (10usize * width as usize + 11) * 4;
        assert_eq!(&frame[unscaled..unscaled + 3], &[0, 0, 0]);
    }

    #[test]
    fn responsive_centered_text_uses_measured_bitmap_width() {
        let (width, height) = (800, 1200);
        let mut frame = vec![0u8; width as usize * height as usize * 4];
        responsive_text_centered(&mut frame, width, height, 200.0, 40.0, 2, "OK", 0xF5EEF8);
        assert!(
            frame
                .chunks_exact(4)
                .any(|pixel| pixel[..3] == [0xF5, 0xEE, 0xF8])
        );
    }

    #[test]
    fn responsive_background_is_cover_cropped_and_invalid_frames_are_ignored() {
        let library = Library::new(Vec::new(), "/games".to_string());
        let (width, height) = (800, 1200);
        let plain = library.render_responsive(width, height);
        let background = vec![240u8; WIDTH * HEIGHT * 4];
        let with_background =
            library.render_responsive_with_background(width, height, Some(&background));
        assert_ne!(plain, with_background);
        assert_eq!(with_background.len(), plain.len());

        let invalid = library.render_responsive_with_background(width, height, Some(&[255; 16]));
        assert_eq!(invalid, plain);
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
    fn responsive_navigation_filters_games_and_homebrew_and_refreshes_settings() {
        let dir = std::env::temp_dir().join(format!("aceris-responsive-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("sample.3dsx"), b"fixture").unwrap();
        fs::write(dir.join("retail.3ds"), b"fixture").unwrap();
        let mut library = Library::new(Vec::new(), dir.display().to_string());
        library.select_page(LibraryPage::Games);

        // Refresh from the library header while the Games section is selected.
        assert_eq!(
            library.click_responsive(680, 384, 800, 1200),
            ClickResult::Ignored
        );
        assert_eq!(library.entries().len(), 2);
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(
            library.selected_entry().unwrap().path.extension().unwrap(),
            "3ds"
        );

        // Switch to Homebrew and verify the .3dsx-only filter.
        assert_eq!(
            library.click_responsive(400, 250, 800, 1200),
            ClickResult::Ignored
        );
        assert_eq!(library.page(), LibraryPage::Homebrew);
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(
            library.selected_entry().unwrap().path.extension().unwrap(),
            "3dsx"
        );

        // Settings exposes the same real refresh action.
        assert_eq!(
            library.click_responsive(700, 250, 800, 1200),
            ClickResult::Ignored
        );
        assert_eq!(library.page(), LibraryPage::Settings);
        assert_eq!(
            library.click_responsive(400, 1_040, 800, 1200),
            ClickResult::Ignored
        );
        assert!(library.status.is_some());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn favorites_and_recent_are_restored_after_reopening_the_library() {
        let dir =
            std::env::temp_dir().join(format!("aceris-library-persistence-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let alpha = dir.join("alpha.3ds");
        let beta = dir.join("beta.3ds");
        fs::write(&alpha, b"fixture").unwrap();
        fs::write(&beta, b"fixture").unwrap();

        {
            let mut library = Library::new(scan_games(&dir), dir.display().to_string());
            library.select_page(LibraryPage::Games);
            assert!(library.toggle_selected_favorite());
            library.mark_launched(&beta);
        }

        let reopened = Library::new(scan_games(&dir), dir.display().to_string());
        assert_eq!(reopened.favorite_count(), 1);
        assert_eq!(reopened.recent_count(), 1);
        assert_eq!(
            fs::read_to_string(dir.join(".aceris").join("favorites.txt")).unwrap(),
            "alpha.3ds"
        );
        assert_eq!(
            fs::read_to_string(dir.join(".aceris").join("recent.txt")).unwrap(),
            "beta.3ds"
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn favorite_count_ignores_files_missing_from_the_game_folder() {
        let dir =
            std::env::temp_dir().join(format!("aceris-favorite-count-{}", std::process::id()));
        fs::create_dir_all(dir.join(".aceris")).unwrap();
        fs::write(dir.join("alpha.3ds"), b"fixture").unwrap();
        fs::write(
            dir.join(".aceris").join("favorites.txt"),
            "alpha.3ds\nremoved.3ds",
        )
        .unwrap();

        let library = Library::new(scan_games(&dir), dir.display().to_string());
        assert_eq!(library.favorite_count(), 1);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn landscape_settings_refresh_button_matches_its_visible_bounds() {
        let dir =
            std::env::temp_dir().join(format!("aceris-landscape-settings-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("sample.3ds"), b"fixture").unwrap();
        let mut library = Library::new(Vec::new(), dir.display().to_string());
        library.select_page(LibraryPage::Settings);

        assert_eq!(
            library.click_responsive(278, 315, 1000, 600),
            ClickResult::Ignored
        );
        assert_eq!(library.entries().len(), 1);
        assert!(library.status_message().is_some());

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn home_continue_uses_the_most_recent_game() {
        let dir = std::env::temp_dir().join(format!("aceris-home-recent-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("latest.3ds");
        fs::write(&path, b"fixture").unwrap();
        let entry = Entry {
            title: "Latest game".into(),
            path: path.clone(),
        };
        let mut library = Library::new(vec![entry], dir.display().to_string());
        assert_eq!(library.page(), LibraryPage::Home);
        assert!(library.selected_entry().is_none());
        library.mark_launched(&path);
        assert_eq!(library.selected_entry().unwrap().title, "Latest game");
        assert_eq!(
            library.click_responsive(200, 200, 1000, 600),
            ClickResult::Activated
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn horizontal_grid_navigation_moves_by_rows_and_columns() {
        let mut library = library(18);
        library.move_selection_responsive(1, 1000, 480);
        assert_eq!(library.selected_index(), 3);
        library.move_selection_horizontal_responsive(1, 1000, 480);
        assert_eq!(library.selected_index(), 4);
        library.move_selection_responsive(-1, 1000, 480);
        assert_eq!(library.selected_index(), 1);
    }

    #[test]
    fn library_sort_order_and_external_media_indicator_are_stateful() {
        let entries = vec![
            Entry {
                title: "Alpha".into(),
                path: PathBuf::from("alpha.3ds"),
            },
            Entry {
                title: "Beta".into(),
                path: PathBuf::from("beta.3ds"),
            },
        ];
        let mut library = Library::new(entries, "/games".into());
        library.select_page(LibraryPage::Games);
        assert_eq!(library.selected_entry().unwrap().title, "Alpha");
        library.toggle_sort_order();
        assert_eq!(library.selected_entry().unwrap().title, "Beta");
        library.toggle_sort_order();
        assert_eq!(library.selected_entry().unwrap().title, "Alpha");

        assert!(!library.external_media_detection_available());
        assert!(!library.external_media_playing());
        library.set_external_media_detection_available(true);
        assert!(library.external_media_detection_available());
        library.set_external_media_playing(true);
        assert!(library.external_media_playing());
        library.set_external_media_detection_available(false);
        assert!(!library.external_media_detection_available());
        assert!(!library.external_media_playing());
    }

    #[test]
    fn homebrew_filter_does_not_mix_executable_cartridges() {
        let entries = vec![
            Entry {
                title: "Retail".into(),
                path: PathBuf::from("retail.3ds"),
            },
            Entry {
                title: "Demo".into(),
                path: PathBuf::from("demo.3dsx"),
            },
        ];
        let mut library = Library::new(entries, "/games".into());
        library.select_page(LibraryPage::Games);
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(library.selected_entry().unwrap().title, "Retail");
        library.select_page(LibraryPage::Homebrew);
        assert_eq!(library.visible_entry_count(), 1);
        assert_eq!(library.selected_entry().unwrap().title, "Demo");
        library.select_page(LibraryPage::Settings);
        assert_eq!(library.visible_entry_count(), 0);
        assert!(library.selected_entry().is_none());
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
