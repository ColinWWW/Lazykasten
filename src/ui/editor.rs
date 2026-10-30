//! Editor screen: typing, save/autosave/conflicts, rendering.

use super::*;

impl LazyKasten {
    pub(super) fn on_editor_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        trace_next_frame(window, "keystroke_to_frame");
        // Snippets: "#daily " / "#todo " expand on the trailing space.
        {
            let val = self.editor.read(cx).value().to_string();
            let expanded = if val.ends_with("#daily ") {
                Some(format!("{}- Daily log\n\n## Plan\n\n## Notes\n\n", chrono::Local::now().format("%Y-%m-%d")))
            } else if val.ends_with("#todo ") {
                Some("- [ ] ".to_string())
            } else {
                None
            };
            if let Some(new_text) = expanded {
                let base = if new_text.starts_with("- [ ]") {
                    format!("{}{}", &val[..val.len() - "#todo ".len()], new_text)
                } else {
                    new_text
                };
                self.editor.update(cx, |s, cx| s.set_value(&base, window, cx));
                return;
            }
        }
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
        if self.vim_discard {
            // Vim `:q!`: hide without saving.
            self.vim_discard = false;
            self.note_id = None;
            self.opened_mtime = None;
            self.saved_text = SharedString::default();
            self.editor
                .update(cx, |s, cx| s.set_value("", window, cx));
            cx.notify();
            return;
        }
        let text_before = self.editor.read(cx).value().to_string();
        let untagged = parse_tags(&text_before).is_empty() && !text_before.trim().is_empty();
        self.save(window, cx);
        if self.vim_enabled {
            // A new note begins in Insert mode.
            self.vim.force_mode(crate::vim::Mode::Insert);
            self.vim_sync_readonly(cx);
        }
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
                if self.vim_enabled {
                    // Opening an existing note begins in Normal mode.
                    self.vim.force_mode(crate::vim::Mode::Normal);
                    self.vim_sync_readonly(cx);
                }
            }
            Err(e) => {
                self.status = format!("Error: {e}").into();
            }
        }
        cx.notify();
    }
    pub(super) fn render_editor(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use unicode_segmentation::UnicodeSegmentation as _;
        let theme = row_colors(cx);
        let status = self.status.clone();
        let chips: Vec<String> = self.live_tags.clone();

        let mut chip_row = div().flex().items_center().gap_2();
        for (i, tag) in chips.iter().enumerate() {
            let tag_for_click = tag.clone();
            let color = style::tag_color(tag, theme.background);
            chip_row = chip_row.child(
                div()
                    .id(i)
                    .cursor_pointer()
                    .rounded(px(style::tokens::RADIUS_CHIP))
                    .border_1()
                    .border_color(color)
                    .px(px(style::tokens::CHIP_PAD_X))
                    .py(px(style::tokens::CHIP_PAD_Y))
                    .text_size(px(style::tokens::FONT_MUTED))
                    .text_color(color)
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
        let mut link_bar = div().flex().flex_wrap().gap_2().px_6().py_2();
        for (j, note) in self.link_matches.clone().iter().enumerate() {
            let id_for_click = note.id.clone();
            let title_for_click = note.title.clone();
            link_bar = link_bar.child(
                div()
                    .id(200 + j)
                    .cursor_pointer()
                    .rounded(px(style::tokens::RADIUS_CHIP))
                    .border_1()
                    .border_color(theme.blue)
                    .px(px(style::tokens::CHIP_PAD_X))
                    .py(px(style::tokens::CHIP_PAD_Y))
                    .text_size(px(style::tokens::FONT_MUTED))
                    .text_color(theme.blue)
                    .hover(|s| s.bg(theme.list_hover))
                    .child(format!("\u{27E6}{}\u{27E7}", note.title))
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

        // Vim: block cursor bounds (relative to the textarea) and the
        // mode badge / pending keys for the footer.
        let vim_block = if self.vim_enabled && self.vim.mode != crate::vim::Mode::Insert {
            let s = self.editor.read(cx);
            let text = s.value();
            let cur = s.cursor().min(text.len());
            let end = text[cur..]
                .graphemes(true)
                .next()
                .map(|g| cur + g.len())
                .unwrap_or(cur);
            s.range_to_bounds(&(cur..end))
        } else {
            None
        };
        let vim_ctx = match self.vim.mode {
            crate::vim::Mode::Normal => "VimNormal",
            crate::vim::Mode::Insert => "VimInsert",
            crate::vim::Mode::Visual | crate::vim::Mode::VisualLine => "VimVisual",
            crate::vim::Mode::Command => "VimCommand",
            crate::vim::Mode::Search => "VimSearch",
        };
        let vim_badge: AnyElement = if self.vim_enabled {
            let label = match self.vim.mode {
                crate::vim::Mode::Normal => "-- NORMAL --".to_string(),
                crate::vim::Mode::Insert => "-- INSERT --".to_string(),
                crate::vim::Mode::Visual => "-- VISUAL --".to_string(),
                crate::vim::Mode::VisualLine => "-- VISUAL LINE --".to_string(),
                crate::vim::Mode::Command | crate::vim::Mode::Search => self.vim.pending_display(),
            };
            let pending = if matches!(self.vim.mode, crate::vim::Mode::Command | crate::vim::Mode::Search) {
                String::new()
            } else {
                self.vim.pending_display()
            };
            div()
                .text_color(if self.vim.mode == crate::vim::Mode::Insert { theme.blue } else { theme.muted_foreground })
                .child(if pending.is_empty() { label } else { format!("{label}  {pending}") })
                .into_any_element()
        } else {
            div().into_any_element()
        };

        // Unified footer meta: word count + saved dot.
        let (word_count, is_saved) = {
            let cur = self.editor.read(cx).value();
            (cur.split_whitespace().count(), cur.as_ref() == self.saved_text.as_ref())
        };
        let editor_empty = word_count == 0 && self.note_id.is_none();
        let meta: AnyElement = div()
            .flex()
            .items_center()
            .gap_3()
            .text_size(px(style::tokens::FONT_MUTED))
            .text_color(theme.muted_foreground)
            .child(format!("{word_count} words"))
            .child(if is_saved {
                div().text_color(theme.blue).child("\u{25CF} saved")
            } else {
                div().child("\u{25CB} unsaved")
            })
            .into_any_element();

        let right: AnyElement = if status.is_empty() {
            div()
                .text_color(theme.muted_foreground)
                .child("Alt+J save & hide   Ctrl+O browse   Ctrl+T suggest tags   Ctrl+Shift+P commands   Ctrl+N new")
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
            .key_context(vim_ctx)
            .capture_key_down(cx.listener(Self::on_vim_key_down))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(style::tokens::EDITOR_MAX_W))
                            .px_6()
                            .pt_4()
                            .flex()
                            .flex_col()
                            .children(editor_empty.then(|| {
                                div()
                                    .pb_3()
                                    .text_color(theme.muted_foreground)
                                    .text_size(px(style::tokens::FONT_MUTED))
                                    .child("Start typing \u{2014} end with #tags or a Tag: line.\nCtrl+P jump to a note \u{00B7} Ctrl+Shift+P commands \u{00B7} Ctrl+/ keys\nType [[ to link a note, #daily for today's log.")
                            }))
                            .child(
                                div()
                                    .relative()
                                    .flex_1()
                                    .child(Textarea::new(&self.editor).appearance(false).large().h_full())
                            .children(vim_block.map(|b| {
                                div()
                                    .absolute()
                                    .left(b.origin.x)
                                    .top(b.origin.y)
                                    .w(b.size.width.max(px(9.0)))
                                    .h(b.size.height)
                                    .bg(hsla(theme.blue.h, theme.blue.s, theme.blue.l, 0.45))
                            })),
                        ),
                    ),
            )
            .child(link_bar)
            .child(
                div()
                    .h(px(style::tokens::FOOTER_H))
                    .flex()
                    .items_center()
                    .justify_between()
                    .border_t_1()
                    .border_color(theme.border)
                    .px_4()
                    .child(if self.focus_mode { div().into_any_element() } else { chip_row.into_any_element() })
                    .child(vim_badge)
                    .child(meta)
                    .child(if self.focus_mode { div().into_any_element() } else { right }),
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

impl LazyKasten {
    /// Readonly whenever Vim is on and not in Insert mode (the engine
    /// is then the only writer, so caret shape/IME stay sane).
    pub(super) fn vim_sync_readonly(&mut self, cx: &mut Context<Self>) {
        let readonly = self.vim_enabled && self.vim.mode != crate::vim::Mode::Insert;
        self.editor.update(cx, |s, cx| s.set_readonly(readonly, cx));
    }

    pub fn vim_is_enabled(&self) -> bool {
        self.vim_enabled
    }

    pub fn set_vim_enabled(&mut self, enabled: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.vim_enabled = enabled;
        if enabled {
            let mode = if self.note_id.is_some() {
                crate::vim::Mode::Normal
            } else {
                crate::vim::Mode::Insert
            };
            self.vim.force_mode(mode);
        }
        self.vim_sync_readonly(cx);
        self.toast(if enabled { "Vim mode on" } else { "Vim mode off" }, cx);
        self.status = if enabled { "Vim mode on".into() } else { "Vim mode off".into() };
        self.keep_status = true;
        cx.notify();
    }

    pub(super) fn on_vim_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        use crate::vim::{Key, Mode};
        if !self.vim_enabled || self.screen != Screen::Editor {
            return;
        }
        let ks = &ev.keystroke;
        if self.vim.mode == Mode::Insert {
            if ks.key == "escape" || (ks.modifiers.control && ks.key == "[") {
                self.vim_feed(Key::Esc, window, cx);
                cx.stop_propagation();
            }
            return;
        }
        if ks.modifiers.alt {
            return;
        }
        let key = if ks.key == "escape" {
            Key::Esc
        } else if ks.key == "enter" {
            Key::Enter
        } else if ks.modifiers.control {
            match ks.key.as_str() {
                "r" => Key::Ctrl('r'),
                "d" => Key::Ctrl('d'),
                "u" => Key::Ctrl('u'),
                "[" => Key::Esc,
                "c" if self.vim.mode == Mode::Visual || self.vim.mode == Mode::VisualLine => Key::Esc,
                _ => return,
            }
        } else if let Some(kc) = &ks.key_char {
            match kc.chars().next() {
                Some(c) => Key::Char(c),
                None => return,
            }
        } else if ks.key.chars().count() == 1 {
            Key::Char(ks.key.chars().next().unwrap())
        } else {
            return;
        };
        self.vim_feed(key, window, cx);
        cx.stop_propagation();
    }

    pub(super) fn vim_feed(&mut self, key: crate::vim::Key, window: &mut Window, cx: &mut Context<Self>) {
        use crate::vim::{Key, Mode};
        use unicode_segmentation::UnicodeSegmentation as _;
        let text = self.editor.read(cx).value().to_string();
        let sel = self.editor.read(cx).selected_range();
        let cursor = if sel.is_empty() {
            sel.start.min(text.len())
        } else {
            // Head of the selection: the grapheme before its end.
            let mut p = sel.end.min(text.len());
            if p > sel.start {
                p = text[..p]
                    .graphemes(true)
                    .next_back()
                    .map(|g| p - g.len())
                    .unwrap_or(sel.start);
            }
            p
        };
        let was_insert = self.vim.mode == Mode::Insert;
        if !was_insert {
            self.vim_entry_keys.push(key.clone());
        }
        if matches!(key, Key::Char('.')) && !was_insert {
            self.vim_repeat(window, cx);
            self.vim_sync_readonly(cx);
            cx.notify();
            return;
        }
        let out = self.vim.handle_key(&text, cursor, key.clone());
        self.vim_apply(out, &text, window, cx);
        // Insert-session bookkeeping for dot-repeat.
        if was_insert && matches!(key, Key::Esc | Key::Ctrl('[')) {
            let now = self.editor.read(cx).value().to_string();
            let cur = self.editor.read(cx).cursor();
            let typed = self
                .vim_insert_start
                .filter(|s| *s <= cur && cur <= now.len())
                .map(|s| now[s..cur].to_string())
                .unwrap_or_default();
            self.vim.note_insert_finished(&typed, std::mem::take(&mut self.vim_entry_keys));
            self.vim_insert_start = None;
        }
        if self.vim.mode == Mode::Insert && self.vim_insert_start.is_none() {
            self.vim_insert_start = Some(self.editor.read(cx).cursor());
        }
        if self.vim.mode != Mode::Insert && self.vim.pending_display().is_empty() {
            self.vim_entry_keys.clear();
        }
        self.vim_sync_readonly(cx);
        cx.notify();
    }

    fn vim_repeat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::vim::{Key, Mode};
        let Some((keys, insert)) = self.vim.repeat_change() else {
            return;
        };
        for k in keys {
            let text = self.editor.read(cx).value().to_string();
            let sel = self.editor.read(cx).selected_range();
            let out = self.vim.handle_key(&text, sel.start.min(text.len()), k);
            self.vim_apply(out, &text, window, cx);
            if self.vim.mode == Mode::Insert {
                break;
            }
        }
        if let Some(ins) = insert {
            if self.vim.mode == Mode::Insert {
                let cur = self.editor.read(cx).cursor();
                self.editor.update(cx, |s, cx| {
                    s.set_selected_range(cur..cur, cx);
                    s.replace(&ins, window, cx);
                });
                let text = self.editor.read(cx).value().to_string();
                let cur = self.editor.read(cx).cursor();
                let out = self.vim.handle_key(&text, cur, Key::Esc);
                self.vim_apply(out, &text, window, cx);
            }
        }
        let _ = Mode::Normal;
    }

    fn vim_apply(&mut self, out: crate::vim::Out, text_before: &str, window: &mut Window, cx: &mut Context<Self>) {
        use crate::vim::Effect;
        let mut edits: Vec<&crate::vim::Edit> = out.edits.iter().collect();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start));
        for e in edits {
            let range = e.range.clone();
            let replacement = e.text.clone();
            self.editor.update(cx, |s, cx| {
                s.set_selected_range(range, cx);
                s.replace(&replacement, window, cx);
            });
        }
        match out.selection.clone() {
            Some(sel) => {
                self.editor.update(cx, |s, cx| s.set_selected_range(sel, cx));
            }
            None => {
                let c = out.cursor;
                self.editor.update(cx, |s, cx| s.set_selected_range(c..c, cx));
            }
        }
        match out.effect {
            Effect::Yank(text, _) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
            Effect::PasteRequest(before) => {
                let clip = cx
                    .read_from_clipboard()
                    .and_then(|item| item.text().map(|t| t.to_string()))
                    .unwrap_or_default();
                let text = self.editor.read(cx).value().to_string();
                let out2 = self.vim.paste_text(&text, &clip, out.cursor, before);
                self.vim_apply(out2, &text, window, cx);
            }
            Effect::Undo => {
                window.dispatch_action(Box::new(gpui_kit::component::input::Undo), cx);
            }
            Effect::Redo => {
                window.dispatch_action(Box::new(gpui_kit::component::input::Redo), cx);
            }
            Effect::HideRequest => {
                if let Some(hook) = &self.hide_hook {
                    let _ = hook.send(());
                }
            }
            Effect::ExCommand(cmd) => {
                self.vim_ex(&cmd, window, cx);
            }
            Effect::ScrollHalf(dir) => {
                crate::trace::info(&format!("vim scroll half {dir}"));
            }
            Effect::CenterLine => {
                crate::trace::info("vim zz");
            }
            Effect::RepeatRequest | Effect::None | Effect::Bell => {}
        }
        let _ = text_before;
    }

    fn vim_ex(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        match cmd.trim() {
            "w" => self.save(window, cx),
            "q" => {
                self.save(window, cx);
                if let Some(hook) = &self.hide_hook {
                    let _ = hook.send(());
                }
            }
            "wq" | "x" => {
                self.save(window, cx);
                if let Some(hook) = &self.hide_hook {
                    let _ = hook.send(());
                }
            }
            "q!" => {
                // Discard: the hide path must not save.
                self.vim_discard = true;
                let saved = self.saved_text.clone();
                self.editor.update(cx, |s, cx| s.set_value(saved, window, cx));
                if let Some(hook) = &self.hide_hook {
                    let _ = hook.send(());
                }
            }
            other => {
                self.status = format!("Not an editor command: {other}").into();
                cx.notify();
            }
        }
    }
}
