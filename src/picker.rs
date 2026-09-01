//! bluejay's own folder picker.
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
//! folder is the whole of it, which is all that picking a notes folder needs.

use std::fs;
use std::path::{Path, PathBuf};

use eframe::egui::{self, Ui};

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
        };
        picker.reread();
        picker
    }

    /// Walk into `dir` and read what is inside it.
    fn go(&mut self, dir: PathBuf) {
        self.dir = dir;
        self.reread();
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
        ui.horizontal(|ui| {
            if cancellable && ui.button("Cancel").clicked() {
                outcome = Outcome::Cancelled;
            }
            if ui.button("Choose this folder").clicked() {
                outcome = Outcome::Chosen(self.dir.clone());
            }
        });

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
}
