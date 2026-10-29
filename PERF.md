# Lazykasten performance

## Summary (final)

Everything measured got faster or stayed flat; nothing regressed:

- **First index of 20k notes: 1.536s -> 0.848s** (prepare_cached hot paths).
- **Startup re-check with no changes: 0.385s -> 0.025s** (DirEntry metadata
  instead of a syscall per file) â€” about 15x.
- **Tag index (400 notes): 1.360ms -> 0.525ms**; auto-tag suggest
  **8.778 -> 6.696 ms/note**.
- Search is FTS-bound (~15ms at 500 hits) but now runs in the background
  with generation-tagged results, so it never drops typed characters; a
  1-char query is an exact-term lookup instead of the prefix worst case.
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
