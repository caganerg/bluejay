//! The look: one fixed dark theme, cut to resemble GNOME's Adwaita.
//!
//! Nothing here follows the desktop. The theme is dark and stays dark — there
//! is no light variant to switch to and no portal or settings daemon asked what
//! the session prefers, which is why no crate is carried for it. What it does
//! borrow from GNOME is the palette, the corner radii and, if the desktop has
//! it installed, the UI typeface, so that the window sits next to a GTK4 one
//! without looking like it came from a different decade.
//!
//! Only colour, spacing and type live here; the panes themselves are in
//! `app.rs` and are not this module's business. The palette is the whole
//! window's, not just the chrome's: `markdown.rs` draws the preview straight
//! out of these constants rather than keeping a second set that would drift
//! from them.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::epaint::text::VariationCoords;
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
pub fn adwaita_dark() -> egui::Visuals {
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

/// The desktop's own UI faces, best first, as the file stems they are installed
/// under.
///
/// Adwaita Sans is what current GNOME draws its chrome with; Cantarell is what
/// it used before, and is still what an older desktop has. Both are looked for
/// because either one makes the window match its neighbours, and neither is
/// required: a machine with no GNOME fonts installed falls through to the
/// embedded Inter without noticing.
const UI_FAMILIES: &[(&[&str], &[&str])] = &[
    (
        &["AdwaitaSans-Regular", "AdwaitaSans"],
        &["AdwaitaSans-Bold"],
    ),
    (
        &["Cantarell-Regular", "Cantarell-VF", "Cantarell"],
        &["Cantarell-Bold"],
    ),
];

/// How deep the font directories are walked. Deep enough for the vendor and
/// style subdirectories a distribution files fonts under, and shallow enough
/// that a symlink pointing back up the tree ends the walk rather than looping.
const MAX_FONT_DEPTH: usize = 6;

/// Register the desktop's UI faces, and name what was registered.
///
/// The two lists returned are font-data keys to put at the *front* of the
/// regular and bold sans families. They are empty when this machine has no
/// GNOME font installed, which is the whole of the fallback: the embedded Inter
/// is next on the chain either way, so a missing, unreadable or half-installed
/// family quietly leaves the app looking the way it did before.
pub fn register_system_sans(fonts: &mut egui::FontDefinitions) -> (Vec<String>, Vec<String>) {
    let files = installed_fonts();

    for (regulars, bolds) in UI_FAMILIES {
        let Some((name, bytes)) = regulars.iter().find_map(|stem| read_face(&files, stem)) else {
            continue;
        };
        let regular = egui::FontData::from_owned(bytes);

        // A family that ships one variable file — which is how Adwaita Sans is
        // packaged — has no bold file to find, but it does carry a weight axis,
        // and asking that axis for 700 is a truer bold than borrowing Inter's.
        let bold = bolds
            .iter()
            .find_map(|stem| read_face(&files, stem))
            .map(|(name, bytes)| (name, egui::FontData::from_owned(bytes)))
            .or_else(|| bold_instance(&regular).map(|data| (format!("{name}-wght700"), data)));

        fonts.font_data.insert(name.clone(), Arc::new(regular));
        let bold_head = match bold {
            Some((bold_name, data)) => {
                fonts.font_data.insert(bold_name.clone(), Arc::new(data));
                vec![bold_name]
            }
            None => Vec::new(),
        };
        return (vec![name], bold_head);
    }

    (Vec::new(), Vec::new())
}

/// The same face, told to sit at weight 700 — `None` unless it is a variable
/// font with a weight axis that reaches that far.
fn bold_instance(regular: &egui::FontData) -> Option<egui::FontData> {
    let weight = regular
        .variation_axes()
        .into_iter()
        .find(|axis| axis.tag.to_string() == "wght")?;
    if weight.range.max < 600.0 {
        return None;
    }
    let bold = weight.range.max.min(700.0);

    Some(egui::FontData {
        font: regular.font.clone(),
        index: regular.index,
        tweak: egui::FontTweak {
            coords: VariationCoords::new([(b"wght", bold)]),
            ..regular.tweak.clone()
        },
    })
}

/// Read one installed face by file stem, returning the stem as spelled in
/// `UI_FAMILIES` — that spelling becomes the font-data key — and its bytes.
fn read_face(files: &BTreeMap<String, PathBuf>, stem: &str) -> Option<(String, Vec<u8>)> {
    let path = files.get(&stem.to_ascii_lowercase())?;
    let bytes = fs::read(path).ok()?;
    is_parseable_sfnt(&bytes).then(|| (stem.to_owned(), bytes))
}

/// Does this look like a font file that will parse?
///
/// egui *panics* on a face it cannot read — `FontsImpl::new` unwraps the parse
/// of every registered font — and unlike the two baked into the binary, what is
/// read here came off someone's filesystem: a half-finished download or a
/// truncated package would otherwise take the window down on the first frame
/// that draws text. So the header is checked before the bytes are handed over.
/// A file whose table directory runs past its own end is exactly what a
/// truncated font looks like, and is what the parser refuses on.
fn is_parseable_sfnt(bytes: &[u8]) -> bool {
    // TrueType outlines, Apple's older spelling of the same, and CFF.
    let magic = bytes.get(..4);
    if !matches!(magic, Some(b"\x00\x01\x00\x00" | b"true" | b"OTTO")) {
        return false;
    }

    let Some(count) = bytes.get(4..6) else {
        return false;
    };
    let count = usize::from(u16::from_be_bytes([count[0], count[1]]));
    // A 12-byte header, then one 16-byte record per table: tag, checksum,
    // offset, length.
    let Some(records) = bytes.get(12..12 + count * 16) else {
        return false;
    };

    records.chunks_exact(16).all(|record| {
        // A record is tag, checksum, offset, length — one word each.
        let word = |i: usize| {
            let word = [record[i], record[i + 1], record[i + 2], record[i + 3]];
            u32::from_be_bytes(word) as usize
        };
        let (offset, length) = (word(8), word(12));
        offset
            .checked_add(length)
            .is_some_and(|end| end <= bytes.len())
    })
}

/// Every `.ttf` and `.otf` on the system, by lower-cased file stem.
///
/// This is fontconfig's search path minus fontconfig itself, which would be a
/// C library and a crate to bind it for one lookup made once at startup.
fn installed_fonts() -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    for dir in font_dirs() {
        collect_fonts(&dir, 0, &mut found);
    }
    found
}

/// Where fonts are installed, most specific first: a face the user installed
/// for themselves wins over the distribution's copy of the same name.
fn font_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        dirs.push(PathBuf::from(data_home).join("fonts"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".local/share/fonts"));
        dirs.push(home.join(".fonts"));
    }
    dirs.push(PathBuf::from("/usr/local/share/fonts"));
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs
}

fn collect_fonts(dir: &Path, depth: usize, found: &mut BTreeMap<String, PathBuf>) {
    if depth > MAX_FONT_DEPTH {
        return;
    }
    // A missing or unreadable directory is the ordinary case here, not a
    // failure: most machines have only some of the ones we look in.
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        // `metadata` follows symlinks, so a distribution that files its fonts
        // as links to a store is walked like any other. What keeps that from
        // circling forever is the depth limit, not this.
        let Ok(meta) = fs::metadata(&path) else {
            continue;
        };

        if meta.is_dir() {
            collect_fonts(&path, depth + 1, found);
            continue;
        }

        let is_font = path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|ext| ext.eq_ignore_ascii_case("ttf") || ext.eq_ignore_ascii_case("otf"));
        if !is_font {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(OsStr::to_str) {
            let key = stem.to_ascii_lowercase();
            found.entry(key).or_insert(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// egui panics on a face it cannot parse, so a font that is not whole has
    /// to be turned away before it is registered rather than after.
    #[test]
    fn a_font_that_is_not_whole_is_turned_away() {
        let whole = &include_bytes!("../assets/fonts/Inter-Regular.ttf")[..];
        assert!(is_parseable_sfnt(whole));

        // What a half-finished download looks like: the header still says how
        // many tables there are and where they start, and most of them are no
        // longer there.
        assert!(!is_parseable_sfnt(&whole[..whole.len() / 2]));
        assert!(!is_parseable_sfnt(&whole[..8]));
        assert!(!is_parseable_sfnt(b""));
        assert!(!is_parseable_sfnt(b"this is a note, not a font"));
    }

    /// Whatever this machine happens to have installed, every name handed back
    /// has to have font data behind it: a family chain naming a font that was
    /// never registered panics egui the first time something is drawn with it.
    #[test]
    fn the_faces_it_names_are_the_ones_it_registered() {
        let mut fonts = egui::FontDefinitions::default();
        let (regular, bold) = register_system_sans(&mut fonts);

        for name in regular.iter().chain(&bold) {
            assert!(
                fonts.font_data.contains_key(name),
                "{name} is on a chain but was never registered"
            );
        }
        // A bold with no regular would leave headings in one typeface and the
        // text under them in another.
        assert!(
            regular.len() == 1 || bold.is_empty(),
            "a bold face came back without its regular"
        );
    }
}
