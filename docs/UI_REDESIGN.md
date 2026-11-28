# Aceris UI — redesign specification

Status: approved direction; implementation is incremental and must be validated on desktop and Android.

## Product direction

Rebuild the Aceris application UI from the ground up as a polished console-style library and launcher. The user-provided, unofficial console concept mockup is the direct composition target for the Home screen. Carlos explicitly authorized a close one-to-one recreation of its arrangement and proportions, changing the navigation glyphs to original Aceris symbols. The image is a conceptual reference, not an official console interface.

Keep the existing Aceris color palette. Recreate the composition, hierarchy, spacing, game-cover shelf, side rail and bottom command hints in original Rust drawing code. Do not reuse Nintendo logos, console marks, game artwork, third-party icons or other assets from the reference image.

## Non-negotiable engineering constraints

- All product code remains Rust.
- Desktop and Android share the same UI model, design tokens and interaction contracts wherever practical.
- Keep the emulation core independent from presentation code.
- Do not regress library scanning, import-folder configuration, favorites, recents, search, format filters, sorting, grid/list mode, settings, diagnostics, quick menu, screen-layout controls, virtual controls or game launch.
- Keep persisted settings backward-compatible. New UI preferences must use versioned formats and safe defaults.
- Do not copy code, assets, fonts, tests or structural implementations from other emulators. References are visual/behavioral inspiration only.
- Do not claim a feature or platform is validated until it has been built and tested there.

## Visual system

Use the existing semantic tokens in `aceris-common::brand` as the only source of brand colors. Do not introduce a replacement palette or hard-code competing accent colors.

Use the original Aceris UI typeface from `crates/aceris-core/src/video/font.rs` with a clear scale hierarchy:
- Display: featured game title and page headings.
- Heading: section titles and settings groups.
- Body: descriptions, navigation and actions.
- Label: compact metadata, file format and status.
- Numeric: aligned counters and performance values.

The custom typeface must remain legible at common landscape resolutions. Give primary titles room to breathe, use Semibold sparingly, maintain Portuguese accents and keep secondary labels visibly subordinate. Do not substitute a system font or introduce third-party font assets.

Use a consistent spacing scale, predictable alignment, restrained borders, subtle elevation and rounded surfaces. Motion should be short, purposeful and optional; never make navigation depend on animation.

The visual quality bar is a finished first-party console interface: precise alignment, consistent icon geometry, intentional empty space, clear active/focus states, carefully proportioned cover art, and quiet controller hints. Reuse Aceris colors and original symbols. The concept image defines the Home composition, while every artwork, icon and product mark remains Aceris-owned.

## Application shell

### Desktop

- A fixed, narrow left rail matching the supplied concept's proportions. Place the Aceris mark at the top, followed by five vertically stacked original glyphs for Início, Jogos, Importar, Favoritos and Ajustes, with the power symbol anchored near the bottom. Do not add text labels beside the rail icons. Recentes and Homebrew are compact collection tabs, not extra rail entries.
- Home reproduces the supplied unofficial console concept composition: selected-game identity centered above a single horizontal shelf of large portrait covers, with five positions and the fifth naturally clipped at the right edge when needed.
- Games, Favorites, Recent and Homebrew retain the same left rail and use a compact section header, collection tabs, search/filter/sort controls, a three-column cover grid, a selected-title information panel and a compact bottom command strip.
- Selection uses a clear Aceris red focus treatment. Game details show available cover/icon, title, container format, recent/favorite state and the relevant launch/favorite/import actions.
- Missing cover metadata uses original Aceris geometric artwork in the existing brand palette; user-supplied SMDH icons remain the source for real game icons.
- Do not restore the old horizontal Home tab bar, oversized administration panels or large collection-summary panel. Keep navigation and command hints consistent with the Home concept.
- Controller, keyboard, mouse and touch navigation must map to the same actions; focus must always be visible.

### Android

- Use the same fixed five-option Aceris rail and power action across Home, Games, Favorites, Recent, Homebrew and Settings. Home retains the centered featured identity and portrait-cover shelf; collection pages use the same cover grid and details panel as desktop.
- The Android app is landscape-only; `AndroidManifest.xml` must keep `android:screenOrientation="landscape"`.
- Do not replace the side rail with a top or bottom navigation bar.
- Large touch targets, safe-area awareness, horizontal shelf scrolling and responsive portrait cover cards, designed for landscape phones and tablets.
- Import and folder management must remain discoverable and usable on touch devices.

## Core screens

1. **Home** — the approved console concept composition: selected-game identity centered at the top, one horizontal shelf of portrait game cards, five positions with the fifth clipped at the right edge, and a compact command strip. When the library is empty, show a clear import state without reverting to administration panels.
2. **Games** — complete collection, search, format filters, sorting and grid/list presentation.
3. **Favorites** — saved titles using the same card and list components.
4. **Recent** — recently launched titles and clear empty state.
5. **Homebrew** — homebrew collection with the same consistent browsing behavior.
6. **Game details** — title identity, cover/icon when supplied by valid metadata, file/container information, favorite action and launch action.
7. **Settings** — grouped pages for Library, Performance, Controls, Display and Interface; each setting needs a concise explanation and an obvious current value.
8. **In-game quick menu** — focused overlay with Continue, Restart, Library, Screen layout, Swap screens, Screenshot and available diagnostics. Destructive actions require confirmation.

## Component library

Create reusable, independently testable components rather than drawing every page as a one-off:
- AppShell, NavigationRail, BottomNavigation, TopBar
- SearchField, FilterChip, SegmentedControl
- GameCard, GameRow, FeaturedGame, GameDetails
- SectionHeader, EmptyState, LoadingState, ErrorState
- SettingsGroup, SettingRow, Toggle, ChoiceRow, Slider
- Modal, ConfirmationDialog, Toast, QuickMenu

Components must expose state and actions through explicit Rust data/contracts. Rendering must not own filesystem scanning, emulator execution or persistence logic.

## Interaction and accessibility

- Every interactive element has default, hover (where applicable), focused, pressed, selected, disabled and error states.
- Keyboard, gamepad, mouse and touch input use one navigation/action model.
- Focus order follows visual reading order and focus remains visible.
- Never communicate status by color alone; pair status color with text or an icon.
- Empty, loading, missing-art and error states are designed—not blank screens.
- Respect reduced-motion preferences where the host can expose them.

## Migration strategy

This is a UI replacement, not a visual patch. Implement in small, buildable stages:

1. Inventory current rendering, input, state, persistence and desktop/Android host boundaries. Record existing behavior in regression tests.
2. Establish shared layout primitives, typography, spacing and component-state contracts while preserving the current palette.
3. Replace the Home/library shell and navigation.
4. Rebuild Games, Favorites, Recent and Homebrew using the same reusable cards, search and filters.
5. Rebuild Settings and folder import/management without changing their underlying persistence contracts.
6. Rebuild game details, launch/loading/error feedback and the in-game quick menu.
7. Adapt and validate desktop and Android independently; verify keyboard/gamepad/touch navigation.
8. Remove obsolete UI drawing paths only after equivalent behavior and regression coverage exist.

Each stage must build and test before the next. Do not delete the existing renderer or persistence paths in a single sweeping change.

## Acceptance criteria

- The existing Aceris palette is unchanged.
- Desktop and Android visibly belong to the same product.
- The interface is responsive and does not rely on a fixed 400×480 library canvas for page layout.
- The library remains usable with keyboard, gamepad, mouse and touch as appropriate.
- Folder import, scan settings, search, filters, sorting, favorites, recents and launch behavior continue to work.
- Existing user settings survive the UI migration.
- No copyrighted console assets or copied emulator implementation are introduced.
- Build/test results and platform validation are reported separately and accurately.
