# Lazykasten — 90-second demo

## Setup (before the demo)

```powershell
.\scripts\seed_demo.ps1 -Vault C:\Demo\LazykastenDemo
$env:LAZYKASTEN_VAULT = "C:\Demo\LazykastenDemo"
.\target\release\lazykasten.exe --hidden
```

This seeds 300 tagged notes (Rust, Javascript, Computer Programming, Cooking,
Books, Ideas) plus 20 untagged notes. A second launch while one is running
exits quietly (single instance).

## Script

1. **Capture (10s).** From any app, press **Alt+J**, type a thought with no
   tags — e.g. "borrowing and lifetimes, ownership and the borrow checker
   finally make sense" — and press **Alt+J** again. The window hides
   instantly; the note is saved to `inbox` first, tagging never blocks.
2. **Auto-tagged (10s).** Press **Alt+J** again: the footer reads
   `Auto-tagged: #Rust`. No LLM, no network — the tagger learned from the
   vault's own tagged notes over the local SQLite FTS5 index.
3. **Ctrl+T suggestions (15s).** Type or open another untagged note and press
   **Ctrl+T**: the footer shows suggestions with confidence, e.g. `#Cooking
   83%`. Click one to add it to the note's `Tag:` line; Esc dismisses them.
4. **Sweep the inbox (10s).** Press **Ctrl+O**, then **Ctrl+Shift+T** on the
   tag list. Every inbox note is auto-tagged in the background and the app
   reports "Tagged N of 20 inbox notes"; `#inbox` shrinks or disappears.
5. **Tag index at scale (10s).** In the browser, click `#Rust` — the index
   opens instantly. (At 20,000 notes a 400-note tag index takes ~1.2 ms.)
6. **Search as you type (10s).** Type in the search box: full-text search
   with prefix matching, best first (~13 ms at 20,000 notes).
7. **Links and backlinks (15s).** In the editor type `[[Rus`, click a match
   in the footer list to insert `[[<id>|<title>]]`, save, then open the
   target note: its footer shows "1 backlinks" — click it to list the
   linking notes.
8. **Numbers (10s).** Run in a terminal:
   - `lazykasten.exe --eval-tags` — leave-one-out accuracy: on the demo
     vault, precision 1.000, recall 1.000, 0.82 ms per note. Thresholds:
     apply at confidence ≥ 0.4, suggest at ≥ 0.1, +0.5 when a tag's name
     appears in the note. Precision first: a wrong tag annoys more than a
     missing one.
   - `cargo test --release -- --ignored --nocapture` — 20,000-note
     benchmark: first index 1.6s (target ≤ 3s), auto-tag suggest 8.4 ms
     (target ≤ 20 ms), tag index (all 20k) ~12 ms, search ~13 ms.
     Baseline before this work: index 1.5s, tag-all 11.7 ms, search
     13.8 ms — nothing got slower.

## Fallbacks

- If auto-tagging finds nothing confident, the note simply stays in
  `#inbox` — capture first, organize later.
- Editor styling note: gpui-component's `Textarea` token API makes matched
  text atomic (and live restyling would reset the caret/undo), and its
  syntax highlighter only runs in code-editor mode where tokens are
  unsupported — so tag values are not blue links inside the editor text;
  footer chips are.
