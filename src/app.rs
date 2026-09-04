//! The three-pane note window: tree, editor, preview.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::emath::GuiRounding as _;
use eframe::egui::{self, Ui};

use crate::markdown::{self, Action};
use crate::picker::{self, Picker};
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

/// What became of the buffer when `save_now` was asked to write it.
///
/// The two that are not `Done` are told apart because they end differently: the
/// question has an answer on screen and is worth waiting for, while a write that
/// failed has only said so in the status line, and holding the window hostage to
/// a full disk would leave no way out of it.
#[derive(Debug, PartialEq)]
enum Save {
    /// Written, or there was nothing to write.
    Done,
    /// The note changed underneath the buffer, and the question is now on
    /// screen. Nothing should move until it is answered.
    Asked,
    /// The write itself failed, and said why in the status line.
    Failed,
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

impl Modal {
    /// A question with an empty text box, not yet focused. "Rename" is the one
    /// that seeds the box, and does it with `..Modal::new(..)`.
    fn new(kind: ModalKind, target: PathBuf) -> Self {
        Self {
            kind,
            target,
            input: String::new(),
            focused: false,
        }
    }
}

pub struct App {
    root: PathBuf,
    tree: Node,
    /// Lowercased note name -> path, rebuilt on every refresh.
    index: HashMap<String, PathBuf>,
    open_path: Option<PathBuf>,
    buffer: String,
    /// The open note as it last appeared on disk. Comparing the contents rather
    /// than only metadata also catches writers that preserve timestamps.
    open_disk: Option<String>,
    dirty: bool,
    last_edit: Instant,
    modal: Option<Modal>,
    /// The one item “Copy” or “Cut” put aside, if any. Internal to the window:
    /// nothing is handed to the system clipboard.
    clipboard: Option<(PathBuf, ClipOp)>,
    /// The folder picker, while one is open. Held apart from `modal` because it
    /// carries where it has been browsed to.
    picker: Option<Picker>,
    status: String,
}

impl App {
    pub fn new(root: PathBuf) -> Self {
        // Scanned once, not twice: `refresh` would do both halves of this, but
        // it starts by walking the vault again, and on a large one that is the
        // slower half of opening the window.
        let tree = vault::scan(&root);
        let mut index = HashMap::new();
        vault::build_index(&tree, &mut index);

        Self {
            root,
            tree,
            index,
            open_path: None,
            buffer: String::new(),
            open_disk: None,
            dirty: false,
            last_edit: Instant::now(),
            modal: None,
            clipboard: None,
            picker: None,
            status: String::new(),
        }
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
    ///
    /// "Changed" means the text on disk differs from the copy this window last
    /// saw there, compared immediately before the write. The gap between the
    /// two is not closed, and a write landing inside it is still overwritten.
    /// Shutting it would take a lock the filesystem does not owe us, and it is
    /// microseconds wide against a sync that arrives whenever it arrives.
    ///
    /// What came of it is returned because most callers ask for this on the way
    /// to somewhere else — another note, another vault, a rename — and a buffer
    /// that was not written must not be left behind. Only `Save::Done` says it
    /// is safe to move on.
    #[must_use]
    fn save_now(&mut self) -> Save {
        if !self.dirty {
            return Save::Done;
        }
        let Some(path) = self.open_path.clone() else {
            self.dirty = false;
            return Save::Done;
        };
        // An unanswered question leaves the buffer dirty, so the autosave timer
        // arrives back here on every frame while the modal is up. Recognise the
        // question already standing over this note rather than reading the note
        // back to derive it again: what to do is being waited on, not decided,
        // and deciding it again would also mean a whole file read per frame.
        if let Some(modal) = &self.modal
            && matches!(modal.kind, ModalKind::Conflict)
            && modal.target == path
        {
            return Save::Asked;
        }
        if fs::read_to_string(&path).ok() != self.open_disk {
            // Ask once — and only when nothing else is being asked, so a
            // conflict cannot take the screen from a question already on it.
            if self.modal.is_none() {
                self.modal = Some(Modal::new(ModalKind::Conflict, path));
            }
            return Save::Asked;
        }
        match vault::write_atomic(&path, &self.buffer) {
            Ok(()) => {
                self.dirty = false;
                // Our own write is now the version later edits are based on.
                self.open_disk = Some(self.buffer.clone());
                self.status = format!("Saved {}", name_of(&path));
                Save::Done
            }
            Err(err) => {
                self.status = format!("Could not save {}: {err}", name_of(&path));
                Save::Failed
            }
        }
    }

    fn open(&mut self, path: PathBuf) {
        // The buffer is about to be replaced, so it has to be on disk first.
        // It used to be written "if it could be" and replaced either way, which
        // meant that clicking another note while the open one was in conflict
        // threw the edits away and left the question standing over a note that
        // was no longer open — where answering it did nothing at all.
        if self.save_now() != Save::Done {
            return;
        }
        match fs::read_to_string(&path) {
            Ok(text) => {
                self.open_disk = Some(text.clone());
                self.buffer = text;
                self.open_path = Some(path);
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
        match fs::read_to_string(&path) {
            Ok(text) => {
                self.open_disk = Some(text.clone());
                self.buffer = text;
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
        // Everything below replaces the window's whole state, so an unwritten
        // buffer would go with it — and unlike `open`, without even leaving the
        // question behind.
        if self.save_now() != Save::Done {
            return;
        }
        vault::save_root(&root);
        // Nothing survives the move: the open note, its buffer and the name
        // index all belong to the vault being left behind.
        *self = Self::new(root);
        self.status = format!("Vault: {}", self.root.display());
    }

    /// Follow the open note when `from` becomes `to` — as the note itself, or
    /// as something inside the folder that moved.
    ///
    /// Autosave writes to `open_path` on a timer, so a path left pointing at
    /// the old name would either fail or, once the note is renamed back over
    /// it, write the buffer to a note nobody is looking at.
    fn follow_move(&mut self, from: &Path, to: &Path) {
        let Some(open) = self.open_path.clone() else {
            return;
        };
        if open == from {
            self.open_path = Some(to.to_path_buf());
        } else if let Ok(rest) = open.strip_prefix(from) {
            self.open_path = Some(to.join(rest));
        }
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
                        self.modal = Some(Modal::new(ModalKind::Reload, path))
                    }
                    Some(_) => self.reload_open(),
                    None => {}
                }
            }
            // Opened here rather than answered here: the picker is drawn over
            // the window for as many frames as the browsing takes, where the
            // dialog it replaced blocked the frame it was opened from.
            Cmd::ChangeRoot => self.picker = Some(Picker::new(Some(&self.root))),
            Cmd::NewNote(parent) => self.modal = Some(Modal::new(ModalKind::NewNote, parent)),
            Cmd::NewFolder(parent) => self.modal = Some(Modal::new(ModalKind::NewFolder, parent)),
            Cmd::Rename(path) => {
                let current = if path.is_dir() {
                    name_of(&path)
                } else {
                    // The same stem the sidebar shows for this row.
                    vault::note_stem(&name_of(&path)).to_owned()
                };
                self.modal = Some(Modal {
                    input: current,
                    ..Modal::new(ModalKind::Rename, path)
                })
            }
            Cmd::Delete(path) => self.modal = Some(Modal::new(ModalKind::Delete, path)),
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
        if self.save_now() != Save::Done {
            return;
        }
        let result = match op {
            ClipOp::Copy => vault::copy_recursive(&src, &dest),
            ClipOp::Cut => fs::rename(&src, &dest),
        };
        match result {
            Ok(()) => {
                if op == ClipOp::Cut {
                    self.follow_move(&src, &dest);
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
                // Typing the extension is allowed, in any case; leaving it off
                // is the usual thing and gets it added.
                let file = if vault::note_stem(&name) == name {
                    format!("{name}.md")
                } else {
                    name.clone()
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
                // `create_dir`, not `create_dir_all`: the latter succeeds when
                // the folder is already there, so "New folder" on a name that
                // exists looked like it had made one. `valid_name` rules out
                // separators, so there is never a parent left to create.
                match fs::create_dir(&path) {
                    Ok(()) => self.refresh(),
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                        self.status = format!("{name} already exists");
                    }
                    Err(err) => self.status = format!("Could not create {name}: {err}"),
                }
            }
            ModalKind::Rename => {
                let is_dir = modal.target.is_dir();
                let old = name_of(&modal.target);
                let file = if is_dir {
                    name.clone()
                } else {
                    // The box was seeded with the stem, so put back exactly the
                    // extension it was seeded without, spelled the way it was.
                    // Renaming edits the name; it does not decide what kind of
                    // file this is, and anything less than the exact inverse
                    // drops a suffix on the way back — which is how
                    // "notes.md.md" used to become "notes.md" by being
                    // confirmed unchanged.
                    format!("{name}{}", &old[vault::note_stem(&old).len()..])
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
                if self.save_now() != Save::Done {
                    return;
                }
                match fs::rename(&modal.target, &dest) {
                    Ok(()) => {
                        self.follow_move(&modal.target, &dest);
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
                        // The open note went with it, either as the target or
                        // as something inside the folder that was the target.
                        if let Some(open) = self.open_path.clone()
                            && (open == modal.target || open.starts_with(&modal.target))
                        {
                            self.open_path = None;
                            self.open_disk = None;
                            self.buffer.clear();
                            self.dirty = false;
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
                self.open_disk = fs::read_to_string(&modal.target).ok();
                // This is the answer to the question, not another asking of it.
                let _ = self.save_now();
            }
            // "Keep my version" on a reload asked for by hand: there is nothing
            // to do but leave the buffer alone.
            ModalKind::Reload => {}
        }
    }

    fn sidebar(&mut self, ui: &mut Ui) {
        let mut cmds: Vec<Cmd> = Vec::new();
        // Borrowed, not cloned: the panel closure only reads them, and this
        // runs on every frame the window draws.
        let tree = &self.tree;
        let selected = self.open_path.as_deref();
        let root = self.root.as_path();
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
                            cmds.push(Cmd::Paste(root.to_path_buf()));
                            ui.close();
                        }
                    });
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("New note").clicked() {
                        cmds.push(Cmd::NewNote(root.to_path_buf()));
                    }
                    if ui.button("New folder").clicked() {
                        cmds.push(Cmd::NewFolder(root.to_path_buf()));
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
                        tree_ui(ui, tree, selected, can_paste, &mut cmds);
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

    /// Draw the folder picker, if one is open, and act on what it comes to.
    fn picker_ui(&mut self, ctx: &egui::Context) {
        let Some(mut picker) = self.picker.take() else {
            return;
        };

        let mut outcome = picker::Outcome::Browsing;
        let response = egui::Modal::new(egui::Id::new("bluejay_picker")).show(ctx, |ui| {
            ui.set_min_width(380.0);
            ui.label(
                egui::RichText::new("Open a different folder")
                    .family(egui::FontFamily::Name(crate::PREVIEW_SANS_BOLD.into()))
                    .size(15.0),
            );
            ui.add_space(10.0);
            outcome = picker.ui(ui, true);
        });

        match outcome {
            // `set_root` is what refuses the move while the buffer is unwritten,
            // and raises the question that says why.
            picker::Outcome::Chosen(root) => self.set_root(root),
            picker::Outcome::Cancelled => {}
            // Escape and a click on the backdrop dismiss it too. Nothing has
            // been changed yet, so there is nothing to ask about first.
            picker::Outcome::Browsing if response.should_close() => {}
            picker::Outcome::Browsing => self.picker = Some(picker),
        }
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

        // A real modal, not a window: it draws a backdrop that swallows clicks,
        // so the tree behind cannot be used to start a second operation — which
        // with a `Window` replaced the pending one and lost the question.
        let response = egui::Modal::new(egui::Id::new("bluejay_modal")).show(ctx, |ui| {
            ui.set_min_width(280.0);
            // A modal carries no title bar of its own.
            ui.label(
                egui::RichText::new(title)
                    .family(egui::FontFamily::Name(crate::PREVIEW_SANS_BOLD.into()))
                    .size(15.0),
            );
            ui.add_space(10.0);
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
        });

        // Escape, or a click on the backdrop, dismisses every modal but these
        // two: leaving the question unanswered would only have `save_now` ask
        // it again on the next frame, which reads as a modal that will not
        // close.
        if !matches!(modal.kind, ModalKind::Conflict | ModalKind::Reload) && response.should_close()
        {
            close = true;
        }

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
        // The modal's backdrop stops the pointer but not a shortcut, and while
        // one is open the answer to what should happen to the buffer is exactly
        // what is being asked.
        if self.modal.is_none() && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S))
        {
            // Nothing follows this, so whatever came of it — written, asked
            // about, or refused — is already on screen.
            let _ = self.save_now();
        }

        self.sidebar(ui);
        self.preview(ui);

        let status = self.status.as_str();
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(status).weak().size(12.0));
            });
        });

        self.editor(ui);
        self.modal_ui(&ctx);
        self.picker_ui(&ctx);

        // Autosave once editing has been quiet for a moment.
        if self.dirty {
            let idle = self.last_edit.elapsed();
            if idle >= AUTOSAVE_DELAY {
                let _ = self.save_now();
            } else {
                ctx.request_repaint_after(AUTOSAVE_DELAY - idle);
            }
        }

        // The last chance to write, and the one place a raised question has to
        // hold the window open: closing over it would take the buffer and the
        // question both. A write that merely failed is let through — a window
        // that cannot be closed while a disk is full is worse than the loss.
        if ctx.input(|i| i.viewport().close_requested()) && self.save_now() == Save::Asked {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
    }
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Height of one note row, worked out the way `selectable_label` works its own
/// out, so a row that is skipped reserves exactly what drawing it would have
/// taken. `note_rows_are_the_height_we_reserve` holds the two together.
fn row_height(ui: &Ui) -> f32 {
    // The row a galley is laid out on is rounded to whole pixels, so the text
    // has to be rounded the same way before the padding is added — a fraction
    // of a pixel per row is enough to drift a long vault out of place.
    let text = ui
        .text_style_height(&egui::TextStyle::Body)
        .round_to_pixels(ui.pixels_per_point());
    (text + ui.spacing().button_padding.y * 2.0).max(ui.spacing().interact_size.y)
}

fn tree_ui(
    ui: &mut Ui,
    node: &Node,
    selected: Option<&Path>,
    can_paste: bool,
    cmds: &mut Vec<Cmd>,
) {
    // Rows outside the scrolled viewport are skipped rather than built. Every
    // note row is the same height, so their space can be reserved without
    // laying anything out — and in a vault whose folder holds thousands of
    // notes, all but the ~30 on screen are outside it. Folders are left alone:
    // there are few of them, and how tall an open one is cannot be known
    // without walking what is inside it.
    let row = row_height(ui);
    let visible = ui.clip_rect();

    for child in &node.children {
        if !child.is_dir {
            let top = ui.cursor().top();
            if top + row < visible.top() || top > visible.bottom() {
                // `allocate_space`, not `add_space`: only the former puts the
                // item spacing between rows that a drawn row would have, and
                // without it the reserved column comes up short of the real
                // one by a few pixels per row.
                ui.allocate_space(egui::vec2(ui.available_width(), row));
                continue;
            }
        }
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
                // Only a folder is offered a paste, and only with something
                // waiting on the clipboard.
                entry_menu(ui, &child.path, can_paste, cmds);
            });
        } else {
            let is_selected = selected == Some(child.path.as_path());
            let response = ui.selectable_label(is_selected, child.stem());
            if response.clicked() {
                cmds.push(Cmd::Open(child.path.clone()));
            }
            response.context_menu(|ui| entry_menu(ui, &child.path, false, cmds));
        }
    }
}

/// What every row in the tree offers: put it on the clipboard, rename it,
/// remove it. A folder takes a "Paste" among them and two items of its own
/// above, and that is the whole of the difference between the two menus.
fn entry_menu(ui: &mut Ui, path: &Path, can_paste: bool, cmds: &mut Vec<Cmd>) {
    if ui.button("Copy").clicked() {
        cmds.push(Cmd::Copy(path.to_path_buf()));
        ui.close();
    }
    if ui.button("Cut").clicked() {
        cmds.push(Cmd::Cut(path.to_path_buf()));
        ui.close();
    }
    if can_paste && ui.button("Paste").clicked() {
        cmds.push(Cmd::Paste(path.to_path_buf()));
        ui.close();
    }
    ui.separator();
    if ui.button("Rename").clicked() {
        cmds.push(Cmd::Rename(path.to_path_buf()));
        ui.close();
    }
    if ui.button("Delete").clicked() {
        cmds.push(Cmd::Delete(path.to_path_buf()));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::tests::TempDir;

    /// Move the note's timestamp unmistakably forward, whatever the
    /// filesystem's own resolution is. One test wants this now: what gets
    /// compared is the text, so a timestamp on its own is precisely the change
    /// that must not raise anything.
    fn touch_later(path: &Path) {
        let later = fs::metadata(path).unwrap().modified().unwrap() + Duration::from_secs(10);
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

        assert_eq!(app.save_now(), Save::Asked);

        assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
        assert!(app.dirty, "the edits are still unsaved, not lost");
        assert!(
            matches!(app.modal.as_ref().map(|m| &m.kind), Some(ModalKind::Conflict)),
            "the question has to be put to someone"
        );
    }

    /// Some synchronisers and restore tools deliberately preserve a file's
    /// timestamp. Metadata-only conflict checks cannot see their edits and used
    /// to let autosave overwrite them without asking.
    #[test]
    fn an_external_change_with_the_same_timestamp_is_not_overwritten() {
        let dir = TempDir::new("conflict-preserved-time");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");
        let original_time = fs::metadata(&path).unwrap().modified().unwrap();

        app.buffer = "mine".to_owned();
        app.dirty = true;

        fs::write(&path, "external").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(original_time)
            .unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), original_time);

        assert_eq!(app.save_now(), Save::Asked);
        assert_eq!(fs::read_to_string(&path).unwrap(), "external");
        assert!(app.dirty, "the edits are still unsaved, not lost");
    }

    /// The other half of the same rule. A timestamp is not what is compared any
    /// more, so a note whose mtime moved but whose text did not is nobody's
    /// edit — an archiver reading the vault, a backup touching what it copied —
    /// and putting a question on screen for it would be crying wolf.
    #[test]
    fn a_changed_timestamp_alone_is_not_a_conflict() {
        let dir = TempDir::new("conflict-touched-only");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");

        app.buffer = "mine".to_owned();
        app.dirty = true;

        touch_later(&path);

        assert_eq!(app.save_now(), Save::Done);
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
        assert!(app.modal.is_none(), "there was nothing to ask about");
    }

    /// A question already on screen is waited on, not decided again — even when
    /// the disk goes back to the copy the buffer was based on. Deciding it again
    /// used to let the write through underneath a modal still asking about it,
    /// leaving an answer that then did nothing; it is also what had `save_now`
    /// reading the whole note back on every frame the question was up.
    #[test]
    fn a_pending_conflict_is_not_withdrawn_by_the_disk() {
        let dir = TempDir::new("pending-reverted");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");

        app.buffer = "mine".to_owned();
        app.dirty = true;
        fs::write(&path, "theirs").unwrap();
        assert_eq!(app.save_now(), Save::Asked);

        // Whoever wrote "theirs" puts the note back the way it was.
        fs::write(&path, "original").unwrap();

        assert_eq!(app.save_now(), Save::Asked);
        assert_eq!(fs::read_to_string(&path).unwrap(), "original");
        assert!(app.dirty, "the buffer waits for the answer, it is not written");
        assert!(
            matches!(app.modal.as_ref().map(|m| &m.kind), Some(ModalKind::Conflict)),
            "and the question is still the one being answered"
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

        for _ in 0..3 {
            assert_eq!(app.save_now(), Save::Asked);
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
        assert_eq!(app.save_now(), Save::Asked);

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

        app.reload_open();

        assert_eq!(app.buffer, "theirs");
        assert!(!app.dirty);

        // And the adopted copy lets the next edit save without asking.
        app.buffer = "mine again".to_owned();
        app.dirty = true;
        assert_eq!(app.save_now(), Save::Done);
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine again");
        assert!(app.modal.is_none());
    }

    /// The edits are what the conflict question is there to protect, so nothing
    /// may walk off with them while it is still on screen. Clicking another
    /// note used to: the buffer was replaced, `dirty` cleared, and the question
    /// left standing over a note that was no longer open — where answering it
    /// did nothing at all.
    #[test]
    fn a_pending_conflict_holds_the_open_note() {
        let dir = TempDir::new("conflict_open");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");
        let other = dir.0.join("other.md");
        fs::write(&other, "another note").unwrap();
        app.refresh();

        app.buffer = "mine".to_owned();
        app.dirty = true;
        fs::write(&path, "theirs").unwrap();

        app.apply(Cmd::Open(other.clone()));

        assert_eq!(app.buffer, "mine", "the edits are still here");
        assert!(app.dirty);
        assert_eq!(app.open_path.as_deref(), Some(path.as_path()));
        assert!(
            matches!(app.modal.as_ref().map(|m| &m.kind), Some(ModalKind::Conflict)),
            "and the question is about the note they belong to"
        );

        // Answering it lets the same click through.
        let modal = app.modal.take().expect("conflict raised");
        app.confirm_modal(&modal);
        app.apply(Cmd::Open(other.clone()));
        assert_eq!(app.open_path.as_deref(), Some(other.as_path()));
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
    }

    /// The same rule where there is not even a question left behind: switching
    /// vaults replaces the whole window, modal and all.
    #[test]
    fn a_pending_conflict_holds_the_vault() {
        let dir = TempDir::new("conflict_vault");
        let elsewhere = TempDir::new("conflict_vault_other");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");

        app.buffer = "mine".to_owned();
        app.dirty = true;
        fs::write(&path, "theirs").unwrap();

        app.set_root(elsewhere.0.clone());

        assert_eq!(app.root, dir.0, "the vault cannot change out from under it");
        assert_eq!(app.buffer, "mine");
        assert!(app.dirty);
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

    /// The rename box is seeded with the stem the sidebar shows, so pressing OK
    /// unchanged is a no-op rather than a quiet loss of a suffix.
    #[test]
    fn renaming_offers_the_name_the_sidebar_shows() {
        let dir = TempDir::new("rename-seed");
        let mut app = App::new(dir.0.clone());
        for (file, want) in [
            ("plain.md", "plain"),
            ("notes.md.md", "notes.md"),
            ("Upper.MD", "Upper"),
            ("attachment.txt", "attachment.txt"),
        ] {
            let path = dir.0.join(file);
            fs::write(&path, "").unwrap();
            app.apply(Cmd::Rename(path));
            assert_eq!(app.modal.take().expect("modal").input, want, "{file}");
        }
    }

    /// Confirming that box unchanged must leave the file exactly where it was.
    #[test]
    fn renaming_to_the_same_name_changes_nothing() {
        let dir = TempDir::new("rename-noop");
        let mut app = App::new(dir.0.clone());
        let path = dir.0.join("notes.md.md");
        fs::write(&path, "body").unwrap();

        app.apply(Cmd::Rename(path.clone()));
        let modal = app.modal.take().expect("modal");
        app.confirm_modal(&modal);

        assert!(path.is_file(), "the note must still be notes.md.md");
        assert_eq!(fs::read_to_string(&path).unwrap(), "body");
    }

    /// The open note has to follow the folder it sits in when that folder is
    /// renamed. Autosave writes to `open_path` on a timer, so a path left
    /// behind would keep writing to a name nothing answers to.
    #[test]
    fn the_open_note_follows_its_renamed_folder() {
        let dir = TempDir::new("follow-rename");
        let mut app = App::new(dir.0.clone());
        let folder = dir.0.join("Ideas");
        fs::create_dir(&folder).unwrap();
        let note = folder.join("note.md");
        fs::write(&note, "body").unwrap();
        app.refresh();
        app.open(note);

        app.apply(Cmd::Rename(folder));
        let mut modal = app.modal.take().expect("modal");
        modal.input = "Plans".to_owned();
        app.confirm_modal(&modal);

        let moved = dir.0.join("Plans").join("note.md");
        assert_eq!(app.open_path.as_deref(), Some(moved.as_path()));

        // And the next save lands on the note where it now is.
        app.buffer = "edited".to_owned();
        app.dirty = true;
        assert_eq!(app.save_now(), Save::Done);
        assert_eq!(fs::read_to_string(&moved).unwrap(), "edited");
        assert!(app.modal.is_none(), "a move of our own is not a conflict");
    }

    /// The same rule by the other route: cut the note and paste it elsewhere.
    #[test]
    fn the_open_note_follows_a_cut_and_paste() {
        let dir = TempDir::new("follow-paste");
        let mut app = App::new(dir.0.clone());
        let note = dir.0.join("note.md");
        fs::write(&note, "body").unwrap();
        let archive = dir.0.join("Archive");
        fs::create_dir(&archive).unwrap();
        app.refresh();
        app.open(note.clone());

        app.apply(Cmd::Cut(note.clone()));
        app.apply(Cmd::Paste(archive.clone()));

        let moved = archive.join("note.md");
        assert!(!note.exists(), "the note was moved, not copied");
        assert_eq!(app.open_path.as_deref(), Some(moved.as_path()));

        app.buffer = "edited".to_owned();
        app.dirty = true;
        assert_eq!(app.save_now(), Save::Done);
        assert_eq!(fs::read_to_string(&moved).unwrap(), "edited");
    }

    /// `create_dir_all` succeeded on a folder that was already there, so the
    /// window reported nothing and looked like it had made one.
    #[test]
    fn making_a_folder_that_exists_says_so() {
        let dir = TempDir::new("newfolder");
        let mut app = App::new(dir.0.clone());
        fs::create_dir(dir.0.join("Ideas")).unwrap();

        let modal = Modal {
            kind: ModalKind::NewFolder,
            target: dir.0.clone(),
            input: "Ideas".to_owned(),
            focused: false,
        };
        app.confirm_modal(&modal);

        assert!(app.status.contains("already exists"), "status: {}", app.status);
    }

    fn test_ctx() -> egui::Context {
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        // The real style, not egui's: row height is worked out from the button
        // padding, so the culling tests below have to measure under the padding
        // the sidebar actually draws with.
        crate::theme::apply_style(&ctx);
        ctx
    }

    fn raw_input(w: f32, h: f32) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(w, h),
            )),
            ..Default::default()
        }
    }

    /// Skipping a row reserves `row_height`, so that number has to be what
    /// drawing the row would actually have taken. If egui ever works a
    /// selectable label's height out differently, the reserved space drifts
    /// from the drawn space and the sidebar's scrolling goes wrong.
    #[test]
    fn note_rows_are_the_height_we_reserve() {
        let ctx = test_ctx();
        let mut drawn = 0.0;
        let mut reserved = 0.0;
        ctx.run_ui(raw_input(400.0, 600.0), |ui| {
            reserved = row_height(ui);
            drawn = ui.selectable_label(false, "an ordinary note").rect.height();
        })
        .drop_without_applying_deltas();

        assert!(drawn > 0.0);
        assert!(
            (drawn - reserved).abs() < 0.01,
            "drawn {drawn} vs reserved {reserved}"
        );
    }

    /// Culling must not change how tall the tree is, or the scrollbar would
    /// claim the vault has fewer notes than it does.
    #[test]
    fn skipped_rows_still_take_up_their_space() {
        let dir = TempDir::new("cull");
        let notes = 300;
        for n in 0..notes {
            fs::write(dir.0.join(format!("note {n:04}.md")), "").unwrap();
        }
        let tree = vault::scan(&dir.0);
        assert_eq!(tree.children.len(), notes);

        let ctx = test_ctx();
        let mut tall = 0.0;
        let mut short = 0.0;
        let mut row = 0.0;

        // A viewport tall enough for everything: nothing is skipped.
        ctx.run_ui(raw_input(400.0, 40_000.0), |ui| {
            row = row_height(ui);
            let mut cmds = Vec::new();
            tree_ui(ui, &tree, None, false, &mut cmds);
            tall = ui.min_rect().height();
        })
        .drop_without_applying_deltas();

        // A short one inside a scroll area: almost every row is skipped.
        ctx.run_ui(raw_input(400.0, 300.0), |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    let mut cmds = Vec::new();
                    tree_ui(ui, &tree, None, false, &mut cmds);
                    short = ui.min_rect().height();
                });
        })
        .drop_without_applying_deltas();

        assert!(tall > row * notes as f32, "sanity: {tall} for {notes} rows");
        assert!(
            (tall - short).abs() < 1.0,
            "skipped rows must reserve what drawn rows take: {tall} vs {short}"
        );
    }

    /// Our own writes change what is on disk too, and must not look like
    /// someone else's the next time round.
    #[test]
    fn saving_repeatedly_never_raises_a_conflict() {
        let dir = TempDir::new("repeat");
        let mut app = App::new(dir.0.clone());
        let path = with_note(&mut app, &dir, "original");

        for n in 0..5 {
            app.buffer = format!("edit {n}");
            app.dirty = true;
            assert_eq!(app.save_now(), Save::Done);
            assert!(app.modal.is_none(), "own write mistaken for someone else's");
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), "edit 4");
    }
}
