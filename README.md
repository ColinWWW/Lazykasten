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
| Ctrl+T | Suggest tags for the open note (click a chip to apply) |
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

## Themes

25 bundled themes (Catppuccin, Gruvbox, Tokyo Night, Nord, Dracula,
Solarized, RosÃ© Pine, Kanagawa, Everforest, Ayu, GitHub, Monokai, One Dark,
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
hotkey show ~10-21ms.

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
