# Lazykasten, part 5: finish the graph, protect notes, faster, and polish

Lazykasten is at milestone 20: core app, local auto-tagging, links and backlinks, 25 themes, two performance passes, the garbled-text fix and the graph view. This prompt:
- finishes and fixes the graph
- makes notes impossible to lose
- hits the speed targets that were missed
- adds the features that matter most for a lazy Zettelkasten

**Before you change anything:**
- Re-read `HACKATHON_PROMPT.md` through `HACKATHON_PROMPT_4.md`, `PERF.md` and every file in `src/`.
- All their rules still apply:
  - zero warnings
  - **fully local**, with no LLM, network or telemetry
  - no slow work on the UI thread
  - measure before and after, and nothing may get slower
  - non-ASCII characters in Rust source written as `\u{…}` escapes
  - never write source files with PowerShell text cmdlets
  - verify in the **release** build
  - commit after every milestone

---

## Milestone 21: Finish and fix the graph

### 21.1 Bug: single-letter graph keys can swallow typing
`bind_keys` binds `t`, `o`, `1`, `2`, `3` and `ctrl-enter` in the **`"TagSearch"`** context. That's the wrapper around the search box on *every* browser screen.

**Check first:** in the release build, try typing:
- `tokyo` in the theme picker
- `tomato 123` in the browser search
- `todo` in the graph search box

If any letters go missing, or toggle graph options instead of being typed, that's the bug.

**Fix:**
- Give the graph its own key context, `"GraphView"`, on a focusable div around the canvas, using a `FocusHandle` with `.track_focus(&handle)`.
- Bind the single-letter keys (`t`, `o`, `1`–`3`) **only** in `"GraphView"`.
- When the graph opens, focus the canvas, not the search box. `/` or Ctrl+F focuses the graph search box, and Esc in the search box returns focus to the canvas.
- Keep Tab and Ctrl+Enter working in both, since they never type text.
- Update the footer hints to match.

### 21.2 Click to open, not double-click
The graph opens a node on **double-click**. Prompt 4 asked for a single click, which is also what Obsidian does.
- **A click opens the note** when the mouse moved less than 4 px between mouse-down and mouse-up.
- **Anything more is a drag** that moves the node.
- That removes the conflict between dragging and clicking.

### 21.3 Verify the untested interactions
Dragging a node, opening a node and Ctrl+Enter enlarge were never driven in the real app. Drive them now and screenshot each:
- **drag:** the node follows the cursor, and its neighbors react
- **click:** the note opens in the editor, and Esc goes back to the graph
- **Ctrl+Enter:** the window grows to about 80% of the work area, and leaving the graph restores the 760×540 size

### 21.4 Make UI testing reusable and clean up
- Replace the throwaway scripts with **one committed script**, `scripts/drive.ps1`, a reusable driver for the real window:
  - steps: show, Alt+J, keys, click (client coordinates), drag, scroll, wait, screenshot
  - it checks the window has focus before every action
  - screenshots go to `target/ui-shots/`
- Delete `..\lk_find.py`, `..\lk_gpass.ps1`, and the `lk_*.png` screenshots in `%TEMP%`.
- Add `scripts/smoke.ps1`, which uses `drive.ps1` to walk the main flows on a seeded vault: capture, browse, tag index, search, theme picker, graph. It saves one screenshot per step, so each future milestone can be checked in one command.

---

## Milestone 22: Never lose a note (data safety)

Today the text in the editor exists only in memory until Alt+J, Ctrl+S or Ctrl+N. A crash, power cut or forced restart loses it. And `store.save` writes with `std::fs::write` directly, so a crash mid-write can leave a truncated note.

1. **Atomic saves.** Write to `<id>.md.tmp` in the same folder, then `std::fs::rename` it over `<id>.md`. On Windows, `rename` replaces the existing file. Also in the same folder:
   - the settings write
   - `graph_layout.bin`
   - auto-tag's tag-line append

   During `sync_with_disk`, ignore `*.tmp` files and delete any older than a minute.
2. **Autosave while typing.** After **1.5 s with no typing**, save the open note in the background through the normal `save` path, using the existing "skip if unchanged" check.
   - Never save on every keystroke.
   - Never block typing.
   - Measure that the keystroke-to-frame trace is unchanged.
3. **Don't overwrite outside edits.**
   - Remember each note's mtime when it's opened. Before saving, compare it with the file on disk.
   - If the file was changed outside the app, for example in Obsidian, don't overwrite it. Save the editor text as `<id>-conflict-<timestamp>.md` and show a status message saying so.
   - Add a test for this.
4. **Trash instead of delete.**
   - Saving empty text currently deletes the note. Move it to `<vault>/.lazykasten/trash/` instead.
   - Add a **Delete note** action (Ctrl+Shift+Backspace in the editor) that also moves the note to the trash.
   - Never hard-delete user files.
   - Add a `#trash` screen in the browser that lists trashed notes, with Enter to restore one.

---

## Milestone 23: Hit the missed speed targets

`PERF.md` shows two misses:

### 23.1 Graph layout step: 26 ms against a target of 8 ms or less (20k notes, 26.7k edges)
Measure each of these on its own, and keep what helps:
1. **Flat quadtree.**
   - Build Barnes–Hut as a flat `Vec` of nodes (index children, no `Box`), rebuilt each step in O(n), with the centers of mass computed bottom-up.
   - Use `f32` everywhere.
   - Store positions as two separate `Vec<f32>` (x and y) for cache-friendly loops.
2. **Parallel repulsion.** Each node's force from the tree is independent, so split the nodes across all cores with `std::thread::scope`, or with `rayon = "1"` if that measures better. Springs and position updates stay single-threaded.
3. **Skip settled nodes.** Nodes whose movement stayed below a threshold for 10 steps are "asleep". Skip their spring updates, and wake them when a neighbor moves.
4. **Coarse start for the global graph.** When there's no cached layout, place nodes first by tag cluster, putting each tag's notes around a point, so it starts nearly settled.

**Target:** 8 ms or less per step at 20k notes, and settling the global graph from scratch in 5 s or less. Add both to the benchmark.

### 23.2 Hotkey first frame: 27 ms against a target of 16 ms or less
Measure *later* shows to the painted frame too, the same way, so they can be compared fairly.

If the first show is the slow one, it's almost certainly first-time GPU setup (swapchain and atlas) or font shaping. Warm it at startup:
- Shape the strings the editor screen uses once, through `window.text_system()`.
- Render one frame while hidden, if GPUI allows it without `activate_window()` (pitfall 1 in the first spec).

**Never use `activate_window()`.** If warming isn't possible, document why in `PERF.md`.

---

## Milestone 24: Editor styling (milestone 9, never done)

This was in the user's **original reference screenshot**: `Tag: Javascript, Computer Programming` with the tag values as **blue links**, plus a spellcheck squiggle under misspelled words. Milestone 9 in prompt 2 was skipped.

1. **Find the real API first.**
   - Search the gpui-component 0.8.1 source in `%USERPROFILE%\.cargo\registry\src\` for how the input or editor supports highlighting, or styled text ranges (look for `highlight`, `HighlightTheme`, `TextRun`, `diagnostic`, `squiggle`/`wavy`, `lsp`, `code_editor`).
   - The theme already carries a `HighlightTheme`. Find out what consumes it.
2. **Tags in blue.** Show the `Tag:` line values and `#hashtags` in `theme.blue`. Ctrl+Click opens that tag's index. Do `[[links]]` the same way, with Ctrl+Click opening the linked note.
3. **Spellcheck.**
   - Use the Windows `ISpellChecker` COM API: `ISpellCheckerFactory`, the `windows` crate feature `Win32_Globalization`, and `Win32_System_Com` for COM init.
   - Run it in the background on the visible text after 300 ms with no typing, and cache the results per line.
   - Draw wavy underlines in `theme.red` using the editor's diagnostic or underline support, if it exists.
   - Skip tags, links, URLs and code.
4. **Light Markdown:** headings bold and slightly larger, `**bold**` bold. Only do this if the same mechanism supports it cheaply.
5. **Typing speed must not change.** Check the keystroke-to-frame trace in a 5,000-word note.

If styled ranges really aren't possible without forking gpui-component, write down what you found in `PERF.md` under "Known limits", and move on. Don't hack around it.

---

## Milestone 25: Quick switcher and key help

1. **Ctrl+P quick switcher:** open any note by title.
   - **Fuzzy matching on titles:** the characters must appear in order, and the score favors matches at word starts and runs of consecutive characters.
   - Keep an in-memory `Vec` of `(title_lowercase, id)` loaded in the background at startup, and update it on save.
   - **Target:** 3 ms or less per keystroke at 20k titles. Measure it.
   - Reuse the browser list: Up and Down select, Enter opens, Esc closes.
   - Show recent notes first when the query is empty.
2. **Key help overlay:** **Ctrl+/** or **F1** shows every key binding, grouped by screen. It's generated from one table that `bind_keys` and the footer hints also use, so they can never disagree.

---

## Milestone 26: Live file watching

Notes edited in Obsidian or Notepad while the app runs are only picked up at the next start.
- Watch `<vault>/notes/` with **`notify = "8.2"`**. gpui-component already depends on notify 8.2.1, so this adds almost no build time.
- Collect events and wait 300 ms after the last one, then re-index only the changed paths in the background. Ignore `.tmp` files and the app's own saves: compare against the mtime the app just wrote.
- If the note open in the editor changes on disk:
  - with no unsaved edits, reload it
  - with unsaved edits, keep the editor text and save a conflict copy (milestone 22.3)
- **Idle CPU must stay at 0%.**

---

## Milestone 27: Lazy organizing tools

1. **Rename or merge a tag.**
   - In the tag list, **F2** on a tag asks for a new name.
   - Rewrite every note with that tag, in the background: its `Tag:` line entries and its `#hashtags`. Write the files atomically, then re-index.
   - If the new name already exists, the tags merge.
   - Show "Renamed #js → #Javascript in 214 notes", with the arrow written as `\u{2192}`.
   - Test it on a temp vault.
2. **Undo auto-tagging.**
   - After auto-tagging, the status reads `Auto-tagged: #A #B (Ctrl+Z to undo)`. Ctrl+Z right after opening the window removes the appended `Tag:` line and puts the note back in `inbox`.
   - Keep the undo for the last 10 auto-tagged notes in memory.
3. **Related notes.** If prompt 2's optional "related notes" wasn't built: in the editor footer, **Ctrl+R** lists the 10 most similar notes, using the auto-tagger's neighbor search, without the tagged-only filter.

---

## Milestone 28: Settings and app polish

1. **Settings file.** Extend `settings.toml`, still parsed by hand, to hold:
   - `hotkey`, default `alt+j`, parsed into a `global_hotkey` `HotKey`, with an error status if it can't be registered
   - `vault` path
   - remembered window size and position
2. **Start with Windows.**
   - Add a tray menu checkbox, "Start with Windows". It writes or removes `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\Lazykasten` = `"<exe path>" --hidden`, using the `windows` crate feature `Win32_System_Registry`.
   - Its checked state comes from reading that value at startup.
3. **App icon.**
   - Embed an icon in the exe, so Explorer, Alt+Tab and the taskbar show it. Use the `embed-resource` or `winresource` build-dependency and a `assets/lazykasten.ico` (the purple tile with the white "L", at 16, 32, 48 and 256 px).
   - Use the same art for the tray.
4. **Split `ui.rs`.** At 75 KB it's too big to work in safely. Move each screen into its own module under `src/ui/`:
   - `editor.rs`
   - `browser.rs`
   - `themes.rs` (the picker)
   - `graph_view.rs`
   - `switcher.rs`
   - `mod.rs`, which keeps the shared state and `bind_keys`

   Make **no behavior changes** in this commit. Run all tests and `scripts/smoke.ps1` before and after.

---

## Milestone 29: Wrap up

1. Re-run the 20k benchmark, the felt-latency traces and `scripts/smoke.ps1`. Fill in `PERF.md` with a summary at the top, and include the two targets from milestone 23.
2. Update `README.md`:
   - keys, using the generated table from 25.2
   - data safety: atomic saves, autosave, conflicts, trash
   - tag rename
   - settings and start with Windows
3. Update `DEMO.md`. Add a quick switcher moment, a tag rename across hundreds of notes, and a click from the graph into a note.
4. `cargo build --release` with zero warnings, all tests green, and commit.

## Priorities if time runs out

Do them in this order: **21 (graph fixes) → 22 (never lose a note) → 23 (speed targets) → 24 (editor styling) → 25 (quick switcher) → 26 → 27 → 28 → 29.** Lost notes and swallowed keystrokes are worse than any missing feature.
