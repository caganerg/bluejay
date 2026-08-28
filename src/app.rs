//! The three-pane note window: tree, editor, preview.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

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
    ChangeRoot,
    NewNote(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Copy(PathBuf),
    Cut(PathBuf),
    /// Paste whatever is on the clipboard into this folder.
    Paste(PathBuf),
}

/// Which of the two the clipboard is holding a path for.
#[derive(Clone, Copy, PartialEq)]
enum ClipOp {
    Copy,
    Cut,
}

#[derive(PartialEq)]
enum ModalKind {
    NewNote,
    NewFolder,
    Rename,
    Delete,
    /// The note changed on disk while the buffer held unsaved edits. Raised by
    /// `save_now` rather than by anything the user did.
    Conflict,
    /// A reload was asked for with unsaved edits in the buffer.
    Reload,
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
    /// The open note's timestamp as of the last read or write this window did.
    /// Anything else on disk means someone changed it behind our back.
    open_mtime: Option<SystemTime>,
    dirty: bool,
    last_edit: Instant,
    modal: Option<Modal>,
    /// The one item “Copy” or “Cut” put aside, if any. Internal to the window:
    /// nothing is handed to the system clipboard.
    clipboard: Option<(PathBuf, ClipOp)>,
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
            open_mtime: None,
            dirty: false,
            last_edit: Instant::now(),
            modal: None,
            clipboard: None,
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
    ///
    /// Does nothing if the note changed underneath the buffer, raising the
    /// question instead: autosave fires on a timer and would otherwise write
    /// over whatever arrived — a sync, a `git pull`, an edit in another program
    /// — without anyone having asked it to.
    pub fn save_now(&mut self) {
        if !self.dirty {
            return;
        }
        let Some(path) = self.open_path.clone() else {
            self.dirty = false;
            return;
        };
        if mtime_of(&path) != self.open_mtime {
            // Ask once. Until it is answered the buffer stays dirty and this
            // returns early, so the autosave timer cannot ask again every frame.
            if self.modal.is_none() {
                self.modal = Some(Modal {
                    kind: ModalKind::Conflict,
                    target: path,
                    input: String::new(),
                    focused: false,
                });
            }
            return;
        }
        match vault::write_atomic(&path, &self.buffer) {
            Ok(()) => {
                self.dirty = false;
                // Our own write moved the note's timestamp; adopt it, or the
                // next save would mistake it for someone else's.
                self.open_mtime = mtime_of(&path);
                self.status = format!("Saved {}", name_of(&path));
            }
            Err(err) => self.status = format!("Could not save {}: {err}", name_of(&path)),
        }
    }

    fn open(&mut self, path: PathBuf) {
        self.save_now();
        // Read the timestamp first: a write landing between the two is then
        // newer than what we stored and gets noticed, where the other order
        // would file it under the copy we already hold.
        let mtime = mtime_of(&path);
        match fs::read_to_string(&path) {
            Ok(text) => {
                self.buffer = text;
                self.open_path = Some(path);
                self.open_mtime = mtime;
                self.dirty = false;
                self.status.clear();
            }
            Err(err) => self.status = format!("Could not open {}: {err}", name_of(&path)),
        }
    }

    /// Read the open note back from disk, dropping whatever is in the buffer.
    ///
    /// Deliberately not `open`, which saves first: the whole point of getting
    /// here is that the buffer is the copy being given up.
    fn reload_open(&mut self) {
        let Some(path) = self.open_path.clone() else {
            return;
        };
        let mtime = mtime_of(&path);
        match fs::read_to_string(&path) {
            Ok(text) => {
                self.buffer = text;
                self.open_mtime = mtime;
                self.dirty = false;
                self.status = format!("Reloaded {}", name_of(&path));
            }
            Err(err) => self.status = format!("Could not reload {}: {err}", name_of(&path)),
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
        // Not in the index is not the same as not on disk: the index is only
        // rebuilt on refresh, so a note that arrived since — synced in, pulled
        // in, written by another program — is missing from it. `create_new`
        // fails instead of truncating, which keeps the answer right even when
        // the file appears between this line and the one before it.
        match File::create_new(&path) {
            Ok(mut file) => match file.write_all(format!("# {}\n\n", name.trim()).as_bytes()) {
                Ok(()) => {
                    self.refresh();
                    self.open(path);
                    self.status = format!("Created {name}.md");
                }
                Err(err) => self.status = format!("Could not create {name}.md: {err}"),
            },
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                // It was there all along; take the index's word for nothing.
                self.refresh();
                self.open(path);
            }
            Err(err) => self.status = format!("Could not create {name}.md: {err}"),
        }
    }

    /// Point the window at a different vault and remember it for next launch.
    fn set_root(&mut self, root: PathBuf) {
        if root == self.root {
            return;
        }
        self.save_now();
        vault::save_root(&root);
        // Nothing survives the move: the open note, its buffer and the name
        // index all belong to the vault being left behind.
        *self = Self::new(root);
        self.status = format!("Vault: {}", self.root.display());
    }

    fn apply(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Open(path) => self.open(path),
            Cmd::Reload => {
                self.refresh();
                // The button says "reload from disk", so the open note is part
                // of what it reloads — the tree alone leaves the pane showing a
                // copy the file no longer matches.
                match self.open_path.clone() {
                    Some(path) if self.dirty => {
                        self.modal = Some(Modal {
                            kind: ModalKind::Reload,
                            target: path,
                            input: String::new(),
                            focused: false,
                        })
                    }
                    Some(_) => self.reload_open(),
                    None => {}
                }
            }
            Cmd::ChangeRoot => {
                // The dialog blocks the frame it is opened from, same as the
                // one on the first-run screen. There is nothing to draw
                // underneath it in the meantime.
                if let Some(root) = rfd::FileDialog::new()
                    .set_directory(&self.root)
                    .pick_folder()
                {
                    self.set_root(root);
                }
            }
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
            Cmd::Copy(path) => {
                self.status = format!("Copied “{}”", name_of(&path));
                self.clipboard = Some((path, ClipOp::Copy));
            }
            Cmd::Cut(path) => {
                self.status = format!("Cut “{}”", name_of(&path));
                self.clipboard = Some((path, ClipOp::Cut));
            }
            Cmd::Paste(folder) => self.paste_into(&folder),
        }
    }

    /// Drop the clipboard item into `folder`, copying or moving it depending on
    /// how it got there. The name is made unique rather than overwriting.
    fn paste_into(&mut self, folder: &Path) {
        let Some((src, op)) = self.clipboard.clone() else {
            return;
        };
        if !src.exists() {
            self.clipboard = None;
            self.status = format!("“{}” is no longer there", name_of(&src));
            return;
        }
        // A folder cannot contain itself, and copying one into its own subtree
        // would walk into the copy it is writing.
        if src.is_dir() && vault::is_within(&src, folder) {
            self.status = "Can't paste a folder into itself".to_owned();
            return;
        }
        if op == ClipOp::Cut && src.parent() == Some(folder) {
            self.clipboard = None;
            self.status = format!("“{}” is already there", name_of(&src));
            return;
        }

        let dest = vault::unique_dest(folder, &name_of(&src));
        let verb = match op {
            ClipOp::Copy => "copy",
            ClipOp::Cut => "move",
        };
        // The buffer belongs to a path that is about to move out from under it.
        self.save_now();
        let result = match op {
            ClipOp::Copy => vault::copy_recursive(&src, &dest),
            ClipOp::Cut => fs::rename(&src, &dest),
        };
        match result {
            Ok(()) => {
                if op == ClipOp::Cut {
                    // Autosave has to follow the note, whether it moved itself
                    // or sat inside the folder that did.
                    if let Some(open) = self.open_path.clone() {
                        if open == src {
                            self.open_path = Some(dest.clone());
                        } else if let Ok(rest) = open.strip_prefix(&src) {
                            self.open_path = Some(dest.join(rest));
                        }
                    }
                    self.clipboard = None;
                }
                self.refresh();
                let done = if op == ClipOp::Cut { "Moved" } else { "Copied" };
                self.status = format!("{done} to {}", self.folder_label(folder));
            }
            Err(err) => self.status = format!("Could not {verb} {}: {err}", name_of(&src)),
        }
    }

    /// A folder named the way the sidebar shows it: relative to the vault, or
    /// the vault's own name at the top.
    fn folder_label(&self, path: &Path) -> String {
        match path.strip_prefix(&self.root) {
            Ok(rel) if !rel.as_os_str().is_empty() => format!("{}/", rel.display()),
            _ => format!("{}/", self.tree.name),
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
                // `create_new` is the existence check as well as the create, so
                // there is no gap between the two for a note to appear in.
                match File::create_new(&path) {
                    Ok(_) => {
                        self.refresh();
                        self.open(path);
                    }
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                        self.status = format!("{file} already exists");
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
                                self.open_mtime = None;
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
            // "Keep my version": take the note as it now stands on disk to be
            // the copy the buffer is based on, which lets the write through,
            // and do it straight away rather than waiting for the next pause.
            ModalKind::Conflict => {
                self.open_mtime = mtime_of(&modal.target);
                self.save_now();
            }
            // "Keep my version" on a reload asked for by hand: there is nothing
            // to do but leave the buffer alone.
            ModalKind::Reload => {}
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let mut cmds: Vec<Cmd> = Vec::new();
        let tree = &self.tree;
        let selected = self.open_path.clone();
        let root = self.root.clone();
        let can_paste = self.clipboard.is_some();

        egui::Panel::left("sidebar")
            .resizable(true)
            .default_size(230.0)
            .size_range(150.0..=420.0)
            .show(ui, |ui| {
                ui.add_space(6.0);
                // Names the vault that is open and swaps it. The full path goes
                // in the tooltip, being far too long for the narrowest sidebar.
                let vault = ui
                    .add(egui::Button::new(format!("🗀  {}", tree.name)).truncate())
                    .on_hover_text(format!(
                        "{}\n\nClick to open a different folder.",
                        root.display()
                    ));
                if vault.clicked() {
                    cmds.push(Cmd::ChangeRoot);
                }
                // The vault folder has no row of its own in the tree, so its
                // "Paste" hangs off the header. With an empty clipboard there
                // is nothing to put in the menu, so none is attached.
                if can_paste {
                    vault.context_menu(|ui| {
                        if ui.button("Paste").clicked() {
                            cmds.push(Cmd::Paste(root.clone()));
                            ui.close();
                        }
                    });
                }
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
                        tree_ui(ui, tree, selected.as_deref(), can_paste, &mut cmds);
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
            ModalKind::Conflict => ("Changed on disk", ""),
            ModalKind::Reload => ("Unsaved edits", ""),
        };

        let mut confirm = false;
        let mut close = false;
        // The third answer the two-button modals do not have: keep the buffer
        // and write it, or give it up and take what is on disk.
        let mut reload = false;

        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                if matches!(modal.kind, ModalKind::Conflict | ModalKind::Reload) {
                    let name = name_of(&modal.target);
                    let (question, detail) = match modal.kind {
                        ModalKind::Conflict => (
                            format!("“{name}” changed on disk."),
                            "Your unsaved edits and the file no longer agree.",
                        ),
                        _ => (
                            format!("“{name}” has unsaved edits."),
                            "Reloading throws them away.",
                        ),
                    };
                    ui.label(question);
                    ui.label(egui::RichText::new(detail).weak());
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("Keep my version").clicked() {
                            confirm = true;
                        }
                        if ui.button("Load from disk").clicked() {
                            reload = true;
                        }
                    });
                } else if modal.kind == ModalKind::Delete {
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

                // Escape dismisses every modal but these two: leaving the
                // question unanswered would only have `save_now` ask it again
                // on the next frame, which reads as a modal that will not close.
                if !matches!(modal.kind, ModalKind::Conflict | ModalKind::Reload)
                    && ui.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    close = true;
                }
            });

        if reload {
            self.reload_open();
        } else if confirm {
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

/// A file's modification time, or `None` when it cannot be read — which is
/// also the answer for a file that is no longer there, so a note deleted
/// underneath the buffer reads as changed rather than as unchanged.
fn mtime_of(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|meta| meta.modified()).ok()
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn tree_ui(
    ui: &mut Ui,
    node: &Node,
    selected: Option<&Path>,
    can_paste: bool,
    cmds: &mut Vec<Cmd>,
) {
    for child in &node.children {
        if child.is_dir {
            let header = egui::CollapsingHeader::new(format!("🗀  {}", child.name))
                .id_salt(&child.path)
                .show(ui, |ui| tree_ui(ui, child, selected, can_paste, cmds));
            header.header_response.context_menu(|ui| {
                if ui.button("New note here").clicked() {
                    cmds.push(Cmd::NewNote(child.path.clone()));
                    ui.close();
                }
                if ui.button("New folder here").clicked() {
                    cmds.push(Cmd::NewFolder(child.path.clone()));
                    ui.close();
                }
                ui.separator();
                if ui.button("Copy").clicked() {
                    cmds.push(Cmd::Copy(child.path.clone()));
                    ui.close();
                }
                if ui.button("Cut").clicked() {
                    cmds.push(Cmd::Cut(child.path.clone()));
                    ui.close();
                }
                // Only folders take a paste, and only with something waiting.
                if can_paste && ui.button("Paste").clicked() {
                    cmds.push(Cmd::Paste(child.path.clone()));
                    ui.close();
                }
                ui.separator();
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
                if ui.button("Copy").clicked() {
                    cmds.push(Cmd::Copy(child.path.clone()));
                    ui.close();
                }
                if ui.button("Cut").clicked() {
                    cmds.push(Cmd::Cut(child.path.clone()));
                    ui.close();
                }
                ui.separator();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::tests::TempDir;

    /// Give the note a timestamp that is unmistakably later, whatever the
    /// filesystem's own resolution is — the point of these tests is the
    /// comparison, not how finely the clock happens to tick.
    fn touch_later(path: &Path) {
        let later = mtime_of(path).unwrap() + Duration::from_secs(10);
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(later)
            .unwrap();
    }

    /// Open `app` on a note holding `text`, and return its path.
    fn with_note(app: &mut App, dir: &TempDir, text: &str) -> PathBuf {
        let path = dir.0.join("note.md");
        fs::write(&path, text).unwrap();
        app.refresh();
        app.open(path.clone());
        path
    }

    /// The guarantee C2 is about: autosave fires on a timer, so it must not be
    /// the thing that decides a sync's version loses.
    #[test]
    fn an_external_change_is_not_overwritten() {
        let dir = TempDir::new("conflict");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");

        app.buffer = "mine".to_owned();
        app.dirty = true;

        fs::write(&path, "theirs").unwrap();
        touch_later(&path);

        app.save_now();

        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
        assert!(app.dirty, "the edits are still unsaved, not lost");
        assert!(
            matches!(app.modal.as_ref().map(|m| &m.kind), Some(ModalKind::Conflict)),
            "the question has to be put to someone"
        );
    }

    /// The buffer stays dirty while the question is unanswered, so the autosave
    /// timer calls `save_now` again on every later frame. None of those may
    /// write, and none may lose the buffer while waiting.
    #[test]
    fn an_unanswered_conflict_never_writes() {
        let dir = TempDir::new("pending");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");

        app.buffer = "mine".to_owned();
        app.dirty = true;
        fs::write(&path, "theirs").unwrap();
        touch_later(&path);

        for _ in 0..3 {
            app.save_now();
            // What `modal_ui` does with a modal nobody answered: draws it and
            // puts it back.
            let modal = app.modal.take().expect("still asking");
            assert!(matches!(modal.kind, ModalKind::Conflict));
            app.modal = Some(modal);
        }

        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
        assert_eq!(app.buffer, "mine");
        assert!(app.dirty);
    }

    #[test]
    fn keeping_your_version_writes_it() {
        let dir = TempDir::new("keep");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");

        app.buffer = "mine".to_owned();
        app.dirty = true;
        fs::write(&path, "theirs").unwrap();
        touch_later(&path);
        app.save_now();

        let modal = app.modal.take().expect("conflict raised");
        app.confirm_modal(&modal);

        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
        assert!(!app.dirty);
        assert!(app.modal.is_none(), "the write must not raise it again");
    }

    #[test]
    fn loading_from_disk_discards_the_buffer() {
        let dir = TempDir::new("discard");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");

        app.buffer = "mine".to_owned();
        app.dirty = true;
        fs::write(&path, "theirs").unwrap();
        touch_later(&path);

        app.reload_open();

        assert_eq!(app.buffer, "theirs");
        assert!(!app.dirty);

        // And the adopted timestamp lets the next edit save without asking.
        app.buffer = "mine again".to_owned();
        app.dirty = true;
        app.save_now();
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine again");
        assert!(app.modal.is_none());
    }

    /// The button says "reload from disk"; with nothing unsaved it should just
    /// do that, rather than only rebuilding the tree.
    #[test]
    fn reload_without_edits_picks_up_the_new_text() {
        let dir = TempDir::new("reload");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");
        assert_eq!(app.buffer, "original");

        fs::write(&path, "changed elsewhere").unwrap();
        touch_later(&path);

        app.apply(Cmd::Reload);

        assert_eq!(app.buffer, "changed elsewhere");
        assert!(app.modal.is_none(), "nothing to ask about with a clean buffer");
    }

    /// With unsaved edits the same button has to ask instead of discarding.
    #[test]
    fn reload_with_edits_asks_first() {
        let dir = TempDir::new("reload-dirty");
        let mut app = App::new(dir.0.clone());
        with_note(&mut app, &dir, "original");

        app.buffer = "mine".to_owned();
        app.dirty = true;
        app.apply(Cmd::Reload);

        assert_eq!(app.buffer, "mine", "the edits survive until answered");
        assert!(matches!(
            app.modal.as_ref().map(|m| &m.kind),
            Some(ModalKind::Reload)
        ));
    }

    /// Our own writes move the timestamp too, and must not look like someone
    /// else's the next time round.
    #[test]
    fn saving_repeatedly_never_raises_a_conflict() {
        let dir = TempDir::new("repeat");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");

        for n in 0..5 {
            app.buffer = format!("edit {n}");
            app.dirty = true;
            app.save_now();
            assert!(app.modal.is_none(), "own write mistaken for someone else's");
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "edit 4");
    }
}
