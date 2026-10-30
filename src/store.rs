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


/// Read a note file as text: strip a leading UTF-8 BOM (PowerShell 5.1's
/// `Set-Content -Encoding UTF8` writes one) and decode lossily, so one
/// Windows-1252 note from an old editor can never abort a whole sync.
/// Returns `(text, needed_lossy_decoding)`.
pub fn decode_note_bytes(bytes: &[u8]) -> (String, bool) {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(text) => (text.to_string(), false),
        Err(_) => (String::from_utf8_lossy(bytes).into_owned(), true),
    }
}

pub struct Store {
    #[allow(dead_code)] // read via data_dir (themes/settings)
    root: PathBuf,
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

        conn.set_prepared_statement_cache_capacity(64);
        conn.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA foreign_keys = ON;
             PRAGMA temp_store = MEMORY; PRAGMA cache_size = -32000; PRAGMA mmap_size = 268435456;",
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
            root: root.to_path_buf(),
            notes_dir,
            conn: Mutex::new(conn),
        })
    }

    /// `<vault>/.lazykasten` — settings, user themes and the trace log live here.
    #[allow(dead_code)] // used by theme settings from milestone 14 on
    pub fn data_dir(&self) -> PathBuf {
        self.root.join(".lazykasten")
    }

    /// Merge FTS segments / let SQLite tune itself. Cheap; run on quit and
    /// after large syncs (which already run in the background).
    pub fn optimize(&self) {
        let conn = self.conn.lock().expect("store lock poisoned");
        let _ = conn.execute("PRAGMA optimize", []);
    }

    /// Lock the index connection. Crate-internal so `autotag` can run its
    /// FTS/vocab queries against the same connection.
    pub(crate) fn lock_conn(&self) -> std::sync::MutexGuard<'_, rusqlite::Connection> {
        self.conn.lock().expect("store lock poisoned")
    }

    pub fn notes_dir(&self) -> PathBuf {
        self.notes_dir.clone()
    }

    /// Re-index only the given changed paths (file watcher). Files the
    /// app itself just wrote are skipped: their indexed mtime already
    /// matches. Returns the note ids that actually changed.
    pub fn sync_paths(&self, paths: &[PathBuf]) -> Result<Vec<String>> {
        let mut changed = Vec::new();
        for path in paths {
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let id = id.to_string();
            if path.exists() {
                let mtime = file_mtime_ms(path)?;
                let known: Option<i64> = {
                    let conn = self.conn.lock().expect("store lock poisoned");
                    conn.query_row("SELECT mtime FROM notes WHERE id = ?1", rusqlite::params![id], |r| r.get(0))
                        .ok()
                };
                if known == Some(mtime) {
                    continue;
                }
                let bytes = std::fs::read(path)?;
                let (text, _) = decode_note_bytes(&bytes);
                let mut conn = self.conn.lock().expect("store lock poisoned");
                let tx = conn.transaction()?;
                Self::index_note(&tx, &id, &text, mtime)?;
                tx.commit()?;
                changed.push(id);
            } else {
                let mut conn = self.conn.lock().expect("store lock poisoned");
                let tx = conn.transaction()?;
                Self::remove_note(&tx, &id)?;
                tx.commit()?;
                changed.push(id);
            }
        }
        Ok(changed)
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
        let bytes = std::fs::read(self.note_path(id))?;
        Ok(decode_note_bytes(&bytes).0)
    }

    /// Save a note. Empty text deletes the file and its index rows.
    pub fn save(&self, id: &str, text: &str) -> Result<()> {
        let t0 = std::time::Instant::now();
        let r = self.save_impl(id, text);
        crate::trace::log("save", t0.elapsed());
        r
    }

    fn save_impl(&self, id: &str, text: &str) -> Result<()> {
        if text.trim().is_empty() {
            // Empty text trashes the note (never a hard delete).
            self.trash_note(id)?;
            return Ok(());
        }
        let path = self.note_path(id);
        atomic_write(&path, text.as_bytes())?;
        let mtime = file_mtime_ms(&path)?;
        let mut conn = self.conn.lock().expect("store lock poisoned");
        let tx = conn.transaction()?;
        Self::index_note(&tx, id, text, mtime)?;
        tx.commit()?;
        Ok(())
    }

    pub fn trash_dir(&self) -> PathBuf {
        self.data_dir().join("trash")
    }

    /// Move a note file to `.lazykasten/trash/` and drop its index rows.
    /// Returns true when a file was moved. Never hard-deletes.
    pub fn trash_note(&self, id: &str) -> Result<bool> {
        let path = self.note_path(id);
        let mut moved = false;
        if path.exists() {
            let dir = self.trash_dir();
            std::fs::create_dir_all(&dir)?;
            let mut target = dir.join(format!("{id}.md"));
            let mut n = 2;
            while target.exists() {
                target = dir.join(format!("{id}-{n}.md"));
                n += 1;
            }
            std::fs::rename(&path, &target)?;
            moved = true;
        }
        let mut conn = self.conn.lock().expect("store lock poisoned");
        let tx = conn.transaction()?;
        Self::remove_note(&tx, id)?;
        tx.commit()?;
        Ok(moved)
    }

    /// Notes currently in the trash, newest id first.
    pub fn trashed_notes(&self) -> Result<Vec<NoteSummary>> {
        let dir = self.trash_dir();
        let mut out = Vec::new();
        if dir.is_dir() {
            for entry in std::fs::read_dir(&dir)? {
                let path = entry?.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let text = std::fs::read(&path)
                    .map(|b| decode_note_bytes(&b).0)
                    .unwrap_or_default();
                out.push(NoteSummary {
                    id: stem.to_string(),
                    title: title_of(&text),
                });
            }
        }
        out.sort_by(|a, b| b.id.cmp(&a.id));
        Ok(out)
    }

    /// Move a trashed note back into `notes/` and re-index it.
    pub fn restore_note(&self, id: &str) -> Result<()> {
        let from = self.trash_dir().join(format!("{id}.md"));
        let to = self.note_path(id);
        if to.exists() {
            anyhow::bail!("a live note already uses id {id}");
        }
        std::fs::rename(&from, &to)?;
        let text = self.read(id)?;
        let mtime = file_mtime_ms(&to)?;
        let mut conn = self.conn.lock().expect("store lock poisoned");
        let tx = conn.transaction()?;
        Self::index_note(&tx, id, &text, mtime)?;
        tx.commit()?;
        Ok(())
    }

    /// The note file's current mtime in ms, if it exists.
    pub fn note_mtime(&self, id: &str) -> Option<i64> {
        file_mtime_ms(&self.note_path(id)).ok()
    }

    /// Save `text` as `<id>-conflict-<timestamp>.md` (a normal indexed
    /// note) without touching the original file. Returns the new id.
    pub fn write_conflict_copy(&self, id: &str, text: &str) -> Result<String> {
        let base = format!(
            "{id}-conflict-{}",
            chrono::Local::now().format("%Y%m%d%H%M%S")
        );
        let mut candidate = base.clone();
        let mut n = 2;
        while self.note_path(&candidate).exists() {
            candidate = format!("{base}-{n}");
            n += 1;
        }
        self.save(&candidate, text)?;
        Ok(candidate)
    }

    /// Rename (or merge) a tag across every note carrying it: `Tag:`
    /// line entries and `#hashtags` are rewritten, files are written
    /// atomically through `save`, then re-indexed by the same path.
    /// Returns how many notes changed.
    pub fn rename_tag(&self, old: &str, new: &str) -> Result<usize> {
        let notes = self.notes_with_tag(old)?;
        let mut changed = 0usize;
        for note in &notes {
            let text = self.read(&note.id)?;
            let (rewritten, hit) = rewrite_tag_in_text(&text, old, new);
            if hit {
                self.save(&note.id, &rewritten)?;
                changed += 1;
            }
        }
        Ok(changed)
    }

    pub fn index_note(conn: &rusqlite::Connection, id: &str, text: &str, mtime: i64) -> Result<()> {
        let mut tags = parse_tags(text);
        if tags.is_empty() {
            tags.push("inbox".to_string());
        }
        let title = title_of(text);
        conn.prepare_cached(
            "INSERT INTO notes(id, title, mtime) VALUES (?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET title = excluded.title, mtime = excluded.mtime",
        )?.execute(rusqlite::params![id, title, mtime])?;
        conn.prepare_cached("DELETE FROM note_tags WHERE note_id = ?1")?.execute(rusqlite::params![id])?;
        for tag in &tags {
            conn.prepare_cached("INSERT OR IGNORE INTO tags(name) VALUES (?1)")?
                .execute(rusqlite::params![tag])?;
            let tag_id: i64 = conn
                .prepare_cached("SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE")?
                .query_row(rusqlite::params![tag], |r| r.get(0))?;
            conn.prepare_cached("INSERT OR IGNORE INTO note_tags(tag_id, note_id) VALUES (?1, ?2)")?
                .execute(rusqlite::params![tag_id, id])?;
        }
        // FTS is keyed by notes.rowid; an upsert keeps the rowid.
        let rowid: i64 = conn
            .prepare_cached("SELECT rowid FROM notes WHERE id = ?1")?
            .query_row(rusqlite::params![id], |r| r.get(0))?;
        conn.prepare_cached("DELETE FROM notes_text WHERE rowid = ?1")?.execute(rusqlite::params![rowid])?;
        conn.prepare_cached("INSERT INTO notes_text(rowid, body) VALUES (?1, ?2)")?
            .execute(rusqlite::params![rowid, text])?;
        conn.prepare_cached("DELETE FROM links WHERE from_id = ?1")?.execute(rusqlite::params![id])?;
        for to_id in parse_links(text) {
            conn.prepare_cached("INSERT OR IGNORE INTO links(from_id, to_id) VALUES (?1, ?2)")?
                .execute(rusqlite::params![id, to_id])?;
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
        let t0 = std::time::Instant::now();
        let r = self.sync_with_disk_impl();
        crate::trace::log("sync_with_disk", t0.elapsed());
        r
    }

    fn sync_with_disk_impl(&self) -> Result<usize> {
        // 1. List *.md files with mtimes.
        let mut files: Vec<(String, PathBuf, i64)> = Vec::new();
        if self.notes_dir.is_dir() {
            for entry in std::fs::read_dir(&self.notes_dir)? {
                let entry = entry?;
                let path = entry.path();
                // Ignore in-flight atomic writes; delete stale .tmp files.
                if path.extension().and_then(|e| e.to_str()) == Some("tmp") {
                    let stale = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .map(|t| t.elapsed().map(|d| d.as_millis() > 60_000).unwrap_or(false))
                        .unwrap_or(false);
                    if stale {
                        let _ = std::fs::remove_file(&path);
                    }
                    continue;
                }
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let stem = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                // DirEntry::metadata reuses the directory listing data on
                // Windows (no extra syscall per file, unlike fs::metadata).
                let mtime = entry
                    .metadata()
                    .and_then(|m| m.modified())
                    .map(|t| t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64)
                    .unwrap_or(0);
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
        let mut lossy_files = 0usize;
        for (id, path, mtime) in &files {
            if db_map.get(id) != Some(mtime) {
                // Read contents before taking the lock. Bytes + lossy
                // decode: one non-UTF-8 file must not fail the sync.
                let bytes = std::fs::read(path)?;
                let (text, lossy) = decode_note_bytes(&bytes);
                if lossy {
                    lossy_files += 1;
                }
                to_index.push((id.clone(), text, *mtime));
            }
        }
        if lossy_files > 0 {
            crate::trace::info(&format!("sync_with_disk: {lossy_files} file(s) needed lossy decoding"));
        }
        let to_remove: Vec<String> = db_map
            .keys()
            .filter(|id| !file_ids.contains(*id))
            .cloned()
            .collect();

        // 3. Re-index in batches of 500 per transaction, releasing the lock
        // between batches so a hotkey query never waits for a whole sync.
        let count = to_index.len();
        for chunk in to_index.chunks(500) {
            let mut conn = self.conn.lock().expect("store lock poisoned");
            let tx = conn.transaction()?;
            for (id, text, mtime) in chunk {
                Self::index_note(&tx, id, text, *mtime)?;
            }
            tx.commit()?;
        }
        if count > 1000 {
            // Merge FTS segments after a big re-index (sync already runs in
            // the background) so queries read fewer pages.
            let conn = self.conn.lock().expect("store lock poisoned");
            let _ = conn.execute("INSERT INTO notes_text(notes_text) VALUES('optimize')", []);
        }
        // 4. Remove rows for vanished files and delete orphan tags.
        {
            let mut conn = self.conn.lock().expect("store lock poisoned");
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
        }
        Ok(count)
    }

    pub fn tags(&self) -> Result<Vec<TagCount>> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare_cached(
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
        let t0 = std::time::Instant::now();
        let r = self.notes_with_tag_impl(tag);
        crate::trace::log("notes_with_tag", t0.elapsed());
        r
    }

    fn notes_with_tag_impl(&self, tag: &str) -> Result<Vec<NoteSummary>> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare_cached(
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

    pub fn backlink_count(&self, id: &str) -> Result<i64> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let n: i64 = conn
            .prepare_cached("SELECT COUNT(*) FROM links WHERE to_id = ?1")?
            .query_row(rusqlite::params![id], |r| r.get(0))?;
        Ok(n)
    }

    /// Notes that link to `id`, newest first.
    pub fn backlinks(&self, id: &str) -> Result<Vec<NoteSummary>> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare_cached(
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
        let mut stmt = conn.prepare_cached("SELECT id, title FROM notes ORDER BY id DESC LIMIT ?1")?;
        let rows = stmt.query_map(rusqlite::params![limit], |r| {
            Ok(NoteSummary { id: r.get(0)?, title: r.get(1)? })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// All notes for the graph: `(rowid, id, title, first tag by name)`.
    pub fn graph_notes(&self) -> Result<Vec<(i64, String, String, Option<String>)>> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare_cached(
            "SELECT n.rowid, n.id, n.title,
                    (SELECT t.name FROM note_tags nt JOIN tags t ON t.id = nt.tag_id
                     WHERE nt.note_id = n.id ORDER BY t.name COLLATE NOCASE LIMIT 1)
             FROM notes n ORDER BY n.rowid",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Link edges as rowid pairs (both endpoints must be indexed notes).
    pub fn graph_links(&self) -> Result<Vec<(i64, i64)>> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare_cached(
            "SELECT nf.rowid, nt.rowid FROM links l
             JOIN notes nf ON nf.id = l.from_id JOIN notes nt ON nt.id = l.to_id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// `(tag, note rowids)` groups, tags ordered by name.
    pub fn graph_tag_notes(&self) -> Result<Vec<(String, Vec<i64>)>> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare_cached(
            "SELECT t.name, n.rowid FROM tags t
             JOIN note_tags nt ON nt.tag_id = t.id JOIN notes n ON n.id = nt.note_id
             ORDER BY t.name COLLATE NOCASE, n.rowid",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
        let mut out: Vec<(String, Vec<i64>)> = Vec::new();
        for row in rows {
            let (tag, rowid) = row?;
            match out.last_mut() {
                Some((t, ids)) if *t == tag => ids.push(rowid),
                _ => out.push((tag, vec![rowid])),
            }
        }
        Ok(out)
    }

    pub fn search(&self, query: &str, limit: i64) -> Result<Vec<NoteSummary>> {
        let t0 = std::time::Instant::now();
        let r = self.search_impl(query, limit);
        crate::trace::log("search", t0.elapsed());
        r
    }

    fn search_impl(&self, query: &str, limit: i64) -> Result<Vec<NoteSummary>> {
        let words: Vec<&str> = query.split_whitespace().collect();
        if words.is_empty() {
            return Ok(Vec::new());
        }
        let parts: Vec<String> = words
            .iter()
            .map(|w| format!("\"{}\"", w.replace('"', "\"\"")))
            .collect();
        // A single 1-character query is an exact-term search: a 1-char
        // prefix is the FTS worst case and rarely what the user means.
        // Prefix matching starts at 2 characters.
        let single_char = words.len() == 1 && words[0].chars().count() == 1;
        let match_expr = if single_char {
            parts.join(" ")
        } else {
            format!("{}*", parts.join(" "))
        };
        // Queries up to 2 characters skip bm25 ranking and return newest
        // first, so SQLite can stop early instead of ranking every hit.
        let short = query.trim().chars().count() <= 2;
        let order = if short { "n.id DESC" } else { "rank" };
        let sql = format!(
            "SELECT n.id, n.title FROM notes_text f
             JOIN notes n ON n.rowid = f.rowid
             WHERE notes_text MATCH ?1
             ORDER BY {order}
             LIMIT ?2"
        );
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare_cached(&sql)?;
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

    /// All `(id, title)` pairs, newest id first (quick-switcher index).
    pub fn notes_titles(&self) -> Result<Vec<(String, String)>> {
        let conn = self.conn.lock().expect("store lock poisoned");
        let mut stmt = conn.prepare_cached("SELECT id, title FROM notes ORDER BY id DESC")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
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

/// Rewrite tag `old` to `new` in a note's text: entries on `Tag:` lines
/// (deduplicated, so a rename into an existing tag merges) and
/// `#hashtags` (skipped when `new` contains whitespace, which a hashtag
/// cannot). Returns the new text and whether anything changed.
pub fn rewrite_tag_in_text(text: &str, old: &str, new: &str) -> (String, bool) {
    let mut changed = false;
    let mut out_lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = is_tag_line(line.trim()) {
            let head = &line[..line.find(':').map(|i| i + 1).unwrap_or(line.len())];
            let mut vals: Vec<String> = Vec::new();
            for v in rest.split(',') {
                let v = v.trim();
                if v.is_empty() {
                    continue;
                }
                let mapped = if v.eq_ignore_ascii_case(old) { new.to_string() } else { v.to_string() };
                if mapped != v {
                    changed = true;
                }
                if !vals.iter().any(|x| x.eq_ignore_ascii_case(&mapped)) {
                    vals.push(mapped);
                } else if mapped.eq_ignore_ascii_case(new) && !v.eq_ignore_ascii_case(old) {
                    // duplicate collapsed by the merge
                    changed = true;
                }
            }
            out_lines.push(format!("{} {}", head, vals.join(", ")));
        } else {
            out_lines.push(line.to_string());
        }
    }
    let mut text = out_lines.join("\n");
    if !new.chars().any(|c| c.is_whitespace()) {
        // Hashtags: `#old` at a word boundary.
        let chars: Vec<char> = text.chars().collect();
        let mut rebuilt = String::new();
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '#'
                && (i == 0 || (!chars[i - 1].is_alphanumeric() && chars[i - 1] != '#'))
            {
                let mut j = i + 1;
                while j < chars.len()
                    && (chars[j].is_alphanumeric() || matches!(chars[j], '-' | '_' | '/' | '.'))
                {
                    j += 1;
                }
                let token: String = chars[i + 1..j].iter().collect();
                if !token.is_empty() && token.eq_ignore_ascii_case(old) {
                    rebuilt.push('#');
                    rebuilt.push_str(new);
                    changed = true;
                    i = j;
                    continue;
                }
            }
            rebuilt.push(chars[i]);
            i += 1;
        }
        text = rebuilt;
    }
    (text, changed)
}

/// Write `bytes` to `path` atomically: `<name>.tmp` in the same folder,
/// then rename over the target (rename replaces on Windows).
pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .map(|n| format!("{}.tmp", n.to_string_lossy()))
        .unwrap_or_else(|| "file.tmp".to_string());
    let tmp = path.with_file_name(name);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
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
    fn test_search_single_letter_is_exact() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        store.save("20260101000001", "A standalone letter note").unwrap();
        store.save("20260101000002", "Apple orchard notes").unwrap();
        // 1-char query: exact term only, so "Apple" must not match "a".
        let hits = store.search("a", 10).unwrap();
        assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), vec!["20260101000001"]);
        // 2-char query keeps prefix matching.
        let hits = store.search("Ap", 10).unwrap();
        assert!(hits.iter().any(|h| h.id == "20260101000002"));
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_search_short_query_newest_first() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        store.save("20260101000001", "About the first note").unwrap();
        store.save("20260101000003", "About the third note").unwrap();
        store.save("20260101000002", "About the second note").unwrap();
        let hits = store.search("Ab", 10).unwrap();
        assert_eq!(
            hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
            vec!["20260101000003", "20260101000002", "20260101000001"]
        );
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_bom_and_lossy_notes_are_indexed() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        // BOM-prefixed tagged note (as PowerShell 5.1 Set-Content writes).
        let mut bom = b"\xEF\xBB\xBF".to_vec();
        bom.extend_from_slice("BOM title\nBody text\nTag: Rust".as_bytes());
        std::fs::write(store.note_path("20260101000001"), &bom).unwrap();
        // Windows-1252 note: 0xE9 is not valid UTF-8 on its own.
        std::fs::write(store.note_path("20260101000002"), b"Caf\xE9 note\nBody\nTag: Cooking").unwrap();
        assert_eq!(store.sync_with_disk().unwrap(), 2);
        assert_eq!(store.notes_with_tag("Rust").unwrap().len(), 1);
        assert_eq!(store.notes_with_tag("Cooking").unwrap().len(), 1);
        assert_eq!(store.notes_with_tag("Rust").unwrap()[0].title, "BOM title");
        let (text, lossy) = decode_note_bytes(b"\xEF\xBB\xBFhello");
        assert_eq!(text, "hello");
        assert!(!lossy);
        let (_, lossy) = decode_note_bytes(b"caf\xE9");
        assert!(lossy);
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
            // 1-3 deterministic links per note (every third note also
            // links forward), so the graph bench sees a realistic web.
            let mut body = format!("Note title {i}\nBody text for note {i} with searchable words.");
            if i > 0 {
                body.push_str(&format!("\nSee [[{:014}]]", 20260101000000u64 + (i / 2) as u64));
                if i % 3 == 0 && i + 7 < n {
                    body.push_str(&format!(" and [[{:014}]]", 20260101000000u64 + (i + 7) as u64));
                }
            }
            std::fs::write(
                store.note_path(&id),
                format!("{body}\nTag: tag{}, common", i % 50),
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

        // Worst cases: short prefix searches, plus the no-change re-check.
        let t0 = Instant::now();
        let hits1 = store.search("a", 500).unwrap();
        let search1_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let _ = &hits1;
        let t0 = Instant::now();
        let hits2 = store.search("se", 500).unwrap();
        let search2_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let _ = &hits2;
        let t0 = Instant::now();
        let rechecked = store.sync_with_disk().unwrap();
        let recheck_secs = t0.elapsed().as_secs_f64();
        assert_eq!(rechecked, 0);

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
        println!("search 1-letter (a): {search1_ms:.3}ms ({} hits)", hits1.len());
        println!("search 2-letter (se): {search2_ms:.3}ms ({} hits)", hits2.len());
        println!("startup re-check (no changes): {recheck_secs:.3}s");
        println!("auto-tag suggest: {autotag_ms:.3}ms per note");

        let t0 = Instant::now();
        let gdata = crate::graph::build(&store, crate::graph::GraphMode::Global, None, 2, false, false).unwrap();
        let graph_load_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let mut glayout = crate::graph::Layout::new(&gdata, &std::collections::HashMap::new());
        let t0 = Instant::now();
        for _ in 0..5 {
            glayout.step(&gdata, &std::collections::HashSet::new());
        }
        let graph_step_ms = t0.elapsed().as_secs_f64() * 1000.0 / 5.0;
        println!(
            "graph: {} nodes, {} edges, data load {graph_load_ms:.3}ms, layout step {graph_step_ms:.3}ms",
            gdata.nodes.len(),
            gdata.edges.len()
        );

        // Settle from scratch (coarse tag-cluster start, no cache).
        let mut settle_layout = crate::graph::Layout::new(&gdata, &std::collections::HashMap::new());
        let t0 = Instant::now();
        let mut steps = 0usize;
        for _ in 0..3000 {
            steps += 1;
            if settle_layout.step(&gdata, &std::collections::HashSet::new()) < 0.08 {
                break;
            }
        }
        println!(
            "graph settle from scratch: {:.3}s in {steps} steps",
            t0.elapsed().as_secs_f64()
        );

        // Quick switcher: fuzzy filter over every title, per keystroke.
        let titles = store.notes_titles().unwrap();
        let index: Vec<(String, String, String)> = titles
            .into_iter()
            .map(|(id, title)| {
                let lower = title.to_lowercase();
                (title, lower, id)
            })
            .collect();
        let t0 = Instant::now();
        let mut hits = 0usize;
        for _ in 0..10 {
            hits = index
                .iter()
                .filter(|(_, lower, _)| crate::ui::fuzzy_score(lower, "note").is_some())
                .count();
        }
        println!(
            "switcher fuzzy ({} titles): {:.3}ms per keystroke ({hits} hits)",
            index.len(),
            t0.elapsed().as_secs_f64() * 1000.0 / 10.0
        );

        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_rename_tag_merges() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        store.save("20260101000001", "One\nTag: js, Web").unwrap();
        store.save("20260101000002", "Two #js note\nTag: Javascript").unwrap();
        store.save("20260101000003", "Three\nTag: js, Javascript").unwrap();
        let changed = store.rename_tag("js", "Javascript").unwrap();
        assert_eq!(changed, 3);
        assert_eq!(store.read("20260101000001").unwrap(), "One\nTag: Javascript, Web");
        assert!(store.read("20260101000002").unwrap().contains("#Javascript"));
        // The merged note keeps one Javascript entry, not two.
        assert_eq!(store.read("20260101000003").unwrap(), "Three\nTag: Javascript");
        assert_eq!(store.notes_with_tag("Javascript").unwrap().len(), 3);
        assert!(store.notes_with_tag("js").unwrap().is_empty());
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_sync_paths_external_edit() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        store.save("20260101000000", "Original\nTag: Rust").unwrap();
        // The app's own save is not a change.
        let own = store.sync_paths(&[store.note_path("20260101000000")]).unwrap();
        assert!(own.is_empty(), "{own:?}");
        // An outside edit is picked up; a delete removes the note.
        std::thread::sleep(std::time::Duration::from_millis(15));
        std::fs::write(store.note_path("20260101000000"), "Edited elsewhere\nTag: Cooking").unwrap();
        let changed = store.sync_paths(&[store.note_path("20260101000000")]).unwrap();
        assert_eq!(changed, vec!["20260101000000".to_string()]);
        assert_eq!(store.notes_with_tag("Cooking").unwrap().len(), 1);
        std::fs::remove_file(store.note_path("20260101000000")).unwrap();
        let changed = store.sync_paths(&[store.note_path("20260101000000")]).unwrap();
        assert_eq!(changed, vec!["20260101000000".to_string()]);
        assert_eq!(store.note_count().unwrap(), 0);
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn test_trash_restore_and_conflict_copy() {
        let root = temp_root();
        let store = Store::open(&root).unwrap();
        store.save("20260101000000", "Keep me\nTag: Rust").unwrap();
        // Atomic save leaves no .tmp behind.
        assert!(!root.join("notes").join("20260101000000.md.tmp").exists());

        // Saving empty text trashes instead of deleting.
        store.save("20260101000000", "").unwrap();
        assert!(!store.note_path("20260101000000").exists());
        assert_eq!(store.note_count().unwrap(), 0);
        let trashed = store.trashed_notes().unwrap();
        assert_eq!(trashed.len(), 1);
        assert_eq!(trashed[0].id, "20260101000000");
        assert_eq!(trashed[0].title, "Keep me");

        // Restore brings it back, indexed.
        store.restore_note("20260101000000").unwrap();
        assert_eq!(store.read("20260101000000").unwrap(), "Keep me\nTag: Rust");
        assert_eq!(store.note_count().unwrap(), 1);
        assert!(store.trashed_notes().unwrap().is_empty());

        // A conflict copy preserves the editor text under a new id and
        // leaves the original file untouched.
        let conflict = store
            .write_conflict_copy("20260101000000", "My offline edits\nTag: Rust")
            .unwrap();
        assert!(conflict.starts_with("20260101000000-conflict-"), "{conflict}");
        assert_eq!(store.read(&conflict).unwrap(), "My offline edits\nTag: Rust");
        assert_eq!(store.read("20260101000000").unwrap(), "Keep me\nTag: Rust");
        drop(store);
        let _ = std::fs::remove_dir_all(&root);
    }
}
