# Lazykasten, part 6: Vim mode and a polish pass

Lazykasten is at milestone 29. Everything works: capture, local auto-tagging, links, themes, the graph, safe saves, the switcher, file watching and settings. This prompt does two things:
- adds an **optional Vim mode** to the editor and the lists
- makes the whole app look **finished and polished**, without slowing anything down

This is a long prompt. Work through it in order and commit after every milestone. If you run out of time, the priorities at the bottom say what matters most.

**Before you change anything:**
- Re-read `HACKATHON_PROMPT.md` through `HACKATHON_PROMPT_5.md`, `PERF.md`, `README.md` and every file in `src/`.
- All their rules still apply:
  - zero warnings
  - **fully local**, with no LLM, network or telemetry
  - no slow work on the UI thread
  - measure before and after, and nothing may get slower
  - non-ASCII characters in Rust source written as `\u{…}` escapes
  - never write source files with PowerShell text cmdlets
  - verify in the **release** build
  - check every visual change with `scripts/smoke.ps1` screenshots
  - commit after every milestone
- **No new crates unless you need them.** Prefer what's already in `Cargo.lock` (for example `unicode-segmentation` comes in through gpui-base). Write down every new dependency and its size cost in `PERF.md`.

### Speed budgets for this prompt
Measure each with `LAZYKASTEN_TRACE=1` and add it to `PERF.md`. Every existing number must stay where it is.

| Thing | Budget |
|---|---|
| Vim key handled (engine only), 5,000-word note | ≤ 0.1 ms |
| Vim key to painted frame | same as a normal keystroke today (2–8 ms) |
| Command palette, per keystroke, all commands | ≤ 1 ms |
| Browse preview: moving the selection to highlight painted | 1 frame; preview text may land a frame later |
| Graph paint with hover dimming, 20k nodes | no slower than today's paint |
| Graph layout step | stays ≤ 8 ms |
| Hotkey first frame | stays ≈ 11 ms, ≤ 16 ms with animations on |
| Idle CPU, window hidden or shown and still | 0% (no timers, no polling, frames only while something moves) |

---

## Milestone 30: Vim mode, the engine

Vim is an **option**: off by default, turned on with `vim = true` in `settings.toml`, a tray checkbox "Vim mode", or the command palette (milestone 35). When it's off, nothing about the editor changes.

### 30.1 Keep the engine separate and testable
Put the logic in a new **`src/vim.rs`** that knows nothing about GPUI:
- **Input:** the note text (a `&Rope` or `&str`), the cursor (byte offset), the selection, and one keystroke.
- **Output:** a list of edits (`range` + replacement text), the new cursor or selection, and the new mode, plus side effects like "yank this text", "save" or "hide".
- **Rules:**
  - Cursor positions are UTF-8 byte offsets that always land on a **grapheme boundary**. `x` on "é" or an emoji deletes the whole character.
  - Never copy the whole note per keystroke. Read through the rope. Don't call `value()` on every key, because it builds a string of the whole note.
  - The engine is pure, so all of Vim can be tested without a window.

### 30.2 Wire it into the editor (`src/ui/editor.rs`)
I checked `gpui-base 0.8.1`, and the text area has everything this needs:
- `TextareaState::cursor()`, `selected_range()`, `set_selected_range(range, cx)`, `set_cursor_position(...)`, `text() -> &Rope`, `insert(...)`, `replace(...)`, `range_to_bounds(&range)`, `cursor_bounds()`, `set_scroll_offset(...)`.
- `set_readonly(true, cx)` **rejects only the user's typing**. The programmatic APIs still edit; see `test_readonly_rejects_user_edits_only` in `gpui-base/src/input/base/state.rs`.

**How to wire it:**
- Wrap the text area in a div with key context `"Vim"` plus one of `"VimNormal"`, `"VimInsert"` or `"VimVisual"` for the current mode.
- **In normal and visual mode:**
  - Put the text area in read-only mode, so a stray letter can never be typed into the note.
  - Use `capture_key_down` on the wrapper to send each keystroke to the engine, apply its edits through the programmatic APIs, and stop the key going further.
- **In insert mode:** the text area behaves exactly as it does today. Only Esc and Ctrl+[ are taken, and they switch to normal mode.
- **Esc:**
  - In insert mode, Esc → normal mode. Bind it in `"VimInsert"`; the deeper context beats the app's `Back` in `"LazyKasten"`.
  - In normal mode, with nothing pending, Esc does what it does today: save and hide.
  - With a half-typed command (like `d2`), Esc cancels it.
- **Starting mode:**
  - A **new capture** (Alt+J) starts in **insert** mode, so you can type the moment the window appears. This is a capture app.
  - A note **opened from browse, the switcher or the graph** starts in **normal** mode.
  - Both are settings: `vim_start_new = "insert"`, `vim_start_open = "normal"`.
- **Cursor:**
  - **Normal mode** draws a **block cursor**: a filled rectangle from `range_to_bounds(cursor..next_grapheme)` in the theme's caret color, with the character redrawn on top in the background color, or a ~40% alpha fill if that's simpler and readable.
  - **Visual mode** uses the normal selection highlight.
  - **Insert mode** uses the normal thin caret.
  - If read-only mode hides the caret or selection, draw them yourself from the bounds APIs.
- **Mode label:** the footer shows `NORMAL`, `INSERT`, `VISUAL` or `V-LINE` (milestone 33 styles it), plus any pending keys, like `d2`.
- **Inline tokens** (the `[[link]]` pills from milestone 34) count as **one character** for motions and for `x`.

### 30.3 V1, the core commands
- **Moving:**
  - `h j k l`
  - `w b e` and `W B E`, `ge`
  - `0 ^ $`
  - `gg G`, and `5G` to go to line 5
  - counts on everything: `3w`, `5j`, `2dd`
- **Insert mode:** `i a I A o O`
- **Editing:**
  - `x X`, `r<char>`, `~`, `J`
  - `dd yy cc`, `D C Y`, `s S`
  - `p P`, which know whether the yank was a whole line
- **Undo:** `u` undo, Ctrl+R redo
  - Each normal-mode command is **one undo step**.
  - A change like `cw` + typed text + Esc should be one step too. If the undo stack can't group them, two steps is acceptable; write down why.
  - Ctrl+R is "related notes" in the app. In `"VimNormal"`, Ctrl+R means redo. Related notes stays on Ctrl+R in insert mode and in the palette. Add this to the key help.
- **Visual mode:**
  - `v` selects by character, `V` by line
  - any motion extends the selection
  - `y d c x > <` act on it, and `o` jumps to the other end
- **Clipboard:**
  - Yanks and deletes go to the **Windows clipboard** (`cx.write_to_clipboard`), and `p` reads from it.
  - To keep line-wise pastes working, remember the last yank's text and whether it was a whole line. If the clipboard still holds that exact text, paste it line-wise.
- **Lines:** `j`/`k` move by **real lines**, as in Vim. `gj`/`gk` move by wrapped display lines, through the text area's `MoveDown`/`MoveUp` actions.

### 30.4 V2, the fuller set
- **Operator + motion:** `d c y` with any motion and count, for example `d2w`, `c$`, `y3j`, `dG`, `d}`.
- **Jumping:**
  - `f t F T` to a character on the line, with `;` and `,` to repeat
  - `%` between matching brackets
  - `{ }` by paragraph (blank-line separated)
- **Text objects:** `iw aw iW aW`, `i" a" i' a'`, `i( a( i[ a[ i{ a{`, `ip ap`. Example: `ciw`, `di"`, `dap`, `yi(`.
- **Repeat:** `.` repeats the last change, including what was typed in insert mode.
- **Search inside the note:**
  - `/` and `?` open a one-line search box in the footer.
  - Enter jumps to the match, then `n` / `N` go to the next and previous.
  - `*` and `#` search for the word under the cursor.
  - Matching is plain text and ignores case unless the search has a capital letter (smartcase).
  - Matches are highlighted with the selection color, or with the bounds APIs if needed.
- **Scrolling:** Ctrl+D and Ctrl+U move half a screen. `zz` puts the cursor line in the middle.
- **Commands:** `:` opens the footer command line:
  - `:w` saves
  - `:q` hides the window, the same as Esc in normal mode
  - `:wq` and `:x` save and hide
  - `:q!` hides without saving, after a confirm toast: "Discard changes? Enter / Esc"
  - `:<number>` goes to that line
  - unknown commands show a toast with the error

### 30.5 Tests
Write **table-driven tests** in `src/vim.rs`. Each test case:
- is a starting text with `|` marking the cursor
- has a key string like `"d2w"` or `"ciwhello<esc>"`
- expects a final text and cursor

Aim for **at least 80 cases**:
- every command above
- counts
- empty lines and the last line with no newline
- non-ASCII text: "naïve café 東京 👍🏽"
- text objects at the start and end of a line
- `.` after `ciw`
- `p` line-wise and character-wise

Add a benchmark that times 10,000 keystrokes of mixed commands on a 5,000-word note. The average must be ≤ 0.1 ms. Put the number in `PERF.md`.

Then **drive it live** with `scripts/drive.ps1`, using the release build with `vim = true`:
1. Capture a new note in insert mode and type a line.
2. Press Esc, then `0`, `dw`, `u`, `A`, type more, Esc.
3. Open a note from browse, which starts in normal mode, then `ciw`, `.`, `:w`.
4. Esc hides the window.

Screenshot each step and check the footer mode label and the block cursor.

---

## Milestone 31: Vim everywhere else

This only applies when `vim = true`.

### 31.1 Lists
Lists are the browse results, tag index, switcher, theme picker, related notes, backlinks and trash.
- The search box keeps focus and typing works as it does today. `j` must type a "j".
- **Esc** in a list's search box (with text or without) **moves focus to the list**. The selected row gets a visible focus ring. In the list:
  - `j k` move down and up, with counts
  - `gg G` go to the top and bottom
  - Ctrl+D / Ctrl+U move half a page
  - Enter opens
  - `/` or `i` go back to the search box
  - **Esc** goes back, as it does today
- Bind these in a new `"ListNav"` key context, so nothing leaks into text boxes.
- When Vim is off, Esc behaves exactly as it does today.
- Footer hints update to match.

### 31.2 Graph
- In `"GraphView"`, `h j k l` pan by 10% of the view and `H J K L` by 40%.
- `+` / `-` zoom, and `0` resets the view.
- These are always on, whether Vim is on or not, because the graph canvas never takes typing.
- If any of them clash with `t`, `o` or `1`–`3`, keep the existing keys and say what you changed.

### 31.3 Key help and README
- Add a **Vim** section to the key help (Ctrl+/), generated from the same table, that only shows when Vim is on.
- Add a full table of supported Vim commands to `README.md`, plus a short "not supported" list: macros, marks, registers other than the clipboard, visual block, `:s`. Be honest.

---

## Milestone 32: Shared style tokens

Before polishing screens one by one, make them share one set of values. Otherwise every screen drifts.

Create **`src/ui/style.rs`** with:
- **Spacing:** a 4 px grid, with `SPACE_1` = 4 px up to `SPACE_8` = 32 px.
- **Type scale:** `TEXT_XS` 11, `TEXT_SM` 12, `TEXT_BASE` 14, `TEXT_LG` 16, `TEXT_XL` 20.
- **Line heights:** UI text 1.35, editor text 1.6.
- **Radius:** `RADIUS_SM` 4, `RADIUS_MD` 6, `RADIUS_LG` 10.
- **Heights:** header, footer, list row and chip.
- **Animation durations,** for milestone 38.

Then **replace the hard-coded sizes and paddings** across `src/ui/*` with these values. Grep for `px(`, `text_sm`, `text_xs`, `rounded_` and `gap_`. This commit should look almost the same; it's a refactor. Take before and after screenshots and say where they differ.

---

## Milestone 33: Typography, spacing and the footer

### 33.1 Typography and spacing
- **Fonts:**
  - **UI font:** Segoe UI, which ships with Windows 10 and 11.
  - **Editor font:** add `editor_font = "Segoe UI"` to `settings.toml`, so people can pick something like `"Cascadia Mono"` or `"Consolas"`.
  - **Don't bundle a font file** unless you measure startup and exe size, and both are fine.
- **Editor column:**
  - The text is a **centered column about 72 characters wide**, with even side padding.
  - In the default 760×540 window that's about the full width minus padding.
  - When the window is enlarged (Ctrl+Enter or focus mode), the column stays readable instead of stretching.
- **Editor text:** line height 1.6 and `TEXT_BASE` size. The note's first line (its title) is slightly larger and bolder **if** that can be done without styled ranges. If it can't, skip it and say so.
- **Header:** every screen gets the same header: same height, same padding, and the screen name on the left in `TEXT_SM`, muted. Nothing should jump when you switch screens.
- **Alignment:** list rows, chips, inputs and the footer all line up on the same left edge.

### 33.2 Footer redesign
One footer component used by every screen, `TEXT_XS`, muted, fixed height.
- **Left side, in this order:**
  1. **Vim mode badge:** a small colored pill. NORMAL uses the theme blue, INSERT green, VISUAL magenta. It only shows when Vim is on.
  2. **The note's title or file name,** shortened with an ellipsis if long.
  3. **Word count.** Counting 5,000 words costs microseconds, but don't do it on every keystroke. Update it at most every 150 ms after typing stops, or count in the background.
  4. **Save state dot:**
     - green = saved
     - yellow = unsaved changes
     - a hollow ring while autosave is writing
     - red = save failed (hover or a toast gives the error)
- **Right side:** key hints for the **current screen and mode**, generated from the `KEY_HELP` table with a `screens` field added. That way the footer hints, key help and command palette can never disagree.
- On non-editor screens, the left side shows the screen's own state, like "1,204 notes" or "#project · 38 notes".
- **The Vim `/` and `:` line takes over the footer** while it's open.

---

## Milestone 34: Tag chips, link pills and icons

### 34.1 Tag colors
- Every tag gets a **stable color**: hash the lowercased tag name with FNV-1a and pick one of the theme's six colors (`red green blue yellow magenta cyan`). The same tag is always the same color in every screen, and it changes with the theme.
- A chip is a pill with the color at about 18% over the background and text in the full color or the foreground color, whichever passes contrast.
- **Extend the existing WCAG contrast tests:** chip text must reach **4.5:1** on its chip background in **every bundled theme** for all six colors. Where a theme fails, pick the foreground instead of the color. The test proves the fallback works.

### 34.2 Where chips appear
- **Browse rows:** up to 3 chips on the right, then `+2` for more.
- **Editor header:** a row of the note's tags above the text column. Clicking one opens that tag's list, the same as `show_tag`. The row updates when auto-tagging adds tags.
- **Tag index:** each tag's name is shown as its chip.
- **Graph:** node colors, in milestone 37.

### 34.3 `[[link]]` pills in the editor
In `PERF.md`, milestone 24 says the text area can't style text, and that's true for colored ranges. But `gpui-base` has **inline tokens**, an atomic pill inside the text, and that's the supported way to show links nicely:
- **The API:**
  - `InlineToken::new(id, text).with_label(label)`
  - `TextareaState::replace_range_with_token(range, token, window, cx)`
  - `Textarea::token(renderer)` for the look
  - `Textarea::on_token_click(...)` and `on_token_hover(...)`
- **The token's `text` is what stays in the document**, so the file on disk keeps the raw `[[id|title]]`. Only the label is shown.
- **When a note loads,** turn every `[[id|title]]` and `[[id]]` into a token: id = the note id, text = the raw link, label = the title, or the id if there's no title. Do it in one pass at load.
- **When typing** completes a link (the `[[` picker inserts one, or the user types `]]`), turn just that link into a token.
- **Clicking a pill opens the linked note.** Ctrl+Click also works. Hovering shows the target's title and first line as a tooltip.
- **Look:** link color (theme blue) text, a subtle underline or muted background, a small link icon from 34.4, and the normal token selection look when selected. Links to missing notes are drawn muted with a strike or a "?" so broken links stand out.
- **Tests:**
  - **Round trip:** load a note with 3 links, edit around them, save, and check the file on disk is byte-for-byte what it should be. This is the most important test in this milestone.
  - **Speed:** a note with 500 links loads and types with no change in keystroke time.
- **Limits:**
  - Backspace on a pill deletes the whole link. Document it.
  - Don't turn `#hashtags` into tokens. People type them all the time, and an atomic token would get in the way.

### 34.4 Icons
- **Embed about 12 small SVG icons** in the exe: search, tag, link, graph, trash, note, settings, keyboard, check, warning, sun/moon and command.
- Use **Lucide** icons. They're ISC-licensed; include the license text in `assets/icons/LICENSE` and mention it in `README.md`.
- **Serve them through an embedded GPUI `AssetSource`** built from `include_bytes!`, so no files sit next to the exe. First check how gpui-component's `Icon` resolves paths (`IconName` → `"icons/<name>.svg"`) and match it.
- **Where:** list rows (note or tag), the header, the command palette, toasts, empty states and the key help.
- **Size:** icons must add **under 100 KB** to the exe. Note the size in `PERF.md`.

---

## Milestone 35: Command palette and toasts

### 35.1 Command palette (Ctrl+Shift+P)
- **Ctrl+Shift+P now opens the command palette.** The theme picker keeps Ctrl+K T and gets a command, "Change theme…".
- **Every action** in the app appears, built from the `KEY_HELP` table. Add a dispatch field to it (a `Box<dyn Action>` builder, or similar), so the palette runs the real action with `window.dispatch_action`. Nothing is hard-coded twice.
- **Each row:** an icon, the command name, and its shortcut on the right, styled like small keycaps.
- **Toggle settings appear too:**
  - Vim mode
  - Start with Windows
  - Focus mode
  - Follow Windows light/dark
  - Animations
- **Search:** fuzzy, reusing the switcher's allocation-free matcher. The **most recently used commands come first** when the box is empty; keep the last 10 in `settings.toml`. Budget: ≤ 1 ms per keystroke.
- **Keys:** Enter runs the command and closes the palette, and Esc closes it. Vim list keys from 31.1 work here too.

### 35.2 Toasts instead of status text
Today, messages go into `status` / `keep_status` and show as footer text. Replace that with **toasts**:
- **Where:** a small card above the footer, bottom center, with an icon, a message and an optional action with its key, for example **"Tagged 12 notes · Ctrl+Z Undo"**.
- **How long:** at most 3 at once. Info toasts last 4 seconds. Errors stay until dismissed with Esc or a click.
- **Animation:** fade and slide in over about 120 ms, using milestone 38's helper. Without animations they just appear.
- **Migrate every call site.** Grep `self.status =` across `src/ui/*`; there are dozens. The footer keeps only lasting state: mode, title, words and the save dot.
- **Timing:** use one timer per toast (`cx.spawn` + `cx.background_executor().timer(duration).await`), not a polling loop. When there are no toasts, nothing runs.

---

## Milestone 36: Browse redesign and empty states

### 36.1 Two-pane browse
- **Layout:** the browse, search and tag screens show the **list on the left (about 45%)** and a **preview of the selected note on the right (about 55%)**. Under 700 px of width, hide the preview.
- **Loading the preview:**
  - Load it **in the background** with a generation number, like search does.
  - The selection highlight moves **in the same frame** you press the key, and the preview text lands when ready, usually the next frame.
  - Show only the first ~4 KB, read-only, with link pills and tag chips.
  - Keep the last 32 previews in a small LRU cache, so going up and down is instant.
- **Rows:**
  - the title in `TEXT_BASE`
  - a one-line snippet in `TEXT_SM`, muted, **with the matching words highlighted**
  - a relative time on the right: "just now", "5m", "2h", "Yesterday", "Mar 3", "2025"
  - tag chips (34.2)
- **Snippet highlights:** use FTS5's `snippet()` with marker characters (for example `'\u{1}'` and `'\u{2}'`) and turn the marked parts into colored text runs. Don't search the text a second time.
- **Speed:** Enter still opens the note in the editor, and nothing gets slower. Measure browse to first frame, search at 500 hits, and holding the down arrow through 200 rows.

### 36.2 Empty and first-run screens
- **First run:** when the vault has no notes, or `onboarded = true` isn't in settings yet, show a short welcome screen in the editor area:
  - "Press **Alt+J** anywhere to capture a thought."
  - "Type. **Esc** saves and hides."
  - "Tags are added for you. **Ctrl+O** browses, **Ctrl+G** shows the graph."

  Show three keycap-style cards. Any key starts a note and sets `onboarded = true`.
- **No search results:** "No notes match "x". Press **Enter** to create a note titled x." Enter does that.
- **Other empty states:**
  - **Empty tag index:** "No tags yet. Write a few notes and they'll appear."
  - **Empty trash:** "Trash is empty."
  - **No backlinks or related notes:** one muted line.
  - **Graph with no links:** "Link notes with `[[` to see connections." The local graph of an unlinked note shows only that note, with this hint.
- **Look:** every empty state is centered, has an icon (34.4), and uses `TEXT_SM`, muted.

---

## Milestone 37: Graph polish

Keep the layout step ≤ 8 ms and don't make painting slower. All of this happens in the existing batched paint, with no extra passes over 20k nodes.

- **Node colors:** a node takes the color of its **most-used tag**, using the same hash as the chips. Untagged notes are muted. `c` toggles colors on and off.
- **Hover:**
  - Hovering a node (the existing hit-testing grid finds it) **highlights it and its direct neighbors**, at full color with their edges brighter.
  - Everything else dims to about 25% alpha.
  - Moving off it restores everything.
  - The node's title shows as a label even when zoomed out.
  - Dimming is just a different alpha in the same batched paths. Don't paint twice.
- **Smooth zoom:** the scroll wheel and `+`/`-` zoom toward the cursor, eased over about 120 ms. Each frame moves part of the way to the target zoom, and frames are requested only until it arrives.
- **Labels:** instead of popping in at a zoom threshold, labels **fade in** as you zoom: alpha goes from 0 to 1 across the last ~30% of the zoom range before they would have appeared. Keep the cached `ShapedLine` labels.
- **Legend:** a small box in the top-right with the **top 6 tags** and their colors, plus "untagged". Clicking a tag in the legend highlights its notes like hover does, and clicking again clears it.
- **Measure:** paint time and layout step at 20k notes, with and without hover, in `PERF.md`.

---

## Milestone 38: Small animations and focus mode

### 38.1 Animations
**The first frame after Alt+J must still paint immediately.** Animations play over a frame that's already visible. They never delay it.
- **Window show:** the content starts at about 0.9 opacity and 4 px lower, then eases to normal over about 90 ms. It must never start invisible.
- **List selection:** the highlight **slides** to the new row over about 60 ms. Holding the arrow key must never lag behind; skip the slide if a key repeats.
- **Toasts:** fade and slide, as in 35.2.
- **Screen switches:** a fade of about 80 ms at most, or none. If it makes anything feel slower, drop it.
- **Off switches:**
  - Read Windows' "Show animations" setting (`SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION)`) at startup.
  - Add `animations = false` to settings and a palette toggle.
  - When animations are off, everything snaps instantly.
- **Helper:** one helper in `style.rs` with the durations and easing. Use GPUI's `AnimationExt::with_animation` with `Animation::new(duration).with_easing(...)`; both are in `gpui-pre 0.3.10`, in `src/elements/animation.rs`. For things it doesn't fit, like the graph zoom, use `request_animation_frame` only while something is moving.
- **Measure:** hotkey first frame with animations on and off. It stays ≤ 16 ms. Idle CPU stays 0% once animations finish.

### 38.2 Focus mode (F11)
- **Entering:** F11, or the palette, enlarges the window like Ctrl+Enter does (about 80% of the work area, centered) and **hides the header, chips and footer**. Only the centered text column is left.
- **Typewriter scrolling:** the line with the cursor stays near the **vertical middle** of the window. After a cursor move that changes the line, adjust with `set_scroll_offset`, using `cursor_bounds()`. Only adjust when the line changes, never on every keystroke within a line.
- **Footer:** a faint word count appears at the bottom only while you're not typing; it fades in after 2 seconds idle.
- **Leaving:** Esc leaves focus mode first, restoring the size and the chrome. The next Esc behaves as usual. In Vim normal mode, the first Esc also leaves focus mode.
- **Vim:** everything works the same in focus mode.

---

## Milestone 39: Follow Windows light/dark and the window frame

### 39.1 Follow Windows light/dark
- **Settings:**
  - `theme_mode = "system"`, `"light"` or `"dark"`
  - `theme_light = "<name>"` and `theme_dark = "<name>"`
  - Defaults: `"system"` with the nicest light and dark themes you already ship.
  - The old single `theme = "Name"` still works and means a fixed theme.
- **Reading the Windows setting:** at startup, read `HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize\AppsUseLightTheme` (a DWORD where 0 means dark).
- **Reacting to changes without polling:**
  - A background thread calls `RegNotifyChangeKeyValue` on that key with an event and waits on it.
  - When it fires, re-read the value and send it to the UI thread through a channel, which applies the theme. Theme switches already cost under 0.1 ms.
  - This is part of the `windows` crate's `Win32_System_Registry` feature, already used for "Start with Windows".
- **Picker and menus:**
  - The theme picker shows which theme is used for light and which for dark when following Windows. Choosing a theme while following asks which slot it goes in, or puts it in the slot for the current mode. Pick the simpler one and document it.
  - Add a tray menu checkbox, "Follow Windows theme".
- **Verify:** switch Windows between light and dark (Settings → Personalization → Colors) while the app runs, and screenshot both.

### 39.2 Window frame (Windows 11 only)
**This machine runs Windows 10, so you can't see any of this.** Keep it small and safe, and say so in `README.md` and the final summary.
- **Rounded corners:** after the window is created, call `DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE (33), DWMWCP_ROUND (2))`. On Windows 10 the call fails harmlessly. Ignore the error, and don't check the version.
- **Mica or blurred background:** **don't add it.** It needs a transparent window and changes every theme's background, and you can't check it here. Write it down as an idea in `README.md`.
- **Check on Windows 10:** nothing changed, and frame time and first frame are the same.

---

## Milestone 40: Screenshot review loop

This milestone polishes the look by checking real screenshots instead of guessing. Do it **after** milestones 30–39.

### 40.1 Capture everything
Extend `scripts/smoke.ps1` with a `-Themes` parameter and a `-Full` switch that captures **every screen and state**:
- **Editor:**
  - empty capture
  - a long note with links and tags
  - Vim normal mode with the block cursor
  - Vim visual selection
  - the `/` search line and the `:` line
- **Browse** with a preview, and with highlighted snippet matches
- **Search with no results**
- **Tag index**
- **The tag rename prompt**
- **Theme picker**
- **Command palette:** empty and with a query
- **Key help**
- **Toasts:** an info toast with an action, and an error toast
- **Graph:**
  - global view
  - local view
  - hovering a node
  - the legend
  - zoomed in with labels
- **Trash:** with items, and empty
- **First-run welcome screen**
- **Focus mode**
- **Window size:** both the default 760×540 and enlarged

Run it in **three themes**:
1. the default dark theme
2. the lightest light theme
3. the theme with the **lowest contrast margin** in your contrast tests, because problems show up there first

### 40.2 Review and fix
Look at each screenshot yourself. Write the problems you find into `docs/ui-review.md`, with the screenshot name and what's wrong. Check each one against this list:
- [ ] Everything lines up on the 4 px grid and on the shared left edge.
- [ ] No text is cut off without an ellipsis. Long titles, long tags and long paths all shorten cleanly.
- [ ] Every piece of text is readable in all three themes. Use the contrast numbers; don't guess.
- [ ] Padding is the same on every screen. The header and footer are the same height everywhere.
- [ ] The focused element is always visible: the search box, a list row, a graph node.
- [ ] Chips never overflow their row or push the time off the edge.
- [ ] Icons are the same size and vertically centered with their text.
- [ ] Nothing jumps when switching screens.
- [ ] Empty states are centered and don't look broken.
- [ ] Nothing is garbled. Every non-ASCII character renders properly; check for mojibake again.

**Then repeat:**
1. Fix the problems.
2. Re-run `smoke.ps1 -Full`.
3. Update `docs/ui-review.md`, marking each item fixed or explaining why it's left.
4. Repeat until a round finds nothing new, **up to 3 rounds**.
5. Commit after each round.

Don't commit the screenshots themselves, except the ones picked for the README in milestone 41.

---

## Milestone 41: Demo polish and wrap up

### 41.1 Demo polish
- **README screenshots:** pick **5–6 of the best screenshots** from milestone 40:
  - the editor with chips and link pills
  - browse with a preview
  - the command palette
  - the graph with hover
  - Vim mode
  - one light theme

  Save them to `docs/img/` as PNGs under 300 KB each and put them at the top of `README.md`.
- **GIF:** only if `ffmpeg` is already installed. Check with `where.exe ffmpeg`; **don't download it**. If it's there, make one short GIF (under 3 MB) of capture → auto-tag → browse → graph. If it isn't, skip the GIF and say so.
- **About box:** add "About Lazykasten" to the tray and palette. It's a small panel with:
  - the version (`CARGO_PKG_VERSION`)
  - "100% local: no network, no accounts, no AI service"
  - the vault path
  - the license
  - a link to the key help

### 41.2 Wrap up
1. Re-run the 20k benchmark, the felt-latency traces, the Vim benchmark and `scripts/smoke.ps1 -Full`. Update `PERF.md` with:
   - a summary at the top
   - every budget from the top of this prompt, with measured or not measured
   - new dependencies and the exe size change
2. Update `README.md`:
   - screenshots
   - Vim mode: how to turn it on, supported commands, what's not supported
   - command palette
   - focus mode
   - follow Windows theme
   - the Windows 11 corner note
3. Update `DEMO.md` with new moments:
   - turn on Vim from the palette and edit a note with `ciw` and `.`
   - browse with the live preview
   - hover in the graph and click the legend
   - focus mode
   - switch Windows to light mode live and watch the app follow
4. Update `KEY_HELP` so key help, footer hints and the palette all agree.
5. `cargo build --release` with zero warnings, all tests green, `smoke.ps1 -Full` clean, and commit.
6. **Clean up:** no stray scripts, screenshots or temp files outside `target/` and `docs/`.
7. **Final summary:** list what was done, what was measured, what couldn't be verified on this machine (the Windows 11 corners, anything else) and anything you skipped and why.

---

## Priorities if time runs out

Do them in this order:
**30 (Vim engine) → 31 (Vim in lists and graph) → 32 (style tokens) → 33 (typography and footer) → 34 (chips, link pills, icons) → 35 (palette and toasts) → 36 (browse and empty states) → 37 (graph polish) → 40 (screenshot review) → 38 (animations and focus mode) → 39 (light/dark, frame) → 41 (wrap up).**

- If time is short, still do **40** (one round is enough) and **41**.
- A working Vim mode and a consistent look matter more than any single extra feature.
- **Nothing in this prompt is allowed to make typing, showing the window or searching slower.** If a polish item costs speed and you can't fix it, drop the item and write down why.
