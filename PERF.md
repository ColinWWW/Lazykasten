# Lazykasten performance

Measured on this machine, release build, 20,000-note benchmark vault
(`cargo test --release -- --ignored --nocapture`). UI timings from
`LAZYKASTEN_TRACE=1` (`<vault>/.lazykasten/trace.log`) and the PowerShell
window-driving method. Trace cost when off: one branch, no formatting, no I/O.

## Baseline (milestone 12, before the performance pass)

| Operation | Before | After (M13) | Final (M15) |
|---|---|---|---|
| First index, 20k notes | 1.536s | 0.888s |  |
| Tag index, 400 notes | 1.360ms | 0.533ms |  |
| Tag index, all 20k | 14.186ms | 13.059ms |  |
| Full-text search `searchable` | 14.556ms | 15.040ms (noise; now off UI thread) |  |
| Search 1-letter `a` (prefix) | 0.188ms (0 hits in bench vault) | 0.086ms (exact-term now) |  |
| Search 2-letter `se` (prefix, 500 hits) | 14.635ms | 14.121ms |  |
| Auto-tag suggest (20-run avg) | 8.778ms/note | 6.594ms/note |  |
| Startup re-check, no changes (20k) | 0.385s | 0.026s |  |
| Startup to window ready (`startup_ready`) | 254ms (320-note vault) | | |
| Hotkey, first show (`hotkey_show_first`) | 14.7ms | | |
| Hotkey, later shows (`hotkey_show`) | 15.3ms (12–17ms external) | | |
| Release exe size | 24,407,552 B | unchanged (fat-LTO trial reverted) |  |

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
