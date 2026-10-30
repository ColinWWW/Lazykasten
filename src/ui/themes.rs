//! Theme picker screen.

use super::*;

impl LazyKasten {
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
    pub(super) fn preview_theme(&mut self, cx: &mut Context<Self>) {
        if let Some(entry) = self.theme_filtered.get(self.selected).and_then(|i| self.theme_entries.get(*i)) {
            let name = entry.name.clone();
            crate::themes::apply_registered(cx, &name);
        }
    }
    pub(super) fn keep_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
}
