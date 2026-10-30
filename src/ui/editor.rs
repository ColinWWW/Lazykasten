//! Editor screen: typing, save/autosave/conflicts, rendering.

use super::*;

impl LazyKasten {
    pub(super) fn on_editor_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn spawn_auto_tag(&mut self, id: String, original_text: String, cx: &mut Context<Self>) {
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
                            Ok(true) => Some((
                                format!(
                                    "Auto-tagged: {} (Ctrl+Z to undo)",
                                    tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")
                                ),
                                Some((bg_id.clone(), tags)),
                            )),
                            Ok(false) => None,
                            Err(e) => Some((format!("Auto-tagging failed: {e}"), None)),
                        }
                    }
                }
                Err(e) => Some((format!("Auto-tagging failed: {e}"), None)),
            }
        });
        cx.spawn(async move |this, cx| {
            if let Some((status, undo)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    if let Some(entry) = undo {
                        this.autotag_undo.push(entry);
                        if this.autotag_undo.len() > 10 {
                            this.autotag_undo.remove(0);
                        }
                    }
                    this.status = status.into();
                    this.keep_status = true;
                    cx.notify();
                });
            }
        })
        .detach();
    }
    pub(super) fn on_suggest_tags(&mut self, _: &SuggestTags, _window: &mut Window, cx: &mut Context<Self>) {
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
    /// One-shot status from the app shell (e.g. a hotkey fallback).
    pub fn set_initial_status(&mut self, msg: String, cx: &mut Context<Self>) {
        self.status = msg.into();
        self.keep_status = true;
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
    pub(super) fn on_delete_note(&mut self, _: &DeleteNote, window: &mut Window, cx: &mut Context<Self>) {
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
    pub(super) fn render_editor(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
    /// File-watcher entry: re-index changed paths in the background,
    /// then reload / flag the open note if it was among them.
    pub fn on_external_files(
        &mut self,
        paths: Vec<std::path::PathBuf>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let task = cx.background_spawn(async move { store.sync_paths(&paths).unwrap_or_default() });
        cx.spawn(async move |this, cx| {
            let changed = task.await;
            if changed.is_empty() {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.refresh_switcher_index(cx);
                if let Some(open) = this.note_id.clone() {
                    if changed.contains(&open) && this.screen == Screen::Editor {
                        if this.editor.read(cx).value() == this.saved_text {
                            this.open_note(&open, window, cx);
                        } else {
                            this.status = "Changed on disk \u{2014} your edits are kept; saving writes a conflict copy".into();
                            this.keep_status = true;
                            cx.notify();
                        }
                    }
                }
            });
        })
        .detach();
    }
}
