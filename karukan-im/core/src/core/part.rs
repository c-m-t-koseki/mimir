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
//!
//! Ctrl+R inside a part registers a word: the part's reading is kept, the
//! word is typed with the same keys (its commits are captured instead of
//! reaching the application), and Enter saves it — to the learning cache
//! for immediate effect and to `user_dicts/registered.tsv` for good.

use super::engine::{EngineAction, EngineResult, InputMethodEngine};
use super::keycode::{KeyEvent, KeyModifiers, Keysym};
use super::preedit::Preedit;
use super::state::InputState;
use crate::config::Settings;

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
    /// Word registration in progress: the reading and the word so far.
    register: Option<Registration>,
}

struct Registration {
    reading: String,
    word: String,
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
            register: None,
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
        let key = &emacs_alias(key);
        if self.register.is_some() && key.is_press && !key.keysym.is_modifier() {
            return self.registration_key(engine, key);
        }
        let result = self.dispatch(engine, key);
        plain_aux(engine, result)
    }

    /// The kana under the live conversion, for the frontend to show beside
    /// it; `None` outside a part or when the preedit already is that kana.
    pub fn live_reading(&self, engine: &InputMethodEngine) -> Option<String> {
        if self.mode != PartMode::Part || self.register.is_some() {
            return None;
        }
        let kana = engine.composing_kana()?;
        let shown = engine.preedit().map(|p| p.text().to_string());
        (shown.as_deref() != Some(kana.as_str())).then_some(kana)
    }

    fn dispatch(&mut self, engine: &mut InputMethodEngine, key: &KeyEvent) -> EngineResult {
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
                Keysym::KEY_R | Keysym::KEY_R_UPPER => self.start_registration(engine),
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
            // A capital in the middle of a syllable (`s` + `E`) is SandS
            // rolling over — Space still held when the next key lands — not
            // a new part: nothing ever ends a part on an unfired consonant.
            if engine.has_pending_romaji()
                && let Some(ch) = first
            {
                return engine.process_key(&press_char(ch));
            }
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
                Keysym::KEY_R | Keysym::KEY_R_UPPER => self.start_registration(engine),
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

    fn start_registration(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        let reading = engine.take_reading();
        self.enter(engine, PartMode::Plain);
        if reading.is_empty() {
            return EngineResult::consumed();
        }
        self.register = Some(Registration {
            reading,
            word: String::new(),
        });
        self.registration_result(engine, EngineResult::consumed())
    }

    /// Keys while registering. With nothing composing, Enter saves, Ctrl+G /
    /// Escape abort and Backspace edits the word; everything else types the
    /// word through the ordinary layer, its commits captured.
    fn registration_key(&mut self, engine: &mut InputMethodEngine, key: &KeyEvent) -> EngineResult {
        let idle = matches!(engine.state(), InputState::Empty);
        let ctrl = key.modifiers.control_key;
        let reg = self.register.as_mut().expect("registering");
        if idle && self.mode != PartMode::Part {
            let abort = key.keysym == Keysym::ESCAPE
                || (ctrl && matches!(key.keysym, Keysym::KEY_G | Keysym::KEY_G_UPPER));
            if abort {
                self.register = None;
                return EngineResult::consumed()
                    .with_action(EngineAction::UpdatePreedit(Preedit::new()));
            }
            if key.keysym == Keysym::RETURN {
                let reg = self.register.take().expect("registering");
                if reg.word.is_empty() {
                    return EngineResult::consumed()
                        .with_action(EngineAction::UpdatePreedit(Preedit::new()));
                }
                engine.learn(&reg.reading, &reg.word);
                save_registered(&reg.reading, &reg.word);
                return EngineResult::consumed().with_action(EngineAction::Commit(reg.word));
            }
            if key.keysym == Keysym::BACKSPACE {
                reg.word.pop();
                return self.registration_result(engine, EngineResult::consumed());
            }
        }
        if self.mode == PartMode::Ascii
            && let Some(ch) = typed_char(key)
        {
            reg.word.push(ch);
            return self.registration_result(engine, EngineResult::consumed());
        }
        let result = self.dispatch(engine, key);
        let mut result = self.registration_result(engine, result);
        result.consumed = true;
        result
    }

    /// Fold the layer's output into the registration: commits go to the
    /// word, and the preedit shows 【reading】word + what is composing.
    fn registration_result(
        &mut self,
        engine: &InputMethodEngine,
        result: EngineResult,
    ) -> EngineResult {
        let reg = self.register.as_mut().expect("registering");
        let mut out = EngineResult {
            consumed: result.consumed,
            actions: Vec::new(),
        };
        for action in result.actions {
            match action {
                EngineAction::Commit(text) => reg.word.push_str(&text),
                EngineAction::UpdatePreedit(_) => {}
                other => out.actions.push(other),
            }
        }
        let composing = engine
            .preedit()
            .map(|p| p.text().to_string())
            .unwrap_or_default();
        let text = format!("【{}】{}{}", reg.reading, reg.word, composing);
        let caret = text.chars().count();
        let mut preedit = Preedit::with_text_underlined(text);
        preedit.set_caret(caret);
        out.actions.push(EngineAction::UpdatePreedit(preedit));
        out
    }

    fn commit_conversion(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        let result = engine.process_key(&press(Keysym::RETURN));
        self.enter(engine, PartMode::Plain);
        result
    }
}

/// Append a registered word to `user_dicts/registered.tsv` (Mozc TSV), read
/// at the next start. Failure only loses persistence: the learning cache
/// already has the word.
fn save_registered(reading: &str, word: &str) {
    use std::io::Write;
    // Tests must never write into the real data directory.
    if cfg!(test) {
        return;
    }
    let Some(dir) = Settings::user_dict_dir() else {
        return;
    };
    let line = format!("{reading}\t{word}\t名詞\tkarukan登録\n");
    let written = std::fs::create_dir_all(&dir).and_then(|_| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("registered.tsv"))?
            .write_all(line.as_bytes())
    });
    if let Err(e) = written {
        tracing::warn!("failed to save registered word: {e}");
    }
}

/// Ctrl+H is Backspace and Ctrl+M is Enter, as in SKK and the Cocoa text
/// system. Unmapped, the engine would pass Ctrl+H to the application, which
/// deletes inside the marked text behind the engine's back — the next
/// keystroke then redraws the preedit and the deleted chars come back.
fn emacs_alias(key: &KeyEvent) -> KeyEvent {
    let m = key.modifiers;
    if !m.control_key || m.alt_key || m.super_key {
        return key.clone();
    }
    let keysym = match key.keysym.0 {
        0x68 | 0x48 => Keysym::BACKSPACE,
        0x6d | 0x4d => Keysym::RETURN,
        _ => return key.clone(),
    };
    KeyEvent::new(keysym, KeyModifiers::default(), key.is_press)
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

/// The engine's aux line carries mode and source markers (`[あ][変換:🤖]`,
/// chunk counters); the candidate window shows just the reading instead.
fn plain_aux(engine: &InputMethodEngine, mut result: EngineResult) -> EngineResult {
    let had_aux = result.actions.iter().any(|a| {
        matches!(
            a,
            EngineAction::UpdateAuxText(_) | EngineAction::HideAuxText
        )
    });
    result.actions.retain(|a| {
        !matches!(
            a,
            EngineAction::UpdateAuxText(_) | EngineAction::HideAuxText
        )
    });
    if had_aux {
        result.actions.push(match engine.state() {
            InputState::Conversion { reading, .. } => EngineAction::UpdateAuxText(reading.clone()),
            _ => EngineAction::HideAuxText,
        });
    }
    result
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
    fn ctrl_r_registers_a_word() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Karukan");
        let ctrl_r = KeyEvent::new(
            Keysym::KEY_R,
            KeyModifiers {
                control_key: true,
                ..Default::default()
            },
            true,
        );
        layer.process_key(&mut engine, &ctrl_r);
        // The word is typed with the usual keys; nothing reaches the app.
        assert_eq!(feed(&mut layer, &mut engine, "lkarukan"), "");
        let ctrl_j = KeyEvent::new(
            Keysym::KEY_J,
            KeyModifiers {
                control_key: true,
                ..Default::default()
            },
            true,
        );
        layer.process_key(&mut engine, &ctrl_j);
        assert_eq!(feed(&mut layer, &mut engine, "Ime "), "");
        let enter = layer.process_key(&mut engine, &press(Keysym::RETURN));
        assert!(matches!(
            enter.actions.as_slice(),
            [EngineAction::Commit(word)] if word == "karukanいめ"
        ));
    }

    fn ctrl(ch: char) -> KeyEvent {
        KeyEvent::new(
            Keysym(ch as u32),
            KeyModifiers {
                control_key: true,
                ..Default::default()
            },
            true,
        )
    }

    #[test]
    fn sands_rollover_capital_continues_the_syllable() {
        let (mut layer, mut engine) = setup();
        // Space+s, then `e` while Space is still down: `SE`, meant `Se`.
        assert_eq!(feed(&mut layer, &mut engine, "SE"), "");
        assert_eq!(engine.preedit().unwrap().text(), "せ");
        // A capital after a whole kana still opens the next part.
        assert_eq!(feed(&mut layer, &mut engine, "Ki "), "せき");
    }

    #[test]
    fn ctrl_h_deletes_inside_the_part() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Kaigi");
        let result = layer.process_key(&mut engine, &ctrl('h'));
        assert!(result.consumed);
        assert_eq!(engine.preedit().unwrap().text(), "かい");
        // Plain mode with nothing composing leaves Ctrl+H to the app.
        feed(&mut layer, &mut engine, " ");
        assert!(!layer.process_key(&mut engine, &ctrl('h')).consumed);
    }

    #[test]
    fn conversion_aux_is_just_the_reading() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Kaigi");
        let result = layer.process_key(&mut engine, &ctrl('n'));
        let aux: Vec<_> = result
            .actions
            .iter()
            .filter_map(|a| match a {
                EngineAction::UpdateAuxText(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(aux, ["かいぎ"]);
        // No model: the preedit is the kana itself, nothing to show beside it.
        layer.process_key(&mut engine, &ctrl('g'));
        assert_eq!(layer.live_reading(&engine), None);
    }

    #[test]
    fn punctuation_closes_part() {
        let (mut layer, mut engine) = setup();
        assert_eq!(feed(&mut layer, &mut engine, "Kaigi."), "かいぎ。");
        assert_eq!(layer.mode(), PartMode::Plain);
    }
}
