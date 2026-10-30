//! Design tokens (prompt 6, milestone 32): the single source for
//! spacing, radius, chip metrics and font sizes in the UI. Screens read
//! these instead of scattering literal `px()` values.

use gpui_kit::Hsla;

/// WCAG contrast ratio between two HSLA colours (relative luminance).
pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    fn lum(c: Hsla) -> f32 {
        let (r, g, b) = (c.h, c.s, c.l);
        // HSL -> RGB
        let chroma = (1.0 - (2.0 * b - 1.0).abs()) * g;
        let x = chroma * (1.0 - ((r * 6.0) % 2.0 - 1.0).abs());
        let m = b - chroma / 2.0;
        let (mut rr, mut gg, mut bb) = if r < 1.0 / 6.0 {
            (chroma, x, 0.0)
        } else if r < 2.0 / 6.0 {
            (x, chroma, 0.0)
        } else if r < 3.0 / 6.0 {
            (0.0, chroma, x)
        } else if r < 4.0 / 6.0 {
            (0.0, x, chroma)
        } else if r < 5.0 / 6.0 {
            (x, 0.0, chroma)
        } else {
            (chroma, 0.0, x)
        };
        rr += m;
        gg += m;
        bb += m;
        let f = |v: f32| {
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(rr) + 0.7152 * f(gg) + 0.0722 * f(bb)
    }
    let (l1, l2) = (lum(a), lum(b));
    let (hi, lo) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
    (hi + 0.05) / (lo + 0.05)
}

/// Nudge a colour's lightness until it reaches `target` contrast
/// against `bg` (hue and saturation preserved), at most 40 steps.
pub fn ensure_contrast(mut color: Hsla, bg: Hsla, target: f32) -> Hsla {
    if contrast_ratio(color, bg) >= target {
        return color;
    }
    let lighten = {
        let mut probe = color;
        probe.l = (probe.l + 0.02).min(1.0);
        contrast_ratio(probe, bg) > contrast_ratio(color, bg)
    };
    for _ in 0..40 {
        color.l = if lighten {
            (color.l + 0.02).min(1.0)
        } else {
            (color.l - 0.02).max(0.0)
        };
        if contrast_ratio(color, bg) >= target {
            break;
        }
    }
    color
}

/// Stable per-tag colour: golden-angle hue from the tag name hash,
/// nudged to at least 4.5:1 contrast against `bg` (shared by editor
/// chips and graph nodes so a tag looks the same everywhere).
pub fn tag_color(name: &str, bg: Hsla) -> Hsla {
    let mut hash: u32 = 0x811c9dc5;
    for b in name.bytes() {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    let hue = ((hash % 360) as f32) / 360.0;
    ensure_contrast(gpui_kit::hsla(hue, 0.65, 0.55, 1.0), bg, 4.5)
}

#[allow(dead_code)] // a token scale: not every step is used yet
pub mod tokens {
    /// 4pt spacing scale.
    pub const SPACE_1: f32 = 4.0;
    pub const SPACE_2: f32 = 8.0;
    pub const SPACE_3: f32 = 12.0;
    pub const SPACE_4: f32 = 16.0;
    pub const SPACE_5: f32 = 24.0;
    pub const SPACE_6: f32 = 32.0;

    pub const RADIUS_CHIP: f32 = 999.0;
    pub const RADIUS_CARD: f32 = 10.0;
    pub const RADIUS_BUTTON: f32 = 6.0;

    pub const CHIP_PAD_X: f32 = 8.0;
    pub const CHIP_PAD_Y: f32 = 2.0;
    pub const ROW_PAD_X: f32 = 10.0;
    pub const ROW_PAD_Y: f32 = 5.0;

    pub const FONT_EDITOR: f32 = 15.0;
    pub const FONT_LIST: f32 = 14.0;
    pub const FONT_MUTED: f32 = 12.5;
    pub const LINE_HEIGHT: f32 = 1.45;

    pub const FOOTER_H: f32 = 34.0;
    pub const EDITOR_MAX_W: f32 = 760.0;
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::hsla;

    #[test]
    fn contrast_math() {
        let black = hsla(0.0, 0.0, 0.0, 1.0);
        let white = hsla(0.0, 0.0, 1.0, 1.0);
        assert!((contrast_ratio(black, white) - 21.0).abs() < 0.1);
        let fixed = ensure_contrast(hsla(0.6, 0.5, 0.45, 1.0), white, 4.5);
        assert!(contrast_ratio(fixed, white) >= 4.4, "{}", contrast_ratio(fixed, white));
    }
}
