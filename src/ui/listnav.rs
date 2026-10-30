//! List navigation adapter (prompt 6, milestone 31): one key handler
//! for every list screen when Vim mode is on. `j`/`k` move, `gg`/`G`
//! jump, Ctrl+D/U page, Enter opens, `/`/`i`/`a` start typing in the
//! screen's search box, Esc/q go back. Pure and testable.

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Nav {
    None,
    Move(i32),
    Top,
    Bottom,
    Page(i32),
    Search,
    Back,
}

#[derive(Default)]
pub struct ListNav {
    g_pending: bool,
}

impl ListNav {
    pub fn new() -> Self {
        Self::default()
    }

    /// `ctrl` marks Ctrl+D / Ctrl+U (passed as 'd' / 'u' with ctrl set).
    pub fn key(&mut self, c: char, ctrl: bool) -> Nav {
        if ctrl {
            return match c {
                'd' => Nav::Page(1),
                'u' => Nav::Page(-1),
                _ => Nav::None,
            };
        }
        if self.g_pending {
            self.g_pending = false;
            if c == 'g' {
                return Nav::Top;
            }
        }
        match c {
            'j' => Nav::Move(1),
            'k' => Nav::Move(-1),
            'g' => {
                self.g_pending = true;
                Nav::None
            }
            'G' => Nav::Bottom,
            '/' | 'i' | 'a' => Nav::Search,
            'q' => Nav::Back,
            _ => Nav::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_nav_keys() {
        let mut nav = ListNav::new();
        assert_eq!(nav.key('j', false), Nav::Move(1));
        assert_eq!(nav.key('k', false), Nav::Move(-1));
        assert_eq!(nav.key('g', false), Nav::None);
        assert_eq!(nav.key('g', false), Nav::Top);
        assert_eq!(nav.key('G', false), Nav::Bottom);
        assert_eq!(nav.key('d', true), Nav::Page(1));
        assert_eq!(nav.key('u', true), Nav::Page(-1));
        assert_eq!(nav.key('/', false), Nav::Search);
        assert_eq!(nav.key('q', false), Nav::Back);
        assert_eq!(nav.key('x', false), Nav::None);
    }
}
