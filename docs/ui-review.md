# UI review (prompt 6, milestone 40)

Screenshot review loop run 2026-10-29 against the release build on the
seeded smoke vault (`scripts/smoke.ps1`, shots in `target/ui-shots/`),
plus dedicated drive shots for the palette and the two-pane preview.

## Reviewed

- `smoke-01-editor.png` — empty editor: hint block reads well, capture
  placeholder visible, max-width column centred. OK.
- `smoke-02-capture.png` — typed text lands in the column. OK.
- `smoke-03-tags.png` — tag list, counts, `#trash` row. OK.
- `smoke-04-search.png` — live search, honest "0 matches". OK.
- `smoke-05-themes.png` — Ctrl+Shift+P now opens the command palette
  (the smoke step predates the palette; the filter correctly shows no
  commands for "tokyo"). See fix 2.
- `smoke-06-graph.png` — graph on the demo seed legitimately has 0
  nodes (every tag over the member cap, orphans hidden). See fix 1.
- `m40-palette.png` — palette lists all commands with shortcuts. OK.
- `m40-preview.png` — two-pane preview shows title, tags, body. OK
  after fix 3.

## Defects found and fixed

1. **Graph search box inherited the previous screen's query.** The
   search input is shared; entering the graph showed the palette's
   leftover text. `open_graph` now clears it.
2. **Palette gave no feedback for a query with no matches.** The
   heading now reads "Commands · no matches".
3. **Preview pane never appeared on this machine.** The 900px logical
   threshold is above the ~833px logical width of a 1250px window at
   150% DPI. Threshold lowered to 780px; verified in `m40-preview.png`.

## Known limits (not fixed here)

- `scripts/drive.ps1` crops shots to the window rect intersected with
  the virtual screen; on this multi-monitor setup the right and bottom
  edges (row dates, the editor footer) fall outside the crop. The
  footer and tag chips were verified in earlier milestone drives and
  by the contrast unit test, but are not visible in the smoke shots.
- Editor text itself still has no styled ranges (gpui-component 0.8.1
  plain Textarea paints none; see PERF.md, milestone 24). `[[link]]`
  affordances live in the pill bar above the footer instead.
