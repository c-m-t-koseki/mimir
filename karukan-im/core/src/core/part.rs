//! Kanji-part input layer: an explicit conversion span in front of the engine.
//!
//! Kana input is plain hiragana, committed as soon as each kana fires. A
//! capital letter (Shift+letter, i.e. SandS) or `q` opens a *kanji part*,
//! which the engine live-converts; a bare Space closes it (commit), after
//! which typing is plain hiragana again. `l` switches to direct ASCII input
//! and Ctrl+J returns to kana, as in SKK.
//!
//! Inside a part, candidate keys are Emacs-style so nothing collides with
//! SandS: Ctrl+N opens the candidate window / next candidate, Ctrl+P the
//! previous one, `a s d f j k l` pick one, Ctrl+G cancels.

use super::engine::{EngineAction, EngineResult, InputMethodEngine};
use super::keycode::{KeyEvent, KeyModifiers, Keysym};
use super::state::InputState;

/// Keys that pick a candidate from the open window, in page order.
pub const SELECT_KEYS: &str = "asdfjkl";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartMode {
    /// Plain hiragana, committed as typed.
    Plain,
    /// Inside a kanji part: the engine composes and live-converts.
    Part,
    /// Direct ASCII input: every key passes through to the application.
    Ascii,
}

pub struct PartLayer {
    mode: PartMode,
}

impl Default for PartLayer {
    fn default() -> Self {
        Self::new()
    }
}

fn merge(mut a: EngineResult, b: EngineResult) -> EngineResult {
    a.consumed |= b.consumed;
    a.actions.extend(b.actions);
    a
}

fn press(keysym: Keysym) -> KeyEvent {
    KeyEvent::press(keysym)
}

fn press_char(ch: char) -> KeyEvent {
    press(Keysym(ch as u32))
}

impl PartLayer {
    pub fn new() -> Self {
        Self {
            mode: PartMode::Plain,
        }
    }

    pub fn mode(&self) -> PartMode {
        self.mode
    }

    /// Set up the engine for plain kana input (live conversion off).
    pub fn attach(&mut self, engine: &mut InputMethodEngine) {
        self.enter(engine, PartMode::Plain);
    }

    fn enter(&mut self, engine: &mut InputMethodEngine, mode: PartMode) {
        self.mode = mode;
        engine.set_live_conversion(mode == PartMode::Part);
    }

    pub fn process_key(&mut self, engine: &mut InputMethodEngine, key: &KeyEvent) -> EngineResult {
        let m = key.modifiers;
        if !key.is_press || key.keysym.is_modifier() || m.alt_key || m.super_key {
            return match self.mode {
                PartMode::Ascii => EngineResult::not_consumed(),
                _ => engine.process_key(key),
            };
        }

        // Ctrl+J / かな: back to kana from anywhere, closing an open part.
        let ctrl_j = m.control_key && matches!(key.keysym, Keysym::KEY_J | Keysym::KEY_J_UPPER);
        if ctrl_j || key.keysym.is_mode_toggle_key() {
            let result = self.close(engine);
            self.enter(engine, PartMode::Plain);
            return merge(EngineResult::consumed(), result);
        }

        match self.mode {
            PartMode::Ascii => EngineResult::not_consumed(),
            PartMode::Plain => self.plain_key(engine, key),
            PartMode::Part => match engine.state() {
                InputState::Conversion { .. } => self.conversion_key(engine, key),
                _ => self.part_key(engine, key),
            },
        }
    }

    /// Focus loss / explicit commit: commit what is open, emitting the same
    /// cleanup actions as the engine's own commit. A part ends with it.
    pub fn flush(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        let result = match (self.mode, engine.state()) {
            (PartMode::Part, _) | (_, InputState::Empty) => engine.commit_result(),
            _ => engine.commit_plain_result(),
        };
        if self.mode == PartMode::Part {
            self.enter(engine, PartMode::Plain);
        }
        result
    }

    /// Commit whatever is open: a part commits its conversion (and learns
    /// it), plain kana commits as typed.
    fn close(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        match (self.mode, engine.state()) {
            (_, InputState::Empty) => EngineResult::consumed(),
            (PartMode::Part, _) => engine.commit_result(),
            _ => engine.commit_plain_result(),
        }
    }

    /// Close what is open and start a new part, feeding `first` (the
    /// lowercased capital) into it.
    fn start_part(&mut self, engine: &mut InputMethodEngine, first: Option<char>) -> EngineResult {
        let result = self.close(engine);
        self.enter(engine, PartMode::Part);
        match first {
            Some(ch) => merge(result, engine.process_key(&press_char(ch))),
            None => result,
        }
    }

    fn switch_to_ascii(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        let result = self.close(engine);
        self.enter(engine, PartMode::Ascii);
        result
    }

    fn plain_key(&mut self, engine: &mut InputMethodEngine, key: &KeyEvent) -> EngineResult {
        if let Some(ch) = typed_char(key) {
            if let Some(first) = part_opener(key, ch) {
                return self.start_part(engine, first);
            }
            if ch == 'l' {
                return self.switch_to_ascii(engine);
            }
            if ch == ' ' && !matches!(engine.state(), InputState::Empty) {
                let result = engine.commit_plain_result();
                return merge(result, engine.process_key(key));
            }
        }
        let result = engine.process_key(key);
        // Plain kana never shows a suggestion window.
        let mut result = without_suggestions(result);
        if matches!(engine.state(), InputState::Composing { .. }) && !engine.has_pending_romaji() {
            result = merge(result, engine.commit_plain_result());
        }
        result
    }

    fn part_key(&mut self, engine: &mut InputMethodEngine, key: &KeyEvent) -> EngineResult {
        let m = key.modifiers;
        if m.control_key {
            return match key.keysym {
                // Ctrl+N: open the candidate window (what Space does upstream).
                Keysym::KEY_N | Keysym::KEY_N_UPPER => {
                    if matches!(engine.state(), InputState::Empty) {
                        EngineResult::consumed()
                    } else {
                        engine.process_key(&press(Keysym::SPACE))
                    }
                }
                Keysym::KEY_G | Keysym::KEY_G_UPPER => self.cancel_part(engine),
                _ => engine.process_key(key),
            };
        }
        let Some(ch) = typed_char(key) else {
            return match key.keysym {
                Keysym::RETURN => {
                    let result = self.close(engine);
                    self.enter(engine, PartMode::Plain);
                    result
                }
                Keysym::ESCAPE => self.cancel_part(engine),
                _ => self.after_edit(engine, key),
            };
        };
        if ch == ' ' {
            let result = self.close(engine);
            self.enter(engine, PartMode::Plain);
            return result;
        }
        if let Some(first) = part_opener(key, ch) {
            return self.start_part(engine, first);
        }
        if ch == 'l' {
            return self.switch_to_ascii(engine);
        }
        if ch == ',' || ch == '.' {
            let result = self.close(engine);
            self.enter(engine, PartMode::Plain);
            return merge(result, self.plain_key(engine, key));
        }
        engine.process_key(key)
    }

    /// Backspace and friends: leaving the part empty drops back to plain.
    fn after_edit(&mut self, engine: &mut InputMethodEngine, key: &KeyEvent) -> EngineResult {
        let result = engine.process_key(key);
        if matches!(engine.state(), InputState::Empty) {
            self.enter(engine, PartMode::Plain);
        }
        result
    }

    fn cancel_part(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        let result = engine.process_key(&press(Keysym::ESCAPE));
        if matches!(engine.state(), InputState::Empty) {
            self.enter(engine, PartMode::Plain);
        }
        result
    }

    fn conversion_key(&mut self, engine: &mut InputMethodEngine, key: &KeyEvent) -> EngineResult {
        let m = key.modifiers;
        if m.control_key {
            return match key.keysym {
                Keysym::KEY_N | Keysym::KEY_N_UPPER => engine.process_key(&press(Keysym::TAB)),
                Keysym::KEY_P | Keysym::KEY_P_UPPER => engine.process_key(&KeyEvent::new(
                    Keysym::TAB,
                    KeyModifiers::new().with_shift(true),
                    true,
                )),
                // Ctrl+G: close the window, back to the part's composition.
                Keysym::KEY_G | Keysym::KEY_G_UPPER => engine.process_key(&press(Keysym::ESCAPE)),
                _ => engine.process_key(key),
            };
        }
        let Some(ch) = typed_char(key) else {
            return match key.keysym {
                Keysym::RETURN => self.commit_conversion(engine),
                _ => engine.process_key(key),
            };
        };
        if let Some(index) = SELECT_KEYS.find(ch).filter(|_| !key.modifiers.shift_key) {
            let result = engine.select_candidate_on_page(index);
            if matches!(engine.state(), InputState::Empty) {
                self.enter(engine, PartMode::Plain);
            }
            return result;
        }
        if ch == ' ' {
            return self.commit_conversion(engine);
        }
        if let Some(first) = part_opener(key, ch) {
            return self.start_part(engine, first);
        }
        // Any other key accepts the selection and keeps typing plain kana.
        let result = self.commit_conversion(engine);
        merge(result, self.process_key(engine, key))
    }

    fn commit_conversion(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        let result = engine.process_key(&press(Keysym::RETURN));
        self.enter(engine, PartMode::Plain);
        result
    }
}

/// The printable character a key types, ignoring Ctrl chords.
fn typed_char(key: &KeyEvent) -> Option<char> {
    if key.modifiers.control_key {
        return None;
    }
    key.to_char()
}

/// `Some(first)` when the key opens a kanji part: a capital (fed lowercased)
/// or `q` (feeds nothing).
fn part_opener(key: &KeyEvent, ch: char) -> Option<Option<char>> {
    if ch.is_ascii_uppercase() || (key.modifiers.shift_key && ch.is_ascii_lowercase()) {
        return Some(Some(ch.to_ascii_lowercase()));
    }
    (ch == 'q').then_some(None)
}

fn without_suggestions(mut result: EngineResult) -> EngineResult {
    result.actions.retain(|a| {
        !matches!(
            a,
            EngineAction::ShowCandidates(_) | EngineAction::UpdateAuxText(_)
        )
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(layer: &mut PartLayer, engine: &mut InputMethodEngine, keys: &str) -> String {
        let mut committed = String::new();
        for ch in keys.chars() {
            let key = if ch.is_ascii_uppercase() {
                KeyEvent::new(
                    Keysym(ch as u32),
                    KeyModifiers::new().with_shift(true),
                    true,
                )
            } else {
                press_char(ch)
            };
            for action in layer.process_key(engine, &key).actions {
                if let EngineAction::Commit(text) = action {
                    committed.push_str(&text);
                }
            }
        }
        committed
    }

    fn setup() -> (PartLayer, InputMethodEngine) {
        let mut engine = InputMethodEngine::new();
        let mut layer = PartLayer::new();
        layer.attach(&mut engine);
        (layer, engine)
    }

    #[test]
    fn plain_kana_commits_as_typed() {
        let (mut layer, mut engine) = setup();
        assert_eq!(feed(&mut layer, &mut engine, "kan"), "か");
        assert!(engine.has_pending_romaji());
        assert_eq!(feed(&mut layer, &mut engine, "ji"), "んじ");
        assert!(matches!(engine.state(), InputState::Empty));
    }

    #[test]
    fn capital_opens_part_and_space_closes_it() {
        let (mut layer, mut engine) = setup();
        // No model loaded: the part commits its reading.
        assert_eq!(feed(&mut layer, &mut engine, "noKaigi"), "の");
        assert_eq!(layer.mode(), PartMode::Part);
        assert_eq!(feed(&mut layer, &mut engine, " "), "かいぎ");
        assert_eq!(layer.mode(), PartMode::Plain);
        assert_eq!(feed(&mut layer, &mut engine, "ha"), "は");
    }

    #[test]
    fn q_opens_part_and_capital_chains_parts() {
        let (mut layer, mut engine) = setup();
        assert_eq!(feed(&mut layer, &mut engine, "qhonjitu"), "");
        assert_eq!(feed(&mut layer, &mut engine, "Kaigi "), "ほんじつかいぎ");
    }

    #[test]
    fn l_goes_ascii_and_ctrl_j_returns() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "l");
        assert_eq!(layer.mode(), PartMode::Ascii);
        assert!(!layer.process_key(&mut engine, &press_char('a')).consumed);
        let ctrl_j = KeyEvent::new(
            Keysym::KEY_J,
            KeyModifiers {
                control_key: true,
                ..Default::default()
            },
            true,
        );
        assert!(layer.process_key(&mut engine, &ctrl_j).consumed);
        assert_eq!(layer.mode(), PartMode::Plain);
        assert_eq!(feed(&mut layer, &mut engine, "a"), "あ");
    }

    #[test]
    fn punctuation_closes_part() {
        let (mut layer, mut engine) = setup();
        assert_eq!(feed(&mut layer, &mut engine, "Kaigi."), "かいぎ。");
        assert_eq!(layer.mode(), PartMode::Plain);
    }
}
