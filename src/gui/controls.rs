//! Custom-drawn, theme-aware push button.
//!
//! Native wx buttons can't take a brand-accent fill or rounded corners, so the
//! action buttons are drawn on a `Panel` via `AutoBufferedPaintDC`. The widget
//! mirrors the slice of the native `Button` API the GUI actually uses
//! (`on_click`, `enable`, `set_label`), and exposes the underlying `Panel` via
//! `Deref` so `set_tooltip` and sizer insertion keep working unchanged.
//!
//! Like the native `Button`, [`ThemeButton`] is a cheap `Copy` handle: the
//! interactive state lives in a thread-local registry keyed by the panel handle,
//! so existing call sites that pass the button around by value keep compiling.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use wxdragon::dc::{AutoBufferedPaintDC, BrushStyle, PenStyle};
use wxdragon::prelude::*;

use super::theme::{self, RADIUS};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum ButtonVariant {
    /// Filled brand-accent button for the primary action.
    Primary,
    /// Outlined surface button for secondary actions.
    Secondary,
    /// Borderless text button for low-emphasis actions.
    Ghost,
    /// Filled destructive button (delete / remove).
    Danger,
}

struct ButtonInner {
    panel: Panel,
    label: RefCell<String>,
    variant: ButtonVariant,
    hover: Cell<bool>,
    pressed: Cell<bool>,
    enabled: Cell<bool>,
    click: RefCell<Option<Box<dyn FnMut(())>>>,
}

thread_local! {
    static REGISTRY: RefCell<HashMap<usize, Rc<ButtonInner>>> = RefCell::new(HashMap::new());
}

/// A rounded, theme-aware button. `Copy` handle, like the native `Button`.
#[derive(Clone, Copy)]
pub(super) struct ThemeButton {
    panel: Panel,
}

/// Create a custom button. Drop-in for
/// `Button::builder(parent).with_label(label).build()`.
pub(super) fn theme_button<W: WxWidget>(
    parent: &W,
    label: &str,
    variant: ButtonVariant,
) -> ThemeButton {
    let panel = Panel::builder(parent).build();
    panel.set_background_style(BackgroundStyle::Paint);

    let inner = Rc::new(ButtonInner {
        panel,
        label: RefCell::new(label.to_string()),
        variant,
        hover: Cell::new(false),
        pressed: Cell::new(false),
        enabled: Cell::new(true),
        click: RefCell::new(None),
    });

    let button = ThemeButton { panel };
    button.apply_min_size(&inner);
    install_handlers(&inner);
    REGISTRY.with(|registry| {
        registry
            .borrow_mut()
            .insert(panel.handle_ptr() as usize, inner)
    });
    button
}

impl ThemeButton {
    /// Register the click callback. Matches the native `on_click(move |_| ..)`
    /// call shape, so existing handlers can be reused verbatim.
    pub(super) fn on_click<F: FnMut(()) + 'static>(&self, callback: F) {
        self.with_inner(|inner| *inner.click.borrow_mut() = Some(Box::new(callback)));
    }

    /// Enable / disable the button (visually dims and stops firing clicks).
    pub(super) fn enable(&self, enable: bool) {
        self.with_inner(|inner| {
            inner.enabled.set(enable);
            if !enable {
                inner.hover.set(false);
                inner.pressed.set(false);
            }
        });
        self.panel.refresh(false, None);
    }

    /// Replace the label, re-measure the minimum size, and repaint.
    pub(super) fn set_label(&self, label: &str) {
        self.with_inner(|inner| {
            *inner.label.borrow_mut() = label.to_string();
            self.apply_min_size(inner);
        });
        self.panel.refresh(false, None);
    }

    fn apply_min_size(&self, inner: &Rc<ButtonInner>) {
        let width = measure_width(&inner.label.borrow());
        self.panel.set_min_size(Size::new(width, 32));
    }

    fn with_inner<R>(&self, f: impl FnOnce(&Rc<ButtonInner>) -> R) -> Option<R> {
        let key = self.panel.handle_ptr() as usize;
        REGISTRY.with(|registry| registry.borrow().get(&key).map(f))
    }
}

impl std::ops::Deref for ThemeButton {
    type Target = Panel;
    fn deref(&self) -> &Panel {
        &self.panel
    }
}

impl WxWidget for ThemeButton {
    fn handle_ptr(&self) -> *mut wxdragon::ffi::wxd_Window_t {
        self.panel.handle_ptr()
    }
}

fn install_handlers(inner: &Rc<ButtonInner>) {
    let panel = inner.panel;

    let paint = inner.clone();
    panel.on_paint(move |event| {
        draw_button(&paint);
        event.skip(true);
    });

    let enter = inner.clone();
    panel.on_mouse_enter(move |event| {
        if enter.enabled.get() {
            enter.hover.set(true);
            enter.panel.refresh(false, None);
        }
        event.skip(true);
    });

    let leave = inner.clone();
    panel.on_mouse_leave(move |event| {
        leave.hover.set(false);
        leave.pressed.set(false);
        leave.panel.refresh(false, None);
        event.skip(true);
    });

    let down = inner.clone();
    panel.on_mouse_left_down(move |event| {
        if down.enabled.get() {
            down.pressed.set(true);
            down.panel.refresh(false, None);
        }
        event.skip(true);
    });

    let up = inner.clone();
    panel.on_mouse_left_up(move |event| {
        let fire = up.pressed.get() && up.hover.get() && up.enabled.get();
        up.pressed.set(false);
        up.panel.refresh(false, None);
        if fire {
            if let Some(callback) = up.click.borrow_mut().as_mut() {
                callback(());
            }
        }
        event.skip(true);
    });
}

/// Approximate label width (CJK glyphs are ~2x ASCII) plus horizontal padding.
fn measure_width(label: &str) -> i32 {
    let text: i32 = label
        .chars()
        .map(|c| if c.is_ascii() { 8 } else { 16 })
        .sum();
    (text + 34).max(72)
}

fn draw_button(inner: &Rc<ButtonInner>) {
    let t = theme::theme();
    let panel = &inner.panel;
    let size = panel.get_size();
    let (w, h) = (size.width, size.height);
    if w <= 0 || h <= 0 {
        return;
    }

    let enabled = inner.enabled.get();
    let hover = inner.hover.get();
    let pressed = inner.pressed.get();

    // Resolve fill / text / border for the variant and interaction state.
    let transparent = Colour::new(0, 0, 0, 0);
    let white = Colour::rgb(255, 255, 255);
    let (fill, text_colour, border): (Option<Colour>, Colour, Option<Colour>) = match inner.variant
    {
        ButtonVariant::Primary => {
            let base = if pressed {
                t.accent.darker(0.12)
            } else if hover {
                t.accent_hover
            } else {
                t.accent
            };
            (Some(base), t.on_accent, None)
        }
        ButtonVariant::Danger => {
            let base = if pressed {
                t.error.darker(0.12)
            } else if hover {
                t.error.lighter(0.06)
            } else {
                t.error
            };
            (Some(base), white, None)
        }
        ButtonVariant::Secondary => {
            let base = if pressed {
                t.bg_muted.darker(0.04)
            } else if hover {
                t.bg_muted
            } else {
                t.bg_card
            };
            (Some(base), t.ink_primary, Some(t.border))
        }
        ButtonVariant::Ghost => {
            let base = if pressed || hover {
                Some(t.bg_muted)
            } else {
                None
            };
            (base, t.accent, None)
        }
    };

    let dc = AutoBufferedPaintDC::new(panel);

    // Clear the whole rect with the parent background so the rounded corners
    // blend into the surface the button sits on.
    let parent_bg = panel
        .get_parent()
        .map(|parent| parent.get_background_color())
        .unwrap_or(t.bg_card);
    dc.set_pen(transparent, 0, PenStyle::Transparent);
    dc.set_brush(parent_bg, BrushStyle::Solid);
    dc.draw_rectangle(0, 0, w, h);

    // Body.
    if let Some(fill) = fill {
        let fill = if enabled { fill } else { t.bg_muted };
        dc.set_pen(transparent, 0, PenStyle::Transparent);
        dc.set_brush(fill, BrushStyle::Solid);
        dc.draw_rounded_rectangle(0, 0, w, h, RADIUS);
    }

    // Border.
    if let Some(border) = border {
        dc.set_brush(transparent, BrushStyle::Transparent);
        dc.set_pen(border, 1, PenStyle::Solid);
        dc.draw_rounded_rectangle(0, 0, w - 1, h - 1, RADIUS);
    }

    // Label.
    let text_colour = if enabled { text_colour } else { t.ink_muted };
    dc.set_text_foreground(text_colour);
    let label = inner.label.borrow();
    let (tw, th) = dc.get_text_extent(&label);
    let tx = ((w - tw) / 2).max(0);
    let ty = ((h - th) / 2).max(0);
    dc.draw_text(&label, tx, ty);
}
