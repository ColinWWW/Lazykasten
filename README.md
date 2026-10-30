# Lazykasten

A lightning-fast "lazy Zettelkasten" note app for Windows. Capture first,
organize later: press **Alt+J** anywhere, type, press **Alt+J** again and the
note is saved, tagged, and indexed.

## Keys

| Key | Action |
|---|---|
| Alt+J | Show / hide the note window (hiding saves the note) |
| Ctrl+S | Save the current note |
| Ctrl+N | Save and start a new note |
| Ctrl+O | Browse tags and notes (again to return to the editor) |
| Ctrl+P | Quick switcher: open any note by fuzzy title match |
| Ctrl+R | Related notes for the open note |
| Ctrl+Z | Undo the last auto-tagging (right after it happens) |
| Ctrl+Shift+Backspace | Move the open note to the trash |
| Ctrl+/ or F1 | Key help (the same table the app shows) |
| F2 | Rename the selected tag (on the tag list; merges if the name exists) |
| Ctrl+T | Suggest tags for the open note (click a chip to apply) |
| Ctrl+G | Open the graph (Local around the open note, else Global) |
| Ctrl+Shift+T | Sweep the inbox with the auto-tagger (on the tag list) |
| Esc | Go back one screen |
| ↑ / ↓ | Move the selection in the browser (while the search box is focused) |
| Enter | Open the selected tag or note |

Tag a note with a line like `Tag: Javascript, Computer Programming` or with
inline `#hashtags`. Untagged notes go to the `inbox` tag. Clicking a tag or a
footer chip opens that tag's index, newest first. The browser search box
searches all note text with prefix matching as you type.

## Auto-tagging

Lazykasten can tag untagged notes for you, learning from the notes you have
already tagged. It runs fully locally — no LLM, no network, nothing is sent
anywhere. It scores a new note's key terms against the SQLite FTS5 index,
finds similar tagged notes, and lets them vote for their tags; a tag whose
name appears in the note gets a boost. The more you tag, the better it gets.

- Hiding (Alt+J) an untagged note saves it to `inbox` instantly, then applies
  tags in the background when confidence is at least 0.4 (at most 3). The
  next time the window opens it shows `Auto-tagged: #A #B`.
- **Ctrl+T** in the editor shows tag suggestions with their confidence as
  outlined chips in the footer. Click one to add it to the note's `Tag:` line;
  Esc dismisses them.
- **Ctrl+Shift+T** on the tag list sweeps every `inbox` note through the
  auto-tagger and reports how many it tagged.
- Measure accuracy with `lazykasten.exe --eval-tags` (vault from
  `%LAZYKASTEN_VAULT%`). It strips each tagged note's tags, re-runs the
  tagger with the note excluded, and prints precision, recall, the share of
  notes with at least one correct tag, and the time per note. Thresholds
  chosen: apply at 0.4, suggest at 0.1, name boost +0.5 — precision matters
  more than recall, since a wrong tag annoys more than a missing one.

## Links between notes

Type `[[` in the editor and matching note titles appear above the footer;
click one to insert `[[<id>|<title>]]`. Links are indexed, and an open note's
footer shows its backlink count — click it to list the notes that link to it.

## Graph

**Ctrl+G** opens a force-directed graph of your notes (Obsidian-style):
note circles coloured by first tag, tag squares, deterministic Barnes-Hut
layout on a background thread.

- **Tab** switches Local (around the open note) <-> Global; **T** toggles
  tag nodes, **O** toggles orphan notes, **1/2/3** set the local depth,
  **Ctrl+Enter** enlarges/restores the window.
- Drag nodes (the layout reheats), drag the background to pan, scroll to
  zoom, hover highlights a node, and a **single click opens the note**
  (under 4px of movement is a click, more is a drag). Esc from the
  opened note returns to the graph. **/** or Ctrl+F focuses the search
  box, which dims non-matching notes; Esc returns to the canvas.
- Global positions persist in `<vault>/.lazykasten/graph_layout.bin`, so
  reopening the global graph does not restart the simulation.

## Themes

25 bundled themes (Catppuccin, Gruvbox, Tokyo Night, Nord, Dracula,
Solarized, Rosé Pine, Kanagawa, Everforest, Ayu, GitHub, Monokai, One Dark,
and the default Lazykasten Dark), all fully local.

- **Ctrl+Shift+P** (or Ctrl+K then T) opens the theme picker. Arrow keys
  preview each theme live; **Enter** keeps it and saves the choice, **Esc**
  reverts. The search box filters by name.
- The choice is stored in `<vault>/.lazykasten/settings.toml` and survives
  restarts.
- Custom themes: drop gpui-component `ThemeSet` JSON files (same format as
  the bundled ones in `themes/`) into `<vault>/.lazykasten/themes/`. They
  load when the picker first opens; invalid files are skipped with a status
  message, never a crash.
- Palette sources and licenses: `themes/LICENSES.md`.

Performance at 20,000 notes (release, see `PERF.md`): first index 0.85s,
no-change re-check 0.025s, tag index (400 notes) 0.5ms, auto-tag 6.7ms,
hotkey first frame ~11ms (swapchain warmed at startup), graph layout step ~8ms at 20k and full settle ~2.6s, quick-switcher fuzzy ~0.3ms per keystroke at 20k titles.

## Data safety

Notes are hard to lose:

- **Atomic saves.** Every write (notes, settings, the graph layout
  cache) goes to a `.tmp` file in the same folder and is renamed over
  the target, so a crash mid-write can never truncate a note. Stale
  `.tmp` files are cleaned up on sync.
- **Autosave.** 1.5s after you stop typing, the open note saves in the
  background through the normal save path.
- **Outside edits win.** If a note's file changed on disk (say, in
  Obsidian) since you opened it, saving writes your text to
  `<id>-conflict-<timestamp>.md` instead of overwriting — and the app
  watches `notes/` live: an outside change reloads an untouched open
  note, or flags the conflict when you have unsaved edits.
- **Trash, never delete.** Saving an empty note or pressing
  Ctrl+Shift+Backspace moves it to `<vault>/.lazykasten/trash/`. The
  `#trash` row at the bottom of the tag list restores notes with Enter.

## Settings and startup

`<vault>/.lazykasten/settings.toml` (hand-parsed `key = "value"` lines):

| Key | Meaning |
|---|---|
| `theme` | Saved theme name (written by the theme picker) |
| `hotkey` | Global show/hide hotkey, e.g. `alt+j` (falls back to Alt+J with a status note) |
| `vault` | Vault path override, read from the default vault's settings |
| `window` | Remembered window `x,y,w,h` (written when the window hides) |

The tray menu has a **Start with Windows** checkbox
(`HKCU\...\Run\Lazykasten` = `"<exe>" --hidden`). The exe carries an
embedded icon (purple tile, white L) for Explorer, Alt+Tab and the
taskbar.

Organizing: **F2** on the tag list renames a tag across every note
(`Tag:` lines and `#hashtags`, written atomically; an existing name
merges), **Ctrl+Z** right after an auto-tag removes it again, and
**Ctrl+R** lists the notes most similar to the open one.

## Vault

Notes are plain Markdown files you own (readable by Obsidian or any editor):

- Location: `%LAZYKASTEN_VAULT%` if set, otherwise `~/Lazykasten`
- Notes: `<vault>/notes/<id>.md`, where `<id>` is a `YYYYMMDDHHMMSS` timestamp
- Index: `<vault>/.lazykasten/index.db` (SQLite; rebuildable, safe to delete —
  it is rebuilt from the files on the next sync)

## Build

Requires the Rust MSVC toolchain (`rust-toolchain.toml` pins
`stable-x86_64-pc-windows-msvc`), Visual Studio Build Tools, and the Windows
SDK.

```powershell
cargo build --release
cargo test
cargo test --release -- --ignored --nocapture   # 20,000-note benchmark
```

Kill any running `lazykasten.exe` before rebuilding; Windows locks the exe
while it runs. Faster edit-compile loop: `cargo run --release`.

## Run

```powershell
.\target\release\lazykasten.exe            # starts showing the window
.\target\release\lazykasten.exe --hidden   # starts in the tray only
```

To start with Windows, put a shortcut to `lazykasten.exe --hidden` in the
startup folder (`shell:startup`).
