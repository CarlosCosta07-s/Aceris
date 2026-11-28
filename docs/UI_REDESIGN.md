# Aceris UI — redesign specification

Status: approved direction; implementation is incremental and must be validated on desktop and Android.

## Product direction

Rebuild the Aceris application UI from the ground up as a polished console-style library and launcher. The Nintendo Switch is the primary product-design reference for clarity, hierarchy, focus states, game discovery and controller-first navigation. The Aceris must retain its own visual identity and must not reproduce Nintendo artwork, icons, logos, exact screen compositions or proprietary assets.

The existing Aceris color palette is the visual identity and is preserved. The redesign changes layout, typography, spacing, components, navigation, interaction states and responsive behavior—not the brand colors.

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

Use a modern, readable sans-serif typography system with clear roles:
- Display: featured game title and major page headings.
- Heading: section titles and settings group titles.
- Body: descriptions, navigation and actions.
- Label: compact metadata, file format and status.
- Numeric: aligned counters, progress and performance values.

Typography must support Portuguese diacritics and Unicode. Avoid the current small, bitmap-like text treatment for primary navigation and page content. Keep text sizes responsive to the available surface.

Use a consistent spacing scale, predictable alignment, restrained borders, subtle elevation and rounded surfaces. Motion should be short, purposeful and optional; never make navigation depend on animation.

## Application shell

### Desktop

- Persistent left navigation rail with Aceris identity and clear destinations: Início, Jogos, Favoritos, Recentes, Homebrew and Configurações.
- Top content bar for page title, search, import action and contextual controls.
- Main content area with a large featured/continue-playing region, followed by a horizontal shelf of five square game covers. Selection uses a clear red focus frame and controller navigation moves across the visible covers before scrolling the shelf.
- A responsive game-card grid with cover art when available, readable title, subtle format metadata and a clear focus/selected state.
- A details panel for the selected title with centered square artwork, title, container format, recent/favorite state, and clear Play, Favorite, Import and Refresh actions.
- Controller and keyboard navigation must mirror pointer navigation; focus must always be visible.

### Android

- Same information architecture and visual language, adapted to touch. Home remains a landscape dashboard with five large, touch-friendly game covers across its main shelf.
- The Android app is landscape-only; `AndroidManifest.xml` must keep `android:screenOrientation="landscape"`.
- Compact top bar and a bottom navigation bar for primary destinations.
- Large touch targets, safe-area awareness, scrollable content and responsive game cards, designed for landscape phones and tablets.
- Home and the primary library experience are designed for landscape only. Android is locked to landscape; the legacy portrait renderer is compatibility-only and must never become an Android product mode.
- Import and folder management must remain discoverable and usable on touch devices.

## Core screens

1. **Home** — featured/continue section, recently played, favorites or curated library highlights, and a prominent Import games action when the library is empty.
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
