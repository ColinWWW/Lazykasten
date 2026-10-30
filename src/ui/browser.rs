//! Browser screens: tags, search, trash, switcher lists, help.

use super::*;

impl LazyKasten {
    /// Screen switches flip instantly and load their rows in the
    /// background; the previous rows stay until the new ones land, and a
    /// generation number drops stale arrivals.
    /// With Vim on, list screens start in list-normal mode (the
    /// container is focused, j/k work); `/` focuses the search box.
    pub(super) fn focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.vim_enabled {
            self.list_insert = false;
            self.graph_focus.focus(window, cx);
        } else {
            self.search.update(cx, |s, cx| s.focus(window, cx));
        }
    }

    pub(super) fn on_list_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        use super::listnav::Nav;
        if !self.vim_enabled || self.screen == Screen::Editor || self.screen == Screen::Graph {
            return;
        }
        if self.renaming.is_some() {
            return;
        }
        let ks = &ev.keystroke;
        if self.list_insert {
            if ks.key == "escape" {
                self.list_insert = false;
                self.graph_focus.focus(window, cx);
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if ks.key == "enter" {
            self.open_selected(window, cx);
            cx.stop_propagation();
            return;
        }
        if ks.modifiers.alt {
            return;
        }
        let (c, ctrl) = if ks.modifiers.control {
            match ks.key.as_str() {
                "d" => ('d', true),
                "u" => ('u', true),
                _ => return,
            }
        } else if let Some(kc) = &ks.key_char {
            match kc.chars().next() {
                Some(c) => (c, false),
                None => return,
            }
        } else {
            return;
        };
        match self.list_nav.key(c, ctrl) {
            Nav::Move(d) => {
                self.move_selection(d as i64, cx);
            }
            Nav::Top => {
                self.selected = 0;
                self.list_scroll.scroll_to_item(self.selected, ScrollStrategy::Top);
                cx.notify();
            }
            Nav::Bottom => {
                let len = self.list_len();
                if len > 0 {
                    self.selected = len - 1;
                    self.list_scroll.scroll_to_item(self.selected, ScrollStrategy::Bottom);
                    cx.notify();
                }
            }
            Nav::Page(d) => {
                self.move_selection((d * 10) as i64, cx);
            }
            Nav::Search => {
                self.list_insert = true;
                self.search.update(cx, |s, cx| s.focus(window, cx));
                cx.notify();
            }
            Nav::Back => {
                self.go_back(window, cx);
            }
            Nav::None => return,
        }
        cx.stop_propagation();
    }

    pub fn show_tags(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.screen_gen += 1;
        let seq = self.screen_gen;
        self.screen = Screen::Tags;
        self.selected = 0;
        self.search
            .update(cx, |s, cx| s.set_value("", window, cx));
        self.focus_list(window, cx);
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
        self.focus_list(window, cx);
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
        self.focus_list(window, cx);
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
        self.focus_list(window, cx);
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
    pub(super) fn list_len(&self) -> usize {
        match self.screen {
            Screen::Tags => self.tags.len() + 1,
            Screen::Trash => self.trash_notes.len(),
            Screen::Switcher => self.switcher_results.len(),
            Screen::Help => KEY_HELP.len(),
            Screen::Related => self.notes.len(),
            Screen::TagIndex(_) | Screen::Search | Screen::Backlinks(_) => self.notes.len(),
            Screen::Themes => self.theme_filtered.len(),
            Screen::Graph => 0,
            Screen::Editor => 0,
        }
    }
    pub(super) fn move_selection(&mut self, delta: i64, cx: &mut Context<Self>) {
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
    pub(super) fn open_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            self.apply_rename(window, cx);
            return;
        }
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
            Screen::TagIndex(_) | Screen::Search | Screen::Backlinks(_) | Screen::Related => {
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
    pub(super) fn open_item(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        trace_next_frame(window, "click_to_frame");
        self.selected = ix;
        self.open_selected(window, cx);
    }
    pub(super) fn heading_text(&self) -> String {
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
            Screen::Related => format!("Related \u{00B7} {} notes", self.notes.len()),
            Screen::Themes => format!("{} themes", self.theme_filtered.len()),
            Screen::Graph => "Graph".to_string(),
            Screen::Editor => String::new(),
        }
    }
    pub(super) fn render_row(&mut self, ix: usize, cx: &mut Context<Self>) -> AnyElement {
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
    pub(super) fn render_browser(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = row_colors(cx);
        let count = self.list_len();
        let heading = self.heading_text();

        div()
            .flex()
            .flex_col()
            .size_full()
            .track_focus(&self.graph_focus)
            .key_context(if self.vim_enabled && !self.list_insert { "ListNav" } else { "ListSearch" })
            .capture_key_down(cx.listener(Self::on_list_key_down))
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
    pub(super) fn on_rename_tag(&mut self, _: &RenameTag, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Tags || self.selected >= self.tags.len() {
            return;
        }
        let old = self.tags[self.selected].name.clone();
        self.renaming = Some(old.clone());
        self.status = format!("Rename #{old}: type the new name, Enter applies, Esc cancels").into();
        self.keep_status = true;
        self.search.update(cx, |s, cx| s.set_value(old, window, cx));
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }
    pub(super) fn apply_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(old) = self.renaming.take() else { return };
        let new = self.search.read(cx).value().trim().to_string();
        if new.is_empty() || new.eq_ignore_ascii_case(&old) {
            self.show_tags(window, cx);
            return;
        }
        let store = self.store.clone();
        let task_old = old.clone();
        let task_new = new.clone();
        let task = cx.background_spawn(async move { store.rename_tag(&task_old, &task_new) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.status = match &result {
                    Ok(n) => format!("Renamed #{old} \u{2192} #{new} in {n} notes"),
                    Err(e) => format!("Error: {e}"),
                }
                .into();
                this.keep_status = true;
                this.show_tags(window, cx);
            });
        })
        .detach();
    }
    pub(super) fn on_sweep_inbox(&mut self, _: &SweepInbox, _window: &mut Window, cx: &mut Context<Self>) {
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
}
