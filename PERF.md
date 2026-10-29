# Lazykasten performance

Measured on this machine, release build, 20,000-note benchmark vault
(`cargo test --release -- --ignored --nocapture`). UI timings from
`LAZYKASTEN_TRACE=1` (`<vault>/.lazykasten/trace.log`) and the PowerShell
window-driving method. Trace cost when off: one branch, no formatting, no I/O.

## Baseline (milestone 12, before the performance pass)

| Operation | Before | After (M13) | Final (M15) |
|---|---|---|---|
| First index, 20k notes | 1.536s | | |
| Tag index, 400 notes | 1.360ms | | |
| Tag index, all 20k | 14.186ms | | |
| Full-text search `searchable` | 14.556ms | | |
| Search 1-letter `a` (prefix) | 0.188ms (0 hits in bench vault) | | |
| Search 2-letter `se` (prefix, 500 hits) | 14.635ms | | |
| Auto-tag suggest (20-run avg) | 8.778ms/note | | |
| Startup re-check, no changes (20k) | 0.385s | | |
| Startup to window ready (`startup_ready`) | 254ms (320-note vault) | | |
| Hotkey, first show (`hotkey_show_first`) | 14.7ms | | |
| Hotkey, later shows (`hotkey_show`) | 15.3ms (12–17ms external) | | |
| Release exe size | 24,407,552 B | | |

Notes:
- `hotkey_show*` measures the show handling on the UI thread (placement,
  `ShowWindow`, focus, `on_show`); first frame paint follows immediately.
- Found while measuring: a pre-existing GPUI Windows re-entrancy panic when
  the first `SetWindowPos` placement moves the window across monitors
  (`RefCell already borrowed` via `handle_move_msg` inside `window.update`).
  To be addressed in milestone 13 by moving placement outside the update.
- The 500-hit searches are dominated by ranking/returning 500 rows; the
  browser UI also ran them on the UI thread (milestone 13.1 fixes that).
