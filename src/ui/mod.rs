//! The single view: editor screen plus tag/search browser screens.

use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::*;

use crate::autotag::Suggestion;
use crate::store::{NoteSummary, Store, TagCount, parse_tags};

actions!(lazykasten, [SaveNote, NewNote, Browse, Back, SelectPrev, SelectNext, SuggestTags, SweepInbox, OpenThemes, OpenGraph, GraphTab, GraphTagsKey, GraphOrphansKey, GraphDepth1, GraphDepth2, GraphDepth3, GraphEnlarge, GraphSearchFocus, DeleteNote, QuickSwitcher, KeyHelp, RenameTag, UndoAutoTag, RelatedNotes]);

/// Fuzzy title match: query chars must appear in order. The score
/// favours word starts and consecutive runs. `None` = no match.
pub fn fuzzy_score(title_lower: &str, query_lower: &str) -> Option<i64> {
    if query_lower.is_empty() {
        return Some(0);
    }
    // Allocation-free scan: one pass over the title's chars, so a
    // keystroke over 20k titles stays within its 3ms budget.
    let mut score: i64 = 0;
    let mut chars = title_lower.chars().enumerate();
    let mut prev_idx: Option<usize> = None;
    let mut prev_char: Option<char> = None;
    for qc in query_lower.chars() {
        let mut hit = None;
        for (idx, c) in chars.by_ref() {
            if c == qc {
                hit = Some(idx);
                break;
            }
            prev_char = Some(c);
        }
        let idx = hit?;
        score += 1;
        if prev_idx.map(|p| p + 1) == Some(idx) {
            score += 5;
        }
        if idx == 0 || prev_char.map(|c| !c.is_alphanumeric()).unwrap_or(false) {
            score += 8;
        }
        prev_idx = Some(idx);
        prev_char = Some(qc);
    }
    Some(score - title_lower.len() as i64 / 16)
}

/// One key table for the help overlay (and the bindings in
/// `bind_keys`): (screen, keys, what it does).
pub const KEY_HELP: &[(&str, &str, &str)] = &[
    ("Everywhere", "Alt+J", "Show / hide (saves the open note)"),
    ("Everywhere", "Ctrl+/", "This key help (also F1)"),
    ("Editor", "Ctrl+S", "Save now"),
    ("Editor", "Ctrl+N", "Save and start a new note"),
    ("Editor", "Ctrl+O", "Browse tags"),
    ("Editor", "Ctrl+P", "Quick switcher: open a note by title"),
    ("Editor", "Ctrl+T", "Suggest tags for this note"),
    ("Editor", "Ctrl+G", "Open the graph around this note"),
    ("Editor", "Ctrl+Shift+P", "Theme picker (also Ctrl+K then T)"),
    ("Editor", "Ctrl+Shift+Backspace", "Move this note to trash"),
    ("Editor", "Esc", "Dismiss suggestions / back from graph-opened note"),
    ("Browser", "Typing", "Search all notes"),
    ("Browser", "Up / Down", "Move the selection"),
    ("Browser", "Enter", "Open the tag / note / restore from #trash"),
    ("Browser", "Ctrl+Shift+T", "Auto-tag every #inbox note (on the tag list)"),
    ("Browser", "Esc", "Back"),
    ("Graph", "Tab", "Local / Global"),
    ("Graph", "T", "Toggle tag nodes"),
    ("Graph", "O", "Toggle orphans"),
    ("Graph", "1 / 2 / 3", "Local depth"),
    ("Graph", "/ or Ctrl+F", "Focus the graph search box"),
    ("Graph", "Ctrl+Enter", "Enlarge / restore the window"),
    ("Graph", "Click", "Open the note (drag moves it instead)"),
    ("Graph", "Esc", "Back (from search: back to the canvas)"),
    ("Themes", "Up / Down", "Preview the theme live"),
    ("Themes", "Enter", "Keep and save the theme"),
    ("Themes", "Esc", "Revert to the previous theme"),
];

/// Register keys. Must run after `gpui_kit::init` so the more specific
/// `"TagSearch > Input"` context wins over the input's own `"Input"` bindings.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-s", SaveNote, Some("LazyKasten")),
        KeyBinding::new("ctrl-n", NewNote, Some("LazyKasten")),
        KeyBinding::new("ctrl-o", Browse, Some("LazyKasten")),
        KeyBinding::new("ctrl-t", SuggestTags, Some("LazyKasten")),
        KeyBinding::new("ctrl-shift-t", SweepInbox, Some("LazyKasten")),
        KeyBinding::new("ctrl-shift-p", OpenThemes, Some("LazyKasten")),
        KeyBinding::new("ctrl-g", OpenGraph, Some("LazyKasten")),
        KeyBinding::new("tab", GraphTab, Some("TagSearch")),
        KeyBinding::new("tab", GraphTab, Some("LazyKasten")),
        KeyBinding::new("tab", GraphTab, Some("GraphView")),
        // Single-letter graph keys live only in the canvas context, so
        // they can never swallow typing in any search box.
        KeyBinding::new("t", GraphTagsKey, Some("GraphView")),
        KeyBinding::new("o", GraphOrphansKey, Some("GraphView")),
        KeyBinding::new("1", GraphDepth1, Some("GraphView")),
        KeyBinding::new("2", GraphDepth2, Some("GraphView")),
        KeyBinding::new("3", GraphDepth3, Some("GraphView")),
        KeyBinding::new("/", GraphSearchFocus, Some("GraphView")),
        KeyBinding::new("ctrl-f", GraphSearchFocus, Some("GraphView")),
        KeyBinding::new("ctrl-enter", GraphEnlarge, Some("TagSearch")),
        KeyBinding::new("ctrl-enter", GraphEnlarge, Some("GraphView")),
        KeyBinding::new("ctrl-k t", OpenThemes, Some("LazyKasten")),
        KeyBinding::new("ctrl-shift-backspace", DeleteNote, Some("LazyKasten")),
        KeyBinding::new("ctrl-p", QuickSwitcher, Some("LazyKasten")),
        KeyBinding::new("f2", RenameTag, Some("TagSearch")),
        KeyBinding::new("ctrl-z", UndoAutoTag, Some("LazyKasten")),
        KeyBinding::new("ctrl-r", RelatedNotes, Some("LazyKasten")),
        KeyBinding::new("ctrl-/", KeyHelp, Some("LazyKasten")),
        KeyBinding::new("f1", KeyHelp, Some("LazyKasten")),
        KeyBinding::new("escape", Back, Some("LazyKasten")),
        KeyBinding::new("up", SelectPrev, Some("TagSearch > Input")),
        KeyBinding::new("down", SelectNext, Some("TagSearch > Input")),
    ]);
}

mod browser;
mod editor;
mod graph_view;
mod switcher;
mod themes;

use graph_view::GraphView;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Screen {
    Editor,
    Tags,
    TagIndex(String),
    Search,
    Backlinks(String),
    Themes,
    Graph,
    Trash,
    Switcher,
    Help,
    Related,
}

/// Schedule a `trace` entry for the next painted frame (no-op cost when
/// tracing is off: the callback is never scheduled).
fn trace_next_frame(window: &mut Window, op: &'static str) {
    if crate::trace::enabled() {
        let t0 = std::time::Instant::now();
        window.on_next_frame(move |_, _| crate::trace::log(op, t0.elapsed()));
    }
}

/// Every themed colour one row/render pass needs, copied out of the theme
/// once per full render instead of cloning the whole Theme per widget.
#[derive(Clone, Copy)]
struct RowColors {
    background: Hsla,
    foreground: Hsla,
    blue: Hsla,
    muted_foreground: Hsla,
    list_hover: Hsla,
    list_active: Hsla,
    border: Hsla,
}

fn row_colors(cx: &App) -> RowColors {
    let t = cx.theme();
    RowColors {
        background: t.background,
        foreground: t.foreground,
        blue: t.blue,
        muted_foreground: t.muted_foreground,
        list_hover: t.list_hover,
        list_active: t.list_active,
        border: t.border,
    }
}





pub struct LazyKasten {
    store: Arc<Store>,
    editor: Entity<TextareaState>,
    search: Entity<InputState>,
    note_id: Option<String>,
    saved_text: SharedString,
    live_tags: Vec<String>,
    screen: Screen,
    tags: Vec<TagCount>,
    notes: Vec<NoteSummary>,
    selected: usize,
    list_scroll: UniformListScrollHandle,
    status: SharedString,
    keep_status: bool,
    suggestions: Vec<Suggestion>,
    link_matches: Vec<NoteSummary>,
    backlink_count: usize,
    note_total: i64,
    search_gen: u64,
    link_gen: u64,
    theme_entries: Vec<crate::themes::ThemeEntry>,
    theme_filtered: Vec<usize>,
    theme_original: String,
    themes_loaded: bool,
    screen_gen: u64,
    opened_mtime: Option<i64>,
    autosave_gen: u64,
    trash_notes: Vec<NoteSummary>,
    trash_count: usize,
    switcher_index: Vec<(String, String, String)>,
    switcher_results: Vec<NoteSummary>,
    renaming: Option<String>,
    autotag_undo: Vec<(String, Vec<String>)>,
    graph: Option<GraphView>,
    graph_focus: FocusHandle,
    return_to_graph: bool,
    graph_loading: bool,
    graph_seq: u64,
    g_local: bool,
    g_depth: usize,
    g_tags: bool,
    g_orphans: bool,
    g_center: Option<String>,
    _subscriptions: Vec<Subscription>,
}

impl LazyKasten {
    pub fn new(store: Arc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(true)
                .placeholder("Capture a thought\u{2026}  (Tag: a, b  or  #hashtags)")
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search all notes\u{2026}"));

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe_in(&editor, window, |this, _editor, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::Change) {
                this.on_editor_change(window, cx);
            }
        }));
        subscriptions.push(cx.subscribe_in(&search, window, |this, _search, ev: &InputEvent, window, cx| {
            match ev {
                InputEvent::Change => this.on_search_change(window, cx),
                InputEvent::PressEnter { .. } => this.open_selected(window, cx),
                _ => {}
            }
        }));

        // Quick-switcher title index: loaded in the background at
        // startup, refreshed after saves.
        {
            let index_store = store.clone();
            let task = cx.background_spawn(async move { index_store.notes_titles().unwrap_or_default() });
            cx.spawn(async move |this, cx| {
                let rows = task.await;
                let _ = this.update(cx, |this, _cx| {
                    this.switcher_index = rows
                        .into_iter()
                        .map(|(id, title)| {
                            let lower = title.to_lowercase();
                            (title, lower, id)
                        })
                        .collect();
                });
            })
            .detach();
        }

        Self {
            store,
            editor,
            search,
            note_id: None,
            saved_text: SharedString::default(),
            live_tags: Vec::new(),
            screen: Screen::Editor,
            tags: Vec::new(),
            notes: Vec::new(),
            selected: 0,
            list_scroll: UniformListScrollHandle::new(),
            status: SharedString::default(),
            keep_status: false,
            suggestions: Vec::new(),
            link_matches: Vec::new(),
            backlink_count: 0,
            note_total: 0,
            search_gen: 0,
            link_gen: 0,
            theme_entries: Vec::new(),
            theme_filtered: Vec::new(),
            theme_original: String::new(),
            themes_loaded: false,
            screen_gen: 0,
            opened_mtime: None,
            autosave_gen: 0,
            trash_notes: Vec::new(),
            trash_count: 0,
            switcher_index: Vec::new(),
            switcher_results: Vec::new(),
            renaming: None,
            autotag_undo: Vec::new(),
            graph: None,
            graph_focus: cx.focus_handle(),
            return_to_graph: false,
            graph_loading: false,
            graph_seq: 0,
            g_local: false,
            g_depth: 2,
            g_tags: true,
            g_orphans: false,
            g_center: None,
            _subscriptions: subscriptions,
        }
    }


    fn on_search_change(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Help || self.renaming.is_some() {
            return;
        }
        let query = self.search.read(cx).value().to_string();
        if self.screen == Screen::Graph {
            self.search_gen += 1;
            let seq = self.search_gen;
            if query.trim().is_empty() {
                if let Some(g) = &self.graph {
                    g.vp.borrow_mut().query = None;
                }
                cx.notify();
                return;
            }
            let store = self.store.clone();
            let q = query.clone();
            let task = cx.background_spawn(async move {
                store.search(&q, 200).unwrap_or_default().into_iter().map(|n| n.id).collect::<std::collections::HashSet<_>>()
            });
            cx.spawn(async move |this, cx| {
                let ids = task.await;
                let _ = this.update(cx, |this, cx| {
                    if this.search_gen == seq {
                        if let Some(g) = &this.graph {
                            g.vp.borrow_mut().query = Some(ids);
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
            return;
        }
        if self.screen == Screen::Themes {
            let q = query.trim().to_lowercase();
            self.theme_filtered = self
                .theme_entries
                .iter()
                .enumerate()
                .filter(|(_, e)| q.is_empty() || e.name.to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect();
            self.selected = 0;
            self.preview_theme(cx);
            cx.notify();
            return;
        }
        if self.screen == Screen::Switcher {
            // In-memory fuzzy filter over the title index (no disk, no
            // SQL): scores computed locally per keystroke.
            let q = query.trim().to_lowercase();
            if q.is_empty() {
                self.switcher_results = self
                    .switcher_index
                    .iter()
                    .take(200)
                    .map(|(title, _, id)| NoteSummary { id: id.clone(), title: title.clone() })
                    .collect();
            } else {
                let mut scored: Vec<(i64, &String, &String)> = self
                    .switcher_index
                    .iter()
                    .filter_map(|(title, lower, id)| fuzzy_score(lower, &q).map(|s| (s, title, id)))
                    .collect();
                scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
                scored.truncate(200);
                self.switcher_results = scored
                    .into_iter()
                    .map(|(_, title, id)| NoteSummary { id: id.clone(), title: title.clone() })
                    .collect();
            }
            self.selected = 0;
            cx.notify();
            return;
        }
        self.search_gen += 1;
        let seq = self.search_gen;
        if query.trim().is_empty() {
            self.screen = Screen::Tags;
            self.selected = 0;
            cx.notify();
            return;
        }
        // Search in the background; the previous results stay on screen
        // until the new ones arrive, and stale generations are dropped.
        self.screen = Screen::Search;
        cx.notify();
        let store = self.store.clone();
        let task = cx.background_spawn(async move { store.search(&query, 200).unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let notes = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.search_gen == seq {
                    this.notes = notes;
                    this.selected = 0;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Called when the window is shown: fresh editor, cursor ready.
    pub fn on_show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Editor;
        // A status set by background auto-tagging survives one show so the
        // user sees what happened to the note they just hid.
        if self.keep_status {
            self.keep_status = false;
        } else {
            self.status = SharedString::default();
        }
        self.editor.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }




    fn on_open_graph(&mut self, _: &OpenGraph, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Graph {
            self.open_graph(window, cx);
        }
    }

    fn on_graph_tab(&mut self, _: &GraphTab, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Graph {
            self.g_local = !self.g_local;
            if self.g_local && self.g_center.is_none() {
                self.g_center = self.note_id.clone().or_else(|| {
                    self.graph.as_ref().and_then(|g| g.data.nodes.iter().find(|n| !n.is_tag).map(|n| n.id.clone()))
                });
            }
            self.load_graph(window, cx);
        }
    }

    fn on_graph_tags_key(&mut self, _: &GraphTagsKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Graph {
            self.g_tags = !self.g_tags;
            self.load_graph(window, cx);
        }
    }

    fn on_graph_orphans_key(&mut self, _: &GraphOrphansKey, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Graph {
            self.g_orphans = !self.g_orphans;
            self.load_graph(window, cx);
        }
    }

    fn on_graph_depth(&mut self, depth: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Graph {
            self.g_depth = depth;
            self.load_graph(window, cx);
        }
    }

    fn on_graph_depth1(&mut self, _: &GraphDepth1, window: &mut Window, cx: &mut Context<Self>) {
        self.on_graph_depth(1, window, cx);
    }

    fn on_graph_depth2(&mut self, _: &GraphDepth2, window: &mut Window, cx: &mut Context<Self>) {
        self.on_graph_depth(2, window, cx);
    }

    fn on_graph_depth3(&mut self, _: &GraphDepth3, window: &mut Window, cx: &mut Context<Self>) {
        self.on_graph_depth(3, window, cx);
    }

    fn on_graph_enlarge(&mut self, _: &GraphEnlarge, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Graph {
            self.toggle_graph_enlarge(window);
            cx.notify();
        }
    }

    fn on_graph_search_focus(&mut self, _: &GraphSearchFocus, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Graph {
            self.search.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    fn on_open_themes(&mut self, _: &OpenThemes, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Themes {
            self.open_themes(window, cx);
        }
    }

















    // -- graph --------------------------------------------------------------






















    // -- actions -----------------------------------------------------------

    fn on_save_note(&mut self, _: &SaveNote, window: &mut Window, cx: &mut Context<Self>) {
        self.save(window, cx);
    }

    fn on_new_note(&mut self, _: &NewNote, window: &mut Window, cx: &mut Context<Self>) {
        self.commit(window, cx);
        self.screen = Screen::Editor;
        self.editor.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn on_browse(&mut self, _: &Browse, window: &mut Window, cx: &mut Context<Self>) {
        trace_next_frame(window, "browse_to_frame");
        if self.screen == Screen::Graph {
            self.leave_graph(window, cx);
            return;
        }
        match self.screen {
            Screen::Editor => {
                self.commit(window, cx);
                self.show_tags(window, cx);
            }
            _ => {
                self.screen = Screen::Editor;
                self.editor.update(cx, |s, cx| s.focus(window, cx));
                cx.notify();
            }
        }
    }

    fn on_back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.take().is_some() {
            self.show_tags(window, cx);
            return;
        }
        if self.screen == Screen::Related {
            self.screen = Screen::Editor;
            self.editor.update(cx, |s, cx| s.focus(window, cx));
            cx.notify();
            return;
        }
        // Esc first dismisses Ctrl+T suggestions / `[[` matches in the editor.
        if self.screen == Screen::Editor && (!self.suggestions.is_empty() || !self.link_matches.is_empty()) {
            self.suggestions.clear();
            self.link_matches.clear();
            cx.notify();
            return;
        }
        if self.screen == Screen::Editor && self.return_to_graph {
            self.return_to_graph = false;
            self.screen = Screen::Graph;
            self.start_sim(cx);
            self.graph_focus.focus(window, cx);
            cx.notify();
            return;
        }
        if self.screen == Screen::Graph {
            // Esc in the graph search box returns focus to the canvas.
            if self.search.read(cx).focus_handle(cx).is_focused(window) {
                self.graph_focus.focus(window, cx);
                cx.notify();
                return;
            }
            self.leave_graph(window, cx);
            return;
        }
        match self.screen {
            Screen::Themes => {
                let original = self.theme_original.clone();
                crate::themes::apply_registered(cx, &original);
                self.screen = Screen::Editor;
                self.editor.update(cx, |s, cx| s.focus(window, cx));
                cx.notify();
            }
            Screen::TagIndex(_) | Screen::Search | Screen::Backlinks(_) => self.show_tags(window, cx),
            Screen::Trash => self.show_tags(window, cx),
            Screen::Switcher | Screen::Help => {
                self.screen = Screen::Editor;
                self.editor.update(cx, |s, cx| s.focus(window, cx));
                cx.notify();
            }
            Screen::Tags => {
                self.screen = Screen::Editor;
                self.editor.update(cx, |s, cx| s.focus(window, cx));
                cx.notify();
            }
            Screen::Editor | Screen::Graph | Screen::Related => {}
        }
    }

    fn on_select_prev(&mut self, _: &SelectPrev, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Themes {
            trace_next_frame(window, "theme_preview_to_frame");
        }
        self.move_selection(-1, cx);
    }

    fn on_select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Themes {
            trace_next_frame(window, "theme_preview_to_frame");
        }
        self.move_selection(1, cx);
    }

    // -- rendering ---------------------------------------------------------





}

/// The text after the last unclosed `[[`, if any.
fn open_link_query(text: &str) -> Option<String> {
    // Scan at most the last 200 characters on each keystroke.
    let from = text.char_indices().rev().nth(199).map(|(i, _)| i).unwrap_or(0);
    let seg = &text[from..];
    let start = seg.rfind("[[")? + from;
    let tail = &text[start + 2..];
    if tail.contains("]]") || tail.contains('\n') {
        None
    } else {
        Some(tail.to_string())
    }
}

/// Replace the last unclosed `[[query` with `[[id|title]]`.
fn complete_link(text: &str, id: &str, title: &str) -> String {
    match text.rfind("[[") {
        Some(start) => format!("{}[[{}|{}]]", &text[..start], id, title),
        None => text.to_string(),
    }
}

/// `20261028143205` (optionally suffixed) -> `2026-10-28 14:32`.
fn date_of_id(id: &str) -> String {
    let b = id.as_bytes();
    if b.len() >= 12 && b[..12].iter().all(|c| c.is_ascii_digit()) {
        format!(
            "{}-{}-{} {}:{}",
            &id[0..4],
            &id[4..6],
            &id[6..8],
            &id[8..10],
            &id[10..12]
        )
    } else {
        id.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{complete_link, open_link_query};

    #[test]
    #[test]
    fn test_fuzzy_score() {
        use super::fuzzy_score;
        assert!(fuzzy_score("rust note", "rn").is_some());
        assert!(fuzzy_score("rust note", "xyz").is_none());
        assert!(fuzzy_score("rust note", "nr").is_none(), "order matters");
        // Word-start matches beat mid-word ones.
        let direct = fuzzy_score("note list", "nl").unwrap();
        let scattered = fuzzy_score("panel", "nl").unwrap();
        assert!(direct > scattered, "{direct} vs {scattered}");
        let run = fuzzy_score("notebook", "note").unwrap();
        let sparse = fuzzy_score("xenotype", "note").unwrap();
        assert!(run > sparse, "{run} vs {sparse}");
    }

    #[test]
    fn test_link_completion() {
        assert_eq!(open_link_query("hello [[Rus"), Some("Rus".to_string()));
        assert_eq!(open_link_query("hello [[a]] done"), None);
        assert_eq!(open_link_query("hello"), None);
        assert_eq!(
            complete_link("hello [[Rus", "20260101", "Rust note"),
            "hello [[20260101|Rust note]]"
        );
        assert_eq!(complete_link("hello", "20260101", "T"), "hello");
    }

    /// Guard: no mojibake (UTF-8 read as Windows-1252 and saved back) may
    /// ship in source or theme files again. Patterns are built from escapes
    /// so this file itself stays pure ASCII around them.
    #[test]
    fn test_no_mojibake_in_source_or_themes() {
        let patterns: Vec<String> = vec![
            "\u{00E2}\u{20AC}".to_string(),
            "\u{00E2}\u{2020}".to_string(),
            "\u{00C2}\u{00B7}".to_string(),
            "\u{00C3}".to_string(),
            "\u{FFFD}".to_string(),
        ];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut checked = 0usize;
        for dir in ["src", "themes"] {
            let mut stack = vec![root.join(dir)];
            while let Some(path) = stack.pop() {
                for entry in std::fs::read_dir(&path).unwrap() {
                    let entry = entry.unwrap();
                    let p = entry.path();
                    if p.is_dir() {
                        stack.push(p);
                    } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["rs", "json", "md"].contains(&e)) {
                        let text = std::fs::read_to_string(&p)
                            .unwrap_or_else(|e| panic!("{}: {e}", p.display()));
                        for pat in &patterns {
                            assert!(!text.contains(pat.as_str()), "{} contains mojibake {pat:?}", p.display());
                        }
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked >= 25, "guard only checked {checked} files");
    }
}

impl Render for LazyKasten {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = row_colors(cx);
        let root = div()
            .key_context("LazyKasten")
            .on_action(cx.listener(Self::on_save_note))
            .on_action(cx.listener(Self::on_new_note))
            .on_action(cx.listener(Self::on_browse))
            .on_action(cx.listener(Self::on_back))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_select_next))
            .on_action(cx.listener(Self::on_suggest_tags))
            .on_action(cx.listener(Self::on_sweep_inbox))
            .on_action(cx.listener(Self::on_open_themes))
            .on_action(cx.listener(Self::on_open_graph))
            .on_action(cx.listener(Self::on_graph_tab))
            .on_action(cx.listener(Self::on_graph_tags_key))
            .on_action(cx.listener(Self::on_graph_orphans_key))
            .on_action(cx.listener(Self::on_graph_depth1))
            .on_action(cx.listener(Self::on_graph_depth2))
            .on_action(cx.listener(Self::on_graph_depth3))
            .on_action(cx.listener(Self::on_graph_enlarge))
            .on_action(cx.listener(Self::on_graph_search_focus))
            .on_action(cx.listener(Self::on_delete_note))
            .on_action(cx.listener(Self::on_quick_switcher))
            .on_action(cx.listener(Self::on_key_help))
            .on_action(cx.listener(Self::on_rename_tag))
            .on_action(cx.listener(Self::on_undo_auto_tag))
            .on_action(cx.listener(Self::on_related_notes))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground);

        match self.screen {
            Screen::Editor => root.child(self.render_editor(window, cx)),
            Screen::Graph => root.child(self.render_graph(window, cx)),
            _ => root.child(self.render_browser(window, cx)),
        }
    }
}
