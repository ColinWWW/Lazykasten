//! The single view: editor screen plus tag/search browser screens.

use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::*;

use crate::autotag::Suggestion;
use crate::store::{NoteSummary, Store, TagCount, parse_tags};

actions!(lazykasten, [SaveNote, NewNote, Browse, Back, SelectPrev, SelectNext, SuggestTags, SweepInbox, OpenThemes, OpenGraph, GraphTab, GraphTagsKey, GraphOrphansKey, GraphDepth1, GraphDepth2, GraphDepth3, GraphEnlarge, GraphSearchFocus, DeleteNote, QuickSwitcher, KeyHelp]);

/// Fuzzy title match: query chars must appear in order. The score
/// favours word starts and consecutive runs. `None` = no match.
pub fn fuzzy_score(title_lower: &str, query_lower: &str) -> Option<i64> {
    if query_lower.is_empty() {
        return Some(0);
    }
    let title: Vec<char> = title_lower.chars().collect();
    let mut score: i64 = 0;
    let mut ti = 0usize;
    let mut prev_idx: Option<usize> = None;
    for qc in query_lower.chars() {
        let mut found = None;
        while ti < title.len() {
            if title[ti] == qc {
                found = Some(ti);
                break;
            }
            ti += 1;
        }
        let idx = found?;
        score += 1;
        if prev_idx == Some(idx.wrapping_sub(1)) && idx > 0 {
            score += 5;
        }
        if idx == 0 || !title[idx - 1].is_alphanumeric() {
            score += 8;
        }
        prev_idx = Some(idx);
        ti = idx + 1;
    }
    Some(score - title.len() as i64 / 16)
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
        KeyBinding::new("ctrl-/", KeyHelp, Some("LazyKasten")),
        KeyBinding::new("f1", KeyHelp, Some("LazyKasten")),
        KeyBinding::new("escape", Back, Some("LazyKasten")),
        KeyBinding::new("up", SelectPrev, Some("TagSearch > Input")),
        KeyBinding::new("down", SelectNext, Some("TagSearch > Input")),
    ]);
}

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

/// Camera + interaction state shared with the canvas paint closures.
#[derive(Default)]
struct GraphViewport {
    bounds: Bounds<Pixels>,
    cam: [f32; 2],
    zoom: f32,
    centered: bool,
    center_on: Option<usize>,
    hover: Option<usize>,
    query: Option<std::collections::HashSet<String>>,
    bounds_origin: [f32; 2],
}

#[derive(Default)]
struct GraphPaint {
    nodes: Vec<(usize, [f32; 2], bool, bool)>,
    edges: Vec<([f32; 2], [f32; 2])>,
    labels: Vec<(usize, [f32; 2])>,
}

/// Node colour: the theme blue hue-rotated by the first tag, so colour
/// comes from the theme and grouping stays visible across theme changes.
fn graph_node_color(colors: &RowColors, node: &crate::graph::GNode) -> Hsla {
    if node.is_tag {
        return colors.muted_foreground;
    }
    let mut h: u64 = 0xcbf29ce484222325;
    for b in node.first_tag.as_deref().unwrap_or("").as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let base = colors.blue;
    Hsla {
        h: (base.h + (h % 6) as f32 * 0.09).fract(),
        s: base.s,
        l: base.l,
        a: base.a,
    }
}

struct GraphView {
    data: std::sync::Arc<crate::graph::GraphData>,
    pos: std::sync::Arc<std::sync::Mutex<std::sync::Arc<Vec<[f32; 2]>>>>,
    pinned: std::sync::Arc<std::sync::Mutex<std::collections::HashSet<usize>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    settled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    vp: std::rc::Rc<std::cell::RefCell<GraphViewport>>,
    labels: std::rc::Rc<std::cell::RefCell<(String, std::collections::HashMap<usize, ShapedLine>)>>,
    grid: std::collections::HashMap<(i32, i32), Vec<u32>>,
    grid_stamp: (usize, i32, i32, u32),
    snap: usize,
    drag_node: Option<usize>,
    drag_moved: bool,
    down_pos: [f32; 2],
    pan: Option<([f32; 2], [f32; 2])>,
    enlarged: bool,
    prev_size: Option<(f32, f32)>,
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

    fn on_editor_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        trace_next_frame(window, "keystroke_to_frame");
        // Autosave: 1.5s after the last keystroke, through the normal
        // save path (which skips unchanged text). Debounced by gen.
        self.autosave_gen += 1;
        let seq = self.autosave_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1500))
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.autosave_gen == seq && this.screen == Screen::Editor {
                    this.save(window, cx);
                }
            });
        })
        .detach();
        let text = self.editor.read(cx).value();
        // Fast path: with no '#' or ':' anywhere and no tags yet, the note
        // cannot gain tags; skip the full parse on every keystroke.
        let tags = if self.live_tags.is_empty() && !text.contains('#') && !text.contains(':') {
            Vec::new()
        } else {
            parse_tags(&text)
        };
        if tags != self.live_tags {
            self.live_tags = tags;
            cx.notify();
        }
        // `[[` completion runs in the background (never on the typing path);
        // a generation number drops stale results. Matches show in the
        // footer; clicking one inserts the link.
        self.link_gen += 1;
        let seq = self.link_gen;
        match open_link_query(&text) {
            Some(q) => {
                let store = self.store.clone();
                let query = q.trim().to_string();
                let task = cx.background_spawn(async move {
                    if query.is_empty() {
                        store.recent_notes(8).unwrap_or_default()
                    } else {
                        store.search(&query, 8).unwrap_or_default()
                    }
                });
                cx.spawn(async move |this, cx| {
                    let matches = task.await;
                    let _ = this.update(cx, |this, cx| {
                        if this.link_gen == seq && matches != this.link_matches {
                            this.link_matches = matches;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            None => {
                if !self.link_matches.is_empty() {
                    self.link_matches.clear();
                    cx.notify();
                }
            }
        }
    }

    fn on_search_change(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Help {
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

    /// Save the current note and reset to a blank editor. The save itself
    /// stays instant; auto-tagging of an untagged note runs afterwards in
    /// the background and never delays the hide.
    pub fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text_before = self.editor.read(cx).value().to_string();
        let untagged = parse_tags(&text_before).is_empty() && !text_before.trim().is_empty();
        self.save(window, cx);
        let saved_id = self.note_id.clone();
        self.note_id = None;
        self.opened_mtime = None;
        self.saved_text = SharedString::default();
        self.live_tags.clear();
        self.suggestions.clear();
        self.link_matches.clear();
        self.backlink_count = 0;
        self.editor
            .update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
        if untagged {
            if let Some(id) = saved_id {
                self.spawn_auto_tag(id, text_before, cx);
            }
        }
    }

    /// Background auto-tag for one just-saved inbox note.
    fn spawn_auto_tag(&mut self, id: String, original_text: String, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let bg_id = id.clone();
        let bg_text = original_text.clone();
        let task = cx.background_spawn(async move {
            match crate::autotag::suggest_tags(&store, &bg_id, &bg_text) {
                Ok(suggestions) => {
                    let tags = crate::autotag::confident_tags(&suggestions);
                    if tags.is_empty() {
                        None
                    } else {
                        match crate::autotag::apply_tags_if_unchanged(&store, &bg_id, &bg_text, &tags) {
                            Ok(true) => Some(format!(
                                "Auto-tagged: {}",
                                tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")
                            )),
                            Ok(false) => None,
                            Err(e) => Some(format!("Auto-tagging failed: {e}")),
                        }
                    }
                }
                Err(e) => Some(format!("Auto-tagging failed: {e}")),
            }
        });
        cx.spawn(async move |this, cx| {
            if let Some(status) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.status = status.into();
                    this.keep_status = true;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn on_suggest_tags(&mut self, _: &SuggestTags, _window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Editor {
            return;
        }
        let text = self.editor.read(cx).value().to_string();
        let exclude = self.note_id.clone().unwrap_or_default();
        let store = self.store.clone();
        let task = cx.background_spawn(async move {
            crate::autotag::suggest_tags(&store, &exclude, &text)
                .unwrap_or_default()
                .into_iter()
                .filter(|s| s.confidence >= crate::autotag::SUGGEST_THRESHOLD)
                .take(5)
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let suggestions = task.await;
            let _ = this.update(cx, |this, cx| {
                this.suggestions = suggestions;
                cx.notify();
            });
        })
        .detach();
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

    fn on_sweep_inbox(&mut self, _: &SweepInbox, _window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Tags {
            return;
        }
        let store = self.store.clone();
        let task = cx.background_spawn(async move {
            let inbox = store.notes_with_tag("inbox").unwrap_or_default();
            let total = inbox.len();
            let mut tagged = 0usize;
            for note in &inbox {
                let Ok(text) = store.read(&note.id) else { continue };
                let Ok(suggestions) = crate::autotag::suggest_tags(&store, &note.id, &text) else { continue };
                let tags = crate::autotag::confident_tags(&suggestions);
                if crate::autotag::apply_tags_if_unchanged(&store, &note.id, &text, &tags).unwrap_or(false) {
                    tagged += 1;
                }
            }
            let tags = store.tags().unwrap_or_default();
            (tagged, total, tags)
        });
        cx.spawn(async move |this, cx| {
            let (tagged, total, tags) = task.await;
            let _ = this.update(cx, |this, cx| {
                this.status = format!("Tagged {tagged} of {total} inbox notes").into();
                this.keep_status = true;
                this.tags = tags;
                cx.notify();
            });
        })
        .detach();
    }

    pub fn save(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value();
        // Never rewrite an unchanged note (would bump its mtime for nothing).
        if text == self.saved_text {
            return;
        }
        let id = match &self.note_id {
            Some(id) => id.clone(),
            None => {
                let id = self.store.new_id();
                self.note_id = Some(id.clone());
                id
            }
        };
        // Never overwrite an outside edit: if the file changed on disk
        // since this note was opened, write a conflict copy instead.
        if !text.trim().is_empty() && self.opened_mtime.is_some() {
            let expected = self.opened_mtime;
            if self.store.note_mtime(&id) != expected {
                match self.store.write_conflict_copy(&id, &text) {
                    Ok(conflict) => {
                        self.status = format!(
                            "Changed outside the app \u{2014} your text saved as {conflict}.md"
                        )
                        .into();
                        self.keep_status = true;
                        self.saved_text = text;
                        self.opened_mtime = self.store.note_mtime(&id);
                    }
                    Err(e) => {
                        self.status = format!("Error: {e}").into();
                    }
                }
                cx.notify();
                return;
            }
        }
        match self.store.save(&id, &text) {
            Ok(()) => {
                self.status = if text.trim().is_empty() {
                    "Moved empty note to trash".into()
                } else {
                    format!("Saved {id}.md").into()
                };
                self.saved_text = text;
                self.opened_mtime = self.store.note_mtime(&id);
                self.refresh_switcher_index(cx);
            }
            Err(e) => {
                self.status = format!("Error: {e}").into();
            }
        }
        cx.notify();
    }

    fn refresh_switcher_index(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let task = cx.background_spawn(async move { store.notes_titles().unwrap_or_default() });
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

    fn on_quick_switcher(&mut self, _: &QuickSwitcher, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Editor {
            self.save(window, cx);
        }
        self.screen = Screen::Switcher;
        self.selected = 0;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.search.update(cx, |s, cx| s.focus(window, cx));
        // Empty query: recent notes first (the index is id-desc).
        self.switcher_results = self
            .switcher_index
            .iter()
            .take(200)
            .map(|(title, _, id)| NoteSummary { id: id.clone(), title: title.clone() })
            .collect();
        cx.notify();
    }

    fn on_key_help(&mut self, _: &KeyHelp, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Help;
        self.selected = 0;
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn on_delete_note(&mut self, _: &DeleteNote, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Editor {
            return;
        }
        if let Some(id) = self.note_id.clone() {
            match self.store.trash_note(&id) {
                Ok(_) => {
                    self.status = format!("Moved {id}.md to trash").into();
                    self.keep_status = true;
                }
                Err(e) => {
                    self.status = format!("Error: {e}").into();
                    cx.notify();
                    return;
                }
            }
        }
        self.note_id = None;
        self.opened_mtime = None;
        self.saved_text = SharedString::default();
        self.live_tags.clear();
        self.suggestions.clear();
        self.editor.update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
    }

    pub fn open_note(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.store.read(id) {
            Ok(text) => {
                self.live_tags = parse_tags(&text);
                // Backlink count arrives in the background (one indexed
                // COUNT query); the footer updates when it lands.
                self.backlink_count = 0;
                let store = self.store.clone();
                let bg_id = id.to_string();
                let check_id = bg_id.clone();
                let task = cx.background_spawn(async move {
                    store.backlink_count(&bg_id).unwrap_or(0)
                });
                cx.spawn(async move |this, cx| {
                    let count = task.await;
                    let _ = this.update(cx, |this, cx| {
                        if this.note_id.as_deref() == Some(check_id.as_str()) {
                            this.backlink_count = count.max(0) as usize;
                            cx.notify();
                        }
                    });
                })
                .detach();
                self.link_matches.clear();
                self.suggestions.clear();
                self.saved_text = text.clone().into();
                self.opened_mtime = self.store.note_mtime(id);
                self.note_id = Some(id.to_string());
                self.editor
                    .update(cx, |s, cx| s.set_value(text, window, cx));
                self.screen = Screen::Editor;
                self.editor.update(cx, |s, cx| s.focus(window, cx));
            }
            Err(e) => {
                self.status = format!("Error: {e}").into();
            }
        }
        cx.notify();
    }

    /// Open the theme picker. All bundled/user themes are parsed only on
    /// first open; moving the selection previews live, Enter keeps and
    /// saves, Esc reverts to `theme_original`.
    pub fn open_themes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.themes_loaded {
            let (entries, invalid) = crate::themes::load_picker_themes(cx, &self.store.data_dir());
            self.theme_entries = entries;
            self.themes_loaded = true;
            if !invalid.is_empty() {
                self.status = format!("Skipped invalid theme file(s): {}", invalid.join(", ")).into();
                self.keep_status = true;
            }
        }
        self.theme_original = cx.theme().theme_name().to_string();
        self.theme_filtered = (0..self.theme_entries.len()).collect();
        self.selected = self
            .theme_entries
            .iter()
            .position(|e| e.name == self.theme_original)
            .unwrap_or(0);
        self.screen = Screen::Themes;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn preview_theme(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.theme_filtered.get(self.selected).and_then(|i| self.theme_entries.get(*i)) {
            let name = entry.name.clone();
            crate::themes::apply_registered(cx, &name);
        }
    }

    fn keep_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(entry) = self.theme_filtered.get(self.selected).and_then(|i| self.theme_entries.get(*i)) {
            let name = entry.name.clone();
            crate::themes::apply_registered(cx, &name);
            if name != self.theme_original {
                let dir = self.store.data_dir();
                let name_for_write = name.clone();
                cx.background_spawn(async move {
                    crate::themes::write_saved_theme(&dir, &name_for_write);
                })
                .detach();
            }
            self.theme_original = name;
        }
        self.screen = Screen::Editor;
        self.editor.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    // -- graph --------------------------------------------------------------

    fn stop_graph(&mut self) {
        if let Some(g) = &self.graph {
            g.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        self.graph_seq += 1;
    }

    pub fn open_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Editor {
            self.save(window, cx);
        }
        self.g_center = self.note_id.clone();
        self.g_local = self.g_center.is_some();
        self.load_graph(window, cx);
    }

    fn load_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_graph();
        let seq = self.graph_seq;
        self.graph = None;
        self.graph_loading = true;
        self.screen = Screen::Graph;
        self.graph_focus.focus(window, cx);
        cx.notify();
        let store = self.store.clone();
        let mode = if self.g_local { crate::graph::GraphMode::Local } else { crate::graph::GraphMode::Global };
        let center = self.g_center.clone();
        let (depth, tags_on, orphans) = (self.g_depth, self.g_tags, self.g_orphans);
        let dir = self.store.data_dir();
        let task = cx.background_spawn(async move {
            let data = crate::graph::build(&store, mode, center.as_deref(), depth, tags_on, orphans).ok()?;
            let cache = if mode == crate::graph::GraphMode::Global {
                crate::graph::load_cache(&dir)
            } else {
                std::collections::HashMap::new()
            };
            let layout = crate::graph::Layout::new(&data, &cache);
            Some((data, layout.pos))
        });
        cx.spawn(async move |this, cx| {
            let loaded = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.graph_seq != seq {
                    return;
                }
                this.graph_loading = false;
                if let Some((data, pos)) = loaded {
                    let center_idx = this
                        .g_center
                        .as_ref()
                        .and_then(|id| data.by_id.get(id).copied());
                    let mut vp = GraphViewport { zoom: 1.0, ..Default::default() };
                    vp.center_on = center_idx.or(if data.nodes.is_empty() { None } else { Some(0) });
                    this.graph = Some(GraphView {
                        data: std::sync::Arc::new(data),
                        pos: std::sync::Arc::new(std::sync::Mutex::new(std::sync::Arc::new(pos))),
                        pinned: std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
                        stop: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        settled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                        vp: std::rc::Rc::new(std::cell::RefCell::new(vp)),
                        labels: std::rc::Rc::new(std::cell::RefCell::new((String::new(), std::collections::HashMap::new()))),
                        grid: std::collections::HashMap::new(),
                        grid_stamp: (usize::MAX, 0, 0, 0),
                        snap: 0,
                        drag_node: None,
                        drag_moved: false,
                        down_pos: [0.0, 0.0],
                        pan: None,
                        enlarged: false,
                        prev_size: None,
                    });
                    this.start_sim(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn start_sim(&mut self, cx: &mut Context<Self>) {
        let Some(g) = &self.graph else { return };
        if g.running.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        g.settled.store(false, std::sync::atomic::Ordering::SeqCst);
        let seq = self.graph_seq;
        let data = g.data.clone();
        let pos = g.pos.clone();
        let pinned = g.pinned.clone();
        let stop = g.stop.clone();
        let running = g.running.clone();
        let settled = g.settled.clone();
        let settled_ui = settled.clone();
        let is_global = !self.g_local;
        let dir = self.store.data_dir();
        let (tx, rx) = async_channel::unbounded::<()>();
        std::thread::spawn(move || {
            let initial = (**pos.lock().unwrap()).clone();
            let mut layout = crate::graph::Layout::from_positions(&data, initial);
            for _ in 0..3000 {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let pins: std::collections::HashSet<usize> = pinned.lock().unwrap().clone();
                {
                    let shared = pos.lock().unwrap();
                    for &i in &pins {
                        if i < layout.pos.len() {
                            layout.pos[i] = shared[i];
                        }
                    }
                }
                let max_move = layout.step(&data, &pins);
                *pos.lock().unwrap() = std::sync::Arc::new(layout.pos.clone());
                if max_move < 0.08 {
                    settled.store(true, std::sync::atomic::Ordering::SeqCst);
                    if is_global {
                        crate::graph::save_cache(&dir, &data, &layout.pos);
                    }
                    let _ = tx.try_send(());
                    break;
                }
                let _ = tx.try_send(());
                std::thread::sleep(std::time::Duration::from_millis(8));
            }
            running.store(false, std::sync::atomic::Ordering::SeqCst);
        });
        cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                let mut done = false;
                let _ = this.update(cx, |this, cx| {
                    if this.graph_seq == seq {
                        if let Some(g) = &mut this.graph {
                            g.snap += 1;
                        }
                        if settled_ui.load(std::sync::atomic::Ordering::SeqCst) {
                            done = true;
                            // Re-center once the layout settles (nodes
                            // drift from their hash start positions).
                            if let Some(g) = &this.graph {
                                let idx = if this.g_local {
                                    this.g_center.as_ref().and_then(|id| g.data.by_id.get(id).copied())
                                } else {
                                    Some(0)
                                };
                                if let Some(idx) = idx {
                                    if !g.data.nodes.is_empty() {
                                        g.vp.borrow_mut().center_on = Some(idx);
                                    }
                                }
                            }
                        }
                        cx.notify();
                    } else {
                        done = true;
                    }
                });
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    fn graph_hit(&mut self, local: [f32; 2]) -> Option<usize> {
        let g = self.graph.as_mut()?;
        let (cam, zoom) = {
            let vp = g.vp.borrow();
            (vp.cam, vp.zoom)
        };
        let world = [(local[0] - cam[0]) / zoom, (local[1] - cam[1]) / zoom];
        let stamp = (g.snap, cam[0] as i32, cam[1] as i32, zoom.to_bits());
        if g.grid_stamp != stamp {
            let pos = g.pos.lock().unwrap();
            g.grid = crate::graph::build_grid(&pos, 24.0);
            g.grid_stamp = stamp;
        }
        let pos = g.pos.lock().unwrap();
        crate::graph::nearest(&g.grid, &pos, world, 12.0 / zoom, 24.0)
    }

    fn graph_local(&self, ev_pos: Point<Pixels>) -> [f32; 2] {
        let Some(g) = &self.graph else { return [0.0, 0.0] };
        let vp = g.vp.borrow();
        [f32::from(ev_pos.x) - vp.bounds_origin[0], f32::from(ev_pos.y) - vp.bounds_origin[1]]
    }

    fn graph_mouse_down(&mut self, ev: &MouseDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if ev.button != MouseButton::Left || self.graph.is_none() {
            return;
        }
        let local = self.graph_local(ev.position);
        let hit = self.graph_hit(local);
        match hit {
            Some(i) => {
                if let Some(g) = &mut self.graph {
                    g.drag_node = Some(i);
                    g.drag_moved = false;
                    g.down_pos = local;
                    g.pinned.lock().unwrap().insert(i);
                }
                self.start_sim(cx);
            }
            None => {
                if let Some(g) = &mut self.graph {
                    let cam = g.vp.borrow().cam;
                    g.pan = Some((local, cam));
                }
            }
        }
        cx.notify();
    }

    fn graph_mouse_move(&mut self, ev: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let local = self.graph_local(ev.position);
        enum Act {
            Drag(usize, [f32; 2]),
            Pan([f32; 2]),
            Hover,
        }
        let act = {
            let Some(g) = &self.graph else { return };
            let vp = g.vp.borrow();
            let world = [(local[0] - vp.cam[0]) / vp.zoom, (local[1] - vp.cam[1]) / vp.zoom];
            if let Some(i) = g.drag_node {
                Act::Drag(i, world)
            } else if let Some((start, cam0)) = g.pan {
                Act::Pan([cam0[0] + local[0] - start[0], cam0[1] + local[1] - start[1]])
            } else {
                Act::Hover
            }
        };
        match act {
            Act::Drag(i, world) => {
                if let Some(g) = &mut self.graph {
                    let dx = local[0] - g.down_pos[0];
                    let dy = local[1] - g.down_pos[1];
                    if dx * dx + dy * dy >= 16.0 {
                        g.drag_moved = true;
                    }
                    let mut guard = g.pos.lock().unwrap();
                    std::sync::Arc::make_mut(&mut *guard)[i] = world;
                    g.snap += 1;
                }
                cx.notify();
            }
            Act::Pan(cam) => {
                if let Some(g) = &self.graph {
                    g.vp.borrow_mut().cam = cam;
                }
                cx.notify();
            }
            Act::Hover => {
                let hit = self.graph_hit(local);
                if let Some(g) = &mut self.graph {
                    let changed = g.vp.borrow().hover != hit;
                    if changed {
                        g.vp.borrow_mut().hover = hit;
                        cx.notify();
                    }
                }
            }
        }
    }

    fn graph_mouse_up(&mut self, _ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        let mut center_on: Option<usize> = None;
        let mut open_id: Option<String> = None;
        if let Some(g) = &mut self.graph {
            if let Some(i) = g.drag_node.take() {
                g.pinned.lock().unwrap().remove(&i);
                if !g.drag_moved {
                    let node = &g.data.nodes[i];
                    if node.is_tag {
                        center_on = Some(i);
                    } else {
                        open_id = Some(node.id.clone());
                    }
                }
            }
            g.pan = None;
        }
        if let Some(id) = open_id {
            self.stop_graph();
            self.return_to_graph = true;
            self.open_note(&id, window, cx);
            return;
        }
        if let (Some(i), Some(g)) = (center_on, &self.graph) {
            let pos = g.pos.lock().unwrap();
            let p = pos[i];
            let mut vp = g.vp.borrow_mut();
            let (w, h) = (f32::from(vp.bounds.size.width), f32::from(vp.bounds.size.height));
            vp.cam = [w / 2.0 - p[0] * vp.zoom, h / 2.0 - p[1] * vp.zoom];
        }
        cx.notify();
    }

    fn graph_wheel(&mut self, ev: &ScrollEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if self.graph.is_none() {
            return;
        }
        let local = self.graph_local(ev.position);
        let delta = ev.delta.pixel_delta(px(16.0));
        if let Some(g) = &self.graph {
            let mut vp = g.vp.borrow_mut();
            let world = [(local[0] - vp.cam[0]) / vp.zoom, (local[1] - vp.cam[1]) / vp.zoom];
            let factor = (1.0 - f32::from(delta.y) / 300.0).clamp(0.7, 1.4);
            vp.zoom = (vp.zoom * factor).clamp(0.15, 3.0);
            vp.cam = [local[0] - world[0] * vp.zoom, local[1] - world[1] * vp.zoom];
        }
        cx.notify();
    }

    fn leave_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.stop_graph();
        self.restore_graph_size(window);
        self.screen = Screen::Editor;
        self.editor.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn restore_graph_size(&mut self, window: &mut Window) {
        if let Some(g) = &mut self.graph {
            if g.enlarged {
                if let Some((w, h)) = g.prev_size {
                    window.resize(size(px(w), px(h)));
                }
                g.enlarged = false;
            }
        }
    }

    fn toggle_graph_enlarge(&mut self, window: &mut Window) {
        let Some(g) = &mut self.graph else { return };
        if g.enlarged {
            self.restore_graph_size(window);
            return;
        }
        let viewport = window.viewport_size();
        g.prev_size = Some((f32::from(viewport.width), f32::from(viewport.height)));
        // Current-monitor work area via Win32, converted to logical pixels.
        let scale = window.scale_factor().max(1.0);
        let (mut w, mut h) = (f32::from(viewport.width), f32::from(viewport.height));
        unsafe {
            use windows::Win32::Foundation::RECT;
            use windows::Win32::UI::WindowsAndMessaging::{
                SPI_GETWORKAREA, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW,
            };
            let mut rc = RECT::default();
            if SystemParametersInfoW(
                SPI_GETWORKAREA,
                0,
                Some(&mut rc as *mut _ as *mut std::ffi::c_void),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            )
            .is_ok()
            {
                w = (rc.right - rc.left) as f32 / scale;
                h = (rc.bottom - rc.top) as f32 / scale;
            }
        }
        window.resize(size(px(w), px(h)));
        g.enlarged = true;
    }

    /// Screen switches flip instantly and load their rows in the
    /// background; the previous rows stay until the new ones land, and a
    /// generation number drops stale arrivals.
    pub fn show_tags(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen_gen += 1;
        let seq = self.screen_gen;
        self.screen = Screen::Tags;
        self.selected = 0;
        self.search
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
        let store = self.store.clone();
        let task = cx.background_spawn(async move {
            (
                store.tags().unwrap_or_default(),
                store.note_count().unwrap_or(0),
                store.trashed_notes().map(|n| n.len()).unwrap_or(0),
            )
        });
        cx.spawn(async move |this, cx| {
            let (tags, total, trash) = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.screen_gen == seq {
                    this.tags = tags;
                    this.note_total = total;
                    this.trash_count = trash;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn show_trash(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen_gen += 1;
        let seq = self.screen_gen;
        self.screen = Screen::Trash;
        self.selected = 0;
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
        let store = self.store.clone();
        let task = cx.background_spawn(async move { store.trashed_notes().unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let notes = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.screen_gen == seq {
                    this.trash_count = notes.len();
                    this.trash_notes = notes;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn show_tag(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.screen_gen += 1;
        let seq = self.screen_gen;
        self.screen = Screen::TagIndex(tag.to_string());
        self.selected = 0;
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
        let store = self.store.clone();
        let tag = tag.to_string();
        let task = cx.background_spawn(async move { store.notes_with_tag(&tag).unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let notes = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.screen_gen == seq {
                    this.notes = notes;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub fn show_backlinks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.note_id.clone() else { return };
        self.screen_gen += 1;
        let seq = self.screen_gen;
        self.screen = Screen::Backlinks(id.clone());
        self.selected = 0;
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
        let store = self.store.clone();
        let task = cx.background_spawn(async move { store.backlinks(&id).unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let notes = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.screen_gen == seq {
                    this.notes = notes;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn list_len(&self) -> usize {
        match self.screen {
            Screen::Tags => self.tags.len() + 1,
            Screen::Trash => self.trash_notes.len(),
            Screen::Switcher => self.switcher_results.len(),
            Screen::Help => KEY_HELP.len(),
            Screen::TagIndex(_) | Screen::Search | Screen::Backlinks(_) => self.notes.len(),
            Screen::Themes => self.theme_filtered.len(),
            Screen::Graph => 0,
            Screen::Editor => 0,
        }
    }

    fn move_selection(&mut self, delta: i64, cx: &mut Context<Self>) {
        let len = self.list_len();
        if len == 0 {
            return;
        }
        let next = self.selected as i64 + delta;
        self.selected = next.clamp(0, len as i64 - 1) as usize;
        self.list_scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        if self.screen == Screen::Themes {
            self.preview_theme(cx);
        }
        cx.notify();
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.screen {
            Screen::Tags => {
                if self.selected == self.tags.len() {
                    self.show_trash(window, cx);
                } else if let Some(tag) = self.tags.get(self.selected) {
                    let name = tag.name.clone();
                    self.show_tag(&name, window, cx);
                }
            }
            Screen::Trash => {
                if let Some(note) = self.trash_notes.get(self.selected) {
                    let id = note.id.clone();
                    let id_for_status = id.clone();
                    let store = self.store.clone();
                    let task = cx.background_spawn(async move {
                        let r = store.restore_note(&id);
                        let notes = store.trashed_notes().unwrap_or_default();
                        (r, notes)
                    });
                    cx.spawn(async move |this, cx| {
                        let (r, notes) = task.await;
                        let _ = this.update(cx, |this, cx| {
                            this.status = match r {
                                Ok(()) => format!("Restored {id_for_status}.md").into(),
                                Err(e) => format!("Error: {e}").into(),
                            };
                            this.keep_status = true;
                            this.trash_count = notes.len();
                            this.trash_notes = notes;
                            if this.selected >= this.trash_notes.len() && this.selected > 0 {
                                this.selected -= 1;
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                }
            }
            Screen::TagIndex(_) | Screen::Search | Screen::Backlinks(_) => {
                if let Some(note) = self.notes.get(self.selected) {
                    let id = note.id.clone();
                    self.return_to_graph = false;
                    self.open_note(&id, window, cx);
                }
            }
            Screen::Themes => self.keep_theme(window, cx),
            Screen::Switcher => {
                if let Some(note) = self.switcher_results.get(self.selected) {
                    let id = note.id.clone();
                    self.return_to_graph = false;
                    self.open_note(&id, window, cx);
                }
            }
            Screen::Help => {}
            Screen::Graph => {}
            Screen::Editor => {}
        }
    }

    fn open_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        trace_next_frame(window, "click_to_frame");
        self.selected = ix;
        self.open_selected(window, cx);
    }

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
            Screen::Editor | Screen::Graph => {}
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

    fn render_editor(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = row_colors(cx);
        let status = self.status.clone();
        let chips: Vec<String> = self.live_tags.clone();

        let mut chip_row = div().flex().items_center().gap_2();
        for (i, tag) in chips.iter().enumerate() {
            let tag_for_click = tag.clone();
            chip_row = chip_row.child(
                div()
                    .id(i)
                    .cursor_pointer()
                    .text_color(theme.blue)
                    .hover(|s| s.bg(theme.list_hover))
                    .child(format!("#{tag}"))
                    .on_click(cx.listener(move |this, _ev, window, cx| {
                        let tag = tag_for_click.clone();
                        this.commit(window, cx);
                        this.show_tag(&tag, window, cx);
                    })),
            );
        }

        // Ctrl+T suggestions: outlined chips with confidence; click adds the
        // tag to the note's Tag: line.
        for (j, suggestion) in self.suggestions.clone().iter().enumerate() {
            let tag_for_click = suggestion.tag.clone();
            chip_row = chip_row.child(
                div()
                    .id(100 + j)
                    .cursor_pointer()
                    .text_color(theme.blue)
                    .border_1()
                    .border_color(theme.blue)
                    .rounded_md()
                    .px_2()
                    .hover(|s| s.bg(theme.list_hover))
                    .child(format!("#{} {}%", suggestion.tag, (suggestion.confidence * 100.0).round() as i64))
                    .on_click(cx.listener(move |this, _ev, window, cx| {
                        let tag = tag_for_click.clone();
                        let text = this.editor.read(cx).value().to_string();
                        let new_text = crate::autotag::add_tag_to_text(&text, &tag);
                        this.editor.update(cx, |s, cx| s.set_value(new_text.clone(), window, cx));
                        this.live_tags = parse_tags(&new_text);
                        this.suggestions.retain(|s| !s.tag.eq_ignore_ascii_case(&tag));
                        cx.notify();
                    })),
            );
        }

        if self.note_id.is_some() {
            let count = self.backlink_count;
            chip_row = chip_row.child(
                div()
                    .id("backlinks")
                    .cursor_pointer()
                    .text_color(theme.blue)
                    .hover(|s| s.bg(theme.list_hover))
                    .child(format!("{count} backlinks"))
                    .on_click(cx.listener(move |this, _ev, window, cx| {
                        this.show_backlinks(window, cx);
                    })),
            );
        }

        // `[[` matches shown above the footer; click inserts [[id|title]].
        let mut link_bar = div().flex().flex_col().px_6();
        for (j, note) in self.link_matches.clone().iter().enumerate() {
            let id_for_click = note.id.clone();
            let title_for_click = note.title.clone();
            link_bar = link_bar.child(
                div()
                    .id(200 + j)
                    .cursor_pointer()
                    .text_color(theme.blue)
                    .hover(|s| s.bg(theme.list_hover))
                    .child(note.title.clone())
                    .on_click(cx.listener(move |this, _ev, window, cx| {
                        let text = this.editor.read(cx).value().to_string();
                        let new_text = complete_link(&text, &id_for_click, &title_for_click);
                        this.editor.update(cx, |s, cx| s.set_value(new_text.clone(), window, cx));
                        this.live_tags = parse_tags(&new_text);
                        this.link_matches.clear();
                        cx.notify();
                    })),
            );
        }
        let link_bar: AnyElement = if self.link_matches.is_empty() {
            div().into_any_element()
        } else {
            link_bar.border_t_1().border_color(theme.border).into_any_element()
        };

        let right: AnyElement = if status.is_empty() {
            div()
                .text_color(theme.muted_foreground)
                .child("Alt+J save & hide   Ctrl+O browse   Ctrl+T suggest tags   Ctrl+Shift+P themes   Ctrl+N new")
                .into_any_element()
        } else {
            div()
                .text_color(theme.muted_foreground)
                .child(status)
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .flex_1()
                    .px_6()
                    .pt_4()
                    .child(Textarea::new(&self.editor).appearance(false).large().h_full()),
            )
            .child(link_bar)
            .child(
                div()
                    .h_8()
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(theme.border)
                    .px_4()
                    .child(chip_row)
                    .child(right),
            )
    }

    fn heading_text(&self) -> String {
        match &self.screen {
            Screen::Tags => {
                format!("{} tags \u{00B7} {} notes", self.tags.len(), self.note_total)
            }
            Screen::TagIndex(tag) => format!("#{tag} \u{00B7} {} notes", self.notes.len()),
            Screen::Search => {
                if self.notes.len() >= 200 {
                    "200+ matches".to_string()
                } else {
                    format!("{} matches", self.notes.len())
                }
            }
            Screen::Backlinks(_) => format!("Backlinks \u{00B7} {} notes", self.notes.len()),
            Screen::Trash => format!("#trash \u{00B7} {} notes (Enter restores)", self.trash_notes.len()),
            Screen::Switcher => format!("Quick switch \u{00B7} {} of {} notes", self.switcher_results.len(), self.switcher_index.len()),
            Screen::Help => "Keys".to_string(),
            Screen::Themes => format!("{} themes", self.theme_filtered.len()),
            Screen::Graph => "Graph".to_string(),
            Screen::Editor => String::new(),
        }
    }

    fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = row_colors(cx);
        let selected = ix == self.selected;
        let mut row = div()
            .id(ix)
            .h_8()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .rounded_md()
            .cursor_pointer()
            .hover(|s| s.bg(theme.list_hover));
        if selected {
            row = row.bg(theme.list_active);
        }

        row = match &self.screen {
            Screen::Tags if ix == self.tags.len() => row
                .child(
                    div()
                        .flex_1()
                        .text_color(theme.muted_foreground)
                        .child("#trash"),
                )
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(format!("{}", self.trash_count)),
                ),
            Screen::Trash => {
                let note = &self.trash_notes[ix];
                row.child(div().flex_1().overflow_hidden().child(note.title.clone()))
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(date_of_id(&note.id)),
                    )
            }
            Screen::Tags => {
                let tag = &self.tags[ix];
                row.child(
                    div()
                        .flex_1()
                        .text_color(theme.blue)
                        .child(format!("#{}", tag.name)),
                )
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(format!("{}", tag.count)),
                )
            }
            Screen::Themes => {
                let entry = &self.theme_entries[self.theme_filtered[ix]];
                // Three clear columns (name / mode / swatches) with wide
                // gaps; every swatch is bordered so light swatches stay
                // visible on light themes and dark ones on dark themes.
                let mut swatches = div().flex().items_center().gap_1();
                for sw in entry.swatches {
                    swatches = swatches.child(
                        div()
                            .w_4()
                            .h_4()
                            .rounded_sm()
                            .bg(rgb(sw))
                            .border_1()
                            .border_color(theme.border),
                    );
                }
                let display_name = if entry.name == self.theme_original {
                    format!("\u{2713} {}", entry.name)
                } else {
                    entry.name.clone()
                };
                row.gap_4()
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(display_name),
                    )
                    .child(
                        div()
                            .w_12()
                            .mr_2()
                            .flex()
                            .justify_end()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(if entry.dark { "dark" } else { "light" }),
                    )
                    .child(swatches)
            }
            Screen::Switcher => {
                let note = &self.switcher_results[ix];
                row.child(div().flex_1().overflow_hidden().child(note.title.clone()))
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(date_of_id(&note.id)),
                    )
            }
            Screen::Help => {
                let (screen, keys, desc) = KEY_HELP[ix];
                row.child(
                    div()
                        .w(px(110.0))
                        .text_color(theme.muted_foreground)
                        .child(screen),
                )
                .child(div().w(px(210.0)).text_color(theme.blue).child(keys))
                .child(div().flex_1().overflow_hidden().child(desc))
            }
            _ => {
                let note = &self.notes[ix];
                row.child(div().flex_1().overflow_hidden().child(note.title.clone()))
                    .child(
                        div()
                            .text_color(theme.muted_foreground)
                            .child(date_of_id(&note.id)),
                    )
            }
        };

        row.on_click(cx.listener(move |this, _ev, window, cx| {
            this.open_item(ix, window, cx);
        }))
        .into_any_element()
    }

    fn render_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = row_colors(cx);
        if let Some(g) = &self.graph {
            if !g.settled.load(std::sync::atomic::Ordering::SeqCst) {
                window.request_animation_frame();
            }
        }
        let status = if self.graph_loading {
            "Loading graph\u{2026}".to_string()
        } else if let Some(g) = &self.graph {
            let settled = g.settled.load(std::sync::atomic::Ordering::SeqCst);
            format!(
                "{} nodes \u{00B7} {} edges \u{00B7} {}{}{}",
                g.data.nodes.len(),
                g.data.edges.len(),
                if self.g_local { "Local" } else { "Global" },
                if settled { "" } else { " \u{00B7} settling\u{2026}" },
                match &self.g_center {
                    Some(c) if self.g_local => format!(" \u{00B7} center {c}"),
                    _ => String::new(),
                },
            )
        } else {
            "No graph".to_string()
        };

        let canvas_el: AnyElement = if let Some(g) = &self.graph {
            let data = g.data.clone();
            let pos_arc = g.pos.clone();
            let vp = g.vp.clone();
            let labels = g.labels.clone();
            let hover = g.vp.borrow().hover;
            let query = g.vp.borrow().query.clone();
            canvas(
                {
                    let data = data.clone();
                    let pos_arc = pos_arc.clone();
                    let vp = vp.clone();
                    move |bounds, _window, _cx| {
                        let pos = pos_arc.lock().unwrap();
                        let mut v = vp.borrow_mut();
                        v.bounds = bounds;
                        v.bounds_origin = [f32::from(bounds.origin.x), f32::from(bounds.origin.y)];
                        if !v.centered {
                            v.cam = [f32::from(bounds.size.width) / 2.0, f32::from(bounds.size.height) / 2.0];
                            v.centered = true;
                        }
                        if let Some(idx) = v.center_on.take() {
                            if let Some(p) = pos.get(idx) {
                                v.cam = [
                                    f32::from(bounds.size.width) / 2.0 - p[0] * v.zoom,
                                    f32::from(bounds.size.height) / 2.0 - p[1] * v.zoom,
                                ];
                            }
                        }
                        let (cam, zoom) = (v.cam, v.zoom);
                        drop(v);
                        let to_screen = |p: [f32; 2]| -> [f32; 2] {
                            [f32::from(bounds.origin.x) + p[0] * zoom + cam[0], f32::from(bounds.origin.y) + p[1] * zoom + cam[1]]
                        };
                        let in_view = |s: [f32; 2], m: f32| {
                            s[0] > f32::from(bounds.origin.x) - m
                                && s[0] < f32::from(bounds.origin.x) + f32::from(bounds.size.width) + m
                                && s[1] > f32::from(bounds.origin.y) - m
                                && s[1] < f32::from(bounds.origin.y) + f32::from(bounds.size.height) + m
                        };
                        let mut paint = GraphPaint::default();
                        for (i, node) in data.nodes.iter().enumerate() {
                            let s = to_screen(pos[i]);
                            if in_view(s, 40.0) {
                                let dim = query.as_ref().map(|q| !node.is_tag && !q.contains(&node.id)).unwrap_or(false);
                                paint.nodes.push((i, s, node.is_tag, dim));
                            }
                        }
                        if data.nodes.len() <= 4000 {
                            for (a, b) in &data.edges {
                                let (sa, sb) = (to_screen(pos[*a as usize]), to_screen(pos[*b as usize]));
                                if in_view(sa, 0.0) || in_view(sb, 0.0) {
                                    paint.edges.push((sa, sb));
                                }
                            }
                        }
                        // Labels: all when zoomed in, else the 150
                        // highest-degree visible nodes (plus the hover).
                        let mut label_idx: Vec<usize> = paint.nodes.iter().map(|(i, _, _, _)| *i).collect();
                        if zoom <= 0.8 {
                            label_idx.sort_by_key(|&i| std::cmp::Reverse(data.degree[i]));
                            label_idx.truncate(150);
                        }
                        if let Some(h) = hover {
                            if paint.nodes.iter().any(|(i, _, _, _)| *i == h) && !label_idx.contains(&h) {
                                label_idx.push(h);
                            }
                        }
                        for (i, s, _, _) in &paint.nodes {
                            if label_idx.contains(i) {
                                paint.labels.push((*i, *s));
                            }
                        }
                        paint
                    }
                },
                move |bounds, paint, window, cx| {
                    let theme_name = cx.theme().theme_name().to_string();
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        // Edges: one stroke path for everything visible.
                        let mut edge_color = colors.muted_foreground;
                        edge_color.a = 0.35;
                        let mut path = PathBuilder::stroke(px(1.0));
                        for (a, b) in &paint.edges {
                            path.move_to(point(px(a[0]), px(a[1])));
                            path.line_to(point(px(b[0]), px(b[1])));
                        }
                        if let Ok(p) = path.build() {
                            window.paint_path(p, edge_color);
                        }
                        // Nodes.
                        for (i, s, is_tag, dim) in &paint.nodes {
                            let node = &data.nodes[*i];
                            let r = if *is_tag {
                                5.0
                            } else {
                                (3.0 + (data.degree[*i] as f32).sqrt() * 1.1).min(10.0)
                            };
                            let mut color = graph_node_color(&colors, node);
                            if *dim {
                                color.a *= 0.15;
                            }
                            let rect = Bounds::new(
                                point(px(s[0] - r), px(s[1] - r)),
                                size(px(r * 2.0), px(r * 2.0)),
                            );
                            if *is_tag {
                                window.paint_quad(fill(rect, color));
                            } else {
                                window.paint_quad(fill(rect, color).corner_radii(px(r)));
                            }
                            if hover == Some(*i) {
                                let mut ring = colors.foreground;
                                ring.a = 0.9;
                                let rect2 = Bounds::new(
                                    point(px(s[0] - r - 3.0), px(s[1] - r - 3.0)),
                                    size(px((r + 3.0) * 2.0), px((r + 3.0) * 2.0)),
                                );
                                window.paint_quad(fill(rect2, ring).corner_radii(px(r + 3.0)));
                                window.paint_quad(fill(rect, color).corner_radii(px(r)));
                            }
                        }
                        // Labels (shaped lines cached per node and theme).
                        let font = window.text_style().font();
                        let mut cache = labels.borrow_mut();
                        if cache.0 != theme_name {
                            cache.1.clear();
                            cache.0 = theme_name.clone();
                        }
                        for (i, s) in &paint.labels {
                            let node = &data.nodes[*i];
                            let shaped = cache.1.entry(*i).or_insert_with(|| {
                                window.text_system().shape_line(
                                    node.label.clone().into(),
                                    px(11.0),
                                    &[TextRun {
                                        len: node.label.len(),
                                        font: font.clone(),
                                        color: colors.foreground,
                                        background_color: None,
                                        underline: None,
                                        strikethrough: None,
                                    }],
                                    None,
                                )
                            });
                            let _ = shaped.paint(
                                point(px(s[0] + 8.0), px(s[1] - 6.0)),
                                px(14.0),
                                TextAlign::Left,
                                None,
                                window,
                                cx,
                            );
                        }
                    });
                },
            )
            .size_full()
            .into_any_element()
        } else {
            div()
                .flex_1()
                .flex()
                .items_center()
                .justify_center()
                .text_color(colors.muted_foreground)
                .child(if self.graph_loading { "Loading graph\u{2026}" } else { "No graph" })
                .into_any_element()
        };

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .px_6()
                    .pt_4()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(Input::new(&self.search).appearance(true))
                    .child(div().text_color(colors.muted_foreground).child(status)),
            )
            .child(
                div()
                    .flex_1()
                    .track_focus(&self.graph_focus)
                    .key_context("GraphView")
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::graph_mouse_down))
                    .on_mouse_move(cx.listener(Self::graph_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::graph_mouse_up))
                    .on_scroll(cx.listener(Self::graph_wheel))
                    .child(canvas_el),
            )
            .child(
                div()
                    .h_8()
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(colors.border)
                    .px_4()
                    .text_color(colors.muted_foreground)
                    .child("Tab Local/Global   T tag nodes   O orphans   1/2/3 depth   / search   Ctrl+Enter enlarge   drag nodes   wheel zoom   click opens   Esc back"),
            )
    }

    fn render_browser(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = row_colors(cx);
        let count = self.list_len();
        let heading = self.heading_text();

        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .key_context("TagSearch")
                    .px_6()
                    .pt_4()
                    .child(Input::new(&self.search).cleanable(true)),
            )
            .child(
                div()
                    .px_6()
                    .pt_2()
                    .text_color(theme.muted_foreground)
                    .child(heading),
            )
            .child(
                uniform_list(
                    "lazykasten-list",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                        range.map(|ix| this.render_row(ix, cx)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.list_scroll)
                .flex_1(),
            )
            .child(
                div()
                    .h_8()
                    .flex()
                    .items_center()
                    .border_t_1()
                    .border_color(theme.border)
                    .px_4()
                    .text_color(theme.muted_foreground)
                    .child(match self.screen {
                        Screen::Tags => "\u{2191}\u{2193} select   Enter open   Esc back   Ctrl+Shift+T sweep inbox   Alt+J hide",
                        Screen::Themes => "\u{2191}\u{2193} preview   Enter keep   Esc revert   Alt+J hide",
                        _ => "\u{2191}\u{2193} select   Enter open   Esc back   Alt+J hide",
                    }),
            )
    }
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
