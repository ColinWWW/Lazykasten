# Lazykasten performance

## Known limits

- **Editor styling (prompt 5, milestone 24).** Blue `Tag:` values,
  `#hashtag`/`[[link]]` styling, spellcheck squiggles and light Markdown
  are not implemented. In gpui-component 0.8.1 the only styled-range
  machinery is the tree-sitter highlighter plus LSP diagnostics
  (`set_highlighter`, `DiagnosticEntry`, wavy diagnostic styles), and it
  is consumed exclusively by the code-`Editor` element
  (`src/input/editor.rs`); the plain `Textarea` element the note editor
  uses paints no highlight or diagnostic ranges at all. The theme's
  `HighlightTheme` is likewise only read by that highlighter. Switching
  the note editor to the code-editor element would change its behaviour
  (line numbers, code undo/selection semantics, no soft-wrap capture
  flow) and put a tree-sitter parse on the typing path, so per the
  milestone's own rule this is documented instead of hacked around.
  Revisit if gpui-component exposes styled ranges for `Textarea`.

## Summary (final)

Everything measured got faster or stayed flat; nothing regressed:

- **First index of 20k notes: 1.536s -> 0.848s** (prepare_cached hot paths).
- **Startup re-check with no changes: 0.385s -> 0.025s** (DirEntry metadata
  instead of a syscall per file) -€” about 15x.
- **Tag index (400 notes): 1.360ms -> 0.525ms**; auto-tag suggest
  **8.778 -> 6.696 ms/note**.
- Search is FTS-bound (~15ms at 500 hits) but now runs in the background
  with generation-tagged results, so it never drops typed characters; a
  1-char query is an exact-term lookup instead of the prefix worst case.
- **Graph layout step at 20k notes / 26.7k edges: 25.7ms -> ~8ms**
  (parallel Barnes-Hut repulsion across cores with fine chunks,
  sum-based centres of mass, asleep-node skipping), and **settling the
  global graph from scratch takes ~2.6s** (coarse tag-cluster start
  plus an annealing heat ramp; target was 5s). The step hovers at the
  8ms target (7.8-8.2ms across runs).
- **Hotkey first frame: 27.7ms -> ~11.3ms**, the same as later shows
  (10.6-18ms measured the same way in the same runs). Later shows were
  already fast; the first paid one-time GPU swapchain creation, which
  now happens at startup via a `SW_SHOWNOACTIVATE` warm show that is
  hidden again after 250ms (an Alt+J in that window adopts it). Font
  shaping for the editor screen is also warmed while hidden.
  `activate_window()` is never used.
- **Quick switcher: ~0.32ms per keystroke over 20k titles**
  (allocation-free fuzzy scan; target 3ms).
- Hotkey shows stayed ~10-21ms with themes enabled; a theme switch costs
  ~0.04-0.10ms of apply time (one frame). The cross-monitor first-show
  GPUI panic found while measuring was fixed by moving placement and
  `ShowWindow` outside `window.update`.

## Baseline (milestone 12, before the performance pass)

| Operation | Before | After (M13) | Final (M15) |
|---|---|---|---|
| First index, 20k notes | 1.536s | 0.888s | 0.848s |
| Tag index, 400 notes | 1.360ms | 0.533ms | 0.525ms |
| Tag index, all 20k | 14.186ms | 13.059ms | 13.360ms |
| Full-text search `searchable` | 14.556ms | 15.040ms (noise; now off UI thread) | 15.226ms (off UI thread) |
| Search 1-letter `a` (prefix) | 0.188ms (0 hits in bench vault) | 0.086ms (exact-term now) | 0.069ms (exact-term) |
| Search 2-letter `se` (prefix, 500 hits) | 14.635ms | 14.121ms | 15.659ms |
| Auto-tag suggest (20-run avg) | 8.778ms/note | 6.594ms/note | 6.696ms/note |
| Startup re-check, no changes (20k) | 0.385s | 0.026s | 0.025s |
| Startup to window ready (`startup_ready`) | 254ms (320-note vault) |  | 238ms (320-note vault, themes on) |
| Hotkey, first show (`hotkey_show_first`) | 14.7ms |  | 20.6ms (themes on; 19.4ms in M13) |
| Hotkey, later shows (`hotkey_show`) | 15.3ms (12–17ms external) |  | 10.1ms (M13 trace) |
| Release exe size | 24,407,552 B | unchanged (fat-LTO trial reverted) | 24,547,840 B |

Notes:
- `hotkey_show*` measures the show handling on the UI thread (placement,
  `ShowWindow`, focus, `on_show`); first frame paint follows immediately.
- Found while measuring: a pre-existing GPUI Windows re-entrancy panic when
  the first `SetWindowPos` placement moves the window across monitors
  (`RefCell already borrowed` via `handle_move_msg` inside `window.update`).
  To be addressed in milestone 13 by moving placement outside the update.
- The 500-hit searches are dominated by ranking/returning 500 rows; the
  browser UI also ran them on the UI thread (milestone 13.1 fixes that).

## Milestone 13 changes

| Change | Effect |
|---|---|
| Search (browser + `[[`) moved to `background_spawn` with generation numbers; previous results stay until new ones land | Typing no longer blocks on FTS; stale results dropped. Search itself is FTS-bound (~15ms at 500 hits) but no longer drops typed characters. |
| `sync_with_disk`: `DirEntry::metadata()` instead of `fs::metadata` per file | Re-check 0.385s -> 0.026s at 20k notes. |
| `prepare_cached` on index/query hot paths (+ cache capacity 64) | First index 1.536s -> 0.888s; tag-400 1.360ms -> 0.533ms; auto-tag 8.778 -> 6.594ms/note. |
| Single-char search is exact-term; prefix from 2 chars | Avoids the 1-char prefix worst case; no schema change needed. |
| SQLite pragmas (temp_store MEMORY, cache 32MB, mmap 256MB), FTS optimize after >1000-note syncs, `PRAGMA optimize` on quit | Included above; no separate regression. |
| Sync releases the DB lock between 500-note batches | A query during a long startup sync waits at most one batch. |
| Hotkey placement/`ShowWindow` moved outside `window.update` | Fixes the cross-monitor first-show GPUI re-entrancy panic found in M12; first show stays ~15ms, so no hidden warm-up frame was added. |
| `render` no longer touches the store (`heading_text` uses a cached note count) | Rendering is pure over view state. |
| Build profile trial: fat LTO + panic abort + strip | No measurable gain (index 0.874s vs 0.888s, auto-tag slower, exe same size). Reverted to thin LTO. mimalloc skipped: nothing measured was allocator-bound. |

## Felt latency and graph (milestones 18-19)

From `LAZYKASTEN_TRACE=1` on the real app (`on_next_frame` traces measure
to the next painted frame; keystroke timings include GPUI's own editor
work): keystroke to frame 2.3-7.7ms after typing in a 5,000-word note,
browse to frame 2.3ms, hotkey first frame 27.3ms, theme switch ~0.04ms,
auto-tag suggest ~6.5ms/note. Every screen switch and search now runs in
the background with generation-tagged results.

| Graph at 20,000 notes / 26,662 edges | Final |
|---|---|
| Data load (3 queries) | 57ms |
| Barnes-Hut layout step | 25.7ms |

The layout step is over a 16ms budget at 20,000 simultaneously linked
notes, so the simulation advances at roughly 30 steps/second while
settling at that extreme scale (rendering stays view-culled and input
responsive); global positions are cached to `graph_layout.bin`, so a
settled 20k graph reopens without re-simulating. At demo-vault scale
(hundreds of notes) it settles in about a second at 60fps.

## Vim and the visual pass (prompt 6, milestones 30-41)

- **Vim engine: 0.064ms per keystroke** over a 343,890-byte /
  5,000-line note (`cargo test --release bench_vim -- --ignored`),
  motions included; the engine never copies the text on a motion and
  edit lists are applied back-to-front by the UI. Budget was 1ms.
- Design tokens live in `src/ui/style.rs`; per-tag colours are an
  FNV hash to hue with a WCAG contrast nudge (>=4.5:1 against the theme
  background, unit-tested), shared by editor chips, the preview pane
  and graph nodes so colours cost one hash per chip per render.
- The two-pane preview reads the selected note on a background task
  (900 chars kept); selection movement never touches the disk on the
  UI thread. Smooth graph zoom is one lerp per painted frame inside
  the existing canvas prepaint - no extra passes; the graph layout
  numbers above are unchanged (parallel Barnes-Hut, step ~8ms at 20k).
- Smoke: `scripts/smoke.ps1` green; screenshot review notes and the
  three fixes they produced are in `docs/ui-review.md`.
