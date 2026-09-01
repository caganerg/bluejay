//! Filesystem side of things: the notes tree, the name index, and the tiny
//! config file remembering which folder is the vault.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

/// One entry in the sidebar tree. Directories carry their children; files don't.
pub struct Node {
    pub path: PathBuf,
    pub name: String,
    pub is_dir: bool,
    pub children: Vec<Node>,
}

impl Node {
    /// File name without the `.md` extension, which is what we show and what
    /// `[[wiki links]]` are matched against.
    pub fn stem(&self) -> &str {
        if self.is_dir {
            &self.name
        } else {
            note_stem(&self.name)
        }
    }
}

/// `name` without its `.md` extension, whatever case that was written in.
///
/// The one place the rule lives: the sidebar labels rows with it, the rename
/// box is seeded from it, and `unique_dest` splits names on it, and all three
/// have to agree on what counts as the extension.
///
/// Split at the dot rather than a fixed distance from the end — that distance
/// can land inside a multi-byte character, where slicing panics. And
/// `strip_suffix`, in effect, not `trim_end_matches`: the latter takes the
/// suffix off as many times as it appears, turning "notes.md.md" into "notes"
/// and quietly dropping one on the way back.
pub fn note_stem(name: &str) -> &str {
    match name.rfind('.') {
        Some(dot) if name[dot..].eq_ignore_ascii_case(".md") => &name[..dot],
        _ => name,
    }
}

/// Whether the tree should show this file.
///
/// The extension is matched without regard to case: a note another program
/// wrote as `.MD` is a note, and matching exactly made it invisible here —
/// missing from the sidebar and from the wiki-link index both.
fn is_note(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
}

/// Recursively read `root`, keeping only directories and `.md` files and
/// skipping anything hidden or symlinked. Unreadable directories come back
/// empty.
///
/// Symlinks are left out rather than followed. One pointing back at an ancestor
/// makes the walk revisit the same folders over and over, and while the kernel
/// stops any single path at its symlink limit, a folder holding more than one
/// such link multiplies at every level: three of them is enough to turn a
/// three-directory vault into millions of nodes, on the thread drawing the
/// window. `file_type` reports the link itself, so it also saves the extra
/// `stat` per entry that `Path::is_dir` costs.
pub fn scan(root: &Path) -> Node {
    let mut node = Node {
        name: root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| root.to_string_lossy().into_owned()),
        path: root.to_path_buf(),
        is_dir: true,
        children: Vec::new(),
    };

    let Ok(entries) = fs::read_dir(root) else {
        return node;
    };

    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            node.children.push(scan(&path));
        } else if file_type.is_file() && is_note(&path) {
            node.children.push(Node {
                path,
                name,
                is_dir: false,
                children: Vec::new(),
            });
        }
    }

    node.children.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| lowered(&a.name).cmp(lowered(&b.name)))
    });
    node
}

/// A name lower-cased as it is read, for ordering the sidebar.
///
/// `to_lowercase` would answer the same, but it answers with a new `String`,
/// and this is asked on every comparison of every sort — of which there is one
/// per folder after every rename, paste and new note.
fn lowered(name: &str) -> impl Iterator<Item = char> + '_ {
    name.chars().flat_map(char::to_lowercase)
}

/// Map lowercased file stem -> path, for resolving `[[Note Name]]` anywhere in
/// the vault.
///
/// Walked a level at a time rather than one branch at a time, so that the
/// nearest of two notes with the same name wins: depth-first indexed everything
/// inside the first folder before the notes sitting beside it, which made
/// `[[Foo]]` open a copy buried in a subfolder while one at the top of the
/// vault went unseen. Within a level the tree is already sorted, so which of
/// two equally near notes wins is at least stable.
pub fn build_index(node: &Node, index: &mut HashMap<String, PathBuf>) {
    let mut level = vec![node];
    while !level.is_empty() {
        let mut below = Vec::new();
        for node in level {
            for child in &node.children {
                if child.is_dir {
                    below.push(child);
                } else {
                    index
                        .entry(child.stem().to_lowercase())
                        .or_insert_with(|| child.path.clone());
                }
            }
        }
        level = below;
    }
}

/// Where the vault path is remembered.
fn config_file() -> Option<PathBuf> {
    let base = config_dir(std::env::var_os("XDG_CONFIG_HOME"), std::env::home_dir())?;
    Some(base.join("bluejay").join("vault.txt"))
}

/// `$XDG_CONFIG_HOME`, or `~/.config` when it is unset — the one rule the
/// config file's location follows, taken apart from the environment so it can
/// be tested without writing to it.
///
/// A relative `XDG_CONFIG_HOME` is ignored rather than resolved, as the base
/// directory specification asks: resolving it would put the config wherever the
/// app happened to be started from, and the next launch from another directory
/// would not find it.
fn config_dir(xdg: Option<std::ffi::OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg.map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.map(|home| home.join(".config")))
}

/// The last used vault path, if it still exists.
///
/// Only the line ending is trimmed. A general `trim` also eats the spaces a
/// folder name is allowed to end with, and the vault would then silently fail
/// to load and send the user back to the first-run picker.
pub fn load_root() -> Option<PathBuf> {
    let path = decode_root(&fs::read(config_file()?).ok()?);
    (path.is_dir()).then_some(path)
}

/// The path a config file's bytes name: its first line, taken as it is.
fn decode_root(bytes: &[u8]) -> PathBuf {
    let line = bytes.split(|b| *b == b'\n').next().unwrap_or(bytes);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    PathBuf::from(OsStr::from_bytes(line))
}

/// Remember `root` for the next launch.
///
/// Written as raw bytes rather than through `to_string_lossy`, which replaces
/// anything that is not UTF-8 and hands back a path that no longer names the
/// folder it came from.
pub fn save_root(root: &Path) {
    if let Some(file) = config_file() {
        if let Some(parent) = file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(file, root.as_os_str().as_bytes());
    }
}

/// Write `contents` to `path` without ever leaving a half-written note there.
///
/// `fs::write` truncates before it writes, so a crash or a full disk between
/// the two leaves the note cut short — and autosave reopens that window every
/// time typing pauses, rather than only when someone reaches for Ctrl+S. A
/// temporary file beside the note, renamed over the top once it is complete,
/// makes the swap atomic instead: readers see the old note or the new one and
/// nothing in between.
///
/// The temporary name starts with a dot so `scan` skips it in the one case it
/// outlives the write — the process dying between the two steps.
pub fn write_atomic(path: &Path, contents: &str) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(path.file_name().unwrap_or_default());
    tmp_name.push(".bluejay-tmp");
    let tmp = parent.join(tmp_name);

    let written = File::create(&tmp).and_then(|mut file| {
        file.write_all(contents.as_bytes())?;
        // Renaming only orders the directory entry. Without this the contents
        // can still be in flight, and a crash leaves the note's name pointing
        // at an empty file — losing it more thoroughly than a torn write would.
        file.sync_all()
    });

    // The rename replaces the note with the scratch file, permissions and all,
    // and a fresh file only carries whatever the umask allows. Without this a
    // note the user had made private would come back readable by everyone.
    if let Ok(meta) = fs::metadata(path) {
        let _ = fs::set_permissions(&tmp, meta.permissions());
    }

    match written.and_then(|()| fs::rename(&tmp, path)) {
        Ok(()) => Ok(()),
        Err(err) => {
            // The note on disk is untouched; only the scratch file is not.
            let _ = fs::remove_file(&tmp);
            Err(err)
        }
    }
}

/// Reject names that would escape the folder they are created in, and names
/// the tree could not show afterwards.
///
/// The leading dot is refused because `scan` skips hidden entries: a note
/// created under such a name would vanish from the sidebar the moment it was
/// written, and a `[[wiki link]]` naming one would find nothing in the index no
/// matter how many times it was followed.
pub fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && !name.starts_with('.')
        && !name.contains('/')
        && !name.contains('\\')
        && !name.chars().any(char::is_control)
}

/// A free path for `name` inside `dir`, appending " (2)", " (3)" … before the
/// `.md` extension until nothing is in the way. Pasting never overwrites.
pub fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    // Folders are the only other thing the tree shows, and they carry no
    // extension worth preserving. Slicing at the stem keeps the extension
    // spelled the way it arrived, so a ".MD" file does not gain a ".md" twin.
    let stem = note_stem(name);
    let ext = &name[stem.len()..];
    let mut n: u32 = 2;
    loop {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

/// Copy a file, or a directory and everything under it, to `dest`, which must
/// not exist yet. Unlike `scan`, this takes the folder as it really is: hidden
/// files and non-markdown attachments come along.
///
/// Symlinks are the one thing left behind, for two reasons. A link back to an
/// ancestor would make this walk the copy it is writing, filling the disk
/// rather than merely hanging as `scan` does; and `fs::copy` reads through a
/// link, so one pointing outside the vault would quietly materialise a real
/// copy of whatever it names inside it.
pub fn copy_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    // `symlink_metadata` describes the link, never its target.
    if fs::symlink_metadata(src)?.is_dir() {
        fs::create_dir(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            if entry.file_type()?.is_symlink() {
                continue;
            }
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dest).map(|_| ())
    }
}

/// Whether `folder` is `dir` itself or sits somewhere under it.
///
/// `Path::starts_with` compares components, so it only answers correctly while
/// both paths are spelled the way the tree walked them; a symlinked route to
/// the same folder passes it. Resolving both first closes that, and the lexical
/// answer is kept for the case where resolving is not possible — a path that
/// does not exist cannot be canonicalised, and refusing the paste is the safer
/// reading of an unreadable one.
pub fn is_within(dir: &Path, folder: &Path) -> bool {
    match (fs::canonicalize(dir), fs::canonicalize(folder)) {
        (Ok(dir), Ok(folder)) => folder.starts_with(dir),
        _ => folder.starts_with(dir),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    /// A scratch directory that removes itself, so the walk tests can build a
    /// real tree — the thing they are about cannot be faked with paths alone.
    /// Shared with `app`'s tests, which need one for the same reason.
    pub(crate) struct TempDir(pub(crate) PathBuf);

    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "bluejay-test-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).expect("scratch dir");
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn count(node: &Node) -> usize {
        1 + node.children.iter().map(count).sum::<usize>()
    }

    /// Two notes of the same name: `[[Foo]]` has to find the one nearest the
    /// top of the vault, not whichever branch the walk happened to enter first.
    #[test]
    fn the_nearest_note_of_a_name_wins() {
        let dir = TempDir::new("index_depth");
        fs::create_dir(dir.0.join("Archive")).unwrap();
        fs::create_dir(dir.0.join("Archive").join("Older")).unwrap();
        fs::write(dir.0.join("Archive").join("Older").join("Foo.md"), "deep").unwrap();
        fs::write(dir.0.join("Archive").join("Bar.md"), "one down").unwrap();
        fs::write(dir.0.join("Foo.md"), "at the top").unwrap();

        let mut index = HashMap::new();
        build_index(&scan(&dir.0), &mut index);

        assert_eq!(index.get("foo"), Some(&dir.0.join("Foo.md")));
        // And a name that only exists deeper is still found.
        assert_eq!(
            index.get("bar"),
            Some(&dir.0.join("Archive").join("Bar.md"))
        );
    }

    /// Three links back to an ancestor in one folder used to multiply at every
    /// level until the walk ran out of time, on the thread drawing the window.
    /// The kernel's own symlink limit does not help: it bounds the depth of a
    /// single path, not how many paths there are.
    #[test]
    fn a_symlink_loop_does_not_explode() {
        let dir = TempDir::new("loop");
        let sub = dir.0.join("notes").join("sub");
        fs::create_dir_all(&sub).unwrap();
        fs::write(dir.0.join("notes").join("a.md"), "# a").unwrap();
        for link in ["l1", "l2", "l3"] {
            symlink("..", sub.join(link)).unwrap();
        }

        let tree = scan(&dir.0.join("notes"));
        // notes + sub + a.md, and nothing the links point back at.
        assert_eq!(count(&tree), 3, "symlinks must not be walked");
    }

    /// `fs::copy` reads through a link, so following one would put a real copy
    /// of whatever it names inside the vault.
    #[test]
    fn copying_leaves_symlinks_behind() {
        let dir = TempDir::new("copy");
        let src = dir.0.join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("real.md"), "# real").unwrap();
        fs::write(dir.0.join("outside.md"), "secret").unwrap();
        symlink(dir.0.join("outside.md"), src.join("link.md")).unwrap();

        let dest = dir.0.join("dest");
        copy_recursive(&src, &dest).unwrap();

        assert!(dest.join("real.md").is_file());
        assert!(
            !dest.join("link.md").exists(),
            "a link out of the vault must not be materialised inside it"
        );
    }

    #[test]
    fn is_within_sees_through_a_symlinked_route() {
        let dir = TempDir::new("within");
        let notes = dir.0.join("notes");
        fs::create_dir_all(notes.join("inner")).unwrap();
        symlink(&notes, dir.0.join("link")).unwrap();

        let lexical = dir.0.join("link").join("inner");
        assert!(!lexical.starts_with(&notes), "the trap this guards against");
        assert!(is_within(&notes, &lexical), "the same folder either way");
        assert!(is_within(&notes, &notes));
        assert!(!is_within(&notes, &dir.0));
    }

    #[test]
    fn writes_and_leaves_no_scratch_file_behind() {
        let dir = TempDir::new("atomic");
        let note = dir.0.join("note.md");

        write_atomic(&note, "# first").unwrap();
        assert_eq!(fs::read_to_string(&note).unwrap(), "# first");

        // Overwriting is the case autosave actually exercises.
        write_atomic(&note, "# second, longer than the first").unwrap();
        assert_eq!(
            fs::read_to_string(&note).unwrap(),
            "# second, longer than the first"
        );

        let left: Vec<_> = fs::read_dir(&dir.0)
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert_eq!(left, ["note.md"], "the scratch file must be gone");
    }

    /// The mode a note carries is the user's business; a rename would otherwise
    /// hand it whatever the umask gave the scratch file.
    #[test]
    fn keeps_the_notes_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new("perms");
        let note = dir.0.join("private.md");
        write_atomic(&note, "secret").unwrap();
        fs::set_permissions(&note, fs::Permissions::from_mode(0o600)).unwrap();

        write_atomic(&note, "still secret").unwrap();

        let mode = fs::metadata(&note).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "permissions must survive the rename");
    }

    /// Should the process die between the write and the rename, whatever is
    /// left has to stay out of the sidebar rather than showing up as a note.
    #[test]
    fn a_leftover_scratch_file_stays_out_of_the_tree() {
        let dir = TempDir::new("scratch");
        fs::write(dir.0.join("real.md"), "# real").unwrap();
        fs::write(dir.0.join(".note.md.bluejay-tmp"), "half written").unwrap();

        let tree = scan(&dir.0);
        let names: Vec<&str> = tree.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["real.md"]);
    }

    #[test]
    fn shows_notes_whatever_case_the_extension_is() {
        let dir = TempDir::new("case");
        fs::write(dir.0.join("lower.md"), "").unwrap();
        fs::write(dir.0.join("Upper.MD"), "").unwrap();
        fs::write(dir.0.join("mixed.Md"), "").unwrap();
        fs::write(dir.0.join("notes.txt"), "").unwrap();

        let tree = scan(&dir.0);
        let mut stems: Vec<&str> = tree.children.iter().map(Node::stem).collect();
        stems.sort_unstable();
        assert_eq!(stems, ["Upper", "lower", "mixed"]);
    }

    /// The stem is cut at the dot, not at a fixed distance from the end, which
    /// would slice through a multi-byte character.
    #[test]
    fn stems_multibyte_names_without_panicking() {
        for (name, want) in [
            ("çğü.md", "çğü"),
            ("çğü", "çğü"),
            ("é", "é"),
            ("éé", "éé"),
            ("notes.md.md", "notes.md"),
        ] {
            let node = Node {
                path: PathBuf::from(name),
                name: name.to_owned(),
                is_dir: false,
                children: Vec::new(),
            };
            assert_eq!(node.stem(), want, "{name:?}");
        }
    }

    /// A folder name may end in a space, and `trim` used to eat it — the vault
    /// then failed to load and the first-run picker came back with no
    /// explanation.
    #[test]
    fn keeps_a_vault_path_exactly_as_written() {
        for path in ["/home/x/notes", "/home/x/notes ", "/home/x/ notes ", "/tmp/a b"] {
            let encoded = PathBuf::from(path);
            assert_eq!(decode_root(encoded.as_os_str().as_bytes()), encoded);
        }
    }

    #[test]
    fn reads_only_the_first_line_of_the_config() {
        assert_eq!(decode_root(b"/home/x/notes\n"), PathBuf::from("/home/x/notes"));
        assert_eq!(decode_root(b"/home/x/notes\r\n"), PathBuf::from("/home/x/notes"));
        assert_eq!(decode_root(b"/home/x/notes\nstray"), PathBuf::from("/home/x/notes"));
    }

    #[test]
    fn puts_the_config_where_xdg_says() {
        let home = Some(PathBuf::from("/home/x"));
        let xdg = |s: &str| Some(std::ffi::OsString::from(s));

        // An absolute XDG_CONFIG_HOME is the config base as it stands.
        assert_eq!(
            config_dir(xdg("/tmp/cfg"), home.clone()),
            Some(PathBuf::from("/tmp/cfg"))
        );
        // Unset, empty or relative all fall back to ~/.config; resolving a
        // relative one would tie the config to the working directory.
        for value in [None, xdg(""), xdg("cfg"), xdg("./cfg")] {
            assert_eq!(
                config_dir(value, home.clone()),
                Some(PathBuf::from("/home/x/.config"))
            );
        }
        // Without a home there is nowhere to fall back to.
        assert_eq!(config_dir(None, None), None);
    }

    /// Paths are bytes on Unix. `to_string_lossy` would swap the invalid ones
    /// for replacement characters and hand back a path naming nothing.
    #[test]
    fn survives_a_path_that_is_not_utf8() {
        let raw = b"/home/x/no\xffte";
        let decoded = decode_root(raw);
        assert_eq!(decoded.as_os_str().as_bytes(), raw);
        assert!(decoded.to_str().is_none(), "the test case has to be invalid UTF-8");
    }

    #[test]
    fn accepts_ordinary_names() {
        for name in ["Note", "a note.md", "Ödev 2", "notes.md.md", "  padded  "] {
            assert!(valid_name(name), "{name:?} should be usable");
        }
    }

    #[test]
    fn rejects_names_that_escape_their_folder() {
        for name in ["", "   ", ".", "..", "a/b", "a\\b", "/etc/passwd", "../../etc"] {
            assert!(!valid_name(name), "{name:?} should be refused");
        }
    }

    /// `scan` skips hidden entries, so a note created under one of these names
    /// would be written and then never appear again.
    #[test]
    fn rejects_hidden_names() {
        for name in [".gizli", ".config", "..hidden", "  .leading"] {
            assert!(!valid_name(name), "{name:?} should be refused");
        }
    }

    /// A newline in a name is legal on disk but breaks every row that shows it.
    #[test]
    fn rejects_control_characters() {
        assert!(!valid_name("a\nb"));
        assert!(!valid_name("a\tb"));
        assert!(!valid_name("a\0b"));
    }
}
