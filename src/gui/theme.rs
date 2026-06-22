//! Centralized GUI design tokens (palette, spacing, type scale) and light/dark
//! theming.
//!
//! Historically the GUI hard-coded ~30 near-identical greys via `Colour::rgb(...)`
//! scattered across `gui.rs` and `gui/*.rs`. This module replaces that with one
//! coherent token set per appearance, so every page shares the same palette and a
//! dark variant becomes possible.
//!
//! Usage: call [`init`] once at startup (before building the frame) with the
//! resolved [`ThemeMode`], then read tokens anywhere via [`theme()`].
//!
//! Some tokens (accent, border, spacing scale, type scale) are defined ahead of
//! the call sites that will consume them in the custom-control work, so the
//! module allows currently-unused items.
#![allow(dead_code)]

use std::cell::RefCell;

use wxdragon::appearance::{Appearance, is_system_dark_mode};
use wxdragon::font::{Font, FontWeight};
use wxdragon::prelude::Colour;

/// User-facing theme preference. Persisted in config as a code string, mirroring
/// the `GuiLocale` pattern used for language.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum ThemeMode {
    /// Follow the OS appearance (recommended).
    #[default]
    System,
    /// Force light.
    Light,
    /// Force dark.
    Dark,
}

impl ThemeMode {
    pub(super) fn from_code(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "system" | "auto" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub(super) fn code(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// The wxWidgets appearance to apply. Must be set before any window is built.
    pub(super) fn appearance(self) -> Appearance {
        match self {
            Self::System => Appearance::System,
            Self::Light => Appearance::Light,
            Self::Dark => Appearance::Dark,
        }
    }

    /// Resolve to a concrete dark/light decision, consulting the OS for `System`.
    pub(super) fn is_dark(self) -> bool {
        match self {
            Self::Light => false,
            Self::Dark => true,
            Self::System => is_system_dark_mode(),
        }
    }
}

// ---------------------------------------------------------------------------
// Spacing & radius scale (appearance-independent).
// ---------------------------------------------------------------------------

/// 4px base spacing grid. Use these instead of ad-hoc sizer margins.
pub(super) const SPACE_XS: i32 = 4;
pub(super) const SPACE_SM: i32 = 8;
pub(super) const SPACE_MD: i32 = 12;
pub(super) const SPACE_LG: i32 = 16;
pub(super) const SPACE_XL: i32 = 24;

/// Corner radius for cards and buttons.
pub(super) const RADIUS: f64 = 8.0;

// ---------------------------------------------------------------------------
// Type scale.
// ---------------------------------------------------------------------------

/// Logical font roles, mapped to absolute point sizes. Point sizes are
/// DPI-independent in wxWidgets, so fixed values still scale with the display.
#[derive(Clone, Copy)]
pub(super) enum TextRole {
    /// Section / card titles.
    Title,
    /// Primary body text.
    Body,
    /// Secondary / caption text.
    Caption,
}

impl TextRole {
    fn point_size(self) -> i32 {
        match self {
            TextRole::Title => 12,
            TextRole::Body => 10,
            TextRole::Caption => 9,
        }
    }

    fn bold(self) -> bool {
        matches!(self, TextRole::Title)
    }
}

/// Build a valid `Font` for the given role.
///
/// We must never hand a widget an *invalid* font: `Font::new()` is
/// default-constructed (not ok), and reading its point size during layout trips
/// a wxWidgets assert (`GetFractionalPointSize: invalid font`). The builder goes
/// through `wxFont`'s real constructor, so the result is always valid.
pub(super) fn font(role: TextRole) -> Font {
    let mut builder = Font::builder().with_point_size(role.point_size());
    if role.bold() {
        builder = builder.with_weight(FontWeight::Bold);
    }
    builder.build().unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Palette.
// ---------------------------------------------------------------------------

/// A full set of color tokens for one appearance. `Copy` so call sites can read a
/// snapshot cheaply via [`theme()`].
#[derive(Clone, Copy)]
pub(super) struct Theme {
    pub(super) is_dark: bool,

    // Surfaces.
    /// Window / tab page background.
    pub(super) bg_app: Colour,
    /// Card / panel surface.
    pub(super) bg_card: Colour,
    /// Slightly tinted card surface (option rows, sub-panels).
    pub(super) bg_card_alt: Colour,
    /// Muted / disabled surface.
    pub(super) bg_muted: Colour,

    // Lines.
    /// Card border / outline.
    pub(super) border: Colour,
    /// Hairline divider, connectors.
    pub(super) divider: Colour,

    // Text.
    /// Primary text.
    pub(super) ink_primary: Colour,
    /// Secondary text (labels, titles on cards).
    pub(super) ink_secondary: Colour,
    /// Muted text (captions, disabled).
    pub(super) ink_muted: Colour,

    // Accent.
    /// Brand accent (primary buttons, links).
    pub(super) accent: Colour,
    /// Accent hover/pressed.
    pub(super) accent_hover: Colour,
    /// Text/icon drawn on top of `accent`.
    pub(super) on_accent: Colour,

    // Semantic states.
    pub(super) ok: Colour,
    pub(super) warn: Colour,
    pub(super) error: Colour,
    pub(super) info: Colour,

    // Soft semantic fills (for banners / pills).
    pub(super) warn_soft: Colour,
    pub(super) error_soft: Colour,

    // Code / JSON viewer palette (Scintilla request-log editor).
    pub(super) code_bg: Colour,
    pub(super) code_fg: Colour,
    pub(super) code_gutter_bg: Colour,
    pub(super) code_line_number: Colour,
    pub(super) code_caret_line: Colour,
    pub(super) code_indent_guide: Colour,
    pub(super) code_find_match: Colour,
    pub(super) code_key: Colour,
    pub(super) code_string: Colour,
    pub(super) code_number: Colour,
    pub(super) code_keyword: Colour,
    pub(super) code_punct: Colour,
}

impl Theme {
    pub(super) const fn light() -> Self {
        Self {
            is_dark: false,

            bg_app: Colour::rgb(237, 240, 245),
            bg_card: Colour::rgb(255, 255, 255),
            bg_card_alt: Colour::rgb(248, 250, 252),
            bg_muted: Colour::rgb(238, 241, 245),

            border: Colour::rgb(208, 214, 222),
            divider: Colour::rgb(150, 159, 172),

            ink_primary: Colour::rgb(23, 28, 36),
            ink_secondary: Colour::rgb(82, 91, 105),
            ink_muted: Colour::rgb(124, 133, 146),

            accent: Colour::rgb(36, 99, 235),
            accent_hover: Colour::rgb(29, 84, 204),
            on_accent: Colour::rgb(255, 255, 255),

            ok: Colour::rgb(28, 127, 89),
            warn: Colour::rgb(169, 104, 24),
            error: Colour::rgb(192, 58, 58),
            info: Colour::rgb(36, 99, 235),

            warn_soft: Colour::rgb(252, 245, 230),
            error_soft: Colour::rgb(252, 238, 238),

            code_bg: Colour::rgb(255, 255, 255),
            code_fg: Colour::rgb(38, 45, 56),
            code_gutter_bg: Colour::rgb(243, 246, 250),
            code_line_number: Colour::rgb(94, 103, 117),
            code_caret_line: Colour::rgb(244, 247, 251),
            code_indent_guide: Colour::rgb(174, 184, 199),
            code_find_match: Colour::rgb(245, 188, 45),
            code_key: Colour::rgb(26, 84, 160),
            code_string: Colour::rgb(20, 124, 74),
            code_number: Colour::rgb(151, 71, 0),
            code_keyword: Colour::rgb(128, 61, 150),
            code_punct: Colour::rgb(92, 99, 112),
        }
    }

    pub(super) const fn dark() -> Self {
        Self {
            is_dark: true,

            bg_app: Colour::rgb(24, 26, 31),
            bg_card: Colour::rgb(33, 36, 43),
            bg_card_alt: Colour::rgb(39, 43, 51),
            bg_muted: Colour::rgb(30, 33, 39),

            border: Colour::rgb(57, 62, 72),
            divider: Colour::rgb(96, 104, 118),

            ink_primary: Colour::rgb(232, 235, 240),
            ink_secondary: Colour::rgb(178, 186, 198),
            ink_muted: Colour::rgb(132, 140, 152),

            accent: Colour::rgb(94, 158, 255),
            accent_hover: Colour::rgb(122, 176, 255),
            on_accent: Colour::rgb(16, 20, 28),

            ok: Colour::rgb(72, 196, 148),
            warn: Colour::rgb(225, 176, 92),
            error: Colour::rgb(238, 112, 112),
            info: Colour::rgb(110, 168, 250),

            warn_soft: Colour::rgb(54, 47, 30),
            error_soft: Colour::rgb(58, 38, 38),

            code_bg: Colour::rgb(28, 31, 37),
            code_fg: Colour::rgb(210, 216, 224),
            code_gutter_bg: Colour::rgb(36, 40, 48),
            code_line_number: Colour::rgb(120, 128, 140),
            code_caret_line: Colour::rgb(38, 43, 52),
            code_indent_guide: Colour::rgb(64, 70, 82),
            code_find_match: Colour::rgb(220, 180, 70),
            code_key: Colour::rgb(110, 168, 250),
            code_string: Colour::rgb(126, 200, 140),
            code_number: Colour::rgb(224, 170, 110),
            code_keyword: Colour::rgb(198, 148, 232),
            code_punct: Colour::rgb(150, 158, 170),
        }
    }

    fn for_mode(mode: ThemeMode) -> Self {
        if mode.is_dark() {
            Self::dark()
        } else {
            Self::light()
        }
    }

    /// `[u8; 4]` RGBA for use with the `IconCanvas` self-drawn bitmaps.
    pub(super) fn rgba(colour: Colour, alpha: u8) -> [u8; 4] {
        [colour.r, colour.g, colour.b, alpha]
    }
}

// ---------------------------------------------------------------------------
// Global active theme.
//
// The GUI runs single-threaded on the wx main loop and the theme is fixed for the
// lifetime of the process (changing it persists to config and prompts a restart,
// matching the language flow). A thread-local snapshot is therefore sufficient and
// avoids any locking on hot paint paths.
// ---------------------------------------------------------------------------

thread_local! {
    static ACTIVE: RefCell<Theme> = const { RefCell::new(Theme::light()) };
}

/// Install the active theme for the given mode. Call once at startup, after
/// `set_appearance` and before building widgets.
pub(super) fn init(mode: ThemeMode) {
    let resolved = Theme::for_mode(mode);
    ACTIVE.with(|cell| *cell.borrow_mut() = resolved);
}

/// Snapshot of the currently active theme.
pub(super) fn theme() -> Theme {
    ACTIVE.with(|cell| *cell.borrow())
}
