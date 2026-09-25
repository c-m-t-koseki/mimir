//! The part layer's key bindings (`[keys]` in config.toml): which keys do
//! what, written Emacs-style — `C-n` (Ctrl+N), `S-Tab` (Shift+Tab), named
//! keys (`Space`, `Return`, `Escape`, `Tab`, `Left`, …) or a character
//! (`q`, `/`). Each action takes any number of them, space-separated.

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::keycode::{KeyEvent, Keysym};

/// `[keys]` as written.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KeysSettings {
    /// Back to kana input
    pub kana: String,
    /// Direct ASCII input
    pub ascii: String,
    /// SKK's abbrev input
    pub abbrev: String,
    /// Drop what is being typed
    pub discard: String,
    /// Open the candidate window
    pub convert: String,
    /// Next / previous candidate in the window
    pub next: String,
    pub prev: String,
    /// Cancel: the window, then the part
    pub cancel: String,
    /// Register a word
    pub register: String,
    /// Complete from earlier input
    pub complete: String,
    /// Commit the reading in katakana
    pub katakana: String,
    /// Close the part (its left side when split), typing goes on
    pub close: String,
    /// Commit everything
    pub commit: String,
    /// Move where the part's conversion stops
    pub left: String,
    pub right: String,
    pub home: String,
    pub end: String,
    /// Commit everything, when nothing is right of the caret
    pub end_commit: String,
    /// The keys picking a candidate from the window, in page order
    pub select: String,
}

impl KeysSettings {
    /// Every binding parses and the select keys fill a page.
    pub fn validate(&self) -> Result<()> {
        Keymap::parse(self).map(|_| ())
    }
}

/// One key: a keysym (letters lowercase) and the modifiers it needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeySpec {
    keysym: Keysym,
    ctrl: bool,
    shift: bool,
}

impl KeySpec {
    pub fn parse(spec: &str) -> Result<Self> {
        let mut rest = spec;
        let (mut ctrl, mut shift) = (false, false);
        loop {
            if let Some(r) = rest.strip_prefix("C-").filter(|r| !r.is_empty()) {
                ctrl = true;
                rest = r;
            } else if let Some(r) = rest.strip_prefix("S-").filter(|r| !r.is_empty()) {
                shift = true;
                rest = r;
            } else {
                break;
            }
        }
        let keysym = match rest {
            "Space" => Keysym::SPACE,
            "Tab" => Keysym::TAB,
            "Return" => Keysym::RETURN,
            "Escape" => Keysym::ESCAPE,
            "BackSpace" => Keysym::BACKSPACE,
            "Delete" => Keysym::DELETE,
            "Left" => Keysym::LEFT,
            "Right" => Keysym::RIGHT,
            "Up" => Keysym::UP,
            "Down" => Keysym::DOWN,
            "Home" => Keysym::HOME,
            "End" => Keysym::END,
            "PageUp" => Keysym::PAGE_UP,
            "PageDown" => Keysym::PAGE_DOWN,
            _ => {
                let mut chars = rest.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if c.is_ascii_graphic() => {
                        Keysym(c.to_ascii_lowercase() as u32)
                    }
                    _ => bail!("unknown key: {spec:?}"),
                }
            }
        };
        Ok(Self {
            keysym,
            ctrl,
            shift,
        })
    }

    fn matches(&self, key: &KeyEvent) -> bool {
        let m = key.modifiers;
        if m.control_key != self.ctrl || m.alt_key || m.super_key {
            return false;
        }
        let keysym = if key.keysym == Keysym::ISO_LEFT_TAB {
            Keysym::TAB
        } else {
            key.keysym
        };
        let shifted = m.shift_key || key.keysym == Keysym::ISO_LEFT_TAB;
        match char::from_u32(keysym.0).filter(char::is_ascii_graphic) {
            // A character carries its own shift (`?`, `A`); a letter binds
            // either case unless Shift is asked for.
            Some(c) => {
                let lower = c.to_ascii_lowercase() as u32;
                lower == self.keysym.0
                    && (!self.shift || shifted || c.is_ascii_uppercase())
                    && (self.shift || self.ctrl || !c.is_ascii_uppercase())
            }
            None => keysym == self.keysym && shifted == self.shift,
        }
    }
}

/// A set of keys bound to one action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keys(Vec<KeySpec>);

impl Keys {
    fn parse(specs: &str) -> Result<Self> {
        specs
            .split_whitespace()
            .map(KeySpec::parse)
            .collect::<Result<_>>()
            .map(Self)
    }

    pub fn hit(&self, key: &KeyEvent) -> bool {
        self.0.iter().any(|spec| spec.matches(key))
    }
}

/// `[keys]`, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    pub kana: Keys,
    pub ascii: Keys,
    pub abbrev: Keys,
    pub discard: Keys,
    pub convert: Keys,
    pub next: Keys,
    pub prev: Keys,
    pub cancel: Keys,
    pub register: Keys,
    pub complete: Keys,
    pub katakana: Keys,
    pub close: Keys,
    pub commit: Keys,
    pub left: Keys,
    pub right: Keys,
    pub home: Keys,
    pub end: Keys,
    pub end_commit: Keys,
    pub select: Vec<char>,
}

impl Keymap {
    /// The keys picking a candidate: exactly one page's worth.
    pub const SELECT_COUNT: usize = super::candidate::CandidateList::DEFAULT_PAGE_SIZE;

    pub fn parse(s: &KeysSettings) -> Result<Self> {
        let select: Vec<char> = s.select.chars().collect();
        let mut unique = select.clone();
        unique.sort_unstable();
        unique.dedup();
        if select.len() != Self::SELECT_COUNT || unique.len() != select.len() {
            bail!(
                "select: {} different keys wanted, got {:?}",
                Self::SELECT_COUNT,
                s.select
            );
        }
        Ok(Self {
            kana: Keys::parse(&s.kana)?,
            ascii: Keys::parse(&s.ascii)?,
            abbrev: Keys::parse(&s.abbrev)?,
            discard: Keys::parse(&s.discard)?,
            convert: Keys::parse(&s.convert)?,
            next: Keys::parse(&s.next)?,
            prev: Keys::parse(&s.prev)?,
            cancel: Keys::parse(&s.cancel)?,
            register: Keys::parse(&s.register)?,
            complete: Keys::parse(&s.complete)?,
            katakana: Keys::parse(&s.katakana)?,
            close: Keys::parse(&s.close)?,
            commit: Keys::parse(&s.commit)?,
            left: Keys::parse(&s.left)?,
            right: Keys::parse(&s.right)?,
            home: Keys::parse(&s.home)?,
            end: Keys::parse(&s.end)?,
            end_commit: Keys::parse(&s.end_commit)?,
            select,
        })
    }

    /// Whether any action is bound to `key`.
    pub fn binds(&self, key: &KeyEvent) -> bool {
        [
            &self.kana,
            &self.ascii,
            &self.abbrev,
            &self.discard,
            &self.convert,
            &self.next,
            &self.prev,
            &self.cancel,
            &self.register,
            &self.complete,
            &self.katakana,
            &self.close,
            &self.commit,
            &self.left,
            &self.right,
            &self.home,
            &self.end,
            &self.end_commit,
        ]
        .iter()
        .any(|keys| keys.hit(key))
    }

    /// The page position `ch` picks.
    pub fn select_index(&self, ch: char) -> Option<usize> {
        self.select.iter().position(|&c| c == ch)
    }
}

impl Default for Keymap {
    /// The shipped `[keys]`.
    fn default() -> Self {
        let settings = crate::config::Settings::default_keys();
        Self::parse(&settings).expect("shipped [keys] parse")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::keycode::KeyModifiers;

    fn key(keysym: u32, ctrl: bool, shift: bool) -> KeyEvent {
        KeyEvent::new(
            Keysym(keysym),
            KeyModifiers::new().with_control(ctrl).with_shift(shift),
            true,
        )
    }

    #[test]
    fn specs_match_their_keys() {
        let c_n = KeySpec::parse("C-n").unwrap();
        assert!(c_n.matches(&key('n' as u32, true, false)));
        assert!(c_n.matches(&key('N' as u32, true, false)));
        assert!(!c_n.matches(&key('n' as u32, false, false)));
        let s_tab = KeySpec::parse("S-Tab").unwrap();
        assert!(s_tab.matches(&key(0xff09, false, true)));
        assert!(s_tab.matches(&key(0xfe20, false, false)));
        assert!(!s_tab.matches(&key(0xff09, false, false)));
        let q = KeySpec::parse("q").unwrap();
        assert!(q.matches(&key('q' as u32, false, false)));
        assert!(!q.matches(&key('Q' as u32, false, true)));
        assert!(
            KeySpec::parse("/")
                .unwrap()
                .matches(&key('/' as u32, false, false))
        );
        assert!(
            KeySpec::parse("Space")
                .unwrap()
                .matches(&key(0x20, false, false))
        );
        assert!(KeySpec::parse("Hyper").is_err());
    }

    #[test]
    fn the_shipped_keys_parse_and_bad_select_keys_are_refused() {
        let mut settings = crate::config::Settings::default_keys();
        assert!(settings.validate().is_ok());
        settings.select = "jkf".into();
        assert!(settings.validate().is_err());
        settings.select = "jjfduire".into();
        assert!(settings.validate().is_err());
    }
}
