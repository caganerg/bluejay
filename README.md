# bluejay

A small native desktop markdown note app in Rust: a file tree, a plain text
editor, and a live preview, side by side. No Electron, no webview, no
JavaScript — just `eframe`/`egui` drawing directly.

```
┌────────────┬──────────────────────┬──────────────────────┐
│ note tree  │ raw markdown (edit)  │ rendered preview     │
└────────────┴──────────────────────┴──────────────────────┘
```

## Requirements

bluejay is **Wayland-only**. The X11 backend is compiled out, not merely
disabled, so a running Wayland compositor is the minimum requirement: there is
no X11 or XWayland fallback, and no runtime detection of one. Started without a
compositor to connect to, it exits with an error rather than opening a window.

- **a running Wayland compositor** (Sway, GNOME, KDE Plasma, Hyprland, …)
- a Rust toolchain
- GTK 3 development files, used by the folder picker on first launch
  (`gtk3-devel` / `libgtk-3-dev`)

## Build and run

```sh
cargo run            # debug
cargo run --release  # noticeably smoother
cargo test           # markdown renderer tests
```

On first launch bluejay asks for the folder your notes live in. The choice is
remembered in `~/.config/bluejay/vault.txt` (delete that file to pick a
different folder).

## What it does

- **Tree** — mirrors the folder structure on disk, showing directories and
  `.md` files (hidden files are skipped). Click a folder to expand it, a note to
  open it. The toolbar creates notes and folders at the vault root; right-click
  any entry for *New note here*, *New folder here*, *Rename* and *Delete*.
  Deleting always asks first, and deleting a folder removes its contents.
- **Editor** — the raw markdown, monospace and plain. No syntax highlighting.
- **Preview** — re-rendered from the editor buffer every frame, so it tracks
  typing immediately. Headings, bold/italic/strikethrough, inline code, fenced
  code blocks, ordered and unordered (and nested) lists, task lists, block
  quotes, horizontal rules and links. External links open in your browser.

## Wiki links

Write `[[Note Name]]` anywhere in a note. In the preview it renders as a
clickable link.

- Clicking it opens the note whose filename (minus `.md`) matches the link text,
  case-insensitively, searched **recursively across the whole vault**. If two
  notes share a name, the first in sorted order wins.
- If no note matches, clicking creates `Note Name.md` **at the vault root**
  (seeded with a `# Note Name` heading) and opens it.

That is the whole linking feature: no backlinks, no graph, no aliases.

## Saving

**Autosave**, chosen because it is the simpler of the two options — there is no
dirty-state bookkeeping to get wrong and nothing to forget. The open note is
written to disk 600 ms after you stop typing, and also when you switch notes,
rename, or close the window. `Ctrl+S` forces a write immediately if you want the
reassurance. A `•` next to the filename means there are unwritten edits; the
status bar at the bottom reports saves and any filesystem errors.

## Layout of the code

| File | Contents |
| --- | --- |
| `src/main.rs` | window setup, dark theme, first-run folder picker |
| `src/app.rs` | the three panes, sidebar commands, modals, autosave |
| `src/markdown.rs` | `pulldown-cmark` events → egui widgets, wiki-link scanning |
| `src/vault.rs` | directory scan, note-name index, config file |

Five dependencies: `eframe`, `pulldown-cmark`, `rfd`, `dirs`, and a direct
`winit` pin that exists only to select Wayland features — see below.

## Windowing backend

`eframe`'s default features enable X11 and Wayland together, so defaults are off
and the wanted features are re-listed without `x11`. Two details are less
obvious than they look:

- **The direct `winit` dependency is never used from code.** eframe's `wayland`
  feature only reaches `winit/wayland`, and dropping `winit/default` would take
  `wayland-csd-adwaita` (client-side window decorations, needed for a titlebar
  on compositors that expect the client to draw one) and `wayland-dlopen`
  (loading libwayland at runtime instead of link time) with it. Cargo unions
  features across the graph, so naming winit directly restores those two without
  bringing `x11` back.
- **`accesskit` is off, and that is what actually removes X11.**
  `accesskit_winit`'s own default feature hard-enables `winit/x11`, and
  `egui-winit` depends on it without `default-features = false`, so enabling
  eframe's `accesskit` feature compiles the X11 backend back in no matter what
  else is set. Turning it off costs AccessKit screen-reader support and removes
  64 crates from the tree.

`x11rb` and `x11rb-protocol` remain in the dependency tree despite all of this.
They come from `arboard`, which depends on `x11rb` unconditionally on Unix for
its X11 clipboard backend, and eframe hardcodes egui-winit's `clipboard` feature
instead of exposing it — so they cannot be removed by any feature flag without
patching eframe and giving up copy/paste. They are inert here: nothing calls
into that backend under Wayland, where `smithay-clipboard` is used instead.
