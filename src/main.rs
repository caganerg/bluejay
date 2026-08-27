#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod markdown;
mod vault;

use std::path::PathBuf;
use std::process::ExitCode;

use eframe::egui;

fn main() -> ExitCode {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([700.0, 400.0])
            .with_title("bluejay"),
        ..Default::default()
    };

    let result = eframe::run_native(
        "bluejay",
        options,
        Box::new(|cc| {
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

fn set_dark_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(0x1e, 0x1f, 0x22);
    visuals.extreme_bg_color = egui::Color32::from_rgb(0x1a, 0x1b, 0x1e);
    visuals.window_fill = egui::Color32::from_rgb(0x24, 0x26, 0x2a);
    visuals.hyperlink_color = egui::Color32::from_rgb(0x7f, 0xa8, 0xf5);
    ctx.set_visuals(visuals);
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

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Bluejay::Ready(app) = self {
            app.save(storage);
        }
    }
}
