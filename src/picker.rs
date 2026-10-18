//! Bluejay's own folder picker.
//!
//! Choosing a vault used to open a GTK dialog — the one window in the app that
//! the app did not draw. It came up in someone else's toolkit, wearing the
//! desktop's theme rather than this one, lettered in whatever fontconfig
//! handed it rather than the typeface carried in the binary; and it cost a link
//! against GTK, Pango, Cairo, fontconfig and FreeType for a window that opens
//! twice in a vault's life. So it is drawn here instead, out of the same
//! widgets, palette and font as everything else.
//!
//! What is given up with it is a real file manager's conveniences: there is no
//! typing a path, no bookmarks, no recent places. Walking down from a starting
//! folder — and making one when the folder wanted is not there yet — is the
//! whole of it, which is all that picking a notes folder needs.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Ui};

use crate::vault;

/// How tall the list of folders is allowed to grow before it scrolls.
const LIST_HEIGHT: f32 = 260.0;

/// One folder that can be walked into.
struct Entry {
    path: PathBuf,
    name: String,
}

/// What a frame of the picker came to.
pub enum Outcome {
    /// Still browsing; nothing has been decided.
    Browsing,
    /// This folder was chosen.
    Chosen(PathBuf),
    /// Dismissed without choosing.
    Cancelled,
}

pub struct Picker {
    dir: PathBuf,
    /// The folders inside `dir`, read when it changes rather than every frame:
    /// this is drawn at 60 fps and a directory read is not free.
    children: Vec<Entry>,
    /// Why `dir` could not be read, when it could not. Kept rather than shown
    /// as an empty list, which would read as "there is nothing here".
    error: Option<String>,
    /// The name typed into the row that makes a folder, while that row is up;
    /// `None` when it is not. Held as the text itself rather than a flag plus a
    /// string, so there is no state in which a row is up with nothing in it.
    creating: Option<String>,
    /// Whether the name box has been handed the keyboard yet. Only the frame it
    /// appears on needs asking; asking every frame would take focus back off
    /// anything the user moved to.
    focused: bool,
    /// What the last press of Create came to, shown under the box until the
    /// name is typed at again.
    create_error: Option<String>,
}

impl Picker {
    /// Open at `start` if it is a folder, otherwise at the home directory, and
    /// at the filesystem root if there is not even one of those.
    pub fn new(start: Option<&Path>) -> Self {
        let dir = start
            .filter(|path| path.is_dir())
            .map(Path::to_path_buf)
            .or_else(|| std::env::home_dir().filter(|home| home.is_dir()))
            .unwrap_or_else(|| PathBuf::from("/"));

        let mut picker = Self {
            dir,
            children: Vec::new(),
            error: None,
            creating: None,
            focused: false,
            create_error: None,
        };
        picker.reread();
        picker
    }

    /// Walk into `dir` and read what is inside it.
    ///
    /// Anything half-typed into the row that makes a folder is dropped: the
    /// name belonged to the folder being left behind.
    fn go(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.creating = None;
        self.create_error = None;
        self.reread();
    }

    /// Make a folder called `name` inside the one being browsed and walk into
    /// it, so that choosing it — the next thing wanted — is one press away.
    ///
    /// Why it could not be made comes back as something to put under the name
    /// box. A folder already at that name is refused rather than reused: it is
    /// the one the user is looking at, with whatever is in it.
    fn create_folder(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if !vault::valid_name(name) {
            return Err("Names cannot be empty, hidden or contain a slash".to_owned());
        }
        let path = self.dir.join(name);
        // `create_dir`, not `create_dir_all`: the latter succeeds on a name
        // that is already taken, and "already there" has to be said.
        fs::create_dir(&path).map_err(|err| match err.kind() {
            io::ErrorKind::AlreadyExists => format!("{name} is already here"),
            _ => format!("Could not create {name}: {err}"),
        })?;
        self.go(path);
        Ok(())
    }

    fn reread(&mut self) {
        self.children.clear();
        match read_folders(&self.dir) {
            Ok(children) => {
                self.children = children;
                self.error = None;
            }
            Err(err) => self.error = Some(format!("Could not read this folder: {err}")),
        }
    }

    /// Draw a frame of the picker. `cancellable` adds the button that dismisses
    /// it, which the first-run screen has no use for — there is nothing behind
    /// it to go back to.
    pub fn ui(&mut self, ui: &mut Ui, cancellable: bool) -> Outcome {
        let mut outcome = Outcome::Browsing;
        let mut walk_into = None;

        // The path can outrun the narrowest dialog, so it truncates and keeps
        // the whole of itself on hover.
        ui.add(egui::Label::new(egui::RichText::new(self.dir.to_string_lossy()).weak()).truncate())
            .on_hover_text(self.dir.to_string_lossy());
        ui.add_space(6.0);

        let parent = self.dir.parent().map(Path::to_path_buf);
        ui.add_enabled_ui(parent.is_some(), |ui| {
            if ui.selectable_label(false, "↑  ..").clicked() {
                walk_into = parent;
            }
        });

        egui::ScrollArea::vertical()
            .id_salt("picker")
            .max_height(LIST_HEIGHT)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if let Some(error) = &self.error {
                    ui.label(egui::RichText::new(error).weak());
                } else if self.children.is_empty() {
                    ui.label(egui::RichText::new("No folders in here.").weak());
                }
                for child in &self.children {
                    if ui
                        .selectable_label(false, format!("🗀  {}", child.name))
                        .clicked()
                    {
                        walk_into = Some(child.path.clone());
                    }
                }
            });

        ui.add_space(8.0);
        // The row that makes a folder stands in place of the buttons rather than
        // above them: with a name half typed, "Choose this folder" is not what
        // the next press means.
        if let Some(mut name) = self.creating.take() {
            // Taken out of `self` and put back, so the buttons can be drawn and
            // acted on without holding a borrow of it across them.
            let mut create = false;
            let mut cancel = false;

            ui.horizontal(|ui| {
                ui.label("New folder");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .hint_text("Name")
                        .desired_width(150.0),
                );
                if !self.focused {
                    response.request_focus();
                    self.focused = true;
                }
                if response.changed() {
                    self.create_error = None;
                }
                let entered =
                    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.button("Create").clicked() || entered {
                    create = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });

            if let Some(error) = &self.create_error {
                ui.label(egui::RichText::new(error).weak());
            }

            if create {
                if let Err(error) = self.create_folder(&name) {
                    // The name goes back on screen to be fixed, with the box
                    // focused again, rather than being thrown away with the row.
                    self.create_error = Some(error);
                    self.creating = Some(name);
                    self.focused = false;
                }
            } else if !cancel {
                self.creating = Some(name);
            }
        } else {
            ui.horizontal(|ui| {
                if ui.button("New folder").clicked() {
                    self.creating = Some(String::new());
                    self.create_error = None;
                    self.focused = false;
                }
                if cancellable && ui.button("Cancel").clicked() {
                    outcome = Outcome::Cancelled;
                }
                if ui.button("Choose this folder").clicked() {
                    outcome = Outcome::Chosen(self.dir.clone());
                }
            });
        }

        if let Some(dir) = walk_into {
            self.go(dir);
        }
        outcome
    }
}

/// The folders directly inside `dir`, in the order the sidebar would show them.
///
/// Hidden folders are skipped, as `vault::scan` skips them: a vault under one
/// would come up empty in the tree. Symlinks, however, are followed here, where
/// `scan` refuses them — a link is a fair way to reach a notes folder, and the
/// walk that made following them dangerous is not this one. It is a single
/// directory, read once per step, so the `stat` behind `is_dir` is affordable.
fn read_folders(dir: &Path) -> std::io::Result<Vec<Entry>> {
    let mut folders: Vec<Entry> = fs::read_dir(dir)?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let path = entry.path();
            path.is_dir().then_some(Entry { path, name })
        })
        .collect();

    // Once per step into a folder, not once per frame, so the allocation the
    // sidebar's sort goes out of its way to avoid is not worth avoiding here.
    folders.sort_by_key(|entry| entry.name.to_lowercase());
    Ok(folders)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::tests::TempDir;

    fn names(picker: &Picker) -> Vec<&str> {
        picker.children.iter().map(|e| e.name.as_str()).collect()
    }

    /// Only folders are offered, hidden ones are not, and they come out in the
    /// order the sidebar would have shown them.
    #[test]
    fn lists_the_folders_and_nothing_else() {
        let dir = TempDir::new("picker-list");
        for folder in ["Notes", "archive", ".git", "Zebra"] {
            fs::create_dir(dir.0.join(folder)).unwrap();
        }
        fs::write(dir.0.join("a-note.md"), "").unwrap();
        fs::write(dir.0.join("readme.txt"), "").unwrap();

        let picker = Picker::new(Some(&dir.0));
        assert_eq!(names(&picker), ["archive", "Notes", "Zebra"]);
    }

    /// Walking down and back up again lands where it started.
    #[test]
    fn walks_into_a_folder_and_back_out() {
        let dir = TempDir::new("picker-walk");
        let inner = dir.0.join("Notes");
        fs::create_dir_all(inner.join("Archive")).unwrap();

        let mut picker = Picker::new(Some(&dir.0));
        picker.go(inner.clone());
        assert_eq!(picker.dir, inner);
        assert_eq!(names(&picker), ["Archive"]);

        picker.go(picker.dir.parent().unwrap().to_path_buf());
        assert_eq!(picker.dir, dir.0);
        assert_eq!(names(&picker), ["Notes"]);
    }

    /// The root has no parent, which is what disables the button that climbs.
    #[test]
    fn the_filesystem_root_has_nowhere_above_it() {
        let picker = Picker::new(Some(Path::new("/")));
        assert_eq!(picker.dir, Path::new("/"));
        assert!(picker.dir.parent().is_none(), "nothing is above the root");
    }

    /// A folder that is not there must not silently look like an empty one.
    #[test]
    fn a_folder_that_cannot_be_read_says_so() {
        let dir = TempDir::new("picker-missing");
        let mut picker = Picker::new(Some(&dir.0));
        assert!(picker.error.is_none());

        picker.go(dir.0.join("not-here"));

        assert!(picker.children.is_empty());
        assert!(
            picker.error.is_some(),
            "an unreadable folder has to explain itself"
        );
    }

    /// A starting point that is not a folder falls back rather than opening on
    /// nothing — the remembered vault may have been deleted since.
    #[test]
    fn falls_back_when_the_starting_folder_is_gone() {
        let picker = Picker::new(Some(Path::new("/nonexistent/vault")));
        assert!(picker.dir.is_dir(), "it has to open somewhere real");
    }

    /// Making the folder the picked one is the point of the row: it is created
    /// and it is walked into, so choosing it is the next press and nothing else.
    #[test]
    fn makes_a_folder_and_walks_into_it() {
        let dir = TempDir::new("picker-new");
        let mut picker = Picker::new(Some(&dir.0));

        assert_eq!(picker.create_folder("  Notes  "), Ok(()));

        assert_eq!(picker.dir, dir.0.join("Notes"), "the name is trimmed");
        assert!(picker.dir.is_dir(), "and the folder is really there");
    }

    /// A name that cannot be a folder — one the tree would then hide or refuse
    /// to make — is turned away without moving.
    #[test]
    fn a_name_that_cannot_be_a_folder_is_refused() {
        let dir = TempDir::new("picker-new-bad");
        let mut picker = Picker::new(Some(&dir.0));

        for name in ["", "   ", ".hidden", "a/b", ".."] {
            assert!(picker.create_folder(name).is_err(), "{name:?}");
        }

        assert_eq!(picker.dir, dir.0, "nothing was walked into");
        assert_eq!(fs::read_dir(&dir.0).unwrap().count(), 0, "and nothing made");
    }

    /// A folder already at that name is the one being looked at, with whatever
    /// is in it, and must not be taken over or walked into.
    #[test]
    fn a_folder_that_is_already_there_is_left_alone() {
        let dir = TempDir::new("picker-new-exists");
        fs::create_dir(dir.0.join("Notes")).unwrap();
        fs::write(dir.0.join("Notes").join("keep.md"), "body").unwrap();
        let mut picker = Picker::new(Some(&dir.0));

        let error = picker.create_folder("Notes").unwrap_err();

        assert!(error.contains("already"), "it has to say so: {error}");
        assert_eq!(picker.dir, dir.0);
        assert_eq!(
            fs::read_to_string(dir.0.join("Notes").join("keep.md")).unwrap(),
            "body",
            "what was in it is untouched"
        );
    }

    /// Both states of the bottom row lay out without panicking — the plain one
    /// with its three buttons, and the one that makes a folder, which is drawn
    /// with a text box egui has to lay out and hand the keyboard to. A panicking
    /// layout, or an id drawn twice, is what this is here to catch.
    #[test]
    fn draws_with_the_name_box_shut_and_open() {
        let dir = TempDir::new("picker-draw");
        fs::create_dir(dir.0.join("Notes")).unwrap();
        let ctx = egui::Context::default();
        crate::install_fonts(&ctx);
        crate::theme::apply_style(&ctx);
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 600.0),
            )),
            ..Default::default()
        };

        let mut picker = Picker::new(Some(&dir.0));
        let mut outcomes = Vec::new();
        ctx.run_ui(input(), |ui| outcomes.push(picker.ui(ui, true)))
            .drop_without_applying_deltas();

        picker.creating = Some("Notes".to_owned());
        ctx.run_ui(input(), |ui| outcomes.push(picker.ui(ui, true)))
            .drop_without_applying_deltas();

        assert!(
            outcomes
                .iter()
                .all(|outcome| matches!(outcome, Outcome::Browsing)),
            "no button was pressed, so nothing was decided"
        );
    }
}
