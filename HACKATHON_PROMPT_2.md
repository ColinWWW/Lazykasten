# Lazykasten, part 2: Muse auto-tagging, editor polish, links, demo

You built Lazykasten from `HACKATHON_PROMPT.md`, and milestones 1–6 are done: it builds, and `cargo test` passes 5 tests. This prompt continues from there.

**Before you change anything:**
- Re-read `HACKATHON_PROMPT.md` and every file in `src/`.
- Every rule in it still applies, especially section 9 (pitfalls) and section 12 (code style).
- Keep zero warnings, keep slow work off the UI thread, and verify UI changes by running the app, not just by compiling.

Work milestone by milestone. After each one, run `cargo build --release` and `cargo test`, run the app against a test vault, and commit.

---

## Milestone 7: Safety net

1. The folder isn't a git repository yet. Run `git init`.
   - Make sure `.gitignore` has `/target`.
   - Commit the current working state as "Lazykasten core app".
   - Commit after every milestone below, so a bad change can be reverted.
2. Never commit API keys. Keys only come from environment variables.
3. Run the 20k benchmark (`cargo test --release -- --ignored --nocapture`) and write down the numbers. Re-run it at the end to prove nothing got slower.

---

## Milestone 8: Lazy auto-tagging with Muse (the hackathon feature)

This is the headline feature for the demo. The user captures a thought with no tags, presses Alt+J, and Lazykasten tags it automatically using Meta's Muse model. Organizing stays lazy.

### Configuration
Read three environment variables in a new module `src/ai.rs`:

| Variable | Meaning |
|---|---|
| `LAZYKASTEN_AI_KEY` | API key. If it's missing or empty, AI tagging is off and the local fallback below is used. |
| `LAZYKASTEN_AI_URL` | Chat completions endpoint. Default to the one in Meta's API docs. |
| `LAZYKASTEN_AI_MODEL` | Default `muse-spark-1.3`. It's fast and cheap, and tagging is a small task. |

**Look up the exact endpoint URL, auth header and request/response format in Meta's API documentation for the hackathon. Don't guess them.** If the API is OpenAI-compatible, use the standard `messages` array and read `choices[0].message.content`.

### Dependencies
```toml
ureq = { version = "3", features = ["json"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
```
ureq 3's API is different from ureq 2, and this sketch isn't verified. Check it against the docs.rs page for the version Cargo resolves:
```rust
let agent: ureq::Agent = ureq::Agent::config_builder()
    .timeout_global(Some(std::time::Duration::from_secs(15)))
    .build()
    .into();
let mut resp = agent.post(&url).header("Authorization", &format!("Bearer {key}")).send_json(&body)?;
let reply: serde_json::Value = resp.body_mut().read_json()?;
```

### Flow
1. **The hotkey must stay instant.** When the user hides a note that has no explicit tags, save it immediately exactly as today (it goes to `inbox`), and hide the window. Then start tagging with `cx.background_spawn`. Never wait for the network on the UI thread.
2. **Build the request.** Send:
   - the note text, cut to about 4,000 characters
   - the vault's existing tag names, the top 200 by count, excluding `inbox`

   Use this system instruction:

   > You tag notes in a personal Zettelkasten. Reply with only JSON: {"tags": ["..."]}. Give 1 to 3 short tags. Strongly prefer tags from the existing list when they fit; invent a new one only if nothing fits. Use Title Case for new tags.
3. **Parse the reply defensively.** Strip any ```` ```json ```` fences, find the first `{…}` object, and read `tags`. Then:
   - Trim each tag, drop empty ones and drop `inbox`.
   - Remove case-insensitive duplicates, and keep at most 3.
   - Snap each tag to the existing spelling when it matches an existing tag case-insensitively.
4. **Write the tags into the file**, because the files are the source of truth and the index must be rebuildable.
   - Append `\n\nTag: A, B` to the note.
   - **Only do this if the file is still exactly the text you sent.** If the user reopened and edited it meanwhile, skip it.
   - Then re-index through `store.save`, which removes it from `inbox`.
5. **Tell the user.** Set the status to `Tagged by Muse: #A #B`, so the user sees it the next time the window opens. If the UI is visible, re-render. On any error (no network, bad JSON, timeout), leave the note in `inbox`. Set a short status like `Muse tagging failed: timeout`, and never panic.
6. **Add the Ctrl+T "Tag now" action in the editor.** It runs the same request for the open note and shows the suggestions as dashed or outlined chips in the footer. Clicking a suggestion adds it to the note's `Tag:` line, creating the line if needed. Esc dismisses them. Bind it in the `"LazyKasten"` context and add it to the footer hints.

### Local fallback (always available, no network)
When there's no key, match the note's words against existing tag names:
- Match case-insensitively, on word boundaries.
- Multi-word tags like "Computer Programming" must match as a phrase.
- Suggest up to 3, preferring longer matches.

Use the fallback for the automatic flow when AI is off, and also add its matches to the candidates sent to Muse.

### Tests (no network in tests)
Write the HTTP call behind a function parameter or small trait, so tests pass a fake. Test:
- reply parsing: fences, extra prose, bad JSON, duplicates, snapping to existing spelling, the 3-tag cap
- appending the tag line
- skipping when the file changed
- the local matcher, including multi-word tags and word boundaries (so `rust` doesn't match `trust`)

### README
Add a "Muse auto-tagging" section covering:
- how to set the env vars
- that the note text is sent to Meta's API only when a key is set
- the Ctrl+T key

---

## Milestone 9: Editor polish (match the reference screenshot)

The user's reference shows `Tag: Javascript, Computer Programming` with the tag values as **blue links** inside the editor text.

1. **Find the real API before writing code.** Look in the gpui-component source that `gpui-kit 0.8.1` pulls in, under `%USERPROFILE%\.cargo\registry\src\`. Search it for how `Textarea`, `InputState` or `EditorState` support highlighting, inline tokens or click handlers on text ranges.
   - The earlier spec guessed at `.token(...)` / `.on_token_click(...)`. That's unverified.
   - Use what actually exists. If there's no way to style ranges in the plain Textarea, check whether the code editor mode with a highlighter can do it.
2. Style the `Tag:` line values and `#hashtags` in `theme.blue`. Ctrl+Click (or click, if the API supports it cleanly) opens that tag's index.
3. Add light Markdown styling only if the same mechanism supports it cheaply: headings bold, `**bold**`.
4. **Typing must not get slower.** Test typing in a 5,000-word note.
5. **Optional, and last:** spellcheck squiggles through the Windows `ISpellChecker` COM API (`Win32_Globalization` feature of `windows`). Only do it if 1–3 went smoothly.

If the editor can't support styled ranges without forking gpui-component, stop. Report what you found, and move on. Don't hack around it.

---

## Milestone 10: Links between notes

1. **Typing `[[`** opens a small popup under the cursor that lists matching note titles. Use the existing FTS search on the text typed after `[[`, top 8.
   - Up and Down select, and Enter inserts `[[<id>|<title>]]`. Esc closes the popup.
   - If a popup is too hard with the available API, show the matches in the footer instead.
2. **Index the links.** Add a `links(from_id, to_id)` table, filled in `index_note`.
   - Bump `SCHEMA_VERSION` to 2 so existing indexes rebuild.
   - Add a parser test.
3. **Show backlinks.** When a note is open, the footer shows "N backlinks". Clicking it lists the linking notes in the browser (a new `Screen::Backlinks(id)`).
4. **Optional, if time allows:** "Related notes" from Muse. Send the note plus the 30 best FTS matches on its title words, and ask which are related. Show the result in the same list.

---

## Milestone 11: Demo readiness

The judges will see a short demo, so make it reliable and impressive.

1. **Demo vault script.** Add `scripts/seed_demo.ps1`, or a `--seed-demo <dir>` flag. It creates about 300 realistic notes on programming, books and ideas, with real-looking titles and tags, plus about 20 untagged notes for Muse to tag live.
2. **Single instance.** A second launch should show the existing window instead of failing on the hotkey. Use a named mutex plus a message to the existing window, or exit quietly.
3. **Live file watching**, only if time allows. Use the `notify` crate, so notes edited in Obsidian or Notepad while the app runs are re-indexed in the background.
4. **Final checks:**
   - `cargo build --release` with zero warnings
   - all tests pass
   - the 20k benchmark is still within the targets in section 10 of the first spec
   - Alt+J is still under 30 ms with AI enabled
5. **Write `DEMO.md`,** a 90-second demo script:
   1. Alt+J from any app, type a thought with no tags, Alt+J.
   2. Alt+J again: "Tagged by Muse: #Javascript".
   3. Ctrl+O, click the tag, and the index appears instantly over 20,000 notes.
   4. Search as you type.
   5. A `[[` link and its backlinks.
   6. The benchmark numbers.

---

## Priorities if time runs out

Do them in this order: **8 (Muse tagging) → 11.1, 11.4 and 11.5 (demo) → 10 → 9 → the rest.** A working AI feature and a smooth demo matter more for the hackathon than editor styling.
