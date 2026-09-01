//! The look: one fixed dark theme, cut to resemble GNOME's Adwaita.
//!
//! Nothing here follows the desktop. The theme is dark and stays dark — there
//! is no light variant to switch to and no portal or settings daemon asked what
//! the session prefers, which is why no crate is carried for it. What it does
//! borrow from GNOME is the palette and the corner radii, so that the window
//! sits next to a GTK4 one without looking like it came from a different
//! decade. Its UI typeface is borrowed too, but by being carried in the binary
//! rather than hunted for on the machine — see `main.rs`.
//!
//! Only colour, spacing and type live here; the panes themselves are in
//! `app.rs` and are not this module's business. The palette is the whole
//! window's, not just the chrome's: `markdown.rs` draws the preview straight
//! out of these constants rather than keeping a second set that would drift
//! from them.

use std::sync::Arc;

use eframe::egui::{self, Color32, CornerRadius, Stroke};

// The libadwaita dark palette. `_BG` names are surfaces, in the order they
// stack: the window, then the sunken views drawn on it, then the raised
// controls drawn on those.
const WINDOW_BG: Color32 = Color32::from_rgb(0x24, 0x24, 0x24);
pub const VIEW_BG: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x1e);
pub const CONTROL_BG: Color32 = Color32::from_rgb(0x30, 0x30, 0x30);
const CONTROL_HOVER_BG: Color32 = Color32::from_rgb(0x38, 0x38, 0x38);
const CONTROL_ACTIVE_BG: Color32 = Color32::from_rgb(0x3d, 0x3d, 0x3d);
/// The Adwaita accent blue, behind selected rows and selected text.
const ACCENT: Color32 = Color32::from_rgb(0x35, 0x84, 0xe4);
pub const LINK: Color32 = Color32::from_rgb(0x78, 0xae, 0xed);
/// Code, in the preview. GNOME's palette red 1, which is the shade the HIG
/// picks for coloured text on a dark background.
pub const CODE_FG: Color32 = Color32::from_rgb(0xf6, 0x61, 0x51);
pub const TEXT: Color32 = Color32::WHITE;
/// Adwaita's dim label colour, for text that is present but secondary.
pub const DIM_TEXT: Color32 = Color32::from_rgb(0x9a, 0x99, 0x96);
/// Separators, indentation lines and the border around a dialog. Not from the
/// named palette: it is the lightest of the control fills, which is what reads
/// as a hairline against the window without turning into a bright rule.
pub const BORDER: Color32 = Color32::from_rgb(0x3d, 0x3d, 0x3d);

/// Buttons, entries, selected rows — Adwaita's control radius.
const CONTROL_RADIUS: CornerRadius = CornerRadius::same(6);
/// Dialogs and menus, which are rounder than the things inside them.
const SURFACE_RADIUS: CornerRadius = CornerRadius::same(12);

/// Point size of the chrome: sidebar rows, buttons, dialogs, status bar.
///
/// This is what egui already defaults to, and is written down rather than left
/// implicit so that the chrome keeps GNOME's text size if a future egui moves
/// its own. The editor and the preview set their own sizes and are unaffected.
const BODY_SIZE: f32 = 13.0;

/// The Adwaita dark palette, over egui's dark visuals.
fn adwaita_dark() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();

    visuals.window_fill = WINDOW_BG;
    visuals.panel_fill = WINDOW_BG;
    // Text entries and other sunken areas.
    visuals.extreme_bg_color = VIEW_BG;
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = SURFACE_RADIUS;
    visuals.menu_corner_radius = SURFACE_RADIUS;

    visuals.selection.bg_fill = ACCENT;
    visuals.selection.stroke = Stroke::new(1.0, TEXT);
    // Despite the name this dresses nothing that leaves the app: the only
    // widget left using it is the `ui.link` a `[[wiki link]]` is drawn with,
    // which takes its underline from here.
    visuals.hyperlink_color = LINK;
    // Without this, "weak" text is the normal colour at 60% alpha; Adwaita has
    // a colour of its own for it.
    visuals.weak_text_color = Some(DIM_TEXT);

    // A button paints `weak_bg_fill` and a checkbox `bg_fill`, so a control
    // that is meant to look like one thing has to be given both.
    for (widget, fill) in [
        (&mut visuals.widgets.inactive, CONTROL_BG),
        (&mut visuals.widgets.hovered, CONTROL_HOVER_BG),
        (&mut visuals.widgets.active, CONTROL_ACTIVE_BG),
        (&mut visuals.widgets.open, CONTROL_ACTIVE_BG),
    ] {
        widget.bg_fill = fill;
        widget.weak_bg_fill = fill;
        // GNOME answers a hover by lightening the fill, not by drawing an
        // outline around it or thickening the label, which is what egui's
        // defaults do here.
        widget.bg_stroke = Stroke::NONE;
        widget.fg_stroke = Stroke::new(1.0, TEXT);
        widget.corner_radius = CONTROL_RADIUS;
    }

    // Everything that is drawn but not clicked: panel backgrounds, labels, and
    // the lines `ui.separator` and the tree indentation draw.
    let flat = &mut visuals.widgets.noninteractive;
    flat.bg_fill = WINDOW_BG;
    flat.weak_bg_fill = WINDOW_BG;
    flat.bg_stroke = Stroke::new(1.0, BORDER);
    flat.fg_stroke = Stroke::new(1.0, TEXT);
    flat.corner_radius = CONTROL_RADIUS;

    visuals
}

/// Install the theme. Called once, while the window is being built.
pub fn apply_style(ctx: &egui::Context) {
    // Pin the preference so egui never reaches for its light palette, and then
    // write the same style into both slots anyway: nothing should be able to
    // land the window in a half-styled state.
    ctx.set_theme(egui::ThemePreference::Dark);

    let mut style = egui::Style {
        visuals: adwaita_dark(),
        ..Default::default()
    };

    // egui packs its rows tighter vertically than GNOME does, and its buttons
    // are barely larger than their labels.
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);

    // The chrome follows the preview's sans so that the editor pane is the only
    // monospaced thing on screen. Only the text styles point at the app's own
    // families; egui's built-in `Proportional` and `Monospace` are untouched,
    // and stay on the back of each family as a fallback — which is what keeps
    // the emoji glyphs the sidebar draws its icons with available.
    for (text_style, font_id) in style.text_styles.iter_mut() {
        font_id.family = egui::FontFamily::Name(match text_style {
            egui::TextStyle::Monospace => crate::EDITOR_MONO.into(),
            egui::TextStyle::Heading => crate::PREVIEW_SANS_BOLD.into(),
            _ => crate::PREVIEW_SANS.into(),
        });
        if matches!(text_style, egui::TextStyle::Body | egui::TextStyle::Button) {
            font_id.size = BODY_SIZE;
        }
    }

    let style = Arc::new(style);
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.set_style_of(theme, style.clone());
    }
}
