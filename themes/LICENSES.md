# Bundled theme palettes

Each JSON in this folder maps a published palette onto Lazykasten's theme
keys by hand (no theme files were copied; the file format is
gpui-component's `ThemeSet`, not any editor's theme format). Palettes are
MIT-licensed unless noted; keep this list with the themes.

| Theme | Palette source | License |
|---|---|---|
| Catppuccin Mocha / Macchiato / Frappe / Latte | Catppuccin palette (catppuccin org) | MIT |
| Gruvbox Dark / Light | Gruvbox by Pavel Pertsev (morhetz) | MIT |
| Tokyo Night / Tokyo Night Storm | Tokyo Night palette (tokyo-night org) | MIT |
| Nord | Nord by Arctic Ice Studio (nordtheme) | MIT |
| Dracula | Dracula palette (dracula org) | MIT |
| One Dark | Atom One Dark palette | MIT |
| Solarized Dark / Light | Solarized by Ethan Schoonover | MIT |
| Rose Pine / Rose Pine Dawn | Rose Pine (rose-pine org) | MIT |
| Kanagawa | Kanagawa by Tommaso Laurenzi (rebelot) | MIT |
| Everforest Dark / Light | Everforest by Sainnhe Park | MIT |
| Ayu Dark / Mirage / Light | Ayu by Ike Ku (ayu-theme) | MIT |
| GitHub Dark / Light | GitHub Primer primitives | MIT |
| Monokai | Monokai palette (classic, public spec) | MIT-style |
| Lazykasten Dark | Lazykasten default (this project) | MIT |

Where a palette's comment/muted color did not reach WCAG contrast on its
own background (4.5:1 for muted and blue, 7:1 for foreground), the mapped
value was nudged toward white or black until it did. The readability tests
in `src/themes.rs` enforce this for every bundled theme.
