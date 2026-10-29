# Build Lazykasten from scratch: full specification

You are an expert Rust and Windows desktop engineer. Build **Lazykasten**, a lightning-fast "lazy Zettelkasten" note app for Windows, from an empty folder. This document is the complete spec: goals, stack with exact versions, architecture, data model, algorithms, UI, verified library APIs, known pitfalls, and a milestone plan with checks.

Work milestone by milestone. After each one, make sure the code compiles with **zero warnings** and passes the listed check before moving on. When this document gives an exact API signature, trust it over your memory: these libraries are newer than most training data.

---

## 1. Product goals

The user wants an Obsidian-style note app that feels as instant as the Ghostty terminal.

1. **Global hotkey popup.** Pressing **Alt+J** anywhere in Windows instantly shows a clean, minimal note window with the cursor ready. Pressing **Alt+J** again saves the note, tags it, indexes it, and hides the window.
2. **Instant feel.** Showing the window must take about one frame (target under 30 ms). Typing must never lag. Never do slow work on the UI thread.
3. **Tags and indexes.** Notes are tagged with a line like `Tag: Javascript, Computer Programming` or with inline `#hashtags`. Clicking a tag opens an **index**: every note with that tag, newest first.
4. **Lazy Zettelkasten.** Capture first, organize later. Notes with no tags automatically go to an `inbox` tag so nothing is lost.
5. **Scale.** It must handle **tens of thousands of notes**. Tag indexes and search must answer in milliseconds at 20,000 notes.
6. **Plain files.** Notes are ordinary `.md` files the user owns, readable by Obsidian or any editor. The database is only a rebuildable index.
7. **Look.** Dark theme, generous padding, tags shown as blue clickable links.

**Platform:** Windows 10/11 only, including high-DPI and multi-monitor setups (for example a 4K monitor at 150% scaling).

---

## 2. The core architecture idea (most important)

Speed comes from **never cold-starting on the hotkey**:

- The app runs **resident in the system tray**. At login it creates its window once, **hidden**.
- Alt+J only calls Win32 `ShowWindow` / `SetForegroundWindow` on that existing window, or hides it. That's why it appears in a frame.
- On hide, the app saves the note to disk and updates the index. A single note saves in under a millisecond.
- The UI is **GPU-rendered (GPUI, from the Zed editor)**, so text rendering and typing are low-latency, unlike Electron (which Obsidian uses).
- **Markdown files are the source of truth. SQLite is the index.** A tag lookup is one indexed query, and search uses SQLite FTS5.

---

## 3. Tech stack (exact versions)

| Layer | Choice |
|---|---|
| Language | Rust, edition 2024 |
| Toolchain | **MSVC**: `stable-x86_64-pc-windows-msvc`, pinned in `rust-toolchain.toml`. Needs Visual Studio Build Tools and the Windows SDK. Don't use the GNU toolchain. |
| UI | **`gpui-kit = "0.8.1"`**: one dependency that bundles GPUI (`gpui-pre 0.3.10`), its platform layer, `gpui-base` and the `gpui-component` widget library. Don't add `gpui` or `gpui-component` separately. |
| Global hotkey | `global-hotkey = "0.9"` |
| Tray icon and menu | `tray-icon = "0.26"` (menus are re-exported as `tray_icon::menu`) |
| Index | `rusqlite = { version = "0.41", features = ["bundled"] }` (SQLite with FTS5) |
| Win32 | `windows = { version = "0.62", features = ["Win32_Foundation", "Win32_Graphics_Gdi", "Win32_UI_HiDpi", "Win32_UI_WindowsAndMessaging"] }` |
| Misc | `anyhow = "1"`, `chrono = "0.4"`, `dirs = "6"`, `async-channel = "2"`, `raw-window-handle = "0.6"` |

`Cargo.toml`:

```toml
[package]
name = "lazykasten"
version = "0.1.0"
edition = "2024"

[dependencies]
gpui-kit = "0.8.1"
global-hotkey = "0.9"
tray-icon = "0.26"
rusqlite = { version = "0.41", features = ["bundled"] }
anyhow = "1"
chrono = "0.4"
dirs = "6"
async-channel = "2"
raw-window-handle = "0.6"
windows = { version = "0.62", features = ["Win32_Foundation", "Win32_Graphics_Gdi", "Win32_UI_HiDpi", "Win32_UI_WindowsAndMessaging"] }

[profile.release]
lto = "thin"
codegen-units = 1
```

`rust-toolchain.toml`:

```toml
[toolchain]
channel = "stable-x86_64-pc-windows-msvc"
```

---

## 4. File layout

```
lazykasten/
├─ Cargo.toml, rust-toolchain.toml, .gitignore (/target), README.md
└─ src/
   ├─ main.rs    app shell: tray, global hotkey, hidden window, show/hide/placement
   ├─ store.rs   notes on disk + SQLite index + tag/title parsing + tests
   └─ ui.rs      the single view: editor screen + browser screens
```

---

## 5. Storage (`store.rs`)

### Vault location
- The root is `%LAZYKASTEN_VAULT%` if set, otherwise `~\Lazykasten` (from `dirs::home_dir()`).
- Notes go in `<root>/notes/<id>.md` and the index in `<root>/.lazykasten/index.db`. Create both folders if they're missing.

### Note IDs
- Zettelkasten timestamp `YYYYMMDDHHMMSS` in local time (`chrono::Local::now().format("%Y%m%d%H%M%S")`).
- If that file already exists, append `-1`, `-2` and so on.

### Database
On open, run `PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;`.

Keep a schema version in `PRAGMA user_version` (start at 1). If the stored version differs, **drop all tables** and set the new version. The index is rebuilt from the files on the next sync.

```sql
CREATE TABLE IF NOT EXISTS notes (
  id    TEXT PRIMARY KEY,        -- keeps an implicit integer rowid (do NOT use WITHOUT ROWID)
  title TEXT NOT NULL,
  mtime INTEGER NOT NULL          -- file modified time, ms since epoch
);
CREATE TABLE IF NOT EXISTS tags (
  id   INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE COLLATE NOCASE
);
CREATE TABLE IF NOT EXISTS note_tags (
  tag_id  INTEGER NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
  note_id TEXT    NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
  PRIMARY KEY (tag_id, note_id)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS note_tags_by_note ON note_tags(note_id);
-- Full text keyed by notes.rowid (NOT by a text id column; see pitfalls)
CREATE VIRTUAL TABLE IF NOT EXISTS notes_text USING fts5(body);
```

### `Store` API
`Store { notes_dir: PathBuf, conn: Mutex<rusqlite::Connection> }`, used from the UI as `Arc<Store>`.

- `open(root) -> Result<Store>`, `default_root() -> PathBuf`, `note_path(id)`, `new_id()`, `read(id) -> Result<String>`
- **`save(id, text)`**
  - If `text.trim()` is empty, delete the file and its index rows.
  - Otherwise write the file, read back its mtime, and run `index_note` inside **one transaction**.
- **`index_note(conn, id, text, mtime)`**, run inside the caller's transaction:
  1. Parse the tags. If there are none, use `["inbox"]`.
  2. Upsert the note: `INSERT … ON CONFLICT(id) DO UPDATE SET title=…, mtime=…`. An upsert keeps the rowid.
  3. `DELETE FROM note_tags WHERE note_id=?`. For each tag, `INSERT OR IGNORE` into tags, look up its id, and insert into `note_tags`.
  4. `SELECT rowid FROM notes WHERE id=?`, then `DELETE FROM notes_text WHERE rowid=?` and `INSERT INTO notes_text(rowid, body) VALUES (?, ?)`.
- **`remove_note(conn, id)`**: delete the `notes_text` row by the note's rowid, then delete from notes. Tag links cascade.
- **`sync_with_disk() -> Result<usize>`**: makes the index match the files.
  1. List the `*.md` files with their mtimes.
  2. Load the `id → mtime` map from the DB.
  3. Re-index only files whose mtime differs, **in batches of 500 per transaction**. Read the file contents before taking the lock.
  4. Remove rows for files that no longer exist, and delete orphan tags.
  5. Return how many notes were re-indexed. This runs in the background at startup.
- **`tags() -> Vec<TagCount{name, count}>`**: tags joined to their notes, grouped, ordered by count descending, then name.
- **`notes_with_tag(tag) -> Vec<NoteSummary{id, title}>`**: newest first (`ORDER BY n.id DESC`). Matching is case-insensitive through `COLLATE NOCASE`.
- **`search(query, limit) -> Vec<NoteSummary>`**: FTS5.
  - Wrap each whitespace-separated word in double quotes, doubling any inner `"`, so user input can't inject FTS syntax.
  - Join the words with spaces and add a trailing `*` for prefix matching while typing.
  - `SELECT n.id, n.title FROM notes_text f JOIN notes n ON n.rowid = f.rowid WHERE notes_text MATCH ? ORDER BY rank LIMIT ?`
- `note_count()`

### Tag parsing: `parse_tags(text) -> Vec<String>`
- **Tag lines:** a line (trimmed) whose part before the first `:` is `tag` or `tags`, case-insensitive. The rest is split on commas, and each piece is trimmed.
- **Hashtags:** for each `#` in other lines:
  - Skip it if the character before is alphanumeric or another `#`.
  - Take the following run of letters, digits, `-`, `_` and `/`.
  - Accept it only if it starts with a letter. So `# Heading`, `issue#12` and `#1` are **not** tags.
- Drop duplicates case-insensitively, keeping the first spelling and order of appearance.
- Example: `"Some #rust thoughts, see #rust and #Async-IO.\nTag: Javascript, Computer Programming\n# Heading"` gives `["rust", "Async-IO", "Javascript", "Computer Programming"]`.

### Title: `title_of(text)`
The first non-empty trimmed line that isn't a tag line, with leading `#`s removed, trimmed and cut to 120 characters. If there's none, use `"Untitled"`.

### Tests (in `store.rs`)
- Tag parsing, including the non-tag cases.
- Title rules.
- A save/index/query round trip: tag counts, case-insensitive lookup, prefix search, editing a note replaces its tags, and saving empty text deletes it.
- External changes: delete one file and add another outside the app, then check `sync_with_disk` returns 1 and a second call returns 0.
- An `#[ignore]` **20,000-note benchmark** that prints timings. Run it with `cargo test --release -- --ignored --nocapture`.
- **Windows:** `drop(store)` before deleting the temp vault folder, because the open DB file locks it.

---

## 6. App shell (`main.rs`)

- At the top: `#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]`, so release builds have no console.
- Open the store and exit with an error if that fails. Then start the app:
  1. `gpui_kit::application().run(move |cx: &mut App| { … })`
  2. Inside: `gpui_kit::init(cx)`, `Theme::change(ThemeMode::Dark, None, cx)`, `ui::bind_keys(cx)`, `cx.set_quit_mode(QuitMode::Explicit)` (a hidden window must never quit the app).
  3. `cx.background_spawn(async move { store.sync_with_disk() }).detach()`
  4. A command channel: `enum Command { Toggle, Show, Quit }` over `async_channel::unbounded()`.
  5. Register Alt+J. If it fails (probably another instance is running), print an error, call `cx.quit()` and return.
  6. Create the tray icon:
     - The icon is a 32×32 RGBA drawn in code (purple tile, white "L"), so no asset files.
     - Tooltip "Lazykasten (Alt+J)".
     - Menu: "Open note\tAlt+J" sends Show, and "Quit Lazykasten" sends Quit.
     - Left click (button Up) sends Toggle. Disable menu-on-left-click.
  7. Open the window **hidden**: `WindowOptions { titlebar: Some(TitlebarOptions { title: Some("Lazykasten".into()), ..Default::default() }), show: false, focus: false, ..Default::default() }`. **Don't set `window_bounds`** (see pitfalls). Inside the build closure:
     - Get the HWND.
     - Create the view.
     - Register `window.on_window_should_close`, which commits the note, hides the window and returns `false`. The close button hides; it never quits.
  8. Unless `--hidden` was passed, send `Show` once at startup.
  9. Run the command loop with `cx.spawn(async move |cx| { while let Ok(cmd) = rx.recv().await { … } }).detach()`:
     - On the first Toggle or Show only, call `place_on_cursor_monitor(hwnd)`.
     - Read visibility with `IsWindowVisible(hwnd)`.
     - **Toggle while visible:** `view.commit(window, cx)`, then `ShowWindow(hwnd, SW_HIDE)`.
     - **Toggle while hidden, or Show:** `ShowWindow(hwnd, SW_SHOW)`, `SetForegroundWindow(hwnd)`, then `view.on_show(window, cx)`.
     - **Quit:** commit, then `cx.quit()`.
     - Run these inside `window.update(cx, |_, window, cx| …)`.
- **`place_on_cursor_monitor(hwnd)`:**
  1. `GetCursorPos`, then `MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)`, then `GetMonitorInfoW` (set `cbSize`) to get the work area `rcWork`.
  2. `GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut x, &mut y)`.
  3. Size is 760×540 logical × dpi/96, clamped to the work area. Center it there.
  4. `SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE)`.
- Keep `GlobalHotKeyManager` and `TrayIcon` alive for the whole run, for example with `Box::leak`.

---

## 7. UI (`ui.rs`)

### Actions and keys
- `actions!(lazykasten, [SaveNote, NewNote, Browse, Back, SelectPrev, SelectNext]);`
- Context `"LazyKasten"` (on the root div): `ctrl-s` → SaveNote, `ctrl-n` → NewNote, `ctrl-o` → Browse, `escape` → Back.
- Context **`"TagSearch > Input"`**: `up` → SelectPrev, `down` → SelectNext. The search box is wrapped in a div with `key_context("TagSearch")`, so arrows move the list selection only while the search box has focus. In the editor they still move the cursor.

### State
`enum Screen { Editor, Tags, TagIndex(String), Search }`

`struct LazyKasten { store: Arc<Store>, editor: Entity<TextareaState>, search: Entity<InputState>, note_id: Option<String>, saved_text: SharedString, live_tags: Vec<String>, screen: Screen, tags: Vec<TagCount>, notes: Vec<NoteSummary>, selected: usize, list_scroll: UniformListScrollHandle, status: SharedString, _subscriptions: Vec<Subscription> }`

### Behavior
- **Editor text changes:** re-parse the tags into `live_tags`, and re-render only if they changed.
- **Search events:**
  - Change: if the query is empty, show `Screen::Tags`. Otherwise `store.search(q, 500)` and show `Screen::Search`. Reset the selection to 0.
  - PressEnter: `open_selected`.
- **`on_show`:** screen = Editor, clear the status, focus the editor.
- **`commit`:** save, then `note_id = None`, `saved_text = ""`, clear the chips and set the editor to `""`.
- **`save`:** if the text equals `saved_text`, do nothing (don't rewrite unchanged notes). Otherwise get or create the id, call `store.save`, set the status ("Saved <id>.md", "Deleted empty note" or the error), and update `saved_text`.
- **`open_note(id)`:** load the text, set `saved_text` and `live_tags`, put the text in the editor, set `note_id`, switch to Editor and focus it.
- **`show_tags`:** reload the tags, screen = Tags, reset the selection, clear and focus the search box.
- **`show_tag(tag)`:** load that tag's notes, screen = TagIndex(tag), reset the selection, focus the search box.
- **Selection:** Up and Down clamp to the list. After moving, call `list_scroll.scroll_to_item(selected, ScrollStrategy::Nearest)`. Enter opens the selected tag or note.
- **Browse (Ctrl+O):** from the editor, commit and show tags. From the browser, go back to the editor.
- **Back (Esc):** TagIndex or Search goes to the tag list. The tag list goes to the editor.

### Layout (dark theme; colors from `cx.theme()`)
- **Root:** `div().key_context("LazyKasten")` with every `.on_action(cx.listener(Self::handler))` attached, `.size_full().bg(theme.background).text_color(theme.foreground)`.
- **Editor screen:**
  - A full-height `Textarea` with `.appearance(false).large()`, inside `px_6 pt_4`.
  - A footer bar (h_8, top border) with the live tag chips on the left in `theme.blue`, with a hover background. Clicking a chip commits the note and opens that tag's index.
  - On the right of the footer, key hints "Alt+J save & hide   Ctrl+O browse   Ctrl+N new", or the status message when there is one.
- **Browser screen:**
  - The search `Input` (`cleanable(true)`, placeholder "Search all notes…") inside the TagSearch div.
  - A small muted heading: "N tags · M notes", "#tag · N notes", or "500+ matches, best first" when results hit the limit (otherwise "N matches").
  - A **virtualized `uniform_list`** of full-width rows (h_8, rounded, pointer cursor, `list_hover` on hover, `list_active` when selected):
    - Tag rows show `#name` in blue (flex_1) and the count, muted and right-aligned.
    - Note rows show the title (flex_1, ellipsis) and a date like `2026-10-28 14:32` parsed from the id, muted and right-aligned.
  - A footer: "↑↓ select   Enter open   Esc back   Alt+J hide".

---

## 8. Verified gpui-kit 0.8.1 API cheat sheet

```rust
use gpui_kit::{prelude::FluentBuilder as _, *};          // GPUI core + .when()
use gpui_kit::component::{ActiveTheme, Sizable, Theme, ThemeMode,
    input::{Input, InputEvent, InputState, Textarea, TextareaState}};

gpui_kit::application().run(|cx: &mut App| { gpui_kit::init(cx); /* … */ });
let (any_window, view) = gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| MyView::new(window, cx)))?;
// open_window(options, &mut App, FnOnce(&mut Window, &mut App) -> Entity<V>) -> Result<(AnyWindowHandle, Entity<V>)>
cx.set_quit_mode(QuitMode::Explicit);
Theme::change(ThemeMode::Dark, None, cx);

// Editor and search box
let editor = cx.new(|cx| TextareaState::new(window, cx).soft_wrap(true).placeholder("…"));
let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search all notes…"));
Textarea::new(&editor).appearance(false).large().h_full();
Input::new(&search).cleanable(true);
let text: SharedString = editor.read(cx).value();
editor.update(cx, |s, cx| { s.set_value("new text", window, cx); s.focus(window, cx); });
// InputEvent::{Change, PressEnter { secondary, shift }, Focus, Blur}
cx.subscribe_in(&editor, window, |this, editor, ev: &InputEvent, window, cx| { /* … */ });

// Actions and keys
gpui_kit::actions!(lazykasten, [SaveNote]);
cx.bind_keys([KeyBinding::new("ctrl-s", SaveNote, Some("LazyKasten"))]);
div().key_context("LazyKasten").on_action(cx.listener(Self::on_save));
fn on_save(&mut self, _: &SaveNote, window: &mut Window, cx: &mut Context<Self>) {}

// Clickable element (needs an id)
div().id("chip").on_click(cx.listener(|this, _ev, window, cx| { /* … */ }));

// Virtualized list
let scroll = UniformListScrollHandle::new();
uniform_list("notes", count, cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
    range.map(|ix| div().id(ix).child("…")).collect::<Vec<_>>()
})).track_scroll(&scroll).flex_1();
scroll.scroll_to_item(ix, ScrollStrategy::Nearest);   // also Top, Center, Bottom

// Theme colors
cx.theme().background / .foreground / .muted_foreground / .border / .blue / .list_hover / .list_active

// Async
cx.spawn(async move |cx| { /* cx: &mut AsyncApp */ let _ = any_window.update(cx, |_root, window, cx| {}); }).detach();
cx.background_spawn(async move { /* off the UI thread */ }).detach();

// Close button
window.on_window_should_close(cx, move |window, cx| { /* … */ false });

// Raw HWND: call the trait method explicitly, because Window also has its own window_handle()
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
let hwnd = match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
    Ok(RawWindowHandle::Win32(h)) => windows::Win32::Foundation::HWND(h.hwnd.get() as *mut _),
    _ => HWND::default(),
};
```

**global-hotkey 0.9:**

```rust
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::{Code, HotKey, Modifiers}};
let manager = GlobalHotKeyManager::new()?;                       // create on the main thread
manager.register(HotKey::new(Some(Modifiers::ALT), Code::KeyJ))?;
Box::leak(Box::new(manager));                                    // must outlive the app
GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
    if e.state == HotKeyState::Pressed { let _ = tx.try_send(Command::Toggle); }
}));
```

**tray-icon 0.26:**

```rust
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent, menu::{Menu, MenuEvent, MenuItem}};
let open = MenuItem::new("Open note\tAlt+J", true, None);
let quit = MenuItem::new("Quit Lazykasten", true, None);
let menu = Menu::new(); menu.append_items(&[&open, &quit])?;
let tray = TrayIconBuilder::new()
    .with_icon(Icon::from_rgba(rgba_vec, 32, 32)?)
    .with_tooltip("Lazykasten (Alt+J)")
    .with_menu(Box::new(menu))
    .with_menu_on_left_click(false)
    .build()?;
Box::leak(Box::new(tray));
MenuEvent::set_event_handler(Some(move |e: MenuEvent| { /* compare e.id with open.id().clone() / quit.id().clone() */ }));
TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e { /* Toggle */ }
}));
```

---

## 9. Pitfalls (already discovered; avoid them)

1. **Hidden windows ignore `window_bounds`.** On Windows, GPUI creates windows with `CW_USEDEFAULT` and applies the requested bounds to a hidden window only inside `activate_window()`. That function also **simulates an Alt keypress**, which fights the Alt+J hotkey. **Don't use `activate_window()` and don't set `window_bounds`.** Size and position the window yourself with `SetWindowPos` on the first show (`place_on_cursor_monitor`), then use raw `ShowWindow` / `SetForegroundWindow`. Without this the window opened at 1920×1025 on a 150% display.
2. **Message loop.** GPUI's Windows message loop (`GetMessageW(None)` on the main thread) also drives the hidden message windows that global-hotkey and tray-icon create, as long as you create them on the main thread inside `run`. Their callbacks run on that thread. Send into an `async_channel` and handle commands in a `cx.spawn` loop.
3. **Slow first index (32 seconds at 20k notes).** There were two causes: one transaction per note, and an FTS table keyed by an `UNINDEXED` text id, which makes every delete a full scan and indexing quadratic. **Batch the transactions, and key FTS rows by `notes.rowid`.** With both fixes it takes about 1.5 s.
4. **Name clash.** `window.window_handle()` resolves to GPUI's own method, which returns `AnyWindowHandle`. Use `HasWindowHandle::window_handle(window)`.
5. **Arrow keys in the search input.** The input binds up/down in context `"Input"`. Override them with the more specific `"TagSearch > Input"`, registered **after** `gpui_kit::init`.
6. **`.when()` needs a trait import:** `use gpui_kit::prelude::FluentBuilder as _;`
7. **Windows file locks.** Drop the SQLite connection before deleting the folder in tests. Kill a running `lazykasten.exe` before rebuilding, because Windows locks the exe while it runs.
8. **Rewriting unchanged notes.** Compare against `saved_text` before saving. Otherwise just opening a note and closing it rewrites the file and changes its mtime.

---

## 10. Milestones and checks

1. **Scaffold.** Create the Cargo project, `rust-toolchain.toml` and dependencies. `cargo build` succeeds.
2. **Storage.** Write `store.rs` with all its tests. `cargo test` is green, and the 20k benchmark in release mode prints a first index under 3 s and queries under 15 ms.
3. **Window and editor.** A hidden GPUI window with the Textarea editor. Running without `--hidden` shows it at about 760×540 logical pixels, centered on the monitor under the mouse.
4. **Hotkey and tray.** Alt+J toggles the window in under 30 ms with the editor focused. Hiding saves `<id>.md`, and the DB lists its tags (untagged notes go to `inbox`). The close button hides. The tray menu has Open and Quit.
5. **Browser.** Ctrl+O shows the tag list with counts. Clicking a tag, or pressing Enter, opens its index. Notes open in the editor. Search works with prefix matching and shows "500+" when capped. Esc goes back a screen at a time. Chips open tag indexes. Up/Down move the selection and the list scrolls to follow it.
6. **Polish.** Zero warnings. README with keys, vault location, build steps and autostart (a shortcut to `lazykasten.exe --hidden` in `shell:startup`).

**Performance targets at 20,000 notes (release build):**

| Operation | Target |
|---|---|
| Hotkey to visible window | 30 ms or less (about 13 ms achieved) |
| Idle memory | about 60 MB or less |
| Save a note | 1 ms or less |
| Tag index, 400 notes | 1 ms or less |
| Tag index, all notes | 15 ms or less |
| Full-text search | 2 ms or less |
| First full index | 3 s or less |
| Startup re-check, nothing changed | 0.5 s or less, in the background |

**How to test the UI.** Seed a throwaway vault with about 2,000 generated notes (random titles, bodies and 1–2 tags each), point the app at it with `LAZYKASTEN_VAULT`, and drive the real window with a PowerShell script:
- Find the window titled "Lazykasten" with `FindWindow([NullString]::Value, "Lazykasten")`. In PowerShell, `$null` becomes an empty string, so it won't match.
- Send Alt+J with `keybd_event`, type with `SendKeys`, and click with `SetCursorPos` plus `mouse_event`.
- Screenshot the client area with `Graphics.CopyFromScreen`, after calling `SetProcessDPIAware()`.
- Check the window has focus before every action, so input never lands in another app.

---

## 11. Stretch goals (if time allows, in order)

1. **Editor styling.** Make the `Tag:` values and `#hashtags` blue clickable links inside the editor text (gpui-component Textarea's inline token API: `.token(...)`, `.on_token_click(...)`). Add spellcheck squiggles through the Windows `ISpellChecker` COM API, and light Markdown styling.
2. **Lazy auto-tagging.** When a note is saved with no tags, suggest tags by matching its words against existing tag names. Run it after the save and never block typing.
3. **Links between notes.** `[[` autocomplete for note links, plus a backlinks list.
4. **Live file watching.** Use the `notify` crate while the app runs.
5. **Settings.** Configurable hotkey and vault path, a start-with-Windows toggle, remembered window size and position, an optional borderless always-on-top dropdown, and a single-instance check.

---

## 12. Code style

- Idiomatic Rust. Use `anyhow::Result` for errors.
- Write brief doc comments on modules and non-obvious functions. Comments should explain *why*, not *what*.
- Keep slow work off the UI thread. Single-note saves (under 1 ms) may run inline.
- Verify UI changes by running the app, not just by compiling it.
