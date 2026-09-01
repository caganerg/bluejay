mod app;
mod markdown;
mod theme;
mod vault;

use std::path::PathBuf;
use std::process::ExitCode;

use eframe::egui;
use eframe::egui::epaint::text::VariationCoords;

/// Font families the panes ask for by name.
///
/// A `FontId` carries a family and a size but no weight, and `RichText::strong`
/// only picks a brighter colour, so a weight that is actually drawn has to be
/// registered as its own family — hence the second sans. There is no bold mono
/// because nothing draws one. Both typefaces are SIL OFL; see
/// `assets/fonts/*-OFL.txt`.
pub const EDITOR_MONO: &str = "editor_mono";
pub const PREVIEW_SANS: &str = "preview_sans";
pub const PREVIEW_SANS_BOLD: &str = "preview_sans_bold";

/// The UI typeface, carried in the binary rather than looked for on the machine.
///
/// This is GNOME's Adwaita Sans, which is a build of Inter — same designer, same
/// version, metrics identical to the byte — parting from it in the one place
/// that shows in a note: its lowercase `l` carries a tail, so `l`, `I` and `1`
/// cannot be read for one another. It used to be loaded off the filesystem where
/// the desktop happened to have it and fall back to plain Inter where it did
/// not, which left the window looking like two different apps depending on whose
/// machine it was on. Carrying it is what makes it one.
///
/// One file covers both weights: it is variable over `wght` 100–900, so the bold
/// is these same outlines asked to sit at 700 rather than a second face of its
/// own. That is also why the optical-size axis is left alone — every size in the
/// window is drawn at the one `opsz` the file defaults to, as it was before.
const UI_SANS: &[u8] = include_bytes!("../assets/fonts/AdwaitaSans-Regular.ttf");
const UI_MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf");

/// The point on the sans's weight axis the bold family is instanced at.
const BOLD_WEIGHT: f32 = 700.0;

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
            theme::apply_style(&cc.egui_ctx);
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

/// The app's typefaces, bound to the families the panes ask for by name.
///
/// egui's own `Proportional` and `Monospace` families are left as they are, so
/// each pane has to name the family it wants; what they hold stays on the back
/// of every new family as a fallback, which is what keeps the emoji fonts
/// available for the glyphs the sidebar draws its icons with.
///
/// Built apart from installing it so that the result can be looked at: a family
/// chain naming font data nobody registered panics egui the first time
/// something is drawn with it, which is a poor thing to find out in a window.
fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let sans_fallback = fonts.families[&egui::FontFamily::Proportional].clone();
    let mono_fallback = fonts.families[&egui::FontFamily::Monospace].clone();

    // The two sans entries are one file read at two points on its weight axis.
    // `from_static` borrows the bytes, so it is in the binary once.
    let sans_bold = egui::FontData {
        tweak: egui::FontTweak {
            coords: VariationCoords::new([(b"wght", BOLD_WEIGHT)]),
            ..Default::default()
        },
        ..egui::FontData::from_static(UI_SANS)
    };

    for (name, family, fallback, data) in [
        (
            "JetBrainsMono-Regular",
            EDITOR_MONO,
            &mono_fallback,
            egui::FontData::from_static(UI_MONO),
        ),
        (
            "AdwaitaSans-Regular",
            PREVIEW_SANS,
            &sans_fallback,
            egui::FontData::from_static(UI_SANS),
        ),
        (
            "AdwaitaSans-Bold",
            PREVIEW_SANS_BOLD,
            &sans_fallback,
            sans_bold,
        ),
    ] {
        fonts
            .font_data
            .insert(name.to_owned(), std::sync::Arc::new(data));
        let mut chain = vec![name.to_owned()];
        chain.extend(fallback.iter().cloned());
        fonts
            .families
            .insert(egui::FontFamily::Name(family.into()), chain);
    }

    fonts
}

fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The bold family is the sans asked to sit at 700, so that axis has to be
    /// there and has to reach that far. Were it not, every bold thing in the
    /// window — each heading, each dialog title, the open note's name — would
    /// quietly be the regular weight, which nothing short of a screenshot
    /// catches. It is asserted rather than assumed because the file it is
    /// asserted of can be replaced.
    #[test]
    fn the_embedded_sans_bolds_by_its_weight_axis() {
        let weight = egui::FontData::from_static(UI_SANS)
            .variation_axes()
            .into_iter()
            .find(|axis| axis.tag == "wght")
            .expect("the UI sans has to be variable over weight");
        assert!(
            weight.range.min <= 400.0,
            "the axis has to reach the regular weight: {}",
            weight.range.min
        );
        assert!(
            weight.range.max >= BOLD_WEIGHT,
            "the axis has to reach {BOLD_WEIGHT}: {}",
            weight.range.max
        );
    }

    /// The axis being there is not the same as egui using it. This lays the same
    /// string out in both sans families and insists the bold one comes out
    /// wider, which is the only thing that actually distinguishes the two: they
    /// are one file, and if the variation coordinates were ignored — dropped by
    /// an egui upgrade, or misspelled here — both families would be the regular
    /// weight and every heading in the window would quietly go light.
    #[test]
    fn the_bold_family_draws_heavier_than_the_regular() {
        let ctx = egui::Context::default();
        install_fonts(&ctx);

        let (mut regular, mut bold) = (0.0, 0.0);
        ctx.run_ui(Default::default(), |ui| {
            // Through the painter, which is the path the text itself takes.
            let width = |ui: &egui::Ui, family: &str| {
                ui.painter()
                    .layout_no_wrap(
                        "Handgloves 0123".to_owned(),
                        egui::FontId::new(15.0, egui::FontFamily::Name(family.into())),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x
            };
            regular = width(ui, PREVIEW_SANS);
            bold = width(ui, PREVIEW_SANS_BOLD);
        })
        .drop_without_applying_deltas();

        assert!(regular > 0.0, "the regular sans laid out nothing");
        assert!(
            bold > regular,
            "the bold family has to be heavier: {bold} vs {regular}"
        );
    }

    /// Every family the panes name has to be bound, and every font on its chain
    /// registered: a chain naming font data nobody registered panics egui the
    /// first time text is drawn with it.
    #[test]
    fn every_family_the_panes_name_is_registered() {
        let fonts = font_definitions();
        for family in [EDITOR_MONO, PREVIEW_SANS, PREVIEW_SANS_BOLD] {
            let chain = fonts
                .families
                .get(&egui::FontFamily::Name(family.into()))
                .unwrap_or_else(|| panic!("{family} is never bound to anything"));
            assert!(!chain.is_empty(), "{family} resolves to no font at all");
            for name in chain {
                assert!(
                    fonts.font_data.contains_key(name),
                    "{family} names {name}, which was never registered"
                );
            }
        }
    }

    /// The emoji faces egui ships stay on the back of each family: the sidebar
    /// draws its folder and reload icons with glyphs neither of our typefaces
    /// has, and they came out as empty boxes when a family replaced that chain
    /// rather than extending it.
    #[test]
    fn the_fallback_faces_stay_behind_our_own() {
        let fonts = font_definitions();
        let default = egui::FontDefinitions::default();
        let proportional = &default.families[&egui::FontFamily::Proportional];
        let chain = &fonts.families[&egui::FontFamily::Name(PREVIEW_SANS.into())];
        assert_eq!(
            chain.first().map(String::as_str),
            Some("AdwaitaSans-Regular")
        );
        assert!(
            proportional.iter().all(|face| chain.contains(face)),
            "egui's own fallbacks must still be reachable: {chain:?}"
        );
    }
}
