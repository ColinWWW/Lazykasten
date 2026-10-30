//! Quick switcher, key help, related notes, auto-tag undo.

use super::*;

impl LazyKasten {
    pub(super) fn refresh_switcher_index(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn on_quick_switcher(&mut self, _: &QuickSwitcher, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Editor {
            self.save(window, cx);
        }
        self.screen = Screen::Switcher;
        self.list_insert = true;
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
    // -- command palette + toasts + preview (prompt 6) ---------------------

    pub(super) fn palette_commands() -> Vec<(&'static str, &'static str, &'static str)> {
        vec![
            ("New note", "Ctrl+N", "new"),
            ("Browse tags", "Ctrl+B", "browse"),
            ("Quick switcher", "Ctrl+P", "switcher"),
            ("Open graph", "Ctrl+G", "graph"),
            ("Themes\u{2026}", "Ctrl+K T", "themes"),
            ("Toggle Vim mode", "", "vim"),
            ("Suggest tags (auto-tag)", "Ctrl+T", "suggest"),
            ("Sweep #inbox", "Ctrl+Shift+I", "sweep"),
            ("Trash", "", "trash"),
            ("Keyboard shortcuts", "Ctrl+/", "help"),
            ("Related notes", "Ctrl+R", "related"),
            ("Toggle focus mode", "Ctrl+.", "focus"),
            ("About Lazykasten", "", "about"),
        ]
    }

    pub(super) fn on_open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen == Screen::Editor {
            self.save(window, cx);
        }
        self.screen = Screen::Palette;
        self.list_insert = true;
        self.selected = 0;
        self.palette_results = (0..Self::palette_commands().len()).collect();
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    pub(super) fn filter_palette(&mut self, query: &str) {
        let q = query.to_lowercase();
        let cmds = Self::palette_commands();
        let mut scored: Vec<(i64, usize)> = cmds
            .iter()
            .enumerate()
            .filter_map(|(i, (name, _, _))| fuzzy_score(&name.to_lowercase(), &q).map(|s| (s, i)))
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        self.palette_results = scored.into_iter().map(|(_, i)| i).collect();
        if self.selected >= self.palette_results.len() {
            self.selected = 0;
        }
    }

    pub(super) fn exec_palette(&mut self, cmd: &str, window: &mut Window, cx: &mut Context<Self>) {
        match cmd {
            "new" => self.on_new_note(&NewNote, window, cx),
            "browse" => {
                self.screen = Screen::Editor;
                self.on_browse(&Browse, window, cx);
            }
            "switcher" => self.on_quick_switcher(&QuickSwitcher, window, cx),
            "graph" => self.on_open_graph(&OpenGraph, window, cx),
            "themes" => self.open_themes(window, cx),
            "vim" => {
                self.set_vim_enabled(!self.vim_enabled, window, cx);
                self.screen = Screen::Editor;
                self.editor.update(cx, |s, cx| s.focus(window, cx));
                cx.notify();
            }
            "suggest" => {
                self.screen = Screen::Editor;
                self.on_suggest_tags(&SuggestTags, window, cx);
            }
            "sweep" => self.on_sweep_inbox(&SweepInbox, window, cx),
            "trash" => self.show_trash(window, cx),
            "help" => self.on_key_help(&KeyHelp, window, cx),
            "related" => {
                self.screen = Screen::Editor;
                self.on_related_notes(&RelatedNotes, window, cx);
            }
            "focus" => self.on_focus_mode(&FocusMode, window, cx),
            "about" => {
                self.screen = Screen::Editor;
                self.editor.update(cx, |s, cx| s.focus(window, cx));
                self.toast("Lazykasten 0.1.0 \u{2014} local-first notes. Alt+J anywhere, files are the source of truth.", cx);
                cx.notify();
            }
            _ => {}
        }
    }

    pub(super) fn on_focus_mode(&mut self, _: &FocusMode, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_mode = !self.focus_mode;
        if self.screen != Screen::Editor {
            self.screen = Screen::Editor;
            self.editor.update(cx, |s, cx| s.focus(window, cx));
        }
        self.toast(if self.focus_mode { "Focus mode on" } else { "Focus mode off" }, cx);
        cx.notify();
    }

    pub(super) fn toast(&mut self, msg: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.toast_gen += 1;
        let seq = self.toast_gen;
        self.toast = Some((msg.into(), seq));
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(3000))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.toast_gen == seq {
                    this.toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Load the selected note's body for the two-pane preview (background
    /// read; stale results dropped by id check at render).
    pub(super) fn load_preview(&mut self, cx: &mut Context<Self>) {
        let Some(note) = self.notes.get(self.selected) else {
            return;
        };
        let id = note.id.clone();
        if self.preview_note.as_ref().is_some_and(|(pid, ..)| *pid == id) {
            return;
        }
        let title = note.title.clone();
        let store = self.store.clone();
        let read_id = id.clone();
        let task = cx.background_spawn(async move { store.read(&read_id) });
        cx.spawn(async move |this, cx| {
            let text = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(text) = text {
                    let body: String = text.chars().take(900).collect();
                    let tags = parse_tags(&text);
                    this.preview_note = Some((id.clone(), title.clone(), body, tags));
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn on_key_help(&mut self, _: &KeyHelp, window: &mut Window, cx: &mut Context<Self>) {
        self.screen = Screen::Help;
        self.selected = 0;
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }
    pub(super) fn on_related_notes(&mut self, _: &RelatedNotes, window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Editor {
            return;
        }
        let text = self.editor.read(cx).value().to_string();
        let exclude = self.note_id.clone().unwrap_or_default();
        let store = self.store.clone();
        let task = cx.background_spawn(async move {
            crate::autotag::related_notes(&store, &exclude, &text, 10).unwrap_or_default()
        });
        cx.spawn(async move |this, cx| {
            let notes = task.await;
            let _ = this.update_in(cx, |this, _window, cx| {
                this.notes = notes;
                this.screen = Screen::Related;
                this.selected = 0;
                cx.notify();
            });
        })
        .detach();
        let _ = window;
    }
    pub(super) fn on_undo_auto_tag(&mut self, _: &UndoAutoTag, _window: &mut Window, cx: &mut Context<Self>) {
        if self.screen != Screen::Editor {
            return;
        }
        // Only right after auto-tagging: the editor must be untouched.
        if self.editor.read(cx).value() != self.saved_text {
            return;
        }
        let Some(open) = self.note_id.clone() else { return };
        let Some(pos) = self.autotag_undo.iter().rposition(|(id, _)| *id == open) else {
            return;
        };
        let (id, tags) = self.autotag_undo.remove(pos);
        let store = self.store.clone();
        let task = cx.background_spawn(async move {
            crate::autotag::remove_appended_tag_line(&store, &id, &tags)
        });
        cx.spawn(async move |this, cx| {
            let done = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                if done.unwrap_or(false) {
                    this.status = "Auto-tagging undone \u{2014} back to #inbox".into();
                    this.keep_status = true;
                    let id = this.note_id.clone();
                    if let Some(id) = id {
                        this.open_note(&id, window, cx);
                    }
                }
            });
        })
        .detach();
    }
}
