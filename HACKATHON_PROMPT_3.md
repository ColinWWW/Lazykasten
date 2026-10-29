# Lazykasten, part 3: themes, and make everything faster

Lazykasten is built and committed: the core app, local auto-tagging (milestone 8), `[[` links and backlinks (10), and the demo work (11). This prompt adds **Helix-style themes** and a **performance pass**.

**The hard rule: nothing may get slower, and the goal is for everything to get faster.** Every change in this prompt is measured before and after. A change that doesn't measurably help is reverted.

**Before you change anything:**
- Re-read `HACKATHON_PROMPT.md`, `HACKATHON_PROMPT_2.md` and every file in `src/`.
- All their rules still apply: zero warnings, no slow work on the UI thread, verify UI changes by running the app, and commit after every milestone.
- Everything stays **fully local**. Don't add an LLM, network calls or telemetry.

---

## Milestone 12: Measure first

You can't make it faster without numbers.

1. **Add a tracing switch.** When the env var `LAZYKASTEN_TRACE=1` is set, append timings to `<vault>/.lazykasten/trace.log`.
   - Release builds have no console, so write to a file.
   - When the variable isn't set, the cost must be a single branch, with no formatting and no I/O.
   - Time these operations:
     - hotkey press to the first frame painted after `ShowWindow` (first show and later shows, separately)
     - app start to the window being ready for the hotkey
     - `sync_with_disk`
     - `save`
     - every `search` call
     - `notes_with_tag`
     - auto-tag suggestion
     - theme switch to the next frame
2. **Measure search worst cases.** Extend the `#[ignore]` 20k benchmark with:
   - a 1-letter prefix search (`a*`), the worst case for FTS prefix queries
   - a 2-letter prefix search
   - an auto-tag run
   - the startup re-check when nothing changed
3. **Record the baseline.** Run `cargo test --release -- --ignored --nocapture` and drive the app with the PowerShell method from the first spec. Record everything in a new `PERF.md` table with a **Before** column.

---

## Milestone 13: Performance pass

Do these one at a time. Measure each against the baseline and keep only the changes that help. Add an **After** column and a one-line note per change to `PERF.md`.

### Known slow spots in the current code (fix these first)
1. **Search runs on the UI thread.** `ui.rs` calls `self.store.search(...)` directly in the input handler, both for the browser search (limit 500) and the `[[` popup (limit 8). Short queries at 20k notes can take long enough to drop typed characters.
   - Run searches with `cx.background_spawn`.
   - Tag each request with an incrementing generation number, and drop any result whose generation isn't the latest, so stale results never overwrite newer ones.
   - The UI keeps showing the previous results until the new ones arrive. Never clear the list to a blank state in between.
2. **The startup re-check does an extra syscall per file.** `sync_with_disk` calls `file_mtime_ms(&path)`, which is `std::fs::metadata(path)`, for every file.
   - On Windows, `DirEntry::metadata()` reuses the data that `FindNextFileW` already returned, with no extra syscall.
   - Use `entry.metadata()?.modified()` in the listing loop, and keep `file_mtime_ms` only for the single file after `save`.
   - Expect the 0.35 s re-check at 20k notes to drop a lot. Measure it.
3. **Statements are re-prepared on every call.** `index_note` and the queries use `conn.execute` / `conn.query_row` with SQL strings. During a 500-note batch that's thousands of re-parses.
   - Switch the hot paths to `conn.prepare_cached(...)`: `index_note`, `tags`, `notes_with_tag`, `search`, the backlinks query and the auto-tagger queries.
   - Raise the cache size if needed with `conn.set_prepared_statement_cache_capacity(64)`.
4. **Single-letter prefix search.**
   - If `a*` at 20k notes is over 5 ms, don't run prefix search for 1-character queries. Run an exact-term search instead, and switch to prefix matching from 2 characters.
   - Alternatively, add `prefix='2 3'` to the FTS table, which makes 2- and 3-character prefixes index lookups. That needs a schema bump to 3, and you should measure the index size cost.

   Pick whichever measures better.

### SQLite tuning (measure each)
- `PRAGMA temp_store = MEMORY; PRAGMA cache_size = -32000; PRAGMA mmap_size = 268435456;` on open.
- After a sync that re-indexed more than 1,000 notes, run `INSERT INTO notes_text(notes_text) VALUES('optimize')` in the background. This merges the FTS segments so queries read fewer pages.
- Run `PRAGMA optimize` on quit.

### Startup and hotkey
- The window must accept Alt+J before `sync_with_disk` finishes. It already runs in the background, so check that nothing on the startup path waits on it, or on the DB lock it holds.
  - If the first Alt+J during a long sync blocks on the `Mutex<Connection>` (for example when opening the editor triggers a query), make sync release the lock between batches. It already batches 500 notes per transaction, so release and re-take the lock per batch.
- **The first show is often slower** than later ones (GPU surface and font cache warm-up). Measure it.
  - If it is over 30 ms, render one frame while hidden at startup to warm the caches.
  - Never use `activate_window()`, which is pitfall 1 in the first spec.

### Build profile
Try these in `[profile.release]`, measuring both speed and exe size:
```toml
panic = "abort"
strip = true
lto = "fat"
```
- Keep `lto = "fat"` only if it measurably helps over `"thin"`.
- A faster allocator (`mimalloc = "0.1"` as the `#[global_allocator]`) is allowed only if it measurably improves typing latency or search time. Otherwise leave it out.

### Rendering
- `render` must never touch the store or the disk. Check every screen.
- Don't clone large `Vec`s inside `render` or `uniform_list` closures. Index into them, or hold them in `Rc`/`Arc` and share them.
- The editor still re-renders only when the live tags actually change. Keep it that way.

---

## Milestone 14: Themes, like Helix

Lazykasten should ship with a large set of built-in themes, have a fast picker with **live preview** (like Helix's `:theme`), and support user theme files. **Switching themes must cost one frame, and themes must add nothing measurable to startup or to the hotkey.**

### Verified gpui-component 0.8.1 theme API
These were read from the crate source in the Cargo registry, so trust them:
```rust
use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeRegistry};   // all re-exported at the crate root

// Register themes from a JSON string in the ThemeSet format below. Names already registered are skipped.
ThemeRegistry::global_mut(cx).load_themes_from_str(json_str)?;      // -> anyhow::Result<()>
ThemeRegistry::global(cx).themes();          // &HashMap<SharedString, Rc<ThemeConfig>>
ThemeRegistry::global(cx).sorted_themes();   // Vec<&Rc<ThemeConfig>>: defaults first, then light, then dark, by name

// Apply one. Theme::update keeps every copy of the colors in step and refreshes all windows.
let config: Rc<ThemeConfig> = ThemeRegistry::global(cx).themes().get(name).cloned().unwrap();
Theme::update(cx, |theme| theme.apply_config(&config));
cx.theme().theme_name();                     // current theme's name
```
**Don't use `ThemeRegistry::watch_dir`.** It starts a `notify` watcher and re-reads the whole folder on every change. You don't need that at startup.

### Theme file format (gpui-component `ThemeSet`)
```json
{
  "name": "Catppuccin",
  "themes": [
    {
      "name": "Catppuccin Mocha",
      "mode": "dark",
      "colors": {
        "background": "#1e1e2e",
        "foreground": "#cdd6f4",
        "border": "#313244",
        "caret": "#f5e0dc",
        "selection.background": "#45475a",
        "muted.foreground": "#a6adc8",
        "list.hover.background": "#313244",
        "list.active.background": "#45475a",
        "list.active.border": "#89b4fa",
        "input.border": "#45475a",
        "popover.background": "#181825",
        "popover.foreground": "#cdd6f4",
        "ring": "#89b4fa",
        "scrollbar.thumb.background": "#585b70",
        "base.blue": "#89b4fa"
      }
    }
  ]
}
```
- The full key list is in `gpui-component-0.8.1/src/theme/schema.rs` (the `#[serde(rename = "...")]` names), and `default-theme.json` next to it is a complete example.
- Any key you leave out falls back to the default dark or light colors.
- **Use hex values only** in the bundled themes, so the tests below can check contrast.
- `base.blue` is what the tag chips and links use (`theme.blue`).

### Bundled themes (about 24)
Ship one JSON file per theme in `themes/`, embedded with `include_str!`, so there's no file I/O at startup.

Use each theme's **official published palette** (the hex values from its own repo or spec), mapped onto the keys above:
- Catppuccin Mocha, Macchiato, Frappé, Latte
- Gruvbox Dark, Gruvbox Light
- Tokyo Night, Tokyo Night Storm
- Nord
- Dracula
- One Dark
- Solarized Dark, Solarized Light
- Rosé Pine, Rosé Pine Dawn
- Kanagawa
- Everforest Dark, Everforest Light
- Ayu Dark, Ayu Mirage, Ayu Light
- GitHub Dark, GitHub Light
- Monokai

Keep the existing default dark theme as "Lazykasten Dark", and make it the default.

**Licenses.** Don't copy Helix's theme files: they're TOML with different keys. Map the palettes yourself. These palettes are almost all MIT licensed. List each theme's source and license in `themes/LICENSES.md`.

### Loading, without costing startup
1. **At startup, parse only the active theme.**
   - Read the saved theme name from settings.
   - Find its embedded string in a static `&[(&str, &str)]` table of name and JSON.
   - Call `load_themes_from_str` once, then apply it. That replaces the current `Theme::change(ThemeMode::Dark, None, cx)` call in `main.rs`, after `gpui_kit::init`.
2. **Parse the rest only when the picker first opens.** That's about 24 small JSONs, a few ms, done once.
3. **Read user themes then too.** Load every `*.json` in `<vault>/.lazykasten/themes/`, also only when the picker first opens. If a file is invalid, skip it and show a status message naming it. Never crash.
4. Measure that startup and the first Alt+J are unchanged from the milestone 13 numbers.

### Theme picker (Ctrl+K T, or Ctrl+Shift+P)
- It opens a new `Screen::Themes`, which reuses the existing browser layout:
  - the search box filters the theme names with a case-insensitive substring match
  - a virtualized list shows each theme's name, a `dark`/`light` label, and 5 small color swatches (background, foreground, blue, border, selection)
- **Live preview:** moving the selection with Up and Down applies that theme right away. Helix does the same.
- **Enter** keeps the theme and saves it. **Esc** reverts to the theme that was active when the picker opened.
- Add the keys to the footer hints and the README.

### Settings file
- Store the choice in `<vault>/.lazykasten/settings.toml` as one `theme = "Name"` line.
- Parse it by hand: it's one key, so don't add a TOML crate.
- Write it in the background, and only when the value changes.
- If the saved name no longer exists, fall back to "Lazykasten Dark".

### Remove hardcoded colors
`main.rs` draws the tray icon with its own RGBA, which is fine. Everything **inside the window** must come from `cx.theme()`. Grep `ui.rs` for `rgb(`, `rgba(`, `hsla(` and `Hsla`, and replace them all. Otherwise some themes will look broken.

### Tests (no App needed)
For every embedded theme JSON:
- it parses as JSON with a `themes` array
- each theme has `name`, a `mode` of `dark` or `light`, and the keys listed above, as valid `#rrggbb` or `#rrggbbaa` hex
- **readability:**
  - WCAG contrast of foreground on background is at least 7:1
  - muted.foreground on background is at least 4.5:1
  - base.blue on background is at least 4.5:1
  - write a small luminance helper for this
- names are unique

The readability check catches badly mapped palettes before a user ever sees them.

### Done when
- Opening the picker, previewing themes and switching all happen in one frame. Measure it with the trace from milestone 12, and target under 16 ms per switch.
- Startup and Alt+J are no slower than the milestone 13 numbers.
- Every bundled theme passes the readability tests.
- The chosen theme survives a restart.

---

## Milestone 15: Wrap up

1. Run the full 20k benchmark and the UI timings again. Fill in the final column of `PERF.md`, with a short summary at the top: what got faster, and by how much.
2. Update `README.md` with a "Themes" section (picker keys, custom theme folder, file format link) and the new benchmark table.
3. Update `DEMO.md`. Add a 10-second theme flip during the demo: open the picker and arrow through 5 themes live. Add the before/after speed numbers to the closing.
4. `cargo build --release` with zero warnings, all tests green, and commit.

## Priorities if time runs out

Do them in this order: **12 (measure) → 13.1–13.3 (the known slow spots) → 14 (themes) → the rest of 13 → 15.** The three known slow spots are the cheapest real speed-ups. Themes are the most visible feature.
