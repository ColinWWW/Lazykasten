//! The single view: editor screen plus tag/search browser screens.

use std::sync::Arc;

use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _};
use gpui_kit::*;

use crate::store::{NoteSummary, Store, TagCount, parse_tags};

actions!(lazykasten, [SaveNote, NewNote, Browse, Back, SelectPrev, SelectNext]);

/// Register keys. Must run after `gpui_kit::init` so the more specific
/// `"TagSearch > Input"` context wins over the input's own `"Input"` bindings.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-s", SaveNote, Some("LazyKasten")),
        KeyBinding::new("ctrl-n", NewNote, Some("LazyKasten")),
        KeyBinding::new("ctrl-o", Browse, Some("LazyKasten")),
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
    _subscriptions: Vec<Subscription>,
}

impl LazyKasten {
    pub fn new(store: Arc<Store>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let editor = cx.new(|cx| {
            TextareaState::new(window, cx)
                .soft_wrap(true)
                .placeholder("Capture a thought…  (Tag: a, b  or  #hashtags)")
        });
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search all notes…"));

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
            _subscriptions: subscriptions,
        }
    }

    fn on_editor_change(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let text = self.editor.read(cx).value();
        let tags = parse_tags(&text);
        // Typing must never lag: only re-render when the chips actually change.
        if tags != self.live_tags {
            self.live_tags = tags;
            cx.notify();
        }
    }

    fn on_search_change(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().to_string();
        if query.trim().is_empty() {
            self.tags = self.store.tags().unwrap_or_default();
            self.screen = Screen::Tags;
        } else {
            self.notes = self.store.search(&query, 500).unwrap_or_default();
            self.screen = Screen::Search;
        }
        self.selected = 0;
        cx.notify();
    }

    /// Called when the window is shown: fresh editor, cursor ready.
    pub fn on_show(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Editor;
        self.status = SharedString::default();
        self.editor.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    /// Save the current note and reset to a blank editor.
    pub fn commit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.save(window, cx);
        self.note_id = None;
        self.saved_text = SharedString::default();
        self.live_tags.clear();
        self.editor
            .update(cx, |s, cx| s.set_value("", window, cx));
        cx.notify();
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
        match self.store.save(&id, &text) {
            Ok(()) => {
                self.status = if text.trim().is_empty() {
                    "Deleted empty note".into()
                } else {
                    format!("Saved {id}.md").into()
                };
                self.saved_text = text;
            }
            Err(e) => {
                self.status = format!("Error: {e}").into();
            }
        }
        cx.notify();
    }

    pub fn open_note(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        match self.store.read(id) {
            Ok(text) => {
                self.live_tags = parse_tags(&text);
                self.saved_text = text.clone().into();
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

    pub fn show_tags(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tags = self.store.tags().unwrap_or_default();
        self.screen = Screen::Tags;
        self.selected = 0;
        self.search
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    pub fn show_tag(&mut self, tag: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.notes = self.store.notes_with_tag(tag).unwrap_or_default();
        self.screen = Screen::TagIndex(tag.to_string());
        self.selected = 0;
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn list_len(&self) -> usize {
        match self.screen {
            Screen::Tags => self.tags.len(),
            Screen::TagIndex(_) | Screen::Search => self.notes.len(),
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
        cx.notify();
    }

    fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match &self.screen {
            Screen::Tags => {
                if let Some(tag) = self.tags.get(self.selected) {
                    let name = tag.name.clone();
                    self.show_tag(&name, window, cx);
                }
            }
            Screen::TagIndex(_) | Screen::Search => {
                if let Some(note) = self.notes.get(self.selected) {
                    let id = note.id.clone();
                    self.open_note(&id, window, cx);
                }
            }
            Screen::Editor => {}
        }
    }

    fn open_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
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
        match self.screen {
            Screen::TagIndex(_) | Screen::Search => self.show_tags(window, cx),
            Screen::Tags => {
                self.screen = Screen::Editor;
                self.editor.update(cx, |s, cx| s.focus(window, cx));
                cx.notify();
            }
            Screen::Editor => {}
        }
    }

    fn on_select_prev(&mut self, _: &SelectPrev, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(-1, cx);
    }

    fn on_select_next(&mut self, _: &SelectNext, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_selection(1, cx);
    }

    // -- rendering ---------------------------------------------------------

    fn render_editor(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
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

        let right: AnyElement = if status.is_empty() {
            div()
                .text_color(theme.muted_foreground)
                .child("Alt+J save & hide   Ctrl+O browse   Ctrl+N new")
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
                let notes = self.store.note_count().unwrap_or(0);
                format!("{} tags · {} notes", self.tags.len(), notes)
            }
            Screen::TagIndex(tag) => format!("#{tag} · {} notes", self.notes.len()),
            Screen::Search => {
                if self.notes.len() >= 500 {
                    "500+ matches, best first".to_string()
                } else {
                    format!("{} matches", self.notes.len())
                }
            }
            Screen::Editor => String::new(),
        }
    }

    fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
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

    fn render_browser(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
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
                    .child("↑↓ select   Enter open   Esc back   Alt+J hide"),
            )
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

impl Render for LazyKasten {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let root = div()
            .key_context("LazyKasten")
            .on_action(cx.listener(Self::on_save_note))
            .on_action(cx.listener(Self::on_new_note))
            .on_action(cx.listener(Self::on_browse))
            .on_action(cx.listener(Self::on_back))
            .on_action(cx.listener(Self::on_select_prev))
            .on_action(cx.listener(Self::on_select_next))
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground);

        match self.screen {
            Screen::Editor => root.child(self.render_editor(window, cx)),
            _ => root.child(self.render_browser(window, cx)),
        }
    }
}
