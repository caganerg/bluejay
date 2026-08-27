//! The three-pane note window: tree, editor, preview.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, Ui};

use crate::markdown::{self, Action};
use crate::vault::{self, Node};

/// How long editing has to pause before the note is written to disk.
const AUTOSAVE_DELAY: Duration = Duration::from_millis(600);

/// Point size of the raw markdown in the editor pane. JetBrains Mono runs a
/// little large for its nominal size, so this sits just under the preview's 15.
const EDITOR_SIZE: f32 = 14.0;

/// Something the sidebar wants done, collected while the tree is borrowed and
/// applied once the panel closure is over.
enum Cmd {
    Open(PathBuf),
    Reload,
    NewNote(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
}

#[derive(PartialEq)]
enum ModalKind {
    NewNote,
    NewFolder,
    Rename,
    Delete,
}

struct Modal {
    kind: ModalKind,
    /// Parent folder for the two "new" modals, the target itself otherwise.
    target: PathBuf,
    input: String,
    focused: bool,
}

pub struct App {
    root: PathBuf,
    tree: Node,
    /// Lowercased note name -> path, rebuilt on every refresh.
    index: HashMap<String, PathBuf>,
    open_path: Option<PathBuf>,
    buffer: String,
    dirty: bool,
    last_edit: Instant,
    modal: Option<Modal>,
    status: String,
}

impl App {
    pub fn new(root: PathBuf) -> Self {
        let mut app = Self {
            tree: vault::scan(&root),
            root,
            index: HashMap::new(),
            open_path: None,
            buffer: String::new(),
            dirty: false,
            last_edit: Instant::now(),
            modal: None,
            status: String::new(),
        };
        app.refresh();
        app
    }

    fn refresh(&mut self) {
        self.tree = vault::scan(&self.root);
        self.index.clear();
        vault::build_index(&self.tree, &mut self.index);
    }

    /// Write the open note if it has unsaved edits.
    pub fn save_now(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = self.open_path.clone() else {
            self.dirty = false;
            return;
        };
        match fs::write(&path, &self.buffer) {
            Ok(()) => {
                self.dirty = false;
                self.status = format!("Saved {}", name_of(&path));
            }
            Err(err) => self.status = format!("Could not save {}: {err}", name_of(&path)),
        }
    }

    fn open(&mut self, path: PathBuf) {
        self.save_now();
        match fs::read_to_string(&path) {
            Ok(text) => {
                self.buffer = text;
                self.open_path = Some(path);
                self.dirty = false;
                self.status.clear();
            }
            Err(err) => self.status = format!("Could not open {}: {err}", name_of(&path)),
        }
    }

    /// Resolve a `[[wiki link]]` anywhere in the vault, creating the note at the
    /// vault root when nothing matches.
    fn open_by_name(&mut self, name: &str) {
        if !vault::valid_name(name) {
            self.status = format!("“{name}” is not a usable note name");
            return;
        }
        if let Some(path) = self.index.get(&name.to_lowercase()).cloned() {
            self.open(path);
            return;
        }
        let path = self.root.join(format!("{}.md", name.trim()));
        match fs::write(&path, format!("# {}\n\n", name.trim())) {
            Ok(()) => {
                self.refresh();
                self.open(path);
                self.status = format!("Created {name}.md");
            }
            Err(err) => self.status = format!("Could not create {name}.md: {err}"),
        }
    }

    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Open(path) => self.open(path),
            Cmd::Reload => self.refresh(),
            Cmd::NewNote(parent) => {
                self.modal = Some(Modal {
                    kind: ModalKind::NewNote,
                    target: parent,
                    input: String::new(),
                    focused: false,
                })
            }
            Cmd::NewFolder(parent) => {
                self.modal = Some(Modal {
                    kind: ModalKind::NewFolder,
                    target: parent,
                    input: String::new(),
                    focused: false,
                })
            }
            Cmd::Rename(path) => {
                let current = if path.is_dir() {
                    name_of(&path)
                } else {
                    name_of(&path).trim_end_matches(".md").to_owned()
                };
                self.modal = Some(Modal {
                    kind: ModalKind::Rename,
                    target: path,
                    input: current,
                    focused: false,
                })
            }
            Cmd::Delete(path) => {
                self.modal = Some(Modal {
                    kind: ModalKind::Delete,
                    target: path,
                    input: String::new(),
                    focused: false,
                })
            }
        }
    }

    fn confirm_modal(&mut self, modal: &Modal) {
        let name = modal.input.trim().to_owned();
        match modal.kind {
            ModalKind::NewNote | ModalKind::NewFolder | ModalKind::Rename
                if !vault::valid_name(&name) =>
            {
                self.status = "Names cannot be empty or contain slashes".to_owned();
                return;
            }
            _ => {}
        }

        match modal.kind {
            ModalKind::NewNote => {
                let file = if name.ends_with(".md") {
                    name.clone()
                } else {
                    format!("{name}.md")
                };
                let path = modal.target.join(&file);
                if path.exists() {
                    self.status = format!("{file} already exists");
                    return;
                }
                match fs::write(&path, String::new()) {
                    Ok(()) => {
                        self.refresh();
                        self.open(path);
                    }
                    Err(err) => self.status = format!("Could not create {file}: {err}"),
                }
            }
            ModalKind::NewFolder => {
                let path = modal.target.join(&name);
                if let Err(err) = fs::create_dir_all(&path) {
                    self.status = format!("Could not create {name}: {err}");
                } else {
                    self.refresh();
                }
            }
            ModalKind::Rename => {
                let is_dir = modal.target.is_dir();
                let file = if is_dir || name.ends_with(".md") {
                    name.clone()
                } else {
                    format!("{name}.md")
                };
                let Some(parent) = modal.target.parent() else {
                    return;
                };
                let dest = parent.join(&file);
                if dest == modal.target {
                    return;
                }
                if dest.exists() {
                    self.status = format!("{file} already exists");
                    return;
                }
                self.save_now();
                match fs::rename(&modal.target, &dest) {
                    Ok(()) => {
                        // Follow the open note if it (or its folder) moved.
                        if let Some(open) = self.open_path.clone() {
                            if open == modal.target {
                                self.open_path = Some(dest.clone());
                            } else if let Ok(rest) = open.strip_prefix(&modal.target) {
                                self.open_path = Some(dest.join(rest));
                            }
                        }
                        self.refresh();
                    }
                    Err(err) => self.status = format!("Could not rename: {err}"),
                }
            }
            ModalKind::Delete => {
                let is_dir = modal.target.is_dir();
                let result = if is_dir {
                    fs::remove_dir_all(&modal.target)
                } else {
                    fs::remove_file(&modal.target)
                };
                match result {
                    Ok(()) => {
                        if let Some(open) = self.open_path.clone() {
                            if open == modal.target || open.starts_with(&modal.target) {
                                self.open_path = None;
                                self.buffer.clear();
                                self.dirty = false;
                            }
                        }
                        self.refresh();
                        self.status = format!("Deleted {}", name_of(&modal.target));
                    }
                    Err(err) => self.status = format!("Could not delete: {err}"),
                }
            }
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let mut cmds: Vec<Cmd> = Vec::new();
        let tree = &self.tree;
        let selected = self.open_path.clone();
        let root = self.root.clone();

        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(230.0)
            .size_range(150.0..=420.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("New note").clicked() {
                        cmds.push(Cmd::NewNote(root.clone()));
                    }
                    if ui.button("New folder").clicked() {
                        cmds.push(Cmd::NewFolder(root.clone()));
                    }
                    if ui.button("⟳").on_hover_text("Reload from disk").clicked() {
                        cmds.push(Cmd::Reload);
                    }
                });
                ui.add_space(4.0);
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("tree")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        tree_ui(ui, tree, selected.as_deref(), &mut cmds);
                    });
            });

        for cmd in cmds {
            self.apply(cmd);
        }
    }

    fn preview(&mut self, ui: &mut Ui) {
        let mut action = None;
        let source = std::mem::take(&mut self.buffer);

        egui::Panel::right("preview")
            .resizable(true)
            .default_size(420.0)
            .size_range(220.0..=1200.0)
            .show(ui, |ui| {
                // The pane keeps the width the user dragged it to. A note too
                // wide to fit — a big table — scrolls sideways instead of
                // pushing the panel out, which egui would otherwise store as
                // the panel's new width and never give back.
                let width = ui.available_width();
                egui::ScrollArea::both()
                    .id_salt("preview")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(6.0);
                        // Prose still wraps at the pane edge; only content that
                        // cannot wrap is allowed past it.
                        ui.set_min_width(width);
                        ui.set_max_width(width);
                        if self.open_path.is_none() {
                            ui.label("Nothing open.");
                        } else {
                            action = markdown::render(ui, &source);
                        }
                        ui.add_space(24.0);
                    });
            });

        self.buffer = source;
        if let Some(Action::OpenNote(name)) = action {
            self.open_by_name(&name);
        }
    }

    fn editor(&mut self, ui: &mut Ui) {
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(path) = self.open_path.clone() else {
                ui.centered_and_justified(|ui| {
                    ui.label("Select a note on the left, or create one.");
                });
                return;
            };

            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let rel = path.strip_prefix(&self.root).unwrap_or(&path);
                ui.label(
                    egui::RichText::new(rel.to_string_lossy())
                        .family(egui::FontFamily::Name(crate::PREVIEW_SANS_BOLD.into()))
                        .strong()
                        .size(14.0),
                );
                if self.dirty {
                    ui.label(egui::RichText::new("•").weak());
                }
            });
            ui.separator();

            egui::ScrollArea::vertical()
                .id_salt("editor")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let response = ui.add(
                        egui::TextEdit::multiline(&mut self.buffer)
                            .font(egui::FontId::new(
                                EDITOR_SIZE,
                                egui::FontFamily::Name(crate::EDITOR_MONO.into()),
                            ))
                            .desired_width(f32::INFINITY)
                            .desired_rows(40)
                            .lock_focus(true)
                            .frame(egui::Frame::NONE),
                    );
                    if response.changed() {
                        self.dirty = true;
                        self.last_edit = Instant::now();
                    }
                });
        });
    }

    fn modal_ui(&mut self, ctx: &egui::Context) {
        let Some(mut modal) = self.modal.take() else {
            return;
        };

        let (title, prompt) = match modal.kind {
            ModalKind::NewNote => ("New note", "Note name"),
            ModalKind::NewFolder => ("New folder", "Folder name"),
            ModalKind::Rename => ("Rename", "New name"),
            ModalKind::Delete => ("Delete", ""),
        };

        let mut confirm = false;
        let mut close = false;

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if modal.kind == ModalKind::Delete {
                    let is_dir = modal.target.is_dir();
                    ui.label(format!("Delete “{}”?", name_of(&modal.target)));
                    if is_dir {
                        ui.label(
                            egui::RichText::new("The folder and everything inside it is removed.")
                                .weak(),
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        if ui.button("Delete").clicked() {
                            confirm = true;
                        }
                    });
                } else {
                    ui.label(prompt);
                    let response = ui.text_edit_singleline(&mut modal.input);
                    if !modal.focused {
                        response.request_focus();
                        modal.focused = true;
                    }
                    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        confirm = true;
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                        if ui.button("OK").clicked() {
                            confirm = true;
                        }
                    });
                }

                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
            });

        if confirm {
            self.confirm_modal(&modal);
        } else if !close {
            self.modal = Some(modal);
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            self.save_now();
        }

        self.sidebar(ui);
        self.preview(ui);

        let status = self.status.clone();
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(status).weak().size(12.0));
            });
        });

        self.editor(ui);
        self.modal_ui(&ctx);

        // Autosave once editing has been quiet for a moment.
        if self.dirty {
            let idle = self.last_edit.elapsed();
            if idle >= AUTOSAVE_DELAY {
                self.save_now();
            } else {
                ctx.request_repaint_after(AUTOSAVE_DELAY - idle);
            }
        }

        if ctx.input(|i| i.viewport().close_requested()) {
            self.save_now();
        }
    }

    fn save(&mut self, _storage: &mut dyn eframe::Storage) {
        self.save_now();
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn tree_ui(ui: &mut Ui, node: &Node, selected: Option<&Path>, cmds: &mut Vec<Cmd>) {
    for child in &node.children {
        if child.is_dir {
            let header = egui::CollapsingHeader::new(format!("🗀  {}", child.name))
                .id_salt(&child.path)
                .show(ui, |ui| tree_ui(ui, child, selected, cmds));
            header.header_response.context_menu(|ui| {
                if ui.button("New note here").clicked() {
                    cmds.push(Cmd::NewNote(child.path.clone()));
                    ui.close();
                }
                if ui.button("New folder here").clicked() {
                    cmds.push(Cmd::NewFolder(child.path.clone()));
                    ui.close();
                }
                if ui.button("Rename").clicked() {
                    cmds.push(Cmd::Rename(child.path.clone()));
                    ui.close();
                }
                if ui.button("Delete").clicked() {
                    cmds.push(Cmd::Delete(child.path.clone()));
                    ui.close();
                }
            });
        } else {
            let is_selected = selected == Some(child.path.as_path());
            let response = ui.selectable_label(is_selected, child.stem());
            if response.clicked() {
                cmds.push(Cmd::Open(child.path.clone()));
            }
            response.context_menu(|ui| {
                if ui.button("Rename").clicked() {
                    cmds.push(Cmd::Rename(child.path.clone()));
                    ui.close();
                }
                if ui.button("Delete").clicked() {
                    cmds.push(Cmd::Delete(child.path.clone()));
                    ui.close();
                }
            });
        }
    }
}
