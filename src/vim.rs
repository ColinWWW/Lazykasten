//! Pure Vim engine: no GPUI, no I/O. One keystroke in, a list of edits
//! plus the new cursor/selection/mode out. All positions are UTF-8 byte
//! offsets on grapheme boundaries. Motions never copy the text; edits
//! are `(range, replacement)` pairs the caller applies back to front.

use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Normal,
    Insert,
    Visual,
    VisualLine,
    Command,
    Search,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Key {
    Char(char),
    Esc,
    Ctrl(char),
    Enter,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    None,
    /// Yank/delete also lands here: (text, linewise). The UI writes it
    /// to the Windows clipboard.
    Yank(String, bool),
    /// Paste requested; the UI passes the clipboard text back via
    /// `Engine::paste_text`.
    PasteRequest(bool),
    /// `.` pressed: the UI replays `repeat_change()` keys itself.
    RepeatRequest,
    Undo,
    Redo,
    HideRequest,
    ExCommand(String),
    ScrollHalf(i32),
    CenterLine,
    Bell,
}

#[derive(Clone, Debug)]
pub struct Out {
    pub edits: Vec<Edit>,
    pub cursor: usize,
    pub selection: Option<Range<usize>>,
    pub effect: Effect,
}

impl Out {
    fn at(cursor: usize) -> Self {
        Out { edits: Vec::new(), cursor, selection: None, effect: Effect::None }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Await {
    Find(char),       // f t F T (the kind), waiting for the target char
    Replace,          // r, waiting for the char
    TextObject(bool), // after i/a in operator or visual: true = inner
    G,                // after g: gg or ge
    Z, // after z: zz
}

#[derive(Clone)]
struct LastChange {
    keys: Vec<Key>,
    insert: Option<String>,
}

pub struct Engine {
    pub mode: Mode,
    count: usize,
    count2: usize,
    operator: Option<char>,
    awaiting: Option<Await>,
    anchor: Option<usize>,
    register: (String, bool),
    last_find: Option<(char, char)>,
    last_search: (String, bool),
    last_change: Option<LastChange>,
    seq: Vec<Key>,
    input_line: String,
    pub search_backward: bool,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

// -- text helpers (all byte offsets) --------------------------------------

fn lines_of(text: &str) -> Vec<(usize, usize)> {
    // (line start, line end without newline); `match_indices` uses
    // memchr, so this stays fast on large notes.
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, _) in text.match_indices('\n') {
        out.push((start, i));
        start = i + 1;
    }
    out.push((start, text.len()));
    out
}

/// (start, end-without-newline) of the line containing `pos`, found by
/// scanning outward only — O(line length), not O(text).
fn line_bounds(text: &str, pos: usize) -> (usize, usize) {
    let pos = pos.min(text.len());
    let start = text[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = text[pos..].find('\n').map(|i| pos + i).unwrap_or(text.len());
    (start, end)
}

fn line_idx(lines: &[(usize, usize)], pos: usize) -> usize {
    let mut idx = 0;
    for (i, (s, e)) in lines.iter().enumerate() {
        if pos >= *s && pos <= *e {
            idx = i;
        }
        if pos < *s {
            break;
        }
    }
    idx
}

fn next_grapheme(text: &str, pos: usize) -> usize {
    if pos >= text.len() {
        return text.len();
    }
    text[pos..]
        .graphemes(true)
        .next()
        .map(|g| pos + g.len())
        .unwrap_or(text.len())
}

fn prev_grapheme(text: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    text[..pos]
        .graphemes(true)
        .next_back()
        .map(|g| pos - g.len())
        .unwrap_or(0)
}

fn clamp_cursor(text: &str, pos: usize) -> usize {
    // Normal-mode cursor sits *on* a grapheme: at most the last
    // grapheme of the line (or the line end on an empty line).
    if text.is_empty() {
        return 0;
    }
    let pos = pos.min(text.len());
    let (s, e) = line_bounds(text, pos);
    if pos >= e && e > s {
        prev_grapheme(text, e)
    } else {
        pos.min(e)
    }
}

fn first_nonblank(text: &str, line: (usize, usize)) -> usize {
    let mut p = line.0;
    while p < line.1 && (text.as_bytes()[p] == b' ' || text.as_bytes()[p] == b'\t') {
        p += 1;
    }
    p
}

#[derive(Clone, Copy, PartialEq)]
enum CharKind {
    Word,
    Punct,
    Space,
}

fn kind_of(c: char) -> CharKind {
    if c.is_whitespace() {
        CharKind::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharKind::Word
    } else {
        CharKind::Punct
    }
}

/// One word step. `big` = WORD (whitespace-delimited). `end` = land on
/// the word's last grapheme instead of the next word's first.
/// Streaming: never collects the text's chars.
fn word_step(text: &str, pos: usize, big: bool, forward: bool, end: bool) -> usize {
    let kind = |c: char| {
        if big {
            if c.is_whitespace() { CharKind::Space } else { CharKind::Word }
        } else {
            kind_of(c)
        }
    };
    if forward {
        let mut it = text[pos..].char_indices().map(|(o, c)| (pos + o, c)).peekable();
        if end {
            // End of the next word: step once, skip spaces, run to the
            // last char of that word.
            it.next();
            while matches!(it.peek(), Some((_, c)) if kind(*c) == CharKind::Space) {
                it.next();
            }
            let Some(&(start, c)) = it.peek() else {
                return clamp_cursor(text, text.len());
            };
            let k = kind(c);
            let mut last = start;
            while matches!(it.peek(), Some((_, c)) if kind(*c) == k) {
                last = it.next().unwrap().0;
            }
            last
        } else {
            // Start of the next word: skip the current run, then spaces.
            if let Some(&(_, c)) = it.peek() {
                let k = kind(c);
                if k != CharKind::Space {
                    while matches!(it.peek(), Some((_, c)) if kind(*c) == k) {
                        it.next();
                    }
                }
            }
            while matches!(it.peek(), Some((_, c)) if kind(*c) == CharKind::Space) {
                it.next();
            }
            match it.peek() {
                Some(&(p, _)) => p,
                None => clamp_cursor(text, text.len()),
            }
        }
    } else {
        let mut it = text[..pos].char_indices().rev().peekable();
        if end {
            // ge: end of the previous word = first non-space met going back.
            it.next();
            while matches!(it.peek(), Some((_, c)) if kind(*c) == CharKind::Space) {
                it.next();
            }
            match it.peek() {
                Some(&(p, _)) => p,
                None => 0,
            }
        } else {
            // b: start of the previous word: skip spaces back, then
            // the word's run back; land on its first char.
            it.next();
            while matches!(it.peek(), Some((_, c)) if kind(*c) == CharKind::Space) {
                it.next();
            }
            let Some(&(_, c)) = it.peek() else { return 0 };
            let k = kind(c);
            let mut first = pos;
            while matches!(it.peek(), Some((_, c)) if kind(*c) == k) {
                first = it.next().unwrap().0;
            }
            first
        }
    }
}

fn matching_bracket(text: &str, pos: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let pairs = [(b'(', b')'), (b'[', b']'), (b'{', b'}')];
    // Find the bracket at or after the cursor on this line.
    let lines = lines_of(text);
    let li = line_idx(&lines, pos);
    let (ls, le) = lines[li];
    let mut p = pos;
    let (open, close, forward) = loop {
        if p > le {
            return None;
        }
        if p < text.len() {
            let c = bytes[p];
            if let Some((o, cl)) = pairs.iter().find(|(o, _)| *o == c) {
                break (*o, *cl, true);
            }
            if let Some((o, cl)) = pairs.iter().find(|(_, cl)| *cl == c) {
                break (*o, *cl, false);
            }
        }
        if p == le {
            return None;
        }
        p = next_grapheme(text, p);
    };
    let _ = ls;
    if forward {
        let mut depth = 0i32;
        let mut i = p;
        while i < text.len() {
            if bytes[i] == open {
                depth += 1;
            } else if bytes[i] == close {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            i += 1;
        }
        None
    } else {
        let mut depth = 0i32;
        let mut i = p as i32;
        while i >= 0 {
            let c = bytes[i as usize];
            if c == close {
                depth += 1;
            } else if c == open {
                depth -= 1;
                if depth == 0 {
                    return Some(i as usize);
                }
            }
            i -= 1;
        }
        None
    }
}

/// All matches of `query` (smartcase) as byte ranges.
fn find_matches(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let sensitive = query.chars().any(|c| c.is_uppercase());
    let hay: Vec<(usize, char)> = text.char_indices().collect();
    let needle: Vec<char> = query.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + needle.len() <= hay.len() {
        let hit = needle.iter().enumerate().all(|(j, nc)| {
            let hc = hay[i + j].1;
            if sensitive { hc == *nc } else { hc.to_lowercase().eq(nc.to_lowercase()) }
        });
        if hit {
            let start = hay[i].0;
            let end = if i + needle.len() < hay.len() {
                hay[i + needle.len()].0
            } else {
                text.len()
            };
            out.push(start..end);
            i += needle.len();
        } else {
            i += 1;
        }
    }
    out
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            mode: Mode::Normal,
            count: 0,
            count2: 0,
            operator: None,
            awaiting: None,
            anchor: None,
            register: (String::new(), false),
            last_find: None,
            last_search: (String::new(), false),
            last_change: None,
            seq: Vec::new(),
            input_line: String::new(),
            search_backward: false,
        }
    }

    pub fn pending_display(&self) -> String {
        match self.mode {
            Mode::Command | Mode::Search => {
                let mut s = if self.mode == Mode::Command {
                    ":".to_string()
                } else if self.search_backward {
                    "?".to_string()
                } else {
                    "/".to_string()
                };
                s.push_str(&self.input_line);
                s
            }
            _ => {
                let mut s = String::new();
                if self.count > 0 {
                    s.push_str(&self.count.to_string());
                }
                if let Some(op) = self.operator {
                    s.push(op);
                }
                if self.count2 > 0 {
                    s.push_str(&self.count2.to_string());
                }
                if let Some(a) = &self.awaiting {
                    s.push(match a {
                        Await::Find(k) => *k,
                        Await::Replace => 'r',
                        Await::TextObject(inner) => {
                            if *inner { 'i' } else { 'a' }
                        }
                        Await::G => 'g',
                        Await::Z => 'z',
                    });
                }
                s
            }
        }
    }

    /// Force a mode from the UI (note opened -> Normal, new note ->
    /// Insert), clearing any half-typed command.
    pub fn force_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.reset_pending();
        self.anchor = None;
    }

    fn reset_pending(&mut self) {
        self.count = 0;
        self.count2 = 0;
        self.operator = None;
        self.awaiting = None;
        self.seq.clear();
    }

    fn total_count(&self) -> usize {
        let a = if self.count == 0 { 1 } else { self.count };
        let b = if self.count2 == 0 { 1 } else { self.count2 };
        a * b
    }

    /// Paste entry point used by the UI with the clipboard contents. If
    /// the clipboard still equals our last yank, its linewise flag is
    /// used; otherwise it pastes character-wise.
    pub fn paste_text(&mut self, text: &str, clip: &str, cursor: usize, before: bool) -> Out {
        let linewise = self.register.0 == clip && self.register.1 && !clip.is_empty();
        let content = if clip.is_empty() { self.register.0.clone() } else { clip.to_string() };
        if content.is_empty() {
            return Out::at(cursor);
        }
        self.do_paste(text, cursor, &content, linewise, before)
    }

    fn do_paste(&mut self, text: &str, cursor: usize, content: &str, linewise: bool, before: bool) -> Out {
        let lines = lines_of(text);
        let li = line_idx(&lines, cursor);
        let (ls, le) = lines[li];
        if linewise {
            let body = content.trim_end_matches('\n');
            if before {
                let ins = format!("{body}\n");
                let mut out = Out::at(ls);
                out.edits.push(Edit { range: ls..ls, text: ins });
                out
            } else if le == text.len() && (text.is_empty() || !text.ends_with('\n')) {
                let ins = format!("\n{body}");
                let mut out = Out::at(le + 1);
                out.edits.push(Edit { range: le..le, text: ins });
                out
            } else {
                let at = le + 1;
                let ins = format!("{body}\n");
                let mut out = Out::at(at);
                out.edits.push(Edit { range: at..at, text: ins });
                out
            }
        } else {
            let at = if before {
                cursor
            } else if cursor >= le && le == ls {
                cursor
            } else {
                next_grapheme(text, cursor)
            };
            let new_text = format!("{}{}{}", &text[..at], content, &text[at..]);
            let end = at + content.len();
            let mut out = Out::at(cursor);
            out.edits.push(Edit { range: at..at, text: content.to_string() });
            out.cursor = if end > at {
                prev_grapheme(&new_text, end)
            } else {
                cursor
            };
            out
        }
    }

    // -- motions -----------------------------------------------------------

    /// Compute a motion target. Returns (target, inclusive, linewise).
    fn motion(&mut self, text: &str, cursor: usize, key: char, count: usize) -> Option<(usize, bool, bool)> {
        let lines = lines_of(text);
        let li = line_idx(&lines, cursor);
        let (ls, le) = lines[li];
        let n = count.max(1);
        match key {
            'h' => {
                let mut p = cursor;
                for _ in 0..n { p = prev_grapheme(text, p).max(ls); if p == ls { break; } }
                Some((p, false, false))
            }
            'l' => {
                let mut p = cursor;
                for _ in 0..n {
                    let nx = next_grapheme(text, p);
                    if nx > le { break; }
                    p = nx;
                    if p >= le { p = clamp_cursor(text, p); break; }
                }
                Some((p, true, false))
            }
            'j' => {
                let target = (li + n).min(lines.len() - 1);
                let col = cursor - ls;
                let (ts, te) = lines[target];
                Some((keep_col(text, ts, te, col), false, true))
            }
            'k' => {
                let target = li.saturating_sub(n);
                let col = cursor - ls;
                let (ts, te) = lines[target];
                Some((keep_col(text, ts, te, col), false, true))
            }
            'w' => {
                let mut p = cursor;
                for _ in 0..n { p = word_step(text, p, false, true, false); }
                Some((p, false, false))
            }
            'W' => {
                let mut p = cursor;
                for _ in 0..n { p = word_step(text, p, true, true, false); }
                Some((p, false, false))
            }
            'e' => {
                let mut p = cursor;
                for _ in 0..n { p = word_step(text, p, false, true, true); }
                Some((p, true, false))
            }
            'E' => {
                let mut p = cursor;
                for _ in 0..n { p = word_step(text, p, true, true, true); }
                Some((p, true, false))
            }
            'b' => {
                let mut p = cursor;
                for _ in 0..n { p = word_step(text, p, false, false, false); }
                Some((p, false, false))
            }
            'B' => {
                let mut p = cursor;
                for _ in 0..n { p = word_step(text, p, true, false, false); }
                Some((p, false, false))
            }
            '0' => Some((ls, false, false)),
            '^' => Some((first_nonblank(text, (ls, le)), false, false)),
            '$' => {
                let target = (li + n - 1).min(lines.len() - 1);
                Some((clamp_cursor(text, lines[target].1), true, false))
            }
            'G' => {
                let target = if self.count == 0 && self.count2 == 0 {
                    lines.len() - 1
                } else {
                    (n - 1).min(lines.len() - 1)
                };
                Some((first_nonblank(text, lines[target]), false, true))
            }
            '%' => matching_bracket(text, cursor).map(|p| (p, true, false)),
            '{' => {
                let mut target = li;
                for _ in 0..n {
                    let mut t = target;
                    if t > 0 { t -= 1; }
                    while t > 0 && !(lines[t].0 == lines[t].1) { t -= 1; }
                    target = t;
                }
                Some((lines[target].0, false, true))
            }
            '}' => {
                let mut target = li;
                for _ in 0..n {
                    let mut t = target;
                    if t < lines.len() - 1 { t += 1; }
                    while t < lines.len() - 1 && !(lines[t].0 == lines[t].1) { t += 1; }
                    target = t;
                }
                Some((lines[target].0, false, true))
            }
            _ => None,
        }
    }

    // -- text objects --------------------------------------------------------

    fn text_object(&mut self, text: &str, cursor: usize, inner: bool, obj: char) -> Option<Range<usize>> {
        let lines = lines_of(text);
        let li = line_idx(&lines, cursor);
        let (ls, le) = lines[li];
        match obj {
            'w' | 'W' => {
                let big = obj == 'W';
                // The word around the cursor: scan outward while the
                // char kind matches the char under the cursor.
                let kind_at = |c: char| {
                    if big {
                        if c.is_whitespace() { CharKind::Space } else { CharKind::Word }
                    } else {
                        kind_of(c)
                    }
                };
                let cur_char = text[cursor.min(text.len())..].chars().next().unwrap_or(' ');
                let k = kind_at(cur_char);
                let mut start = cursor;
                while start > 0 {
                    let p = prev_grapheme(text, start);
                    if kind_at(text[p..].chars().next().unwrap_or(' ')) != k {
                        break;
                    }
                    start = p;
                }
                let mut end = cursor;
                while end < text.len() {
                    let nx = next_grapheme(text, end);
                    if kind_at(text[end..].chars().next().unwrap_or(' ')) != k {
                        break;
                    }
                    end = nx;
                }
                if inner {
                    Some(start..end)
                } else {
                    // a word: include trailing whitespace (or leading).
                    let mut e = end;
                    while e < text.len() && text.as_bytes()[e] == b' ' {
                        e += 1;
                    }
                    if e == end {
                        let mut s = start;
                        while s > ls && text.as_bytes()[s - 1] == b' ' {
                            s -= 1;
                        }
                        Some(s..end)
                    } else {
                        Some(start..e)
                    }
                }
            }
            '"' | '\'' => {
                let line = &text[ls..le];
                let q = obj;
                let positions: Vec<usize> = line
                    .char_indices()
                    .filter(|(_, c)| *c == q)
                    .map(|(i, _)| ls + i)
                    .collect();
                let mut pair = None;
                for w in positions.windows(2) {
                    if cursor >= w[0] && cursor <= w[1] {
                        pair = Some((w[0], w[1]));
                        break;
                    }
                }
                let (a, b) = pair?;
                if inner {
                    Some(a + 1..b)
                } else {
                    Some(a..b + 1)
                }
            }
            '(' | ')' | '[' | ']' | '{' | '}' => {
                let (open, close) = match obj {
                    '(' | ')' => ('(', ')'),
                    '[' | ']' => ('[', ']'),
                    _ => ('{', '}'),
                };
                // Search outward for the enclosing pair.
                let mut depth = 0i32;
                let mut start = None;
                let i = cursor.min(text.len());
                let chars: Vec<(usize, char)> = text.char_indices().collect();
                let idx = chars.iter().position(|(b, _)| *b >= i).unwrap_or(chars.len());
                // Scan backward for open.
                let mut j = idx;
                loop {
                    if j >= chars.len() && idx >= chars.len() {
                        break;
                    }
                    if idx == 0 && j == 0 {
                        break;
                    }
                    if j == 0 {
                        break;
                    }
                    j -= 1;
                    let c = chars[j].1;
                    if c == close {
                        depth += 1;
                    } else if c == open {
                        if depth == 0 {
                            start = Some(chars[j].0);
                            break;
                        }
                        depth -= 1;
                    }
                    if j == 0 {
                        break;
                    }
                }
                let s = start?;
                let e = matching_bracket(text, s)?;
                if inner {
                    Some(s + 1..e)
                } else {
                    Some(s..e + 1)
                }
            }
            'p' => {
                // Paragraph: the blank-line-separated block around cursor.
                let blank = |i: usize| lines[i].0 == lines[i].1;
                if blank(li) {
                    return Some(ls..le);
                }
                let mut top = li;
                while top > 0 && !blank(top - 1) { top -= 1; }
                let mut bot = li;
                while bot + 1 < lines.len() && !blank(bot + 1) { bot += 1; }
                if inner {
                    Some(lines[top].0..lines[bot].1)
                } else {
                    // a paragraph: include the following blank line(s).
                    let end = if bot + 2 < lines.len() {
                        lines[bot + 2].0
                    } else if bot + 1 < lines.len() {
                        text.len()
                    } else {
                        lines[bot].1
                    };
                    Some(lines[top].0..end)
                }
            }
            _ => None,
        }
    }

    // -- operators -----------------------------------------------------------

    fn apply_operator(
        &mut self,
        text: &str,
        cursor: usize,
        range: Range<usize>,
        linewise: bool,
        op: char,
    ) -> Out {
        let lines = lines_of(text);
        let (range, new_text_line) = if linewise {
            // Expand to whole lines, including the trailing newline
            // (except possibly at EOF).
            let a = line_idx(&lines, range.start);
            let b = line_idx(&lines, range.end.saturating_sub(1).max(range.start));
            let start = lines[a].0;
            let end = if b + 1 < lines.len() {
                lines[b + 1].0
            } else {
                lines[b].1
            };
            (start..end, Some(a))
        } else {
            (range, None)
        };
        let yanked = text[range.clone()].to_string();
        match op {
            'y' => {
                self.register = (yanked.clone(), linewise);
                let mut out = Out::at(range.start);
                out.effect = Effect::Yank(yanked, linewise);
                out.cursor = clamp_cursor(text, range.start);
                out
            }
            'd' | 'c' => {
                self.register = (yanked.clone(), linewise);
                let mut out = Out::at(range.start);
                out.edits.push(Edit { range: range.clone(), text: String::new() });
                out.effect = Effect::Yank(yanked, linewise);
                if op == 'c' {
                    self.mode = Mode::Insert;
                    out.cursor = range.start;
                } else if let Some(a) = new_text_line {
                    let _ = a;
                    out.cursor = range.start;
                }
                out
            }
            _ => Out::at(cursor),
        }
    }

    fn finish_motion(&mut self, text: &str, cursor: usize, target: usize, inclusive: bool, linewise: bool) -> Out {
        if linewise && self.operator.is_some() {
            let lines = lines_of(text);
            let a = line_idx(&lines, cursor);
            let b = line_idx(&lines, target);
            let (lo, hi) = (a.min(b), a.max(b));
            let op = self.operator.unwrap();
            let out = self.apply_operator(text, cursor, lines[lo].0..lines[hi].1, true, op);
            self.reset_pending();
            return out;
        }
        let (start, end) = if target >= cursor {
            (cursor, if inclusive { next_grapheme(text, target) } else { target })
        } else {
            (target, if inclusive { next_grapheme(text, cursor) } else { cursor })
        };
        if let Some(op) = self.operator {
            let out = self.apply_operator(text, cursor, start..end, linewise, op);
            self.reset_pending();
            out
        } else if self.mode == Mode::Visual || self.mode == Mode::VisualLine {
            let anchor = self.anchor.unwrap_or(cursor);
            let (s, e) = if self.mode == Mode::VisualLine {
                let lines = lines_of(text);
                let a = line_idx(&lines, anchor);
                let b = line_idx(&lines, target);
                let (lo, hi) = (a.min(b), a.max(b));
                (lines[lo].0, lines[hi].1)
            } else if target >= anchor {
                (anchor, next_grapheme(text, target))
            } else {
                (target, next_grapheme(text, anchor))
            };
            let mut out = Out::at(target);
            out.selection = Some(s..e);
            out
        } else {
            self.reset_pending();
            Out::at(target)
        }
    }
}

impl Engine {
    /// Feed one keystroke. `cursor` is the current (normal-mode) cursor
    /// or selection end; `sel` is the current visual selection if any.
    pub fn handle_key(&mut self, text: &str, cursor: usize, key: Key) -> Out {
        match self.mode {
            Mode::Insert => {
                match key {
                    Key::Esc | Key::Ctrl('[') => {
                        self.mode = Mode::Normal;
                        self.reset_pending();
                        Out::at(prev_grapheme(text, cursor).max(0))
                    }
                    _ => Out::at(cursor),
                }
            }
            Mode::Command | Mode::Search => self.handle_input_line(text, cursor, key),
            _ => self.handle_normal(text, cursor, key),
        }
    }

    fn handle_input_line(&mut self, text: &str, cursor: usize, key: Key) -> Out {
        match key {
            Key::Esc => {
                self.mode = Mode::Normal;
                self.input_line.clear();
                Out::at(cursor)
            }
            Key::Enter => {
                let line = std::mem::take(&mut self.input_line);
                let was_search = self.mode == Mode::Search;
                self.mode = Mode::Normal;
                self.reset_pending();
                if was_search {
                    self.last_search = (line.clone(), self.search_backward);
                    self.do_search(text, cursor, &line, self.search_backward, true)
                } else if let Ok(n) = line.trim().parse::<usize>() {
                    let lines = lines_of(text);
                    let target = n.saturating_sub(1).min(lines.len() - 1);
                    Out::at(first_nonblank(text, lines[target]))
                } else {
                    let mut out = Out::at(cursor);
                    out.effect = Effect::ExCommand(line);
                    out
                }
            }
            Key::Char(c) => {
                self.input_line.push(c);
                Out::at(cursor)
            }
            Key::Ctrl(_) => Out::at(cursor),
        }
    }

    fn do_search(&mut self, text: &str, cursor: usize, query: &str, backward: bool, from_search: bool) -> Out {
        let _ = from_search;
        let matches = find_matches(text, query);
        if matches.is_empty() {
            let mut out = Out::at(clamp_cursor(text, cursor));
            out.effect = Effect::Bell;
            return out;
        }
        let target = if backward {
            matches
                .iter()
                .rev()
                .find(|m| m.start < cursor)
                .or_else(|| matches.last())
        } else {
            matches
                .iter()
                .find(|m| m.start > cursor)
                .or_else(|| matches.first())
        };
        match target {
            Some(m) => Out::at(m.start),
            None => Out::at(cursor),
        }
    }

    fn record_change(&mut self, keys: Vec<Key>) {
        self.last_change = Some(LastChange { keys, insert: None });
    }

    fn handle_normal(&mut self, text: &str, cursor: usize, key: Key) -> Out {
        // Esc: cancel pending, or ask the UI to hide.
        if key == Key::Esc {
            if self.count > 0 || self.operator.is_some() || self.awaiting.is_some() {
                self.reset_pending();
                return Out::at(cursor);
            }
            if self.mode == Mode::Visual || self.mode == Mode::VisualLine {
                self.mode = Mode::Normal;
                self.anchor = None;
                return Out::at(clamp_cursor(text, cursor));
            }
            let mut out = Out::at(cursor);
            out.effect = Effect::HideRequest;
            return out;
        }
        // Input-line modes.
        if self.operator.is_none() && self.awaiting.is_none() {
            match key {
                Key::Char(':') if self.mode == Mode::Normal => {
                    self.mode = Mode::Command;
                    self.input_line.clear();
                    self.reset_pending();
                    return Out::at(cursor);
                }
                Key::Char('/') if self.mode == Mode::Normal => {
                    self.mode = Mode::Search;
                    self.search_backward = false;
                    self.input_line.clear();
                    self.reset_pending();
                    return Out::at(cursor);
                }
                Key::Char('?') if self.mode == Mode::Normal => {
                    self.mode = Mode::Search;
                    self.search_backward = true;
                    self.input_line.clear();
                    self.reset_pending();
                    return Out::at(cursor);
                }
                _ => {}
            }
        }
        // Awaiting a char (f/t/F/T, r, text object, g).
        if let Some(awaiting) = self.awaiting {
            return self.handle_awaiting(text, cursor, awaiting, key);
        }
        match key {
            Key::Ctrl('r') => {
                let mut out = Out::at(cursor);
                out.effect = Effect::Redo;
                out
            }
            Key::Ctrl('d') => {
                let mut out = self.motion(text, cursor, 'j', 10).map(|(t, _, _)| Out::at(t)).unwrap_or(Out::at(cursor));
                out.effect = Effect::ScrollHalf(1);
                out
            }
            Key::Ctrl('u') => {
                let mut out = self.motion(text, cursor, 'k', 10).map(|(t, _, _)| Out::at(t)).unwrap_or(Out::at(cursor));
                out.effect = Effect::ScrollHalf(-1);
                out
            }
            Key::Char(c) => self.handle_char(text, cursor, c),
            Key::Enter => self.motion(text, cursor, 'j', 1).map(|(t, _, _)| Out::at(t)).unwrap_or(Out::at(cursor)),
            Key::Ctrl(_) | Key::Esc => Out::at(cursor),
        }
    }

    fn handle_awaiting(&mut self, text: &str, cursor: usize, awaiting: Await, key: Key) -> Out {
        let Key::Char(c) = key else {
            self.reset_pending();
            return Out::at(cursor);
        };
        match awaiting {
            Await::G => {
                self.awaiting = None;
                if c == 'g' {
                    if let Some(op) = self.operator {
                        // dgg: delete from line 1 to the cursor line.
                        let lines = lines_of(text);
                        let li = line_idx(&lines, cursor);
                        let out = self.apply_operator(
                            text,
                            cursor,
                            lines[0].0..lines[li].1,
                            true,
                            op,
                        );
                        self.reset_pending();
                        return out;
                    }
                    self.finish_motion(text, cursor, first_nonblank(text, lines_of(text)[0]), false, true)
                } else if c == 'e' {
                    let n = self.total_count();
                    let mut p = cursor;
                    for _ in 0..n { p = word_step(text, p, false, false, true); }
                    let op = self.operator;
                    self.reset_pending();
                    if let Some(op) = op {
                        let (s, e) = if p >= cursor { (cursor, next_grapheme(text, p)) } else { (p, next_grapheme(text, cursor)) };
                        self.apply_operator(text, cursor, s..e, false, op)
                    } else {
                        Out::at(p)
                    }
                } else {
                    self.reset_pending();
                    Out::at(cursor)
                }
            }
            Await::Z => {
                self.awaiting = None;
                if c == 'z' {
                    let mut out = Out::at(cursor);
                    out.effect = Effect::CenterLine;
                    out
                } else {
                    Out::at(cursor)
                }
            }
            Await::Find(kind) => {
                self.awaiting = None;
                self.last_find = Some((kind, c));
                self.do_find(text, cursor, kind, c, self.total_count())
            }
            Await::Replace => {
                self.awaiting = None;
                let n = self.total_count();
                self.reset_pending();
                if cursor >= text.len() {
                    return Out::at(cursor);
                }
                let mut end = cursor;
                for _ in 0..n { end = next_grapheme(text, end); }
                let old = text[cursor..end].to_string();
                if old.contains('\n') {
                    return Out::at(cursor);
                }
                let replacement: String = std::iter::repeat_n(c, n).collect();
                let mut out = Out::at(cursor);
                out.edits.push(Edit { range: cursor..end, text: replacement });
                self.record_change(vec![Key::Char('r'), Key::Char(c)]);
                out
            }
            Await::TextObject(inner) => {
                self.awaiting = None;
                let Some(range) = self.text_object(text, cursor, inner, c) else {
                    self.reset_pending();
                    return Out::at(cursor);
                };
                if let Some(op) = self.operator.take() {
                    self.reset_pending();
                    self.apply_operator(text, cursor, range, false, op)
                } else if self.mode == Mode::Visual || self.mode == Mode::VisualLine {
                    self.mode = Mode::Visual;
                    let mut out = Out::at(range.start);
                    out.selection = Some(range.clone());
                    out.cursor = prev_grapheme(text, range.end);
                    out
                } else {
                    self.reset_pending();
                    Out::at(cursor)
                }
            }
        }
    }

    fn do_find(&mut self, text: &str, cursor: usize, kind: char, target: char, count: usize) -> Out {
        let lines = lines_of(text);
        let li = line_idx(&lines, cursor);
        let (ls, le) = lines[li];
        let positions: Vec<usize> = text[ls..le]
            .char_indices()
            .filter(|(_, c)| *c == target)
            .map(|(i, _)| ls + i)
            .collect();
        let found: Option<usize> = match kind {
            'f' => positions.iter().filter(|p| **p > cursor).nth(count - 1).copied(),
            'F' => positions.iter().rev().filter(|p| **p < cursor).nth(count - 1).copied(),
            't' => positions.iter().filter(|p| **p > cursor).nth(count - 1).copied(),
            'T' => positions.iter().rev().filter(|p| **p < cursor).nth(count - 1).copied(),
            _ => None,
        };
        let target_pos = match (kind, found) {
            ('t', Some(p)) => Some(prev_grapheme(text, p)),
            ('T', Some(p)) => Some(next_grapheme(text, p)),
            (_, Some(p)) => Some(p),
            _ => None,
        };
        let Some(tp) = target_pos else {
            return Out::at(cursor);
        };
        if let Some(op) = self.operator {
            let (s, e) = if tp >= cursor {
                (cursor, next_grapheme(text, tp))
            } else {
                (tp, next_grapheme(text, cursor))
            };
            let out = self.apply_operator(text, cursor, s..e, false, op);
            self.reset_pending();
            out
        } else if self.mode == Mode::Visual || self.mode == Mode::VisualLine {
            self.finish_motion(text, cursor, tp, true, false)
        } else {
            Out::at(tp)
        }
    }
}

impl Engine {
    /// Called by the UI when an insert session ends (Esc), with the
    /// text that was typed, so `.` can replay the whole change.
    pub fn note_insert_finished(&mut self, typed: &str, entry_keys: Vec<Key>) {
        if !typed.is_empty() || !entry_keys.is_empty() {
            self.last_change = Some(LastChange {
                keys: entry_keys,
                insert: if typed.is_empty() { None } else { Some(typed.to_string()) },
            });
        }
    }

    /// What `.` should replay: the change's keys, then the typed text.
    pub fn repeat_change(&self) -> Option<(Vec<Key>, Option<String>)> {
        self.last_change
            .as_ref()
            .map(|lc| (lc.keys.clone(), lc.insert.clone()))
    }

    fn visual_range(&self, text: &str, cursor: usize) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        if self.mode == Mode::VisualLine {
            let lines = lines_of(text);
            let a = line_idx(&lines, anchor);
            let b = line_idx(&lines, cursor);
            let (lo, hi) = (a.min(b), a.max(b));
            Some(lines[lo].0..lines[hi].1)
        } else if cursor >= anchor {
            Some(anchor..next_grapheme(text, cursor))
        } else {
            Some(cursor..next_grapheme(text, anchor))
        }
    }

    fn handle_char(&mut self, text: &str, cursor: usize, c: char) -> Out {
        let visual = self.mode == Mode::Visual || self.mode == Mode::VisualLine;
        // Digits accumulate a count (a bare 0 is the line-start motion).
        if c.is_ascii_digit() {
            let d = c as usize - '0' as usize;
            if self.operator.is_some() {
                if d == 0 && self.count2 == 0 {
                    // d0 / c0 / y0
                    let op = self.operator.unwrap();
                    let (t, _, _) = self.motion(text, cursor, '0', 1).unwrap();
                    let out = self.apply_operator(text, cursor, t..cursor, false, op);
                    self.reset_pending();
                    return out;
                }
                self.count2 = self.count2 * 10 + d;
                self.seq.push(Key::Char(c));
                return Out::at(cursor);
            }
            if !(c == '0' && self.count == 0) {
                self.count = self.count * 10 + d;
                return Out::at(cursor);
            }
        }
        match c {
            // Operators.
            'd' | 'c' | 'y' if !visual => {
                if self.operator == Some(c) {
                    // dd / cc / yy: linewise over count lines.
                    let n = self.total_count();
                    let lines = lines_of(text);
                    let li = line_idx(&lines, cursor);
                    let end_line = (li + n - 1).min(lines.len() - 1);
                    let out = self.apply_operator(
                        text,
                        cursor,
                        lines[li].0..lines[end_line].1,
                        true,
                        c,
                    );
                    self.seq.push(Key::Char(c));
                    let keys = self.seq.clone();
                    self.reset_pending();
                    if c != 'y' {
                        self.record_change(keys);
                    }
                    return out;
                }
                if self.operator.is_some() {
                    self.reset_pending();
                    return Out::at(cursor);
                }
                self.operator = Some(c);
                self.seq = vec![Key::Char(c)];
                if self.count > 0 {
                    let mut s = vec![];
                    for d in self.count.to_string().chars() {
                        s.push(Key::Char(d));
                    }
                    s.push(Key::Char(c));
                    self.seq = s;
                }
                Out::at(cursor)
            }
            'g' => {
                self.awaiting = Some(Await::G);
                Out::at(cursor)
            }
            'z' => {
                self.awaiting = Some(Await::Z);
                Out::at(cursor)
            }
            'f' | 't' | 'F' | 'T' => {
                self.awaiting = Some(Await::Find(c));
                Out::at(cursor)
            }
            'r' if !visual => {
                self.awaiting = Some(Await::Replace);
                Out::at(cursor)
            }
            'i' | 'a' if self.operator.is_some() || visual => {
                self.awaiting = Some(Await::TextObject(c == 'i'));
                Out::at(cursor)
            }
            ';' => match self.last_find {
                Some((kind, ch)) => self.do_find(text, cursor, kind, ch, self.total_count().max(1)),
                None => Out::at(cursor),
            },
            ',' => match self.last_find {
                Some((kind, ch)) => {
                    let flipped = match kind {
                        'f' => 'F',
                        'F' => 'f',
                        't' => 'T',
                        'T' => 't',
                        _ => kind,
                    };
                    self.do_find(text, cursor, flipped, ch, self.total_count().max(1))
                }
                None => Out::at(cursor),
            },
            'n' => {
                let (q, b) = self.last_search.clone();
                if q.is_empty() { Out::at(cursor) } else { self.do_search(text, cursor, &q, b, false) }
            }
            'N' => {
                let (q, b) = self.last_search.clone();
                if q.is_empty() { Out::at(cursor) } else { self.do_search(text, cursor, &q, !b, false) }
            }
            '*' | '#' => {
                // Search for the word under the cursor.
                let start = word_step(text, cursor, false, false, false);
                let endw = word_step(text, cursor, false, true, true);
                let word = text[start..next_grapheme(text, endw)].to_string();
                if word.is_empty() {
                    return Out::at(cursor);
                }
                self.last_search = (word.clone(), c == '#');
                self.do_search(text, cursor, &word, c == '#', false)
            }
            '.' => {
                // The UI replays the keys (it owns text mutation).
                let mut out = Out::at(cursor);
                out.effect = Effect::RepeatRequest;
                out
            }
            'u' if !visual => {
                let mut out = Out::at(cursor);
                out.effect = Effect::Undo;
                out
            }
            'p' | 'P' if !visual => {
                let mut out = Out::at(cursor);
                out.effect = Effect::PasteRequest(c == 'P');
                out
            }
            // Visual operators act on the selection.
            'y' | 'd' | 'c' | 'x' if visual => {
                let Some(range) = self.visual_range(text, cursor) else {
                    return Out::at(cursor);
                };
                let op = if c == 'x' { 'd' } else { c };
                let linewise = self.mode == Mode::VisualLine;
                self.mode = if op == 'c' { Mode::Insert } else { Mode::Normal };
                self.anchor = None;
                self.apply_operator(text, cursor, range, linewise, op)
            }
            '>' | '<' if visual => {
                let Some(range) = self.visual_range(text, cursor) else {
                    return Out::at(cursor);
                };
                let lines = lines_of(text);
                let a = line_idx(&lines, range.start);
                let b = line_idx(&lines, range.end.saturating_sub(1).max(range.start));
                let mut edits = Vec::new();
                for li in a..=b {
                    let (ls, le) = lines[li];
                    if c == '>' {
                        edits.push(Edit { range: ls..ls, text: "    ".to_string() });
                    } else {
                        let line = &text[ls..le];
                        let strip = line.len() - line.trim_start_matches(' ').len();
                        let n = strip.min(4);
                        if n > 0 {
                            edits.push(Edit { range: ls..ls + n, text: String::new() });
                        }
                    }
                }
                self.mode = Mode::Normal;
                self.anchor = None;
                let mut out = Out::at(range.start);
                out.edits = edits;
                out
            }
            'o' if visual => {
                let anchor = self.anchor.unwrap_or(cursor);
                self.anchor = Some(cursor);
                let mut out = Out::at(anchor);
                out.selection = self.visual_range(text, anchor);
                out
            }
            'v' => {
                if self.mode == Mode::Visual {
                    self.mode = Mode::Normal;
                    self.anchor = None;
                    let mut out = Out::at(clamp_cursor(text, cursor));
                    out.selection = Some(cursor..cursor);
                    out
                } else {
                    self.mode = Mode::Visual;
                    self.anchor = Some(cursor);
                    let mut out = Out::at(cursor);
                    out.selection = Some(cursor..next_grapheme(text, cursor));
                    out
                }
            }
            'V' => {
                if self.mode == Mode::VisualLine {
                    self.mode = Mode::Normal;
                    self.anchor = None;
                    Out::at(clamp_cursor(text, cursor))
                } else {
                    self.mode = Mode::VisualLine;
                    self.anchor = Some(cursor);
                    let mut out = Out::at(cursor);
                    out.selection = self.visual_range(text, cursor);
                    out
                }
            }
            // Editing.
            'x' if !visual => {
                let n = self.total_count();
                self.reset_pending();
                let lines = lines_of(text);
                let li = line_idx(&lines, cursor);
                let (_, le) = lines[li];
                let mut end = cursor;
                for _ in 0..n {
                    let nx = next_grapheme(text, end);
                    if nx > le { break; }
                    end = nx;
                }
                if end == cursor {
                    return Out::at(cursor);
                }
                self.register = (text[cursor..end].to_string(), false);
                let mut out = Out::at(cursor);
                out.edits.push(Edit { range: cursor..end, text: String::new() });
                out.effect = Effect::Yank(self.register.0.clone(), false);
                self.record_change(vec![Key::Char('x')]);
                out
            }
            'X' if !visual => {
                let n = self.total_count();
                self.reset_pending();
                let lines = lines_of(text);
                let li = line_idx(&lines, cursor);
                let (ls, _) = lines[li];
                let mut start = cursor;
                for _ in 0..n {
                    if start <= ls { break; }
                    start = prev_grapheme(text, start);
                }
                if start == cursor {
                    return Out::at(cursor);
                }
                self.register = (text[start..cursor].to_string(), false);
                let mut out = Out::at(start);
                out.edits.push(Edit { range: start..cursor, text: String::new() });
                out.effect = Effect::Yank(self.register.0.clone(), false);
                self.record_change(vec![Key::Char('X')]);
                out
            }
            '~' if !visual => {
                let n = self.total_count();
                self.reset_pending();
                let mut end = cursor;
                for _ in 0..n { end = next_grapheme(text, end); }
                let seg = &text[cursor..end];
                if seg.contains('\n') || seg.is_empty() {
                    return Out::at(cursor);
                }
                let swapped: String = seg
                    .chars()
                    .map(|ch| {
                        if ch.is_lowercase() {
                            ch.to_uppercase().collect::<String>()
                        } else if ch.is_uppercase() {
                            ch.to_lowercase().collect::<String>()
                        } else {
                            ch.to_string()
                        }
                    })
                    .collect();
                let mut out = Out::at(clamp_cursor(text, end));
                if end >= lines_of(text)[line_idx(&lines_of(text), cursor)].1 {
                    out.cursor = clamp_cursor(text, cursor + swapped.len());
                }
                out.edits.push(Edit { range: cursor..end, text: swapped });
                self.record_change(vec![Key::Char('~')]);
                out
            }
            'J' if !visual => {
                self.reset_pending();
                let lines = lines_of(text);
                let li = line_idx(&lines, cursor);
                if li + 1 >= lines.len() {
                    return Out::at(cursor);
                }
                let (_, le) = lines[li];
                let (ns, _) = lines[li + 1];
                let first = first_nonblank(text, lines[li + 1]);
                let mut out = Out::at(le);
                out.edits.push(Edit { range: le..first, text: " ".to_string() });
                let _ = ns;
                self.record_change(vec![Key::Char('J')]);
                out
            }
            'D' if !visual => {
                self.operator = Some('d');
                let (t, _, _) = self.motion(text, cursor, '$', 1).unwrap();
                let out = self.finish_motion(text, cursor, t, true, false);
                self.record_change(vec![Key::Char('D')]);
                out
            }
            'C' if !visual => {
                self.operator = Some('c');
                let (t, _, _) = self.motion(text, cursor, '$', 1).unwrap();
                let out = self.finish_motion(text, cursor, t, true, false);
                self.record_change(vec![Key::Char('C')]);
                out
            }
            'Y' if !visual => {
                self.operator = Some('y');
                let (t, _, _) = self.motion(text, cursor, '$', 1).unwrap();
                let out = self.finish_motion(text, cursor, t, true, false);
                out
            }
            's' if !visual => {
                let n = self.total_count();
                self.reset_pending();
                let mut end = cursor;
                for _ in 0..n { end = next_grapheme(text, end); }
                self.mode = Mode::Insert;
                let mut out = Out::at(cursor);
                if end > cursor {
                    out.edits.push(Edit { range: cursor..end, text: String::new() });
                }
                out
            }
            'S' if !visual => {
                self.reset_pending();
                let lines = lines_of(text);
                let li = line_idx(&lines, cursor);
                let (ls, le) = lines[li];
                self.mode = Mode::Insert;
                let mut out = Out::at(ls);
                out.edits.push(Edit { range: ls..le, text: String::new() });
                out
            }
            // Insert entries.
            'i' if !visual => {
                self.mode = Mode::Insert;
                Out::at(cursor)
            }
            'a' if !visual => {
                self.mode = Mode::Insert;
                Out::at(next_grapheme(text, cursor))
            }
            'I' if !visual => {
                self.mode = Mode::Insert;
                let lines = lines_of(text);
                let li = line_idx(&lines, cursor);
                Out::at(first_nonblank(text, lines[li]))
            }
            'A' if !visual => {
                self.mode = Mode::Insert;
                let lines = lines_of(text);
                let li = line_idx(&lines, cursor);
                Out::at(lines[li].1)
            }
            'o' if !visual => {
                self.mode = Mode::Insert;
                let lines = lines_of(text);
                let li = line_idx(&lines, cursor);
                let (_, le) = lines[li];
                let mut out = Out::at(le + 1);
                out.edits.push(Edit { range: le..le, text: "\n".to_string() });
                out
            }
            'O' if !visual => {
                self.mode = Mode::Insert;
                let lines = lines_of(text);
                let li = line_idx(&lines, cursor);
                let (ls, _) = lines[li];
                let mut out = Out::at(ls);
                out.edits.push(Edit { range: ls..ls, text: "\n".to_string() });
                out.cursor = ls;
                out
            }
            // Motions (also extend visual selections).
            'h' | 'l' | 'j' | 'k' | 'w' | 'W' | 'e' | 'E' | 'b' | 'B' | '0' | '^' | '$' | '%' | '{' | '}' | 'G' => {
                let n = self.total_count();
                // Vim special case: cw/cW behave like ce/cE.
                let motion_key = if self.operator == Some('c') && c == 'w' {
                    'e'
                } else if self.operator == Some('c') && c == 'W' {
                    'E'
                } else {
                    c
                };
                let Some((target, inclusive, linewise)) = self.motion(text, cursor, motion_key, n) else {
                    self.reset_pending();
                    return Out::at(cursor);
                };
                if self.operator.is_some() {
                    let keys = {
                        let mut s = self.seq.clone();
                        s.push(Key::Char(c));
                        s
                    };
                    let op = self.operator.unwrap();
                    let out = self.finish_motion(text, cursor, target, inclusive, linewise);
                    if op != 'y' {
                        self.record_change(keys);
                    }
                    out
                } else {
                    self.finish_motion(text, cursor, target, inclusive, linewise)
                }
            }
            _ => {
                self.reset_pending();
                let mut out = Out::at(cursor);
                out.effect = Effect::Bell;
                out
            }
        }
    }
}

fn keep_col(text: &str, ts: usize, te: usize, col: usize) -> usize {
    let mut p = ts;
    let mut c = 0usize;
    while c < col && p < te {
        p = next_grapheme(text, p);
        c += 1;
    }
    clamp_cursor(text, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_keys(s: &str) -> Vec<Key> {
        let mut out = Vec::new();
        let mut rest = s;
        while !rest.is_empty() {
            if let Some(tail) = rest.strip_prefix("<esc>") {
                out.push(Key::Esc);
                rest = tail;
            } else if let Some(tail) = rest.strip_prefix("<ctrl-r>") {
                out.push(Key::Ctrl('r'));
                rest = tail;
            } else if let Some(tail) = rest.strip_prefix("<ctrl-d>") {
                out.push(Key::Ctrl('d'));
                rest = tail;
            } else if let Some(tail) = rest.strip_prefix("<ctrl-u>") {
                out.push(Key::Ctrl('u'));
                rest = tail;
            } else if let Some(tail) = rest.strip_prefix("<enter>") {
                out.push(Key::Enter);
                rest = tail;
            } else {
                let c = rest.chars().next().unwrap();
                out.push(Key::Char(c));
                rest = &rest[c.len_utf8()..];
            }
        }
        out
    }

    /// Run keys over `marked` (`|` = cursor). Returns (text, cursor).
    /// Insert-mode typed chars are applied directly, like the UI does.
    fn run(marked: &str, keys: &str) -> (String, usize, Mode) {
        let cursor0 = marked.find('|').expect("cursor marker");
        let mut text = marked.replace('|', "");
        // cursor byte offset in the unmarked text
        let mut cursor = marked[..cursor0].replace('|', "").len();
        let mut engine = Engine::new();
        let mut insert_start: Option<usize> = None;
        let mut entry_keys: Vec<Key> = Vec::new();
        for key in parse_keys(keys) {
            if engine.mode == Mode::Insert {
                match key {
                    Key::Esc | Key::Ctrl('[') => {
                        let typed = if let (Some(s), true) = (insert_start, cursor >= insert_start.unwrap_or(0)) {
                            text[s..cursor].to_string()
                        } else {
                            String::new()
                        };
                        let out = engine.handle_key(&text, cursor, key);
                        cursor = out.cursor;
                        engine.note_insert_finished(&typed, entry_keys.clone());
                        insert_start = None;
                    }
                    Key::Char(c) => {
                        text.insert(cursor, c);
                        cursor += c.len_utf8();
                    }
                    Key::Enter => {
                        text.insert(cursor, '\n');
                        cursor += 1;
                    }
                    _ => {}
                }
                continue;
            }
            entry_keys.push(key.clone());
            let out = engine.handle_key(&text, cursor, key.clone());
            match out.effect {
                Effect::RepeatRequest => {
                    if let Some((rkeys, insert)) = engine.repeat_change() {
                        for rk in rkeys {
                            entry_keys.push(rk.clone());
                            let o = engine.handle_key(&text, cursor, rk);
                            apply(&mut text, &o);
                            cursor = o.cursor;
                            if engine.mode == Mode::Insert {
                                break;
                            }
                        }
                        if let Some(ins) = insert {
                            if engine.mode == Mode::Insert {
                                text.insert_str(cursor, &ins);
                                cursor += ins.len();
                                let o = engine.handle_key(&text, cursor, Key::Esc);
                                cursor = o.cursor;
                            }
                        }
                    }
                    continue;
                }
                Effect::PasteRequest(before) => {
                    let clip = engine.register.0.clone();
                    let o = engine.paste_text(&text, &clip, cursor, before);
                    apply(&mut text, &o);
                    cursor = o.cursor;
                    continue;
                }
                _ => {}
            }
            apply(&mut text, &out);
            cursor = out.cursor;
            if engine.mode == Mode::Insert && insert_start.is_none() {
                insert_start = Some(cursor);
            }
            if engine.mode == Mode::Normal && engine.pending_display().is_empty() {
                entry_keys.clear();
            }
        }
        (text, cursor, engine.mode)
    }

    fn apply(text: &mut String, out: &Out) {
        let mut edits: Vec<&Edit> = out.edits.iter().collect();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
        for e in edits {
            text.replace_range(e.range.clone(), &e.text);
        }
    }

    fn check(marked: &str, keys: &str, expected: &str) {
        let (text, cursor, _mode) = run(marked, keys);
        let mut got = text.clone();
        got.insert(cursor, '|');
        assert_eq!(got, expected, "keys={keys} start={marked}");
    }

    #[test]
    fn vim_table() {
        let cases: &[(&str, &str, &str)] = &[
            // motions
            ("|hello world", "l", "h|ello world"),
            ("h|ello world", "h", "|hello world"),
            ("|hello world", "w", "hello |world"),
            ("|hello world", "e", "hell|o world"),
            ("hello |world", "b", "|hello world"),
            ("hello |world", "ge", "hell|o world"),
            ("|hello world", "$", "hello worl|d"),
            ("hello wo|rld", "0", "|hello world"),
            ("  he|llo", "^", "  |hello"),
            ("|one\ntwo\nthree", "j", "one\n|two\nthree"),
            ("|one\ntwo\nthree", "2j", "one\ntwo\n|three"),
            ("one\ntwo\n|three", "k", "one\n|two\nthree"),
            ("|one\ntwo\nthree", "G", "one\ntwo\n|three"),
            ("one\ntwo\n|three", "gg", "|one\ntwo\nthree"),
            ("|one\ntwo\nthree\nfour", "5G", "one\ntwo\nthree\n|four"),
            ("|hello world", "3l", "hel|lo world"),
            ("|aa bb cc dd", "2w", "aa bb |cc dd"),
            ("|aa bb cc", "dw", "|bb cc"),
            ("|aa bb cc", "d2w", "|cc"),
            ("|aa\nbb\ncc", "2dd", "|cc"),
            ("|hello world", "x", "|ello world"),
            ("hell|o world", "X", "hel|o world"),
            ("|hello world", "D", "|"),
            ("hello |world", "D", "hello |"),
            ("|hello world", "rx", "|xello world"),
            ("|Hello", "~", "h|ello"),
            ("|hello", "3~", "HEL|lo"),
            ("|one\ntwo", "J", "one| two"),
            ("|hello", "A!<esc>", "hello|!"),
            ("|hello", "iX<esc>", "|Xhello"),
            ("|hello", "aX<esc>", "h|Xello"),
            ("hello |world", "I!<esc>", "|!hello world"),
            ("|hello", "o<esc>", "hello|\n"),
            ("|hello", "sX<esc>", "|Xello"),
            ("|hello world", "S<esc>", "|"),
            ("|foo bar baz", "ciwqux<esc>", "qu|x bar baz"),
            ("|foo bar baz", "ciwqux<esc>w.", "qux qu|x baz"),
            // counts and operators
            ("|one two three four", "d3w", "|four"),
            ("|one two three", "c$done<esc>", "don|e"),
            ("|one\ntwo\nthree", "y2j", "|one\ntwo\nthree"),
            ("|one\ntwo\nthree", "dG", "|"),
            ("one\ntwo\n|three", "dgg", "|"),
            ("|hello world foo", "yyp", "hello world foo\n|hello world foo"),
            ("|hello world", "y$lP", "hhello worl|dello world"),
            ("|abc def", "xp", "b|ac def"),
            // text objects
            ("|hello world", "diw", "| world"),
            ("|hello world", "daw", "|world"),
            ("say \"|hi there\" now", "di\"", "say \"|\" now"),
            ("say \"|hi there\" now", "da\"", "say | now"),
            ("(foo |bar)", "di(", "(|)"),
            ("(foo |bar)", "da(", "|"),
            ("|one\n\ntwo three", "dap", "|two three"),
            ("|one\n\ntwo three", "dip", "|\n\ntwo three"),
            // find char
            ("|hello world", "fw", "hello |world"),
            ("|hello world", "tw", "hello| world"),
            ("hello |world", "Fh", "|hello world"),
            ("|a.b.c", "f.", "a|.b.c"),
            ("|a.b.c", "f.;", "a.b|.c"),
            ("|a.b.c", "f.;,", "a|.b.c"),
            ("|foo(bar)", "%", "foo(bar|)"),
            ("foo(|bar)", "%", "foo|(bar)"),
            // paragraphs
            ("|one\n\ntwo\n\nthree", "}", "one\n|\ntwo\n\nthree"),
            ("one\n\ntwo\n\n|three", "{", "one\n\ntwo\n|\nthree"),
            // visual
            ("|hello world", "vly", "|hello world"),
            ("|hello world", "vld", "|llo world"),
            ("|one\ntwo\nthree", "Vjd", "|three"),
            ("|hello world", "v> ", "|    hello world"),
            // search
            ("|foo bar foo", "/foo<enter>", "foo bar |foo"),
            ("foo bar |foo", "?foo<enter>", "|foo bar foo"),
            ("|foo bar foo", "/foo<enter>n", "|foo bar foo"),
            ("|Foo fOO foo", "/foo<enter>", "Foo |fOO foo"),
            ("|foo bar", "*#", "|foo bar"),
            // non-ascii: grapheme-safe x and motions
            ("|na\u{ef}ve caf\u{e9}", "x", "|a\u{ef}ve caf\u{e9}"),
            ("\u{6771}\u{4eac} |\u{30c6}\u{30b9}\u{30c8}", "x", "\u{6771}\u{4eac} |\u{30b9}\u{30c8}"),
            ("|a\u{1f44d}\u{1f3fd}b", "x", "|\u{1f44d}\u{1f3fd}b"),
            ("a|\u{1f44d}\u{1f3fd}b", "x", "a|b"),
            ("|caf\u{e9} noir", "w", "caf\u{e9} |noir"),
            // empty and edge cases
            ("|", "x", "|"),
            ("|hello", "dd", "|"),
            ("hello|", "dd", "|"),
            ("|hello\n", "dd", "|"),
            ("|one\ntwo", "ddp", "two\n|one"),
            ("|hello world", "cw<esc>", "| world"),
            ("ab|cd", "i<esc>", "a|bcd"),
        ];
        for (start, keys, expected) in cases {
            check(start, keys, expected);
        }
    }

    #[test]
    #[ignore]
    fn bench_vim_keystrokes() {
        let mut text = String::new();
        for i in 0..5000 {
            text.push_str(&format!("The quick brown fox {} jumps over the lazy dog near the river bank\n", i));
        }
        let mut engine = Engine::new();
        let mut cursor = 0usize;
        let keys = ['w', 'e', 'b', 'j', 'k', 'l', 'h', 'w', '2', 'w', 'g', 'e', '$', '0'];
        let t0 = std::time::Instant::now();
        let n = 20_000;
        for i in 0..n {
            let out = engine.handle_key(&text, cursor, Key::Char(keys[i % keys.len()]));
            cursor = out.cursor.min(text.len());
        }
        let per = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
        println!("vim: {per:.4}ms per keystroke over {} bytes", text.len());
        assert!(per < 0.1, "vim keystroke too slow: {per}ms");
    }

    #[test]
    fn vim_modes_and_pending() {
        let mut e = Engine::new();
        let _ = e.handle_key("hello", 0, Key::Char('d'));
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(e.pending_display(), "d");
        let _ = e.handle_key("hello", 0, Key::Char('2'));
        assert_eq!(e.pending_display(), "d2");
        let _ = e.handle_key("hello", 0, Key::Esc);
        assert_eq!(e.pending_display(), "");
    }
}


