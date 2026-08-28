//! Filesystem side of things: the notes tree, the name index, and the tiny
//! config file remembering which folder is the vault.

use std::collections::HashMap;
use std::fs;
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
            self.name.strip_suffix(".md").unwrap_or(&self.name)
        }
    }
}

/// Recursively read `root`, keeping only directories and `.md` files and
/// skipping anything hidden. Unreadable directories come back empty.
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
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = path.is_dir();
        if is_dir {
            node.children.push(scan(&path));
        } else if path.extension().is_some_and(|e| e == "md") {
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
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    node
}

/// Map lowercased file stem -> path, for resolving `[[Note Name]]` anywhere in
/// the vault. First match wins, and the tree is sorted, so it is stable.
pub fn build_index(node: &Node, index: &mut HashMap<String, PathBuf>) {
    for child in &node.children {
        if child.is_dir {
            build_index(child, index);
        } else {
            index
                .entry(child.stem().to_lowercase())
                .or_insert_with(|| child.path.clone());
        }
    }
}

fn config_file() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("bluejay").join("vault.txt"))
}

/// The last used vault path, if it still exists.
pub fn load_root() -> Option<PathBuf> {
    let text = fs::read_to_string(config_file()?).ok()?;
    let path = PathBuf::from(text.trim());
    (path.is_dir()).then_some(path)
}

pub fn save_root(root: &Path) {
    if let Some(file) = config_file() {
        if let Some(parent) = file.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(file, root.to_string_lossy().as_bytes());
    }
}

/// Reject names that would escape the folder they are created in.
pub fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && name != "."
        && name != ".."
}

/// A free path for `name` inside `dir`, appending " (2)", " (3)" … before the
/// `.md` extension until nothing is in the way. Pasting never overwrites.
pub fn unique_dest(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    // Folders are the only other thing the tree shows, and they carry no
    // extension worth preserving.
    let (stem, ext) = match name.strip_suffix(".md") {
        Some(stem) => (stem, ".md"),
        None => (name, ""),
    };
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
pub fn copy_recursive(src: &Path, dest: &Path) -> std::io::Result<()> {
    if src.is_dir() {
        fs::create_dir(dest)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_recursive(&entry.path(), &dest.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dest).map(|_| ())
    }
}
