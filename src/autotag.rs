//! Local auto-tagging: nearest neighbors over the existing FTS5 index.
//!
//! No LLM, no network, no new dependencies. The tagger learns from notes the
//! user already tagged: it scores a new note's key terms with tf-idf against
//! the FTS vocabulary, finds similar tagged notes with one bm25 query, and
//! lets those neighbors vote for their tags. A tag whose name literally
//! appears in the note gets a boost. With fewer than 10 tagged notes it uses
//! only those name matches (cold start).

use std::collections::HashMap;

use anyhow::Result;

use crate::store::{Store, is_tag_line, parse_tags};

/// Apply tags at or above this confidence automatically (at most 3).
pub const AUTO_THRESHOLD: f64 = 0.4;
/// Show Ctrl+T suggestions at or above this confidence.
pub const SUGGEST_THRESHOLD: f64 = 0.1;
/// Confidence added when a tag's name appears in the note.
pub const NAME_BOOST: f64 = 0.5;
/// Below this many tagged notes, only name matches are used.
pub const COLD_START_TAGGED: i64 = 10;

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    pub tag: String,
    pub confidence: f64,
}

/// Tokenize roughly like FTS5's `unicode61`: lowercase, split on anything
/// that isn't alphanumeric. Tag lines are ignored; terms shorter than 3
/// characters and pure numbers are dropped.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        if is_tag_line(line.trim()).is_some() {
            continue;
        }
        let mut cur = String::new();
        let flush = |cur: &mut String, out: &mut Vec<String>| {
            if cur.chars().count() >= 3 && !cur.chars().all(|c| c.is_ascii_digit()) {
                out.push(std::mem::take(cur));
            } else {
                cur.clear();
            }
        };
        for ch in line.chars() {
            if ch.is_alphanumeric() {
                cur.extend(ch.to_lowercase());
            } else {
                flush(&mut cur, &mut out);
            }
        }
        flush(&mut cur, &mut out);
    }
    out
}

/// Top key terms of `text`: `tf * ln(N / doc)`, dropping terms present in
/// more than 20% of notes (they act as stopwords). At most 12 terms.
fn key_terms(store: &Store, text: &str) -> Result<Vec<String>> {
    let tokens = tokenize(text);
    if tokens.is_empty() {
        return Ok(Vec::new());
    }
    let mut tf: HashMap<String, usize> = HashMap::new();
    for t in &tokens {
        *tf.entry(t.clone()).or_default() += 1;
    }
    let distinct: Vec<String> = tf.keys().cloned().collect();

    let conn = store.lock_conn();
    let n: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))?;
    if n == 0 {
        return Ok(Vec::new());
    }
    let placeholders: Vec<String> = distinct.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        "SELECT term, doc FROM notes_vocab WHERE term IN ({})",
        placeholders.join(",")
    );
    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> =
        distinct.iter().map(|t| t as &dyn rusqlite::ToSql).collect();
    let rows = stmt.query_map(params.as_slice(), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    let mut docs: HashMap<String, i64> = HashMap::new();
    for row in rows {
        let (term, doc) = row?;
        docs.insert(term, doc);
    }
    drop(stmt);
    drop(conn);

    let nf = n as f64;
    let mut scored: Vec<(String, f64)> = Vec::new();
    for (term, count) in &tf {
        let Some(&doc) = docs.get(term) else { continue };
        if doc as f64 > 0.2 * nf {
            continue;
        }
        let score = *count as f64 * (nf / doc as f64).ln();
        if score > 0.0 {
            scored.push((term.clone(), score));
        }
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    scored.truncate(12);
    Ok(scored.into_iter().map(|(t, _)| t).collect())
}

/// Whole-word / whole-phrase containment. Multi-word tags must match as a
/// phrase. Tags of 3 letters or fewer match case-sensitively, so the word
/// "go" doesn't trigger `Go`. Word boundaries matter: `rust` must not match
/// "trust".
pub fn contains_tag_name(text: &str, tag: &str) -> bool {
    let case_sensitive = tag.chars().count() <= 3;
    let norm = |s: &str| -> Vec<char> {
        if case_sensitive {
            s.chars().collect()
        } else {
            s.chars().flat_map(|c| c.to_lowercase()).collect()
        }
    };
    let hay = norm(text);
    let needle = norm(tag.trim());
    if needle.is_empty() || needle.len() > hay.len() {
        return false;
    }
    let word_char = |c: char| c.is_alphanumeric() || c == '_';
    for start in 0..=(hay.len() - needle.len()) {
        if hay[start..start + needle.len()] != needle[..] {
            continue;
        }
        let before_ok = start == 0 || !word_char(hay[start - 1]);
        let end = start + needle.len();
        let after_ok = end == hay.len() || !word_char(hay[end]);
        if before_ok && after_ok {
            return true;
        }
    }
    false
}

fn all_tag_names(store: &Store) -> Result<Vec<String>> {
    let conn = store.lock_conn();
    let mut stmt = conn.prepare(
        "SELECT name FROM tags WHERE name != 'inbox' COLLATE NOCASE ORDER BY name ASC",
    )?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

fn tagged_note_count(store: &Store) -> Result<i64> {
    let conn = store.lock_conn();
    let n: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT nt.note_id) FROM note_tags nt
         JOIN tags t ON t.id = nt.tag_id WHERE t.name != 'inbox' COLLATE NOCASE",
        [],
        |r| r.get(0),
    )?;
    Ok(n)
}

/// Similar tagged notes as `(note_id, weight)` with `weight = -bm25`
/// (bm25 is negative; more negative means more similar).
fn neighbors(
    store: &Store,
    exclude_id: &str,
    terms: &[String],
) -> Result<Vec<(String, f64)>> {
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let match_expr = terms
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ");
    let conn = store.lock_conn();
    let mut stmt = conn.prepare(
        "SELECT n.id, bm25(notes_text) FROM notes_text f JOIN notes n ON n.rowid = f.rowid
         WHERE notes_text MATCH ?1 AND n.id != ?2
           AND EXISTS (SELECT 1 FROM note_tags nt JOIN tags t ON t.id = nt.tag_id
                       WHERE nt.note_id = n.id AND t.name != 'inbox' COLLATE NOCASE)
         ORDER BY rank LIMIT 20",
    )?;
    let rows = stmt.query_map(rusqlite::params![match_expr, exclude_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, f64>(1)?))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, bm25) = row?;
        out.push((id, -bm25));
    }
    Ok(out)
}

/// Suggest tags for `text`, excluding note `exclude_id` from its own
/// neighbor set. Sorted by confidence descending.
pub fn suggest_tags(store: &Store, exclude_id: &str, text: &str) -> Result<Vec<Suggestion>> {
    let tag_names = all_tag_names(store)?;
    // Canonical spelling lookup, keyed by lowercase name.
    let canonical: HashMap<String, String> = tag_names
        .iter()
        .map(|t| (t.to_lowercase(), t.clone()))
        .collect();

    let mut confidence: HashMap<String, f64> = HashMap::new();

    if tagged_note_count(store)? >= COLD_START_TAGGED {
        let terms = key_terms(store, text)?;
        let neigh = neighbors(store, exclude_id, &terms)?;
        let total: f64 = neigh.iter().map(|(_, w)| *w).sum();
        if total > 0.0 && !neigh.is_empty() {
            // Load the neighbors' tags in one query.
            let placeholders: Vec<String> = neigh.iter().map(|_| "?".to_string()).collect();
            let sql = format!(
                "SELECT nt.note_id, t.name FROM note_tags nt JOIN tags t ON t.id = nt.tag_id
                 WHERE nt.note_id IN ({}) AND t.name != 'inbox' COLLATE NOCASE",
                placeholders.join(",")
            );
            let weights: HashMap<String, f64> = neigh.iter().cloned().collect();
            let conn = store.lock_conn();
            let mut stmt = conn.prepare(&sql)?;
            let params: Vec<&dyn rusqlite::ToSql> =
                neigh.iter().map(|(id, _)| id as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(params.as_slice(), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            let mut tag_weight: HashMap<String, f64> = HashMap::new();
            for row in rows {
                let (note_id, tag) = row?;
                if let Some(w) = weights.get(&note_id) {
                    *tag_weight.entry(tag.to_lowercase()).or_default() += *w;
                }
            }
            drop(stmt);
            drop(conn);
            for (lower, w) in tag_weight {
                confidence.insert(lower, w / total);
            }
        }
    }

    // Name-match boost (also the whole cold-start signal).
    for tag in &tag_names {
        if contains_tag_name(text, tag) {
            *confidence.entry(tag.to_lowercase()).or_default() += NAME_BOOST;
        }
    }

    let mut out: Vec<Suggestion> = confidence
        .into_iter()
        .filter_map(|(lower, conf)| {
            canonical.get(&lower).map(|tag| Suggestion {
                tag: tag.clone(),
                confidence: conf.min(1.0),
            })
        })
        .filter(|s| s.confidence > 0.0)
        .collect();
    out.sort_by(|a, b| {
        b.confidence
            .total_cmp(&a.confidence)
            .then_with(|| a.tag.to_lowercase().cmp(&b.tag.to_lowercase()))
    });
    Ok(out)
}

/// Tags confident enough to apply automatically: `>= 0.4`, at most 3.
pub fn confident_tags(suggestions: &[Suggestion]) -> Vec<String> {
    suggestions
        .iter()
        .filter(|s| s.confidence >= AUTO_THRESHOLD)
        .take(3)
        .map(|s| s.tag.clone())
        .collect()
}

/// Append `\n\nTag: A, B` to a note, per the file-is-source-of-truth rule.
pub fn append_tag_line(text: &str, tags: &[String]) -> String {
    format!("{}\n\nTag: {}", text, tags.join(", "))
}

/// Add `tag` to the note's `Tag:` line, creating the line if needed.
pub fn add_tag_to_text(text: &str, tag: &str) -> String {
    if parse_tags(text).iter().any(|t| t.eq_ignore_ascii_case(tag)) {
        return text.to_string();
    }
    let mut lines: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    for line in &mut lines {
        if is_tag_line(line.trim()).is_some() {
            let trimmed_end = line.trim_end();
            *line = if trimmed_end.ends_with(':') {
                format!("{trimmed_end} {tag}")
            } else {
                format!("{trimmed_end}, {tag}")
            };
            let mut out = lines.join("\n");
            if text.ends_with('\n') {
                out.push('\n');
            }
            return out;
        }
    }
    append_tag_line(text, &[tag.to_string()])
}

/// Write `tags` into the file through `store.save`, but only if the file is
/// still exactly `original_text`. Returns false when skipped.
pub fn apply_tags_if_unchanged(
    store: &Store,
    id: &str,
    original_text: &str,
    tags: &[String],
) -> Result<bool> {
    if tags.is_empty() {
        return Ok(false);
    }
    let current = store.read(id)?;
    if current != original_text {
        return Ok(false);
    }
    store.save(id, &append_tag_line(&current, tags))?;
    Ok(true)
}

/// Strip tag lines and hashtags, for `--eval-tags` (a note must not be
/// judged on the tags it already carries in its text).
pub fn strip_tags(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if is_tag_line(line.trim()).is_some() {
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut kept = String::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '#'
                && (i == 0 || (!chars[i - 1].is_alphanumeric() && chars[i - 1] != '#'))
            {
                let start = i + 1;
                let mut end = start;
                while end < chars.len()
                    && (chars[end].is_alphanumeric() || matches!(chars[end], '-' | '_' | '/'))
                {
                    end += 1;
                }
                if end > start && chars[start].is_alphabetic() {
                    i = end;
                    continue;
                }
            }
            kept.push(chars[i]);
            i += 1;
        }
        out.push(kept);
    }
    out.join("\n")
}

#[derive(Clone, Debug, Default)]
pub struct EvalReport {
    pub notes: usize,
    pub precision: f64,
    pub recall: f64,
    pub hit_rate: f64,
    pub ms_per_note: f64,
    pub applied: usize,
    pub correct: usize,
    pub real_total: usize,
}

/// Leave-one-out accuracy over up to 300 tagged notes, for `--eval-tags`.
pub fn eval_tags(store: &Store) -> Result<EvalReport> {
    let ids: Vec<String> = {
        let conn = store.lock_conn();
        let mut stmt = conn.prepare(
            "SELECT n.id FROM notes n
             WHERE EXISTS (SELECT 1 FROM note_tags nt JOIN tags t ON t.id = nt.tag_id
                           WHERE nt.note_id = n.id AND t.name != 'inbox' COLLATE NOCASE)
             ORDER BY n.id DESC LIMIT 300",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut v = Vec::new();
        for row in rows {
            v.push(row?);
        }
        v
    };

    let mut report = EvalReport::default();
    let started = std::time::Instant::now();
    for id in &ids {
        let Ok(text) = store.read(id) else { continue };
        let real: Vec<String> = parse_tags(&text)
            .into_iter()
            .filter(|t| !t.eq_ignore_ascii_case("inbox"))
            .collect();
        if real.is_empty() {
            continue;
        }
        let suggestions = suggest_tags(store, id, &strip_tags(&text))?;
        let applied = confident_tags(&suggestions);
        let real_lower: Vec<String> = real.iter().map(|t| t.to_lowercase()).collect();
        let hits = applied
            .iter()
            .filter(|t| real_lower.contains(&t.to_lowercase()))
            .count();
        report.notes += 1;
        report.applied += applied.len();
        report.correct += hits;
        report.real_total += real.len();
        if hits > 0 {
            report.hit_rate += 1.0;
        }
    }
    if report.notes > 0 {
        report.ms_per_note = started.elapsed().as_secs_f64() * 1000.0 / report.notes as f64;
        report.hit_rate /= report.notes as f64;
    }
    if report.applied > 0 {
        report.precision = report.correct as f64 / report.applied as f64;
    }
    if report.real_total > 0 {
        report.recall = report.correct as f64 / report.real_total as f64;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "lazykasten-autotag-{}-{}-{}",
            std::process::id(),
            n,
            chrono::Local::now().format("%Y%m%d%H%M%S%f")
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    const RUST_BODY: &str = "borrowing lifetimes ownership borrow checker memory \
        safety compiler trait impl generic mutable reference dangling stack heap";
    const COOK_BODY: &str = "cooking recipe garlic onion olive oil simmer fry \
        pan kitchen dinner taste seasoning boil bake ingredients flavour";

    fn seeded_store() -> (Store, PathBuf) {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        for i in 0..10 {
            store
                .save(
                    &format!("202601010000{i:02}"),
                    &format!("Rust note {i}\n{RUST_BODY} variant{i}\nTag: Rust"),
                )
                .unwrap();
            store
                .save(
                    &format!("202601020000{i:02}"),
                    &format!("Cooking note {i}\n{COOK_BODY} variant{i}\nTag: Cooking"),
                )
                .unwrap();
        }
        // Filler notes keep any one topic under the 20% stopword cutoff.
        for i in 0..40 {
            store
                .save(
                    &format!("202601030000{i:02}"),
                    &format!("Misc note {i}\ngarden travel journal music walk{i} river{i} stone{i}\nTag: Misc"),
                )
                .unwrap();
        }
        (store, root)
    }

    fn cleanup(root: PathBuf) {
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_rust_note_gets_rust_not_cooking() {
        let (store, root) = seeded_store();
        let text = "Thinking about borrowing and lifetimes again\n\
                    ownership and the borrow checker make memory safety click";
        let sugg = suggest_tags(&store, "new", text).unwrap();
        let confident = confident_tags(&sugg);
        assert!(confident.iter().any(|t| t == "Rust"), "got {sugg:?}");
        assert!(!confident.iter().any(|t| t == "Cooking"), "got {sugg:?}");
        drop(store);
        cleanup(root);
    }

    #[test]
    fn test_ambiguous_note_gets_no_auto_tag() {
        let (store, root) = seeded_store();
        let sugg = suggest_tags(&store, "new", "some general thoughts about things today").unwrap();
        assert!(confident_tags(&sugg).is_empty(), "got {sugg:?}");
        drop(store);
        cleanup(root);
    }

    #[test]
    fn test_name_match_rules() {
        assert!(!contains_tag_name("I trust this plan", "rust"));
        assert!(contains_tag_name("Rust is great", "Rust"));
        assert!(contains_tag_name("notes on computer programming today", "Computer Programming"));
        assert!(!contains_tag_name("programming computer today", "Computer Programming"));
        // Short tags are case-sensitive.
        assert!(!contains_tag_name("go to school", "Go"));
        assert!(contains_tag_name("Go is fast", "Go"));
        assert!(!contains_tag_name("said the word", "AI"));
        assert!(contains_tag_name("AI models", "AI"));
    }

    #[test]
    fn test_name_boost_in_suggestions() {
        let (store, root) = seeded_store();
        // "cooking" appears as a word, so Cooking gets the +0.5 boost even
        // though the body vocabulary is about Rust.
        let sugg = suggest_tags(&store, "new", &format!("cooking\n{RUST_BODY}")).unwrap();
        let cooking = sugg.iter().find(|s| s.tag == "Cooking").unwrap();
        assert!(cooking.confidence >= 0.5, "got {sugg:?}");
        drop(store);
        cleanup(root);
    }

    #[test]
    fn test_cold_start_uses_only_name_matches() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        for i in 0..3 {
            store
                .save(&format!("202601010000{i:02}"), &format!("Note {i}\n{RUST_BODY}\nTag: Rust"))
                .unwrap();
        }
        // Body matches Rust notes, but with <10 tagged notes neighbors are
        // skipped and the note never says "rust", so nothing is suggested.
        let sugg = suggest_tags(&store, "new", RUST_BODY).unwrap();
        assert!(sugg.is_empty(), "got {sugg:?}");
        // A literal name match still works in cold start.
        let sugg = suggest_tags(&store, "new", "thinking about Rust today").unwrap();
        assert_eq!(sugg.len(), 1);
        assert_eq!(sugg[0].tag, "Rust");
        drop(store);
        cleanup(root);
    }

    #[test]
    fn test_append_and_skip_when_changed() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        store.save("20260101000001", "Untagged thought").unwrap();
        let original = store.read("20260101000001").unwrap();
        assert!(apply_tags_if_unchanged(&store, "20260101000001", &original, &["Rust".into()]).unwrap());
        let after = store.read("20260101000001").unwrap();
        assert!(after.ends_with("Tag: Rust"), "got {after:?}");
        assert_eq!(store.notes_with_tag("Rust").unwrap().len(), 1);
        assert_eq!(store.notes_with_tag("inbox").unwrap().len(), 0);
        // The stale original no longer matches the file: skip.
        assert!(!apply_tags_if_unchanged(&store, "20260101000001", &original, &["Go".into()]).unwrap());
        assert!(!store.read("20260101000001").unwrap().contains("Go"));
        drop(store);
        cleanup(root);
    }

    #[test]
    fn test_add_tag_to_text() {
        assert_eq!(add_tag_to_text("Body", "Rust"), "Body\n\nTag: Rust");
        assert_eq!(add_tag_to_text("Body\nTag: Go", "Rust"), "Body\nTag: Go, Rust");
        assert_eq!(add_tag_to_text("Body\nTag: rust", "Rust"), "Body\nTag: rust");
    }

    #[test]
    fn test_note_is_never_its_own_neighbor() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        for i in 0..10 {
            store
                .save(&format!("202601020000{i:02}"), &format!("Cooking {i}\n{COOK_BODY}\nTag: Cooking"))
                .unwrap();
        }
        // The only Rust-tagged note; its body never says "rust".
        store.save("20260101000001", &format!("Solo\n{RUST_BODY}\nTag: Rust")).unwrap();
        let text = store.read("20260101000001").unwrap();
        let sugg = suggest_tags(&store, "20260101000001", &strip_tags(&text)).unwrap();
        assert!(!sugg.iter().any(|s| s.tag == "Rust"), "self leaked in: {sugg:?}");
        drop(store);
        cleanup(root);
    }

    #[test]
    fn test_tokenize_rules() {
        // Tag lines ignored, short/pure-number terms dropped.
        assert_eq!(tokenize("Tag: Rust\nBorrowing, lifetimes! a 42 ok"), vec!["borrowing", "lifetimes"]);
    }
}
