# bluejay

A small native desktop markdown note app in Rust: a file tree, a plain text
editor, and a live preview, side by side. No Electron, no webview, no
JavaScript — just `eframe`/`egui` drawing directly.

It works entirely offline. Notes are plain `.md` files in a folder you pick, and
nothing here reaches for the network: there is no account, no sync, no update
check and no telemetry, and no HTTP or TLS crate anywhere in the dependency
tree. Nothing a note contains is fetched either — a `[text](url)` link is drawn
but never opened, and an image is drawn as its link rather than downloaded.

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
- Rust 1.95 or newer

Nothing else. The built binary links `libc`, `libgcc_s` and `libm` and no other
shared library; Wayland itself is opened by name at runtime. There is no GTK, no
Pango, no Cairo, no fontconfig and no FreeType, because every window bluejay
shows is one it draws itself out of its own typefaces.

## Build and run

```sh
cargo run            # debug
cargo run --release  # noticeably smoother
cargo test           # markdown renderer tests
```

On first launch bluejay asks for the folder your notes live in. The choice is
remembered in `~/.config/bluejay/vault.txt` (delete that file to pick a
different folder).

## Desktop entry and icon

The logo lives at `assets/logo.png` (512×512) and is baked into the binary as
the window icon. On Wayland that alone shows nothing: there is no window-icon
protocol winit speaks, so `set_window_icon` is a silent no-op and the icon a
taskbar or app switcher draws comes from a desktop entry matched against the
window's **app id**, which bluejay sets to `bluejay`. Install both to get the
logo out of the binary and onto your bar:

```sh
install -Dm755 target/release/bluejay   ~/.local/bin/bluejay
install -Dm644 assets/logo.png          ~/.local/share/icons/hicolor/512x512/apps/bluejay.png
install -Dm644 assets/bluejay.desktop   ~/.local/share/applications/bluejay.desktop
gtk-update-icon-cache -f ~/.local/share/icons/hicolor 2>/dev/null || true
update-desktop-database ~/.local/share/applications 2>/dev/null || true
```

The `Icon=bluejay` line in the entry is what points at the installed PNG, and
`StartupWMClass=bluejay` is what ties a running window back to the entry. Rename
either the app id or the desktop file and the window goes back to a generic
icon.

## What it does

- **Tree** — mirrors the folder structure on disk, showing directories and
  `.md` files — the extension in any case — while hidden files and symlinks are
  skipped. Click a folder to expand it, a note to open it. The toolbar creates
  notes and folders at the vault root; right-click any entry for *New note
  here*, *New folder here*, *Copy*, *Cut*, *Rename* and *Delete*. Deleting
  always asks first, and deleting a folder removes its contents. *Paste* shows
  up on folders (and on the vault button at the top) once something has been
  copied or cut; a name already taken at the destination gains a “ (2)” rather
  than being overwritten.
- **Editor** — the raw markdown, monospace and plain. No syntax highlighting.
- **Preview** — re-rendered from the editor buffer every frame, so it tracks
  typing immediately. Headings, bold/italic/strikethrough, inline code, fenced
  code blocks, ordered and unordered (and nested) lists, task lists, block
  quotes, horizontal rules and links. A `[text](url)` link is shown in grey with
  its destination on hover, but nothing opens when you click it — see below.

## What a note is not allowed to do

A note is a file like any other — synced in, cloned, downloaded — so the preview
does not assume the person reading one wrote it.

- **Nothing opens a URL.** Opening one meant handing it to the system, which on
  Linux means `xdg-open` starting whichever application had registered the
  scheme — `file:`, `smb:`, or anything a desktop entry claimed. A markdown link
  is drawn in grey with its destination on hover, and the raw `[text](url)` is
  in the editor pane beside it. eframe's `links` feature is off, so the code
  that could open one is not compiled in; `[[wiki links]]`, which only ever move
  between notes in this window, are unaffected.
- **Symlinks are skipped**, both by the tree and by copy/paste. A link pointing
  back at a parent folder would otherwise make the scan revisit the same
  directories until it ran out of time, and copying one that points outside the
  vault would pull a real copy of its target in.
- **Following a `[[wiki link]]` never overwrites.** If a note by that name is
  already on disk it is opened, even when it is missing from the name index
  because it arrived after the last refresh.
- **Names cannot start with a dot** or contain control characters, since the
  tree would not be able to show what they created.

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

Each save goes to a temporary file beside the note and is renamed over it, so a
crash or a full disk leaves the previous note intact rather than a half-written
one. Writing on a timer makes that worth doing: the window between truncating a
file and finishing it reopens every time you pause, not just when you reach for
Ctrl+S.

Because the note is written on a timer, bluejay also checks that it is still the
file it read. If the note changed on disk while you had unsaved edits — a sync,
a `git pull`, an edit in another program — the save stops and asks which copy to
keep instead of quietly winning. The **⟳** button reloads the open note along
with the tree, and asks the same question first if there is anything unsaved.

## Layout of the code

| File | Contents |
| --- | --- |
| `src/main.rs` | window setup, the embedded typefaces, the first-run screen |
| `src/theme.rs` | the fixed Adwaita-dark palette and the spacing that goes with it |
| `src/app.rs` | the three panes, sidebar commands, modals, autosave |
| `src/markdown.rs` | `pulldown-cmark` events → egui widgets, wiki-link scanning |
| `src/vault.rs` | directory scan, note-name index, config file |
| `src/picker.rs` | the folder picker, for first launch and for changing vaults |
| `assets/` | the two typefaces, the logo, and the desktop entry |

Both typefaces are carried in the binary and nothing is read from the system's
font directories, so the window is the same on every machine: Adwaita Sans for
the chrome and the preview — GNOME's build of Inter, whose lowercase `l` has a
tail so it cannot be read as `I` or `1` — and JetBrains Mono for the editor. The
sans is one variable file used at two weights. Both are SIL OFL; the licences
are beside them in `assets/fonts/`.

Three dependencies: `eframe`, `pulldown-cmark`, and a direct `winit` pin that
exists only to select Wayland features — see below. Choosing a vault is drawn by
`picker.rs` rather than handed to a system file dialog, which is what keeps that
window in this theme and this font, and keeps GTK and the five libraries behind
it off the binary. The config file's
location is the base directory specification's one rule — `$XDG_CONFIG_HOME`,
or `~/.config` when it is unset — which `std` already answers, so `dirs` and the
two crates behind it are not carried for it.

## Windowing backend

`eframe`'s default features enable X11 and Wayland together, so defaults are off
and the wanted features are re-listed without `x11`. Two details are less
obvious than they look:

- **The direct `winit` dependency is never used from code.** eframe's `wayland`
  feature only reaches `winit/wayland`, and dropping `winit/default` would take
  `wayland-csd-adwaita-notitle` (client-side window decorations, needed for a
  titlebar on compositors that expect the client to draw one) and
  `wayland-dlopen` (loading libwayland at runtime instead of link time) with it.
  Cargo unions features across the graph, so naming winit directly restores
  those two without bringing `x11` back.

  The decorations are asked for in their **`-notitle`** spelling, which draws
  the titlebar and its buttons but no title text. The titled one renders that
  text itself, and to know what to render it in it runs `gsettings` for GNOME's
  titlebar-font setting and then `fc-match` to turn that name into a file — a
  font off the machine, chosen by two subprocesses at every launch, and the last
  thing that would still have looked different on every desktop. Its colour does
  not vary: egui keeps the native decorations in step with its own theme, which
  is pinned dark. The one thing left following the desktop is which side the
  window buttons sit on, which is a convention worth following.
- **`links` is off, and that is what removes URL opening.** It is the feature
  that wires egui's open-url command to `webbrowser`, which hands the URL to
  `xdg-open` on Linux. Nothing in the preview opens one any more, so the feature
  is left off rather than merely unused, and `webbrowser` and `url` — 27 crates
  with the latter's Unicode tables — leave the tree with it.
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
patching eframe and giving up copy/paste. They are inert here: `egui-winit` asks
`smithay-clipboard` first and only falls through to `arboard` if that fails, so
under Wayland the X11 backend is never the one answering.

It is worth naming, though, because it is the only code in the binary that could
open a socket at all: an X display spelled `host:0` is reached over TCP, which is
how `getaddrinfo` ends up among the binary's imports. The offline claim above is
about what the app does, not about which symbols the linker kept — bluejay never
asks for that path, and it can only be taken by a `DISPLAY` naming a remote host
after the Wayland clipboard has already failed.
