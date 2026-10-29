# Lazykasten, part 4: fix the garbled text, polish, speed, graph view

Lazykasten is at milestone 15: core app, local auto-tagging, links and backlinks, demo work, 25 themes with a live-preview picker, and a first performance pass (`PERF.md`). This prompt:
- fixes the garbled characters
- fixes the theme picker spacing
- does a second speed pass on what the user actually *feels*
- adds an **Obsidian-style graph view** that stays fast at tens of thousands of notes

**Before you change anything:**
- Re-read `HACKATHON_PROMPT.md` through `HACKATHON_PROMPT_3.md`, `PERF.md` and every file in `src/`.
- All their rules still apply:
  - zero warnings
  - fully local, with no LLM or network
  - no slow work on the UI thread
  - verify UI changes by running the **release** build
  - measure before and after
  - commit after every milestone
- **Nothing may get slower.**

---

## Milestone 16: Fix the garbled characters (mojibake)

### What the user sees
- Placeholders read `Capture a thoughtâ€¦` and `Search all notesâ€¦`.
- The heading reads `1 tags Â· 4 notes`.
- The footer shows `â†‘â†“` where the arrows should be.

### Cause
`src/ui.rs` itself contains garbled text, introduced in the milestone 14 commit. UTF-8 text was read as Windows-1252 and saved back as UTF-8, so:
- `…` became `â€¦`
- `·` became `Â·`
- `↑↓` became `â†‘â†“`

This is what happens when a file goes through Windows PowerShell 5.1's `Get-Content` / `Set-Content` without `-Encoding`: they assume the ANSI code page.

### Fix
1. **Fix the 8 broken strings in `ui.rs`.** They're the two placeholders, the `·` in the headings (tags, tag index, backlinks), and the three footer hint strings.
   - **Write every non-ASCII character in Rust source as an escape:** `\u{2026}` for …, `\u{00B7}` for ·, `\u{2191}\u{2193}` for ↑↓, `\u{2713}` for ✓.
   - That keeps the source pure ASCII, so it can't be garbled again.
2. **Sweep the whole repo:** `src/`, `themes/`, `scripts/` and every `.md`. Look for the telltale sequences `â€`, `â†`, `Â`, `Ã` and the replacement character `\u{FFFD}`, and fix each one.
3. **New rule for the rest of the project.** Never write or rewrite source files through PowerShell text cmdlets. Use your file-editing tool.
   - If a script must write text, use `[IO.File]::WriteAllText($path, $text, [Text.UTF8Encoding]::new($false))`, which is UTF-8 with no BOM.
4. **Add a guard test.** A `#[test]` that reads every file under `src/` and `themes/` with `include_str!` or `std::fs`, and fails if it finds `â€`, `â†`, `Â·`, `Ã` or `\u{FFFD}`. A garbled string then never ships again.

### Make note reading robust too
Note files come from other editors, and from `seed_demo.ps1`, which uses `Set-Content -Encoding UTF8`. In PowerShell 5.1 that writes a **BOM**.
- **Strip a leading BOM (`\u{FEFF}`) when reading a note.** Otherwise a note whose first line is `Tag: …` doesn't parse as tagged, and the title can start with an invisible character.
- `store.rs` reads files with `read_to_string(...)?`. One non-UTF-8 file (say, a Windows-1252 note from an old editor) can make the whole `sync_with_disk` fail.
  - Read the bytes instead, decode with `String::from_utf8_lossy`, and never fail the sync because of one file.
  - Report how many files needed lossy decoding in the trace log.
- Add tests: a BOM-prefixed tagged note parses correctly, and a file with invalid UTF-8 is indexed instead of aborting the sync.
- Change `seed_demo.ps1` to write UTF-8 without a BOM, as in rule 3 above.

**Check:** run the release build, screenshot the editor placeholder, the browser heading, the search placeholder and the footer, and confirm they read `…`, `·` and `↑↓`.

---

## Milestone 17: Theme picker layout

The `light`/`dark` label sits right against the first color swatch, and light swatches blur into light backgrounds.

Rework the theme row in `render_row` (`Screen::Themes`) into three clear columns, with `.gap_4()` between them:
1. **Name:** `flex_1`, cut off with an ellipsis if too long. Prefix the current theme with `\u{2713} `.
2. **Mode:** a fixed-width column (about `w_12`), right-aligned, muted, small text, with `mr_2` before the swatches.
3. **Swatches:** `gap_1`. Give each swatch a 1px `theme.border` border, so a white swatch is still visible on a light theme and a near-black one on a dark theme.

**Check:** screenshot the picker in one dark and one light theme. The name, mode and swatches must be clearly separated, and every swatch visible.

---

## Milestone 18: Second speed pass (what the user feels)

The user says things feel slower. `PERF.md` measured the backend, but not the moments the user waits on. Measure those first.

### 18.1 Measure what the user feels
Add these `LAZYKASTEN_TRACE` timings. Each is measured to **the next painted frame**, using `window.on_next_frame(|window, cx| …)`, which is verified to exist in gpui-pre 0.3.10:
- a keystroke in the editor, in a short note and in a 5,000-word note
- Ctrl+O to the tag list on screen
- clicking a tag to its index on screen, for a small tag and for the largest one (`inbox` at 20k notes)
- opening a note from a list to the editor on screen
- a theme preview step in the picker
- the hotkey show, first and later

Record them in a new **"Felt latency"** table in `PERF.md`. The target for every row is **one frame: 16 ms or less.**

### 18.2 Known slow spots in the current code (fix these first)
1. **`render_row` clones the entire theme for every visible row, every frame.** It calls `let theme = cx.theme().clone();`. `Theme` is big: around 100 colors, tokens, fonts and `Rc` configs.
   - Copy only the few `Hsla` values the rows need (`blue`, `muted_foreground`, `list_hover`, `list_active`, `border`) into a small `Copy` struct, once per `render`.
   - Pass that struct into the list closure. Do the same anywhere else `cx.theme().clone()` appears in a render path.
2. **Screen switches still query SQLite on the UI thread:**
   - `show_tag` calls `notes_with_tag`, which is **13 ms for a 20k-note tag**. That's a dropped frame every time.
   - `show_backlinks` calls `backlinks`.
   - Opening a note calls `self.store.backlinks(id)` just to count them.
   - `show_tags` loads the tag list.

   Move all of them to `cx.background_spawn` with generation numbers, exactly like the search fix in milestone 13.
   - **Switch the screen immediately**: show the heading, and keep the old rows until the new ones arrive.
   - For the backlink count, show the footer without it and fill it in when it arrives.
   - Add a cheap `backlink_count(id)` query (`SELECT count(*)`) instead of loading every backlink row just to count them.
3. **The hotkey first show got slower:** 14.7 ms before milestone 12, 20.6 ms now. Find what was added to the show path (`on_show` and the command loop) since milestone 12, and move it off.
   - **Nothing on the show path may touch the disk, the DB or the theme registry.**
   - Startup settings and theme loading must happen before the first show, not during it.
4. **Keystroke cost in long notes.** Every keystroke runs `parse_tags` and `open_link_query` over the **whole** note.
   - If the 5,000-word keystroke trace is over 2 ms, make both incremental. `open_link_query` should look backwards from the cursor only, with at most 200 characters scanned.
   - Re-parse tags only when the edited text contains `#`, `:` or a newline, or when the previous parse found tags on the edited line. Otherwise keep the cached result.
   - Measure before and after.
5. **Search is about 15 ms at 500 hits,** dominated by bm25-ranking every match. Measure these and keep what wins:
   - Show 200 results with a "200+ matches" heading instead of 500. Nobody scrolls past 200 search hits.
   - For queries of 2 characters or fewer, order newest-first (`ORDER BY n.id DESC`) instead of `rank`. Ranking a 2-letter prefix is meaningless anyway, and ordering by id lets SQLite stop early.
   - The FTS `prefix='2 3'` option (schema bump), if it measurably beats both.

### 18.3 Release build reminder
`cargo run` is a **debug** build and is many times slower. Make sure `README.md` and `DEMO.md` say to use `cargo run --release`, or `target\release\lazykasten.exe`, for anything speed-related.

---

## Milestone 19: Graph view (like Obsidian, but fast at 20k notes)

An interactive graph where notes are dots and `[[links]]` are lines, with optional tag nodes. It must stay smooth at tens of thousands of notes, which Obsidian's graph doesn't.

### Modes and keys
- **Ctrl+G** opens the graph: the **local graph** of the open note if there is one, otherwise the **global graph**.
- Inside the graph:
  - **Tab** switches between local and global.
  - **T** toggles tag nodes.
  - **1/2/3** sets the local depth, default 1.
  - **Esc** goes back.
- The **local graph** is the current note plus everything within N link hops, plus notes sharing a tag when tag nodes are on. It's small, so it opens instantly.
- The **global graph** shows every note that has at least one link, plus tag nodes when toggled on. Add an "orphans" toggle (**O**) to include unlinked notes.

### Data (background thread)
- Add `store.graph_edges()` returning `Vec<(i64, i64)>` of note **rowids** from the `links` table, and `store.graph_nodes()` returning `(rowid, id, title, first_tag)`.
- Use integer indices everywhere in the graph. Never use string ids in hot loops.
- Tag nodes: one node per tag, with an edge to each of its notes. Cap it: skip tags with more than 2,000 notes, like `inbox`, which would turn the graph into a hairball.
- Load with `cx.background_spawn`. While it loads, show a "Loading graph…" heading, with the character written as `\u{2026}`.

### Layout: force-directed, off the UI thread
- **Algorithm:**
  - Run on a dedicated background thread.
  - Use a force-directed layout (repulsion between all nodes, springs along edges, gentle gravity to the center) with **Barnes–Hut** (a quadtree, θ ≈ 0.8). That makes repulsion O(n log n) instead of O(n²).
  - Use `Vec<[f32; 2]>` positions and velocities, with plain arrays and no allocation per step.
- **Initial positions:** deterministic. Hash each note id into a position, so the layout looks the same every time.
- **Cache:** after the global layout settles, save the positions to `<vault>/.lazykasten/graph_layout.bin` (rowid plus x, y). Next time, start from the cached positions, so the global graph opens already laid out and only new notes need settling.
- **Hand positions to the UI:** after each step, send a snapshot (an `Arc<Vec<[f32; 2]>>`) through a channel, at most once per frame. The UI keeps only the latest one.
- **Cooling:** the simulation stops when the total movement falls below a threshold. **When settled and idle, the graph uses 0% CPU.** Dragging a node "reheats" it.
- **Target:** one simulation step at 20k nodes and 40k edges in 8 ms or less on the background thread. Add this to the `#[ignore]` benchmark, and extend the 20k generator to add 1–3 random `[[links]]` per note.

### Rendering: one `canvas`, culled, level of detail
These GPUI APIs were checked in the gpui-pre 0.3.10 source:
```rust
canvas(
    |bounds, window, cx| { /* prepaint: compute visible nodes, return data */ },
    |bounds, data, window, cx| {
        window.paint_quad(quad(node_bounds, corner_radii, color, 0., transparent, BorderStyle::default())); // round dot
        window.paint_quad(fill(bounds, bg));
        let mut pb = PathBuilder::stroke(px(1.));
        pb.move_to(a);
        pb.line_to(b);              // many segments in one builder
        if let Ok(path) = pb.build() { window.paint_path(path, edge_color); }
        let line = window.text_system().shape_line(text, font_size, &runs, None); // label (no newlines)
        let _ = line.paint(origin, line_height, TextAlign::Left, None, window, cx);
    },
).size_full()
```
Interaction goes on the wrapping `div`: `.on_mouse_down(MouseButton::Left, …)`, `.on_mouse_move(…)`, `.on_mouse_up(…)` and `.on_scroll_wheel(…)`, all of which exist on `Div`. Call `window.request_animation_frame()` while the simulation runs or a drag is active.

Rules for speed:
- **Cull:** only draw nodes and edges whose positions fall inside the visible bounds, plus a margin.
- **Batch edges:** put all visible edges in **one** `PathBuilder` per color, never one path per edge. Paths are tessellated on the CPU, so measure the cost.
  - If tessellating the visible edges takes over 4 ms, cap the edges drawn per frame. Draw the shortest on-screen edges first.
  - While zoomed out past a threshold, draw only the edges of the hovered or selected node.
- **Level of detail:**
  - When zoomed out, nodes are 2–3 px quads with no labels.
  - When zoomed in, node size grows with link count (degree), and labels show only for on-screen nodes, capped at 150.
  - Cache each node's `ShapedLine` the first time it's shown, and never re-shape a label every frame.
- **Colors come from the theme:**
  - Edges use `theme.border`, and the background is `theme.background`.
  - Each note node is colored by its first tag, hashed into the theme's base colors (`blue`, `green`, `yellow`, `red`, `magenta`, `cyan`, which are fields on `ThemeColor`).
  - Tag nodes use `theme.muted_foreground`.

  The graph has to look right in all 25 themes.

### Interaction
- **Wheel** zooms around the cursor, and **dragging the background** pans.
- **Hovering a node** highlights it and its neighbors, dims everything else, and shows its title.
- **Clicking a node** opens that note in the editor, or that tag's index for a tag node.
- **Dragging a node** moves it and pins it while dragging, and reheats the simulation.
- **The search box filters the graph:** matching nodes stay bright, and the rest dim. Reuse the existing background search with generation numbers.
- **Hit testing** uses a spatial grid rebuilt from each position snapshot. Never loop over all 20k nodes for each mouse move.

### Window size
The 760×540 window is small for a graph. While the graph is open, **Ctrl+Enter** toggles a larger size: 80% of the work area on the cursor's monitor, using the same `SetWindowPos` method as `place_on_cursor_monitor`. Leaving the graph restores the normal size.

### Targets (release build, 20k notes, about 40k edges)
| Operation | Target |
|---|---|
| Open local graph | 1 frame (≤ 16 ms) |
| Open global graph with a cached layout | ≤ 100 ms to the first frame |
| Pan or zoom when settled | 60 fps, paint ≤ 8 ms on the UI thread |
| Simulation step (background) | ≤ 8 ms |
| CPU when settled and idle | 0% |
| Extra memory with the graph open | ≤ 30 MB |

### Tests
- Barnes–Hut repulsion matches brute force within 5% on 300 random nodes.
- The layout is deterministic: same input gives the same positions after N steps.
- The graph loader includes links, tag nodes on and off, the 2,000-note tag cap and orphans on and off.
- The spatial-grid hit test agrees with brute force.
- The layout cache round-trips.

---

## Milestone 20: Wrap up

1. Re-run the 20k benchmark and every felt-latency trace. Fill in the final columns of `PERF.md`, with a summary at the top.
2. Update `README.md` with a "Graph view" section (keys, local and global, tag nodes) and the new tables.
3. Update `DEMO.md`: open the local graph of a note, Tab to global, zoom into a cluster, and click a node to open it. Make sure the demo vault (`seed_demo.ps1`) creates enough `[[links]]` for the graph to look good. Aim for about 2 per note, with clusters by topic.
4. `cargo build --release` with zero warnings, all tests green (including the garbled-text guard), and commit.

## Priorities if time runs out

Do them in this order: **16 (garbled text) → 17 (picker spacing) → 18.1–18.2 (felt speed) → 19 local graph → 19 global graph → the rest.** Broken characters and dropped frames are what users notice first.
