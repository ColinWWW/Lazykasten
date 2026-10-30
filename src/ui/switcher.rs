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
