//! Notes on disk plus a rebuildable SQLite index.
//!
//! Markdown files are the source of truth; SQLite is only an index that can
//! be dropped and rebuilt from the files at any time.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};

const SCHEMA_VERSION: i64 = 2;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagCount {
    pub name: String,
    pub count: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteSummary {
    pub id: String,
    pub title: String,
}

/// Vault root: `%LAZYKASTEN_VAULT%` if set, otherwise `~/Lazykasten`.
pub fn default_root() -> PathBuf {
    if let Ok(v) = std::env::var("LAZYKASTEN_VAULT") {
        if !v.trim().is_empty() {
            return PathBuf::from(v);
        }
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join("Lazykasten")
}

pub(crate) fn is_tag_line(line: &str) -> Option<&str> {
    let (head, rest) = line.split_once(':')?;
    let head = head.trim().to_lowercase();
    if head == "tag" || head == "tags" {
        Some(rest)
    } else {
        None
    }
}

/// Parse tags from note text.
///
/// Tag lines (`Tag: a, b`) and inline `#hashtags` are both recognized.
/// Duplicates are dropped case-insensitively, keeping the first spelling
/// and order of appearance.
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let push = |out: &mut Vec<String>, seen: &mut HashSet<String>, tag: String| {
        let tag = tag.trim().to_string();
        if tag.is_empty() {
            return;
        }
        if seen.insert(tag.to_lowercase()) {
            out.push(tag);
        }
    };

    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = is_tag_line(trimmed) {
            for piece in rest.split(',') {
                push(&mut out, &mut seen, piece.to_string());
            }
            continue;
        }
        // Hashtag scan on non-tag lines.
        let chars: Vec<char> = line.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] != '#' {
                i += 1;
                continue;
            }
            // Skip if preceded by alphanumeric or another '#'.
            let prev_ok = if i == 0 {
                true
            } else {
                let p = chars[i - 1];
                !p.is_alphanumeric() && p != '#'
            };
            if !prev_ok {
                i += 1;
                continue;
            }
            let start = i + 1;
            let mut end = start;
            while end < chars.len()
                && (chars[end].is_alphanumeric() || matches!(chars[end], '-' | '_' | '/'))
            {
                end += 1;
            }
            if end > start && chars[start].is_alphabetic() {
                let tag: String = chars[start..end].iter().collect();
                push(&mut out, &mut seen, tag);
            }
            i = end.max(i + 1);
        }
    }
    out
}

/// Parse `[[id]]` / `[[id|title]]` links. Returns target ids, deduplicated
/// in order of appearance.
pub fn parse_links(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("]]") else { break };
        let inner = &after[..end];
        let id = inner.split('|').next().unwrap_or("").trim();
        if !id.is_empty() && !out.iter().any(|t| t == id) {
            out.push(id.to_string());
        }
        rest = &after[end + 2..];
    }
    out
}

/// Title: first non-empty non-tag line, leading `#`s stripped, max 120 chars.
pub fn title_of(text: &str) -> String {
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if is_tag_line(trimmed).is_some() {
            continue;
        }
        let t = trimmed.trim_start_matches('#').trim();
        if t.is_empty() {
            continue;
        }
        let t: String = t.chars().take(120).collect();
        return t;
    }
    "Untitled".to_string()
}

pub struct Store {
    notes_dir: PathBuf,
    conn: Mutex<rusqlite::Connection>,
}

impl Store {
    pub fn open(root: &Path) -> Result<Store> {
        let notes_dir = root.join("notes");
        let db_dir = root.join(".lazykasten");
        std::fs::create_dir_all(&notes_dir).with_context(|| format!("create {}", notes_dir.display()))?;
        std::fs::create_dir_all(&db_dir).with_context(|| format!("create {}", db_dir.display()))?;

        let db_path = db_dir.join("index.db");
        let mut conn = rusqlite::Connection::open(&db_path)
            .with_context(|| format!("open {}", db_path.display()))?;

        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;",
        )?;

        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != SCHEMA_VERSION {
            let tx = conn.transaction()?;
            tx.execute_batch(
                "DROP TABLE IF EXISTS notes_vocab;
                 DROP TABLE IF EXISTS notes_text;
                 DROP TABLE IF EXISTS links;
                 DROP TABLE IF EXISTS note_tags;
                 DROP TABLE IF EXISTS tags;
                 DROP TABLE IF EXISTS notes;",
            )?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            tx.commit()?;
        }

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS notes (
               id    TEXT PRIMARY KEY,
               title TEXT NOT NULL,
               mtime INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS tags (
               id   INTEGER PRIMARY KEY,
               name TEXT NOT NULL UNIQUE COLLATE NOCASE
             );
             CREATE TABLE IF NOT EXISTS note_tags (
               tag_id  INTEGER NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
               note_id TEXT    NOT NULL REFERENCES notes(id) ON DELETE CASCADE,
               PRIMARY KEY (tag_id, note_id)
             ) WITHOUT ROWID;
             CREATE INDEX IF NOT EXISTS note_tags_by_note ON note_tags(note_id);
             CREATE TABLE IF NOT EXISTS links (
               from_id TEXT NOT NULL,
               to_id   TEXT NOT NULL,
               PRIMARY KEY (from_id, to_id)
             ) WITHOUT ROWID;
             CREATE INDEX IF NOT EXISTS links_by_to ON links(to_id);
             CREATE VIRTUAL TABLE IF NOT EXISTS notes_text USING fts5(body);
             CREATE VIRTUAL TABLE IF NOT EXISTS notes_vocab USING fts5vocab(notes_text, 'row');",
        )?;

        Ok(Store {
            notes_dir,
            conn: Mutex::new(conn),
        })
    }

    /// Lock the index connection. Crate-internal so `autotag` can run its
    /// FTS/vocab queries against the same connection.
    pub(crate) fn lock_conn(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.conn.lock().expect("store lock poisoned")
    }

    pub fn note_path(&self, id: &str) -> PathBuf {
        self.notes_dir.join(format!("{id}.md"))
    }

    pub fn new_id(&self) -> String {
        let base = chrono::Local::now().format("%Y%m%d%H%M%S").to_string();
        if !self.note_path(&base).exists() {
            return base;
        }
        let mut n = 1;
        loop {
            let candidate = format!("{base}-{n}");
            if !self.note_path(&candidate).exists() {
                return candidate;
            }
            n += 1;
        }
    }

    pub fn read(&self, id: &str) -> Result<String> {
        Ok(std::fs::read_to_string(self.note_path(id))?)
    }

    /// Save a note. Empty text deletes the file and its index rows.
    pub fn save(&self, id: &str, text: &str) -> Result<()> {
        if text.trim().is_empty() {
            let path = self.note_path(id);
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            let mut conn = self.conn.lock().expect("store lock poisoned");
            let tx = conn.transaction()?;
            Self::remove_note(&tx, id)?;
            tx.commit()?;
            return Ok(());
        }
        let path = self.note_path(id);
        std::fs::write(&path, text)?;
        let mtime = file_mtime_ms(&path)?;
        let mut conn = self.conn.lock().expect("store lock poisoned");
        let tx = conn.transaction()?;
        Self::index_note(&tx, id, text, mtime)?;
        tx.commit()?;
        Ok(())
    }

    pub fn index_note(conn: &rusqlite::Connection, id: &str, text: &str, mtime: i64) -> Result<()> {
        let mut tags = parse_tags(text);
        if tags.is_empty() {
            tags.push("inbox".to_string());
        }
        let title = title_of(text);
        conn.execute(
            "INSERT INTO notes(id, title, mtime) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET title = excluded.title, mtime = excluded.mtime",
            rusqlite::params![id, title, mtime],
        )?;
        conn.execute("DELETE FROM note_tags WHERE note_id = ?1", rusqlite::params![id])?;
        for tag in &tags {
            conn.execute(
                "INSERT OR IGNORE INTO tags(name) VALUES (?1)",
                rusqlite::params![tag],
            )?;
            let tag_id: i64 = conn.query_row(
                "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                rusqlite::params![tag],
                |r| r.get(0),
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO note_tags(tag_id, note_id) VALUES (?1, ?2)",
                rusqlite::params![tag_id, id],
            )?;
        }
        // FTS is keyed by notes.rowid; an upsert keeps the rowid.
        let rowid: i64 = conn.query_row(
            "SELECT rowid FROM notes WHERE id = ?1",
            rusqlite::params![id],
            |r| r.get(0),
        )?;
        conn.execute("DELETE FROM notes_text WHERE rowid = ?1", rusqlite::params![rowid])?;
        conn.execute(
            "INSERT INTO notes_text(rowid, body) VALUES (?1, ?2)",
            rusqlite::params![rowid, text],
        )?;
        conn.execute("DELETE FROM links WHERE from_id = ?1", rusqlite::params![id])?;
        for to_id in parse_links(text) {
            conn.execute(
                "INSERT OR IGNORE INTO links(from_id, to_id) VALUES (?1, ?2)",
                rusqlite::params![id, to_id],
            )?;
        }
        Ok(())
    }

    pub fn remove_note(conn: &rusqlite::Connection, id: &str) -> Result<()> {
        let rowid: Option<i64> = conn
            .query_row("SELECT rowid FROM notes WHERE id = ?1", rusqlite::params![id], |r| r.get(0))
            .ok();
        if let Some(rowid) = rowid {
            conn.execute("DELETE FROM notes_text WHERE rowid = ?1", rusqlite::params![rowid])?;
        }
        conn.execute("DELETE FROM links WHERE from_id = ?1", rusqlite::params![id])?;
        conn.execute("DELETE FROM notes WHERE id = ?1", rusqlite::params![id])?;
        Ok(())
    }

    /// Make the index match the files. Returns how many notes were re-indexed.
    pub fn sync_with_disk(&self) -> Result<usize> {
        // 1. List *.md files with mtimes.
        let mut files: Vec<(String, PathBuf, i64)> = Vec::new();
        if self.notes_dir.is_dir() {
            for entry in std::fs::read_dir(&self.notes_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let stem = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let mtime = file_mtime_ms(&path).unwrap_or(0);
                files.push((stem, path, mtime));
            }
        }

        // 2. Load id -> mtime map, then release the lock to read file contents.
        let db_map: HashMap<String, i64> = {
            let conn = self.conn.lock().expect("store lock poisoned");
            let mut stmt = conn.prepare("SELECT id, mtime FROM notes")?;
            let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            let mut map = HashMap::new();
            for row in rows {
                let (id, mtime) = row?;
                map.insert(id, mtime);
            }
            map
        };

        let file_ids: HashSet<String> = files.iter().map(|(id, _, _)| id.clone()).collect();
        let mut to_index: Vec<(String, String, i64)> = Vec::new();
        for (id, path, mtime) in &files {
            if db_map.get(id) != Some(mtime) {
                // Read contents before taking the lock.
                let text = std::fs::read_to_string(path)?;
                to_index.push((id.clone(), text, *mtime));
            }
        }
        let to_remove: Vec<String> = db_map
            .keys()
            .filter(|id| !file_ids.contains(*id))
            .cloned()
            .collect();

        let mut conn = self.conn.lock().expect("store lock poisoned");
        // 3. Re-index in batches of 500 per transaction.
        let count = to_index.len();
        for chunk in to_index.chunks(500) {
            let tx = conn.transaction()?;
            for (id, text, mtime) in chunk {
                Self::index_note(&tx, id, text, *mtime)?;
            }
            tx.commit()?;
        }
        // 4. Remove rows for vanished files and delete orphan tags.
        if !to_remove.is_empty() {
            let tx = conn.transaction()?;
            for id in &to_remove {
                Self::remove_note(&tx, id)?;
            }
            tx.execute("DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM note_tags)", [])?;
            tx.commit()?;
        } else {
            conn.execute("DELETE FROM tags WHERE id NOT IN (SELECT tag_id FROM note_tags)", [])?;
        }
        Ok(count)
    }

    pub fn tags(&self) -> Result<Vec<TagCount>> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT t.name, COUNT(*) AS c FROM tags t
             JOIN note_tags nt ON nt.tag_id = t.id
             GROUP BY t.id
             ORDER BY c DESC, t.name ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(TagCount {
                name: r.get(0)?,
                count: r.get(1)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn notes_with_tag(&self, tag: &str) -> Result<Vec<NoteSummary>> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT n.id, n.title FROM notes n
             JOIN note_tags nt ON nt.note_id = n.id
             JOIN tags t ON t.id = nt.tag_id
             WHERE t.name = ?1 COLLATE NOCASE
             ORDER BY n.id DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![tag], |r| {
            Ok(NoteSummary {
                id: r.get(0)?,
                title: r.get(1)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Notes that link to `id`, newest first.
    pub fn backlinks(&self, id: &str) -> Result<Vec<NoteSummary>> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT n.id, n.title FROM links l JOIN notes n ON n.id = l.from_id
             WHERE l.to_id = ?1 ORDER BY n.id DESC",
        )?;
        let rows = stmt.query_map(rusqlite::params![id], |r| {
            Ok(NoteSummary { id: r.get(0)?, title: r.get(1)? })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Most recent notes, for the empty `[[` completion list.
    pub fn recent_notes(&self, limit: i64) -> Result<Vec<NoteSummary>> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare("SELECT id, title FROM notes ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt.query_map(rusqlite::params![limit], |r| {
            Ok(NoteSummary { id: r.get(0)?, title: r.get(1)? })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn search(&self, query: &str, limit: i64) -> Result<Vec<NoteSummary>> {
        let words: Vec<&str> = query.split_whitespace().collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let parts: Vec<String> = words
            .iter()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect();
        let match_expr = format!("{}*", parts.join(" "));
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare(
            "SELECT n.id, n.title FROM notes_text f
             JOIN notes n ON n.rowid = f.rowid
             WHERE notes_text MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![match_expr, limit], |r| {
            Ok(NoteSummary {
                id: r.get(0)?,
                title: r.get(1)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn note_count(&self) -> Result<i64> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM notes", [], |r| r.get(0))?;
        Ok(n)
    }
}

pub fn file_mtime_ms(path: &Path) -> Result<i64> {
    let meta = std::fs::metadata(path)?;
    let modified = meta.modified()?;
    let dur = modified
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    Ok(dur.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_root() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "lazykasten-test-{}-{}-{}",
            std::process::id(),
            n,
            chrono::Local::now().format("%Y%m%d%H%M%S%f")
        ));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn test_parse_tags_example() {
        let text = "Some #rust thoughts, see #rust and #Async-IO.\nTag: Javascript, Computer Programming\n# Heading";
        assert_eq!(
            parse_tags(text),
            vec!["rust", "Async-IO", "Javascript", "Computer Programming"]
        );
    }

    #[test]
    fn test_parse_tags_non_tags() {
        // `# Heading`, `issue#12` and `#1` are not tags.
        assert_eq!(parse_tags("# Heading"), Vec::<String>::new());
        assert_eq!(parse_tags("issue#12 still broken"), Vec::<String>::new());
        assert_eq!(parse_tags("#1 priority"), Vec::<String>::new());
        assert_eq!(parse_tags("## Heading"), Vec::<String>::new());
        // Case-insensitive dedup keeps first spelling.
        assert_eq!(parse_tags("#Rust and #rust and #RUST"), vec!["Rust"]);
        // Tag line variants.
        assert_eq!(parse_tags("tags: alpha, Beta"), vec!["alpha", "Beta"]);
        assert_eq!(parse_tags("TAG: One"), vec!["One"]);
        // Hashtag with slash/underscore/dash.
        assert_eq!(parse_tags("see #a/b_c-d end"), vec!["a/b_c-d"]);
    }

    #[test]
    fn test_parse_links() {
        assert_eq!(
            parse_links("see [[20260101|Some Title]] and [[20260102]] and [[20260101]] again [[unclosed"),
            vec!["20260101", "20260102"]
        );
        assert_eq!(parse_links("no links, just [one] bracket"), Vec::<String>::new());
        assert_eq!(parse_links("[[]] [[|empty]]"), Vec::<String>::new());
    }

    #[test]
    fn test_links_and_backlinks_round_trip() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        store.save("20260101000001", "Target note").unwrap();
        store.save("20260101000002", "Linker\nsee [[20260101000001|Target note]]").unwrap();
        store.save("20260101000003", "Other\nno link here").unwrap();
        let backs = store.backlinks("20260101000001").unwrap();
        assert_eq!(backs.len(), 1);
        assert_eq!(backs[0].id, "20260101000002");
        // Editing the link away removes the backlink.
        store.save("20260101000002", "Linker\nlink removed").unwrap();
        assert!(store.backlinks("20260101000001").unwrap().is_empty());
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_title_of() {
        assert_eq!(title_of("Hello world\nbody"), "Hello world");
        assert_eq!(title_of("Tag: x\n## My Note\nbody"), "My Note");
        assert_eq!(title_of("Tag: x\n"), "Untitled");
        assert_eq!(title_of(""), "Untitled");
        assert_eq!(title_of("\n\n  First  \n"), "First");
        let long = "x".repeat(200);
        assert_eq!(title_of(&long).chars().count(), 120);
    }

    #[test]
    fn test_save_index_query_round_trip() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        store.save("20260101000001", "Alpha note\nTag: Rust, Programming").unwrap();
        store.save("20260101000002", "Beta note about #rust and code").unwrap();
        store.save("20260101000003", "Untagged thought").unwrap();

        let tags = store.tags().unwrap();
        let rust = tags.iter().find(|t| t.name.eq_ignore_ascii_case("rust")).unwrap();
        assert_eq!(rust.count, 2);
        let inbox = tags.iter().find(|t| t.name == "inbox").unwrap();
        assert_eq!(inbox.count, 1);

        // Case-insensitive tag lookup.
        let notes = store.notes_with_tag("RUST").unwrap();
        assert_eq!(notes.len(), 2);
        // Newest first.
        assert_eq!(notes[0].id, "20260101000002");

        // Prefix search while typing.
        let hits = store.search("Alp", 10).unwrap();
        assert!(hits.iter().any(|h| h.id == "20260101000001"), "prefix search: {hits:?}");

        // Editing replaces tags.
        store.save("20260101000001", "Alpha note\nTag: Go").unwrap();
        let rust_after = store.tags().unwrap().into_iter().find(|t| t.name.eq_ignore_ascii_case("rust")).unwrap();
        assert_eq!(rust_after.count, 1);

        // Saving empty deletes.
        store.save("20260101000001", "   ").unwrap();
        assert!(!store.note_path("20260101000001").exists());
        assert_eq!(store.note_count().unwrap(), 2);

        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_sync_with_disk_external_changes() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        store.save("20260101000001", "Keep me\nTag: a").unwrap();
        store.save("20260101000002", "Delete me\nTag: b").unwrap();
        assert_eq!(store.sync_with_disk().unwrap(), 0);

        // External changes: delete one file, add another.
        std::fs::remove_file(store.note_path("20260101000002")).unwrap();
        let new_path = store.note_path("20260101000003");
        std::fs::write(&new_path, "External note\nTag: c").unwrap();

        assert_eq!(store.sync_with_disk().unwrap(), 1);
        assert_eq!(store.sync_with_disk().unwrap(), 0);
        assert_eq!(store.note_count().unwrap(), 2);
        assert_eq!(store.notes_with_tag("c").unwrap().len(), 1);

        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    #[ignore]
    fn bench_20k_notes() {
        use std::time::Instant;
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        let n = 20_000;
        for i in 0..n {
            let id = format!("{:014}", 20260101000000u64 + i as u64);
            std::fs::write(
                store.note_path(&id),
                format!("Note title {i}\nBody text for note {i} with searchable words.\nTag: tag{}, common", i % 50),
            )
            .unwrap();
        }
        let t0 = Instant::now();
        let indexed = store.sync_with_disk().unwrap();
        let index_secs = t0.elapsed().as_secs_f64();
        assert_eq!(indexed, n);

        let t0 = Instant::now();
        let tag_notes = store.notes_with_tag("tag1").unwrap();
        let tag_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(tag_notes.len(), 400);

        let t0 = Instant::now();
        let all_notes = store.notes_with_tag("common").unwrap();
        let all_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(all_notes.len(), n);

        let t0 = Instant::now();
        let hits = store.search("searchable", 500).unwrap();
        let search_ms = t0.elapsed().as_secs_f64() * 1000.0;
        assert!(!hits.is_empty());

        let t0 = Instant::now();
        let mut last = Vec::new();
        for _ in 0..20 {
            last = crate::autotag::suggest_tags(
                &store,
                "bench-new-note",
                "A note about searchable words and body text for note tagging",
            )
            .unwrap();
        }
        let autotag_ms = t0.elapsed().as_secs_f64() * 1000.0 / 20.0;
        let _ = last;

        println!("first index of {n} notes: {index_secs:.3}s");
        println!("tag index (400 notes): {tag_ms:.3}ms");
        println!("tag index (all notes): {all_ms:.3}ms");
        println!("full-text search: {search_ms:.3}ms");
        println!("auto-tag suggest: {autotag_ms:.3}ms per note");

        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }
}
