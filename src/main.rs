mod app;
mod markdown;
mod vault;

use std::path::PathBuf;
use std::process::ExitCode;

use eframe::egui;

/// Font families the panes ask for by name.
///
/// A `FontId` carries a family and a size but no weight, and `RichText::strong`
/// only picks a brighter colour, so a weight that is actually drawn has to be
/// registered as its own family — hence the second Inter. There is no bold mono
/// because nothing draws one. Both typefaces are SIL OFL; see
/// `assets/fonts/*-OFL.txt`.
pub const EDITOR_MONO: &str = "editor_mono";
pub const PREVIEW_SANS: &str = "preview_sans";
pub const PREVIEW_SANS_BOLD: &str = "preview_sans_bold";

/// Wayland app id, and the basename the desktop entry must carry.
const APP_ID: &str = "bluejay";

fn main() -> ExitCode {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([700.0, 400.0])
        .with_title("bluejay")
        // eframe would derive this from the app name anyway, but the desktop
        // entry has to match it exactly, so it is pinned here rather than left
        // to a default: `bluejay.desktop` is what a compositor looks up to find
        // the icon, and it finds it by this app id.
        .with_app_id(APP_ID);

    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let result = eframe::run_native(
        "bluejay",
        options,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
            set_dark_theme(&cc.egui_ctx);
            Ok(Box::new(Bluejay::new()))
        }),
    );

    // The X11 backend is compiled out, so there is nothing to fall back to and
    // nothing to detect: report why the window could not open and stop.
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("bluejay: could not open a window: {err}");
            eprintln!(
                "bluejay is built for Wayland only. It needs a running Wayland \
                 compositor; there is no X11 or XWayland fallback."
            );
            ExitCode::FAILURE
        }
    }
}

/// The window icon, decoded from the logo baked into the binary.
///
/// Wayland has no window-icon protocol that winit speaks, so this reaches the
/// compositor nowhere and the taskbar icon comes from `bluejay.desktop` instead
/// — see the README. It is set anyway because it costs one decode at startup
/// and is the only thing that would carry the logo on any other backend; a
/// failure here is not worth refusing to start over, so a bad decode just
/// leaves eframe's default icon in place.
fn load_icon() -> Option<egui::IconData> {
    match eframe::icon_data::from_png_bytes(&include_bytes!("../assets/logo.png")[..]) {
        Ok(icon) => Some(icon),
        Err(err) => {
            eprintln!("bluejay: could not decode the window icon: {err}");
            None
        }
    }
}

/// Register JetBrains Mono and Inter as named families.
///
/// egui's own `Proportional` and `Monospace` families are left as they are, so
/// each pane has to name the family it wants; what they hold stays on the back
/// of every new family as a fallback, which is what keeps the emoji fonts
/// available for the glyphs the sidebar draws its icons with.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let sans_fallback = fonts.families[&egui::FontFamily::Proportional].clone();
    let mono_fallback = fonts.families[&egui::FontFamily::Monospace].clone();

    for (name, family, fallback, bytes) in [
        (
            "JetBrainsMono-Regular",
            EDITOR_MONO,
            &mono_fallback,
            &include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf")[..],
        ),
        (
            "Inter-Regular",
            PREVIEW_SANS,
            &sans_fallback,
            &include_bytes!("../assets/fonts/Inter-Regular.ttf")[..],
        ),
        (
            "Inter-Bold",
            PREVIEW_SANS_BOLD,
            &sans_fallback,
            &include_bytes!("../assets/fonts/Inter-Bold.ttf")[..],
        ),
    ] {
        fonts.font_data.insert(
            name.to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(bytes)),
        );
        let mut chain = vec![name.to_owned()];
        chain.extend(fallback.iter().cloned());
        fonts
            .families
            .insert(egui::FontFamily::Name(family.into()), chain);
    }

    ctx.set_fonts(fonts);
}

fn set_dark_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(0x1e, 0x1f, 0x22);
    visuals.extreme_bg_color = egui::Color32::from_rgb(0x1a, 0x1b, 0x1e);
    visuals.window_fill = egui::Color32::from_rgb(0x24, 0x26, 0x2a);
    // Despite the name this no longer dresses anything that leaves the app: the
    // only widget left using it is the `ui.link` a `[[wiki link]]` is drawn
    // with, which takes its underline from here.
    visuals.hyperlink_color = egui::Color32::from_rgb(0x7f, 0xa8, 0xf5);
    ctx.set_visuals(visuals);

    // The chrome — sidebar, buttons, modals, status bar — follows the preview's
    // Inter so that the editor pane is the only monospaced thing on screen.
    // Only the styles point at the new families; the built-in ones are untouched.
    ctx.all_styles_mut(|style| {
        for (text_style, font_id) in style.text_styles.iter_mut() {
            font_id.family = egui::FontFamily::Name(match text_style {
                egui::TextStyle::Monospace => EDITOR_MONO.into(),
                egui::TextStyle::Heading => PREVIEW_SANS_BOLD.into(),
                _ => PREVIEW_SANS.into(),
            });
        }
    });
}

/// Either the first-run folder picker, or the note window once we have a vault.
enum Bluejay {
    Setup { error: Option<String> },
    Ready(Box<app::App>),
}

impl Bluejay {
    fn new() -> Self {
        match vault::load_root() {
            Some(root) => Self::Ready(Box::new(app::App::new(root))),
            None => Self::Setup { error: None },
        }
    }
}

impl eframe::App for Bluejay {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let mut chosen: Option<PathBuf> = None;

        match self {
            Bluejay::Setup { error } => {
                egui::CentralPanel::default().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(ui.available_height() * 0.3);
                        ui.heading("bluejay");
                        ui.add_space(8.0);
                        ui.label("Pick the folder your .md notes live in.");
                        ui.add_space(16.0);
                        if ui.button("Choose folder…").clicked() {
                            match rfd::FileDialog::new().pick_folder() {
                                Some(path) => chosen = Some(path),
                                None => *error = Some("No folder chosen.".to_owned()),
                            }
                        }
                        if let Some(error) = error {
                            ui.add_space(12.0);
                            ui.colored_label(egui::Color32::from_rgb(0xe0, 0x8a, 0x8a), error.as_str());
                        }
                    });
                });
            }
            Bluejay::Ready(app) => eframe::App::ui(app.as_mut(), ui, frame),
        }

        if let Some(root) = chosen {
            vault::save_root(&root);
            *self = Bluejay::Ready(Box::new(app::App::new(root)));
        }
    }
}
