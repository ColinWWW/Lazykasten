//! Helix-style themes: bundled palettes embedded at compile time, a saved
//! choice in `<vault>/.lazykasten/settings.toml`, and user themes loaded
//! lazily when the picker first opens. Everything is local; startup parses
//! only the active theme.

use std::path::Path;

use gpui_kit::component::{Theme, ThemeRegistry};

pub const DEFAULT_THEME: &str = "Lazykasten Dark";

/// `(theme name, ThemeSet JSON)` for every bundled theme. Parsed lazily:
/// only the active one at startup, the rest when the picker first opens.
pub const BUNDLED: &[(&str, &str)] = &[
    ("Catppuccin Mocha", include_str!("../themes/catppuccin-mocha.json")),
    ("Catppuccin Macchiato", include_str!("../themes/catppuccin-macchiato.json")),
    ("Catppuccin Frappe", include_str!("../themes/catppuccin-frappe.json")),
    ("Catppuccin Latte", include_str!("../themes/catppuccin-latte.json")),
    ("Gruvbox Dark", include_str!("../themes/gruvbox-dark.json")),
    ("Gruvbox Light", include_str!("../themes/gruvbox-light.json")),
    ("Tokyo Night", include_str!("../themes/tokyo-night.json")),
    ("Tokyo Night Storm", include_str!("../themes/tokyo-night-storm.json")),
    ("Nord", include_str!("../themes/nord.json")),
    ("Dracula", include_str!("../themes/dracula.json")),
    ("One Dark", include_str!("../themes/one-dark.json")),
    ("Solarized Dark", include_str!("../themes/solarized-dark.json")),
    ("Solarized Light", include_str!("../themes/solarized-light.json")),
    ("Rose Pine", include_str!("../themes/rose-pine.json")),
    ("Rose Pine Dawn", include_str!("../themes/rose-pine-dawn.json")),
    ("Kanagawa", include_str!("../themes/kanagawa.json")),
    ("Everforest Dark", include_str!("../themes/everforest-dark.json")),
    ("Everforest Light", include_str!("../themes/everforest-light.json")),
    ("Ayu Dark", include_str!("../themes/ayu-dark.json")),
    ("Ayu Mirage", include_str!("../themes/ayu-mirage.json")),
    ("Ayu Light", include_str!("../themes/ayu-light.json")),
    ("GitHub Dark", include_str!("../themes/github-dark.json")),
    ("GitHub Light", include_str!("../themes/github-light.json")),
    ("Monokai", include_str!("../themes/monokai.json")),
    ("Lazykasten Dark", include_str!("../themes/lazykasten-dark.json")),
];

pub fn embedded_json(name: &str) -> Option<&'static str> {
    BUNDLED.iter().find(|(n, _)| *n == name).map(|(_, j)| *j)
}

/// Parse the settings file by hand (`key = "value"` per line).
pub fn settings_map(text: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                out.insert(key.trim().to_string(), value.to_string());
            }
        }
    }
    out
}

pub fn read_settings(data_dir: &Path) -> std::collections::HashMap<String, String> {
    std::fs::read_to_string(data_dir.join("settings.toml"))
        .map(|text| settings_map(&text))
        .unwrap_or_default()
}

/// Write the whole settings map back, atomically, keys sorted.
pub fn write_settings(data_dir: &Path, map: &std::collections::HashMap<String, String>) {
    let _ = std::fs::create_dir_all(data_dir);
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    let mut text = String::new();
    for k in keys {
        text.push_str(&format!("{k} = \"{}\"\n", map[k]));
    }
    let _ = crate::store::atomic_write(&data_dir.join("settings.toml"), text.as_bytes());
}

/// Parse the one-key settings file by hand (`theme = "Name"`).
pub fn parse_settings(text: &str) -> Option<String> {
    settings_map(text).get("theme").cloned()
}

pub fn saved_theme_name(data_dir: &Path) -> String {
    std::fs::read_to_string(data_dir.join("settings.toml"))
        .ok()
        .and_then(|text| parse_settings(&text))
        .unwrap_or_else(|| DEFAULT_THEME.to_string())
}

pub fn write_saved_theme(data_dir: &Path, name: &str) {
    let mut map = read_settings(data_dir);
    map.insert("theme".to_string(), name.to_string());
    write_settings(data_dir, &map);
}

/// Register a theme JSON with the registry (already-registered names are
/// skipped by the registry itself).
pub fn register_json(cx: &mut gpui_kit::App, json: &str) -> anyhow::Result<()> {
    ThemeRegistry::global_mut(cx).load_themes_from_str(json)
}

/// Apply an already-registered theme by name. Returns false if unknown.
pub fn apply_registered(cx: &mut gpui_kit::App, name: &str) -> bool {
    let t0 = std::time::Instant::now();
    let config = ThemeRegistry::global(cx).themes().get(name).cloned();
    match config {
        Some(config) => {
            Theme::update(cx, |theme| theme.apply_config(&config));
            crate::trace::log("theme_switch", t0.elapsed());
            true
        }
        None => false,
    }
}

/// Startup path: register and apply only the saved theme (falling back to
/// the default). User themes are searched only if the saved name is not
/// bundled, so a saved user theme still survives a restart.
pub fn apply_saved(cx: &mut gpui_kit::App, data_dir: &Path) {
    let saved = saved_theme_name(data_dir);
    if let Some(json) = embedded_json(&saved) {
        let _ = register_json(cx, json);
    } else if let Ok(entries) = std::fs::read_dir(data_dir.join("themes")) {
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|e| e.to_str()) == Some("json") {
                if let Ok(text) = std::fs::read_to_string(entry.path()) {
                    let _ = register_json(cx, &text);
                }
            }
        }
    }
    if !apply_registered(cx, &saved) {
        if let Some(json) = embedded_json(DEFAULT_THEME) {
            let _ = register_json(cx, json);
        }
        apply_registered(cx, DEFAULT_THEME);
    }
}

#[derive(Clone, Debug)]
pub struct ThemeEntry {
    pub name: String,
    pub dark: bool,
    /// background, foreground, blue, border, selection as `0xrrggbb`.
    pub swatches: [u32; 5],
}

fn extract_color(json: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\"");
    let start = json.find(&pat)? + pat.len();
    let rest = json[start..].trim_start_matches([' ', ':']);
    let rest = rest.strip_prefix('"')?;
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Load every bundled and user theme into the registry (picker first open
/// only) and return the picker entries plus the names of invalid user files.
/// Invalid files are skipped, never fatal.
pub fn load_picker_themes(
    cx: &mut gpui_kit::App,
    data_dir: &Path,
) -> (Vec<ThemeEntry>, Vec<String>) {
    let mut blue_by_name: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (name, json) in BUNDLED {
        if let Some(blue) = extract_color(json, "base.blue") {
            blue_by_name.insert(name.to_string(), blue);
        }
        let _ = register_json(cx, json);
    }
    let mut invalid = Vec::new();
    let themes_dir = data_dir.join("themes");
    if let Ok(read) = std::fs::read_dir(&themes_dir) {
        let mut files: Vec<_> = read.flatten().collect();
        files.sort_by_key(|e| e.file_name());
        for entry in files {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy().to_string();
            match std::fs::read_to_string(&path).map_err(|e| e.to_string())
                .and_then(|text| {
                    if let Some(blue) = extract_color(&text, "base.blue") {
                        // A user file may hold several themes; the swatch
                        // blue is per file, which is close enough for a chip.
                        blue_by_name.insert(format!("file:{file_name}"), blue);
                    }
                    register_json(cx, &text).map_err(|e| e.to_string())
                }) {
                Ok(()) => {}
                Err(_) => invalid.push(file_name),
            }
        }
    }
    // User themes registered from a file share that file's blue.
    let file_blues: Vec<String> = blue_by_name
        .iter()
        .filter(|(k, _)| k.starts_with("file:"))
        .map(|(_, v)| v.clone())
        .collect();

    let mut entries = Vec::new();
    for config in ThemeRegistry::global(cx).sorted_themes() {
        let name = config.name.to_string();
        if name.starts_with("Default ") {
            continue; // gpui's built-ins; Lazykasten ships its own default
        }
        let c = &config.colors;
        let pick = |v: &Option<gpui_kit::SharedString>| v.as_ref().map(|s| s.to_string());
        let blue = blue_by_name
            .get(&name)
            .cloned()
            .or_else(|| file_blues.first().cloned())
            .or_else(|| pick(&c.selection));
        let sw = |v: Option<String>| v.map(|s| hex_u32(&s)).unwrap_or(0x888888);
        entries.push(ThemeEntry {
            dark: config.mode.is_dark(),
            swatches: [
                sw(pick(&c.background)),
                sw(pick(&c.foreground)),
                sw(blue),
                sw(pick(&c.border)),
                sw(pick(&c.selection)),
            ],
            name,
        });
    }
    (entries, invalid)
}

/// `#rrggbb` / `#rrggbbaa` -> `(r, g, b)` (alpha ignored for contrast).
pub fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let h = s.strip_prefix('#')?;
    if h.len() != 6 && h.len() != 8 {
        return None;
    }
    Some((
        u8::from_str_radix(&h[0..2], 16).ok()?,
        u8::from_str_radix(&h[2..4], 16).ok()?,
        u8::from_str_radix(&h[4..6], 16).ok()?,
    ))
}

/// WCAG relative luminance of a hex color.
#[allow(dead_code)] // exercised by the readability tests
pub fn luminance(hex: &str) -> f64 {
    let (r, g, b) = parse_hex(hex).unwrap_or((0, 0, 0));
    let f = |v: u8| {
        let v = v as f64 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b)
}

/// WCAG contrast ratio between two hex colors.
#[allow(dead_code)] // exercised by the readability tests
pub fn contrast(a: &str, b: &str) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// Hex string -> `0xrrggbb` for `gpui::rgb(...)` swatches.
pub fn hex_u32(s: &str) -> u32 {
    parse_hex(s).map(|(r, g, b)| ((r as u32) << 16) | ((g as u32) << 8) | b as u32).unwrap_or(0x888888)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED_KEYS: &[&str] = &[
        "background", "foreground", "border", "caret", "selection.background",
        "muted.foreground", "list.hover.background", "list.active.background",
        "list.active.border", "input.border", "popover.background",
        "popover.foreground", "ring", "scrollbar.thumb.background", "base.blue",
    ];

    #[test]
    fn test_parse_settings() {
        assert_eq!(parse_settings("theme = \"Nord\"\n"), Some("Nord".to_string()));
        assert_eq!(parse_settings("other = 1\n"), None);
        assert_eq!(parse_settings("theme = \"\"\n"), None);
    }

    #[test]
    fn test_bundled_themes_parse_and_readability() {
        let mut names: Vec<String> = Vec::new();
        for (entry_name, json) in BUNDLED {
            let value: serde_json::Value = serde_json::from_str(json)
                .unwrap_or_else(|e| panic!("{entry_name}: invalid JSON: {e}"));
            let themes = value["themes"].as_array().expect("themes array");
            assert!(!themes.is_empty(), "{entry_name}: no themes");
            for theme in themes {
                let name = theme["name"].as_str().expect("theme name");
                assert_eq!(name, *entry_name, "BUNDLED name must match the JSON name");
                let mode = theme["mode"].as_str().expect("theme mode");
                assert!(mode == "dark" || mode == "light", "{name}: bad mode {mode}");
                let colors = theme["colors"].as_object().expect("colors object");
                for key in REQUIRED_KEYS {
                    let v = colors.get(*key).and_then(|v| v.as_str())
                        .unwrap_or_else(|| panic!("{name}: missing color {key}"));
                    assert!(parse_hex(v).is_some(), "{name}: {key} not #rrggbb(aa): {v}");
                }
                let bg = colors["background"].as_str().unwrap();
                let fg = colors["foreground"].as_str().unwrap();
                let muted = colors["muted.foreground"].as_str().unwrap();
                let blue = colors["base.blue"].as_str().unwrap();
                assert!(contrast(fg, bg) >= 7.0, "{name}: fg/bg {:.2} < 7", contrast(fg, bg));
                assert!(contrast(muted, bg) >= 4.5, "{name}: muted/bg {:.2} < 4.5", contrast(muted, bg));
                assert!(contrast(blue, bg) >= 4.5, "{name}: blue/bg {:.2} < 4.5", contrast(blue, bg));
                names.push(name.to_string());
            }
        }
        let mut dedup = names.clone();
        dedup.sort();
        dedup.dedup();
        assert_eq!(names.len(), dedup.len(), "theme names must be unique");
        assert!(names.len() >= 24, "expected about 24 bundled themes, got {}", names.len());
    }

    #[test]
    fn test_contrast_helper() {
        assert!((contrast("#000000", "#ffffff") - 21.0).abs() < 0.1);
        assert!((contrast("#ffffff", "#ffffff") - 1.0).abs() < 0.01);
        assert_eq!(parse_hex("#89b4fa"), Some((0x89, 0xb4, 0xfa)));
        assert_eq!(parse_hex("red"), None);
    }
}
