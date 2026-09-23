//! Kanji-part input layer: an explicit conversion span in front of the engine.
//!
//! In SKK style (`[conversion] skk_style`), kana input is plain hiragana,
//! committed as soon as each kana fires, and a capital letter (Shift+letter,
//! i.e. SandS) opens a *kanji part*; otherwise any letter opens one. The
//! engine live-converts the part; a bare Space closes it (commit), after
//! which typing goes on as before. `l` switches to direct ASCII input
//! and Ctrl+J returns to kana, as in SKK.
//!
//! `q` inside a part commits its reading in katakana, as in SKK.
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
use crate::config::settings::PartHint;

/// Longest committed text a one-char part is joined to for learning.
const RECENT_MAX_CHARS: usize = 12;

/// Completions listed in the hint panel.
const HINT_COMPLETIONS: usize = 3;

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
    /// What [`Self::hint`] reports while a part is typed.
    hint: PartHint,
    /// SKK style: only a capital opens a part. The layer's own default
    /// (its tests are written for it); `[conversion] skk_style` sets it.
    skk_style: bool,
    /// Word registration in progress: the reading and the word so far.
    register: Option<Registration>,
    /// What was committed just before, `(reading, text)`: the last part
    /// plus the plain kana typed after it. A one-char part (て, 中) is no
    /// word on its own and is learned joined to this.
    recent: Option<(String, String)>,
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
            hint: PartHint::default(),
            skk_style: true,
            register: None,
            recent: None,
        }
    }

    pub fn with_hint(hint: PartHint) -> Self {
        Self {
            hint,
            ..Self::new()
        }
    }

    pub fn with_skk_style(self, skk_style: bool) -> Self {
        Self { skk_style, ..self }
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
        let composing = !matches!(engine.state(), InputState::Empty);
        let ctrl = key.is_press && key.modifiers.control_key && !key.modifiers.alt_key;
        // Nothing composing: every Ctrl chord but our own Ctrl+J belongs to
        // the application (Ctrl+U kills the line, Ctrl+K kills to its end…),
        // so it never reaches the engine and its bindings.
        let ctrl_j = matches!(key.keysym, Keysym::KEY_J | Keysym::KEY_J_UPPER);
        if !composing && ctrl && !ctrl_j {
            return EngineResult::not_consumed();
        }
        if composing && ctrl && matches!(key.keysym, Keysym::KEY_U | Keysym::KEY_U_UPPER) {
            return self.discard(engine);
        }
        let result = self.dispatch(engine, key);
        let mut result = plain_aux(engine, result);
        // A Ctrl chord the engine leaves unbound must not reach the
        // application mid-composition: its text edit (Ctrl+K, Ctrl+W, …)
        // would act on the marked text behind the engine's back, and the
        // next keystroke would redraw what it deleted.
        if composing && ctrl && self.mode != PartMode::Ascii {
            result.consumed = true;
        }
        result
    }

    /// Ctrl+U: drop what is being composed (readline's kill-line), part
    /// and all, back to plain kana.
    fn discard(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        engine.take_reading();
        if self.mode == PartMode::Part {
            self.enter(engine, PartMode::Plain);
        }
        EngineResult::consumed()
            .with_action(EngineAction::UpdatePreedit(Preedit::new()))
            .with_action(EngineAction::HideCandidates)
            .with_action(EngineAction::HideAuxText)
    }

    /// Text for the small panel under an open part (`[display] part_hint`):
    /// earlier-committed words the reading completes to, or the kana under
    /// the live conversion when it differs from the preedit.
    pub fn hint(&self, engine: &InputMethodEngine) -> Option<String> {
        if self.mode != PartMode::Part || self.register.is_some() {
            return None;
        }
        match self.hint {
            PartHint::None => None,
            PartHint::Completion => {
                let words = engine.completions(HINT_COMPLETIONS);
                (!words.is_empty()).then(|| words.join("  "))
            }
            PartHint::Reading => {
                let kana = engine.composing_kana()?;
                let shown = engine.preedit().map(|p| p.text().to_string());
                (shown.as_deref() != Some(kana.as_str())).then_some(kana)
            }
        }
    }

    fn dispatch(&mut self, engine: &mut InputMethodEngine, key: &KeyEvent) -> EngineResult {
        let m = key.modifiers;
        // Mode keys first: かな arrives as Super_R, a modifier keysym, and
        // must work from ASCII mode too, where every other key passes by.
        if key.is_press {
            // 英数: direct ASCII input, closing what is open (`l` does the same
            // from kana input; かな / Ctrl+J come back).
            if key.keysym == Keysym::EISU_TOGGLE {
                let result = self.close(engine);
                self.enter(engine, PartMode::Ascii);
                return merge(EngineResult::consumed(), result);
            }

            // Ctrl+J / かな: back to kana from anywhere, closing an open part.
            let ctrl_j = m.control_key && matches!(key.keysym, Keysym::KEY_J | Keysym::KEY_J_UPPER);
            if ctrl_j || key.keysym.is_mode_toggle_key() {
                let result = self.close(engine);
                self.enter(engine, PartMode::Plain);
                // Also leave the engine's own katakana mode (Ctrl+K).
                engine.process_key(&press(Keysym::SUPER_R));
                return merge(EngineResult::consumed(), result);
            }
        }

        if !key.is_press || key.keysym.is_modifier() || m.alt_key || m.super_key {
            return match self.mode {
                PartMode::Ascii => EngineResult::not_consumed(),
                _ => engine.process_key(key),
            };
        }

        // No katakana mode: Ctrl+K is the application's (kill-line) unless
        // something is composing.
        if m.control_key && matches!(key.keysym, Keysym::KEY_K | Keysym::KEY_K_UPPER) {
            return match (self.mode, engine.state()) {
                (PartMode::Ascii, _) | (_, InputState::Empty) => EngineResult::not_consumed(),
                _ => EngineResult::consumed(),
            };
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
            (PartMode::Part, InputState::Composing { .. }) => {
                engine.settle_pending();
                self.learn_part(engine);
                engine.commit_unlearned_result()
            }
            (PartMode::Part, _) => {
                self.recent = None;
                engine.commit_result()
            }
            _ => {
                let result = engine.commit_plain_result();
                self.note_plain_commits(&result);
                result
            }
        }
    }

    /// Learn the part about to be committed. A one-char result (て, 中) is
    /// no word of its own: it is learned joined to what was committed just
    /// before it (入力 + 中 → にゅうりょくちゅう→入力中), or not at all.
    fn learn_part(&mut self, engine: &mut InputMethodEngine) {
        let Some((reading, text)) = engine.composing_commit_preview() else {
            return;
        };
        if text.is_empty() {
            return;
        }
        let (reading, text) = if text.chars().count() == 1 {
            let Some((prev_reading, prev_text)) = self.recent.take() else {
                return;
            };
            let joined = (
                format!("{prev_reading}{reading}"),
                format!("{prev_text}{text}"),
            );
            engine.learn_after(&joined.0, &joined.1, &prev_text);
            joined
        } else {
            engine.learn(&reading, &text);
            (reading, text)
        };
        self.recent = (text.chars().count() <= RECENT_MAX_CHARS).then_some((reading, text));
    }

    /// Plain kana committed after a part extends `recent` (会議 + の); any
    /// other text — punctuation, ASCII — ends it.
    fn note_plain_commits(&mut self, result: &EngineResult) {
        for action in &result.actions {
            let EngineAction::Commit(text) = action else {
                continue;
            };
            self.recent = match self.recent.take() {
                Some((r, t))
                    if text.chars().all(|c| matches!(c, 'ぁ'..='ゖ' | 'ー'))
                        && t.chars().count() + text.chars().count() <= RECENT_MAX_CHARS =>
                {
                    Some((r + text, t + text))
                }
                _ => None,
            };
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
            if z_sequence(engine, ch) {
                let result = without_suggestions(engine.feed_romaji(ch));
                let result = merge(result, engine.commit_plain_result());
                self.note_plain_commits(&result);
                return result;
            }
            if let Some(first) = part_opener(key, ch) {
                return self.start_part(engine, first);
            }
            if ch == 'l' {
                return self.switch_to_ascii(engine);
            }
            if !self.skk_style && ch.is_ascii_lowercase() {
                return self.start_part(engine, Some(ch));
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
        self.note_plain_commits(&result);
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
                Keysym::KEY_I | Keysym::KEY_I_UPPER => engine.open_completions(),
                _ => engine.process_key(key),
            };
        }
        let Some(ch) = typed_char(key) else {
            return match key.keysym {
                // Tab and Ctrl+I are the same key (and the same thing):
                // complete from what was typed before.
                Keysym::TAB => engine.open_completions(),
                Keysym::RETURN => {
                    let result = self.close(engine);
                    self.enter(engine, PartMode::Plain);
                    result
                }
                Keysym::ESCAPE => self.cancel_part(engine),
                _ => self.after_edit(engine, key),
            };
        };
        if z_sequence(engine, ch) {
            return engine.feed_romaji(ch);
        }
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
        if ch == 'q' && !matches!(engine.state(), InputState::Empty) {
            return self.commit_katakana(engine);
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
                Keysym::KEY_N | Keysym::KEY_N_UPPER | Keysym::KEY_I | Keysym::KEY_I_UPPER => {
                    engine.process_key(&press(Keysym::TAB))
                }
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
        if ch == 'q' {
            return self.commit_katakana(engine);
        }
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

    /// `q`: commit the part's reading in katakana, and learn it so the live
    /// conversion offers it next time (てすと → テスト).
    fn commit_katakana(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        engine.settle_pending();
        let reading = engine.take_reading();
        let text = mimir_engine::hiragana_to_katakana(&reading);
        engine.learn(&reading, &text);
        self.recent = Some((reading, text.clone()));
        self.enter(engine, PartMode::Plain);
        EngineResult::consumed()
            .with_action(EngineAction::Commit(text))
            .with_action(EngineAction::HideCandidates)
            .with_action(EngineAction::HideAuxText)
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
    let line = format!("{reading}\t{word}\t名詞\tmimir登録\n");
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

/// A key the layer would take as a command (`l`, Space, `,`, `.`) that
/// instead completes a pending `z` sequence: zl → →, z + Space → 　, ….
fn z_sequence(engine: &InputMethodEngine, ch: char) -> bool {
    matches!(ch, 'l' | ' ' | ',' | '.') && engine.pending_romaji() == "z"
}

/// `Some(first)` when the key opens a kanji part: a capital, fed lowercased.
fn part_opener(key: &KeyEvent, ch: char) -> Option<Option<char>> {
    (ch.is_ascii_uppercase() || (key.modifiers.shift_key && ch.is_ascii_lowercase()))
        .then(|| Some(ch.to_ascii_lowercase()))
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

    fn engine_learning_on(engine: &mut InputMethodEngine) {
        engine.enable_learning_for_test();
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
    fn z_sequences_beat_mode_keys() {
        let (mut layer, mut engine) = setup();
        assert_eq!(feed(&mut layer, &mut engine, "zl"), "→");
        assert_eq!(feed(&mut layer, &mut engine, "z "), "\u{3000}");
        assert_eq!(feed(&mut layer, &mut engine, "z."), "…");
        assert_eq!(layer.mode(), PartMode::Plain);
        assert_eq!(feed(&mut layer, &mut engine, "Kazl"), "");
        assert_eq!(layer.mode(), PartMode::Part);
        assert_eq!(feed(&mut layer, &mut engine, " "), "か→");
    }

    #[test]
    fn without_skk_style_any_letter_opens_a_part() {
        let (layer, mut engine) = setup();
        let mut layer = layer.with_skk_style(false);
        assert_eq!(feed(&mut layer, &mut engine, "kaigi"), "");
        assert_eq!(layer.mode(), PartMode::Part);
        assert_eq!(feed(&mut layer, &mut engine, " "), "かいぎ");
        // After the part closes, the next letter opens another.
        assert_eq!(feed(&mut layer, &mut engine, "ha,"), "は、");
        assert_eq!(feed(&mut layer, &mut engine, "l"), "");
        assert_eq!(layer.mode(), PartMode::Ascii);
    }

    #[test]
    fn q_commits_the_part_in_katakana() {
        let (mut layer, mut engine) = setup();
        assert_eq!(feed(&mut layer, &mut engine, "noTesutoq"), "のテスト");
        assert_eq!(layer.mode(), PartMode::Plain);
        // From the candidate window too, and a trailing n settles as ン.
        feed(&mut layer, &mut engine, "Pan");
        layer.process_key(&mut engine, &ctrl('n'));
        assert_eq!(feed(&mut layer, &mut engine, "q"), "パン");
    }

    #[test]
    fn capital_chains_parts() {
        let (mut layer, mut engine) = setup();
        assert_eq!(feed(&mut layer, &mut engine, "Honjitu"), "");
        assert_eq!(feed(&mut layer, &mut engine, "Kaigi "), "ほんじつかいぎ");
    }

    #[test]
    fn ctrl_k_is_not_katakana() {
        let (mut layer, mut engine) = setup();
        assert!(!layer.process_key(&mut engine, &ctrl('k')).consumed);
        assert_eq!(feed(&mut layer, &mut engine, "ka"), "か");
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
        feed(&mut layer, &mut engine, "Mimir");
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
        assert_eq!(feed(&mut layer, &mut engine, "lmimir"), "");
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
            [EngineAction::Commit(word)] if word == "mimirいめ"
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
        layer.hint = PartHint::Reading;
        assert_eq!(layer.hint(&engine), None);
    }

    #[test]
    fn ctrl_u_discards_the_composition() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Kaigi");
        let result = layer.process_key(&mut engine, &ctrl('u'));
        assert!(result.consumed);
        assert!(matches!(engine.state(), InputState::Empty));
        assert_eq!(layer.mode(), PartMode::Plain);
        // Nothing composing: Ctrl+U is the application's again.
        assert!(!layer.process_key(&mut engine, &ctrl('u')).consumed);
        // Other unbound chords are swallowed while composing.
        feed(&mut layer, &mut engine, "Kai");
        assert!(layer.process_key(&mut engine, &ctrl('w')).consumed);
        assert_eq!(engine.preedit().unwrap().text(), "かい");
    }

    fn learned(engine: &InputMethodEngine) -> Vec<(String, String)> {
        engine
            .learning_entries()
            .into_iter()
            .map(|(r, s, _, _)| (r, s))
            .collect()
    }

    #[test]
    fn one_char_part_is_learned_joined_to_what_came_before() {
        let (mut layer, mut engine) = setup();
        engine_learning_on(&mut engine);
        // No model: parts commit their reading. かいぎ, plain の, then て.
        feed(&mut layer, &mut engine, "Kaigi noTe ");
        let learned = learned(&engine);
        assert!(learned.contains(&("かいぎ".into(), "かいぎ".into())));
        assert!(learned.contains(&("かいぎのて".into(), "かいぎのて".into())));
        assert!(!learned.iter().any(|(r, _)| r == "て"));
    }

    #[test]
    fn punctuation_ends_what_a_one_char_part_joins() {
        let (mut layer, mut engine) = setup();
        engine_learning_on(&mut engine);
        feed(&mut layer, &mut engine, "Kaigi.Te ");
        assert!(!learned(&engine).iter().any(|(r, _)| r.ends_with('て')));
    }

    #[test]
    fn eisu_and_kana_keys_switch_modes_inside_mimir() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Kai");
        let result = layer.process_key(&mut engine, &press(Keysym::EISU_TOGGLE));
        assert!(result.consumed);
        assert!(matches!(result.actions.as_slice(), [EngineAction::Commit(t), ..] if t == "かい"));
        assert_eq!(layer.mode(), PartMode::Ascii);
        assert!(!layer.process_key(&mut engine, &press_char('a')).consumed);
        // The macOS frontend sends かな as Super_R.
        assert!(
            layer
                .process_key(&mut engine, &press(Keysym::SUPER_R))
                .consumed
        );
        assert_eq!(layer.mode(), PartMode::Plain);
    }

    #[test]
    fn punctuation_closes_part() {
        let (mut layer, mut engine) = setup();
        assert_eq!(feed(&mut layer, &mut engine, "Kaigi."), "かいぎ。");
        assert_eq!(layer.mode(), PartMode::Plain);
    }
}
