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
| Esc | Go back one screen |
| ↑ / ↓ | Move the selection in the browser (while the search box is focused) |
| Enter | Open the selected tag or note |

Tag a note with a line like `Tag: Javascript, Computer Programming` or with
inline `#hashtags`. Untagged notes go to the `inbox` tag. Clicking a tag or a
footer chip opens that tag's index, newest first. The browser search box
searches all note text with prefix matching as you type.

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
while it runs.

## Run

```powershell
.\target\release\lazykasten.exe            # starts showing the window
.\target\release\lazykasten.exe --hidden   # starts in the tray only
```

To start with Windows, put a shortcut to `lazykasten.exe --hidden` in the
startup folder (`shell:startup`).
