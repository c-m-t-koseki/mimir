//! Kanji-part input layer: an explicit conversion span in front of the engine.
//!
//! In SKK style (`[conversion] skk_style`), kana input is plain hiragana,
//! committed as soon as each kana fires, and a capital letter (Shift+letter,
//! i.e. SandS) opens a *kanji part*; otherwise any letter opens one. The
//! engine live-converts the part; a bare Space closes it (commit), after
//! which typing goes on as before. `l` switches to direct ASCII input
//! and Ctrl+J returns to kana, as in SKK.
//!
//! Moving the caret inside a part (Ctrl+B / Ctrl+F, ← →) sets how much of
//! it converts: the reading left of the caret opens in the candidate
//! window, the rest waits converted on its own (今日|の天気). Space or a
//! picked candidate commits the left side and the rest goes on as the
//! part, its first word taken up next; anything else ending the part
//! commits both, the rest as shown. The pieces go to the history and the
//! learning cache as one, once the last is committed: a pick is learned
//! with what came after it.
//!
//! `q` inside a part commits its reading in katakana, as in SKK. `/` opens
//! SKK's abbrev input, marked ▽ as there: letters go in as typed, Space
//! converts them (github → GitHub), Enter commits them as they are.
//!
//! Inside a part, candidate keys are Emacs-style so nothing collides with
//! SandS: Ctrl+N opens the candidate window / next candidate, Ctrl+P the
//! previous one, `j k f d u i r e` pick one, Ctrl+G cancels.
//!
//! Ctrl+R inside a part registers a word: the part's reading is kept, the
//! word is typed with the same keys (its commits are captured instead of
//! reaching the application), and Enter saves it — to the learning cache
//! for immediate effect and to `user_dicts/registered.tsv` for good.

use std::time::{Duration, Instant};

use super::engine::{EngineAction, EngineResult, InputMethodEngine};
use super::keycode::{KeyEvent, KeyModifiers, Keysym};
use super::keymap::Keymap;
use super::phrase::{align, next_word};
use super::preedit::{AttributeType, Preedit, PreeditAttribute};
use super::state::InputState;
use crate::config::Settings;
use crate::config::settings::{PartHint, ReadingDisplay};

/// Longest committed text a one-char part is joined to for learning.
const RECENT_MAX_CHARS: usize = 12;

/// Completions listed in the hint panel.
const HINT_COMPLETIONS: usize = 3;

/// A frontend's timer fires about when asked, never exactly: this much
/// early still counts as due.
const TIMER_SLACK: Duration = Duration::from_millis(30);

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
    /// Which keys do what (`[keys]`).
    keys: Keymap,
    /// The open part is abbrev input (`/`).
    abbrev: bool,
    /// The part right of the caret, held out of the conversion (the engine
    /// composes only the left side).
    split: Option<Split>,
    /// The candidate window's selection was moved (Ctrl+N/P): a–l then
    /// type on instead of picking.
    stepped: bool,
    /// How the reading under the live conversion shows.
    reading_display: ReadingDisplay,
    /// `ReadingDisplay::Tail`: typed chars that stay kana.
    tail_chars: usize,
    /// `ReadingDisplay::Tail`: the pause before they convert too.
    tail_delay_ms: u64,
    /// The pause after a deletion.
    tail_delete_delay_ms: u64,
    /// The last key deleted from the part: `Some(true)` when what it took
    /// was a kana standing on its own, leaving the conversion untouched
    /// (規定で → 規定), `Some(false)` when it cut into a kanji run.
    deleted: Option<bool>,
    /// Typing paused (`refresh`): the tail shows converted too.
    settled: bool,
    /// The waits the layer keeps on its own (`[display]`).
    timing: Timing,
    /// When the last key came.
    last_key: Instant,
    /// When the layer last rewrote the preedit on its own (the tail
    /// converted on a pause).
    rewritten_at: Option<Instant>,
    /// What showed before that rewrite, until the next key: a commit
    /// arriving right after it takes this instead.
    before_rewrite: Option<String>,
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

/// The waits of `[display]` beyond the tail's: see `DisplaySettings`.
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub rewrite_hold_ms: u64,
    pub commit_guard_ms: u64,
    /// 0: never.
    pub auto_commit_ms: u64,
}

impl Timing {
    /// Nothing held, guarded or committed on its own: what the tests of
    /// everything else assume.
    const OFF: Self = Self {
        rewrite_hold_ms: 0,
        commit_guard_ms: 0,
        auto_commit_ms: 0,
    };
}

fn ms(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

/// The part right of the caret: its reading, and that converted on its own
/// — what shows after the left side, and what commits with it.
struct Split {
    reading: String,
    text: String,
}

impl Split {
    /// `reading` converted as live typing would show it (the engine's
    /// composition is replaced; the caller composes the left side next).
    fn converted(engine: &mut InputMethodEngine, reading: String) -> Self {
        engine.reopen(&reading);
        let text = engine
            .preedit()
            .map(|p| p.text().to_string())
            .unwrap_or_else(|| reading.clone());
        Self { reading, text }
    }
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
            keys: Keymap::default(),
            abbrev: false,
            split: None,
            stepped: false,
            reading_display: ReadingDisplay::None,
            tail_chars: 1,
            tail_delay_ms: 250,
            tail_delete_delay_ms: 600,
            deleted: None,
            settled: false,
            timing: Timing::OFF,
            last_key: Instant::now(),
            rewritten_at: None,
            before_rewrite: None,
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

    pub fn with_reading_display(
        self,
        reading_display: ReadingDisplay,
        tail_chars: usize,
        tail_delay_ms: u64,
        tail_delete_delay_ms: u64,
    ) -> Self {
        Self {
            reading_display,
            tail_chars,
            tail_delay_ms,
            tail_delete_delay_ms,
            ..self
        }
    }

    /// Whether the preedit holds its typed tail back in kana now: only
    /// right after a kana fired — an unfired consonant (規定 + d) keeps the
    /// conversion as it was.
    fn holds_tail(&self, engine: &InputMethodEngine) -> bool {
        self.reading_display == ReadingDisplay::Tail
            && !self.settled
            && self.split.is_none()
            && self.mode == PartMode::Part
            && composing(engine)
            // The window's head is converted to its end: the preedit shows
            // the same, not a kana tail.
            && !engine.previewing()
            && !engine.has_pending_romaji()
            && self.deleted != Some(true)
    }

    pub fn with_timing(self, timing: Timing) -> Self {
        Self { timing, ..self }
    }

    pub fn with_keys(self, keys: Keymap) -> Self {
        Self { keys, ..self }
    }

    /// The keys picking a candidate, in page order, as the frontend labels
    /// the window with them: capitals, whatever state the window is in.
    pub fn select_keys(&self) -> String {
        self.keys
            .select
            .iter()
            .map(|c| c.to_ascii_uppercase())
            .collect()
    }

    pub fn with_skk_style(self, skk_style: bool) -> Self {
        Self { skk_style, ..self }
    }

    /// Whether a–l pick from the candidate window (not after its selection
    /// was stepped): the frontend labels the rows only then.
    pub fn picks_by_letter(&self) -> bool {
        !self.stepped
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
        if mode != PartMode::Part {
            self.split = None;
            self.record_pieces(engine);
        }
        engine.set_live_conversion(mode == PartMode::Part);
        if self.abbrev {
            self.abbrev = false;
            engine.end_abbrev();
        }
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
        let kana = self.keys.kana.hit(key);
        if !composing && ctrl && !kana && !self.abbrev {
            return EngineResult::not_consumed();
        }
        if composing && key.is_press && self.keys.discard.hit(key) {
            return self.discard(engine);
        }
        if key.is_press && !key.keysym.is_modifier() {
            // Right after the tail converted on its own, a key keeps it so.
            self.settled &= self
                .rewritten_at
                .is_some_and(|at| at.elapsed() < ms(self.timing.rewrite_hold_ms));
            self.last_key = Instant::now();
            self.deleted = (key.keysym == Keysym::BACKSPACE
                && self.mode == PartMode::Part
                && self::composing(engine))
            .then(|| deletes_a_lone_kana(engine));
        }
        let result = self.dispatch(engine, key);
        let result = self.follow_chain(engine, result);
        let result = self.preview_conversion(engine, result);
        // What showed before the rewrite matters for this key only.
        if key.is_press && !key.keysym.is_modifier() {
            self.before_rewrite = None;
        }
        self.learn_pick(engine);
        let mut result = plain_aux(engine, result);
        self.decorate(engine, &mut result);
        // A Ctrl chord the engine leaves unbound must not reach the
        // application mid-composition: its text edit (Ctrl+K, Ctrl+W, …)
        // would act on the marked text behind the engine's back, and the
        // next keystroke would redraw what it deleted.
        if composing && ctrl && self.mode != PartMode::Ascii {
            result.consumed = true;
        }
        result
    }

    /// A completion just taken (or a word of one): the history entry's next
    /// word follows in a window of its own, with nothing typed. Any other
    /// state lets a chain go.
    fn follow_chain(
        &mut self,
        engine: &mut InputMethodEngine,
        result: EngineResult,
    ) -> EngineResult {
        if !matches!(engine.state(), InputState::Empty) {
            if !engine.chaining() {
                engine.forget_chain();
            }
            return result;
        }
        let Some(next) = engine.continue_chain() else {
            return result;
        };
        self.enter(engine, PartMode::Part);
        self.stepped = false;
        merge(result, next)
    }

    /// Keys in a chain's window: Ctrl+N / Ctrl+P / Ctrl+I step, Shift + a
    /// select key picks, and once stepped, Enter / Space commit. Anything
    /// else closes it without committing and goes on as typed (Ctrl+G /
    /// Escape only close it). `None` for the keys the window takes.
    fn chain_key(
        &mut self,
        engine: &mut InputMethodEngine,
        key: &KeyEvent,
    ) -> Option<EngineResult> {
        let k = &self.keys;
        let picks = typed_char(key).is_some_and(|ch| {
            (key.modifiers.shift_key || ch.is_ascii_uppercase())
                && k.select_index(ch.to_ascii_lowercase()).is_some()
        });
        let steps = k.next.hit(key) || k.prev.hit(key);
        let commits = self.stepped && (k.commit.hit(key) || k.close.hit(key));
        if picks || steps || commits {
            return None;
        }
        let cancel = k.cancel.hit(key);
        let mut closed = engine.end_chain();
        self.enter(engine, PartMode::Plain);
        if cancel {
            closed.consumed = true;
            return Some(closed);
        }
        Some(merge(closed, self.process_key(engine, key)))
    }

    /// While a part is typed, its window shows what a conversion would
    /// open on, so Shift + a select key picks from the same list Ctrl+N
    /// shows. Only a window the engine shows is replaced: `[display]
    /// candidate_window = "conversion"` keeps it closed.
    fn preview_conversion(
        &self,
        engine: &mut InputMethodEngine,
        mut result: EngineResult,
    ) -> EngineResult {
        if self.mode != PartMode::Part || self.abbrev || !composing(engine) {
            return result;
        }
        let Some(at) = result
            .actions
            .iter()
            .position(|a| matches!(a, EngineAction::ShowCandidates(_)))
        else {
            return result;
        };
        if let Some(candidates) = engine.preview_conversion() {
            result.actions[at] = EngineAction::ShowCandidates(candidates);
        }
        result
    }

    /// Everything the layer adds to the engine's preedit: ▽ for abbrev
    /// input, the reading held right of the caret, and the reading of the
    /// live conversion (`[display] reading_display`).
    fn decorate(&self, engine: &InputMethodEngine, result: &mut EngineResult) {
        if self.abbrev {
            mark_abbrev(engine, result);
            return;
        }
        let live = self.mode == PartMode::Part && composing(engine);
        if self.holds_tail(engine) {
            let reading = engine.reading();
            for action in &mut result.actions {
                if let EngineAction::UpdatePreedit(preedit) = action
                    && let Some(shown) =
                        kana_tail_preedit(&reading, preedit.text(), self.tail_chars)
                {
                    *preedit = shown;
                }
            }
        }
        if let Some(split) = &self.split {
            show_split(&split.text, result);
        }
        if live && self.reading_display == ReadingDisplay::Inline {
            let reading = engine.reading();
            let head = engine
                .preedit()
                .map(|p| p.text().to_string())
                .unwrap_or_default();
            if let Some((_, _, kana)) = inline_kanji(&reading, &head) {
                for action in &mut result.actions {
                    if let EngineAction::UpdatePreedit(preedit) = action
                        && !preedit.is_empty()
                    {
                        let mut shown = Preedit::with_text(format!("{}〈{kana}〉", preedit.text()));
                        shown.set_caret(preedit.caret());
                        let mut attributes = preedit.attributes().to_vec();
                        let end = shown.text().chars().count();
                        attributes.push(PreeditAttribute::new(
                            preedit.text().chars().count(),
                            end,
                            AttributeType::Underline,
                        ));
                        shown.set_attributes(attributes);
                        *preedit = shown;
                    }
                }
            }
        }
    }

    /// Typing paused: what is due now — the tail held back in kana
    /// converts, or, left alone long enough, the part commits.
    pub fn refresh(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        if self.holds_tail(engine) {
            return self.settle_tail(engine);
        }
        if self
            .auto_commit_at(engine)
            .is_some_and(|at| at <= Instant::now() + TIMER_SLACK)
        {
            let result = self.close(engine);
            self.enter(engine, PartMode::Plain);
            return result;
        }
        EngineResult::consumed()
    }

    /// Redraw the preedit with nothing held back in kana.
    fn settle_tail(&mut self, engine: &InputMethodEngine) -> EngineResult {
        let before = engine
            .preedit()
            .and_then(|p| kana_tail_preedit(&engine.reading(), p.text(), self.tail_chars))
            .map(|p| p.text().to_string());
        self.settled = true;
        let Some(preedit) = engine.preedit() else {
            return EngineResult::consumed();
        };
        if before.as_deref().is_some_and(|b| b != preedit.text()) {
            self.rewritten_at = Some(Instant::now());
            self.before_rewrite = before;
        }
        let mut result =
            EngineResult::consumed().with_action(EngineAction::UpdatePreedit(preedit.clone()));
        self.decorate(engine, &mut result);
        result
    }

    /// When a part left alone commits on its own: typed into and composing
    /// only — not with the candidate window open, cut at the caret, an
    /// unfired consonant, abbrev input or a registration — and never
    /// sooner than the hold after a rewrite.
    fn auto_commit_at(&self, engine: &InputMethodEngine) -> Option<Instant> {
        let t = self.timing;
        let idle = t.auto_commit_ms > 0
            && self.mode == PartMode::Part
            && composing(engine)
            && self.split.is_none()
            && !self.abbrev
            && self.register.is_none()
            && !engine.has_pending_romaji();
        if !idle {
            return None;
        }
        let at = self.last_key + ms(t.auto_commit_ms);
        Some(match self.rewritten_at {
            Some(rewritten) => at.max(rewritten + ms(t.rewrite_hold_ms)),
            None => at,
        })
    }

    /// When the frontend should call [`Self::refresh`], from now: while a
    /// kana tail is held back, the pause after the last key; then, when the
    /// part would commit on its own.
    pub fn refresh_after_ms(&self, engine: &InputMethodEngine) -> Option<u64> {
        let due = if self.holds_tail(engine) {
            let delay = match self.deleted {
                Some(_) => self.tail_delete_delay_ms,
                None => self.tail_delay_ms,
            };
            self.last_key + ms(delay)
        } else {
            self.auto_commit_at(engine)?
        };
        Some(due.saturating_duration_since(Instant::now()).as_millis() as u64)
    }

    /// A commit right after the layer rewrote the preedit on its own: what
    /// showed before the rewrite is what goes in (the user was looking at
    /// that when they pressed the key).
    fn guarded_commit(&mut self, engine: &mut InputMethodEngine) -> Option<EngineResult> {
        let recent = self
            .rewritten_at
            .is_some_and(|at| at.elapsed() < ms(self.timing.commit_guard_ms));
        if !recent || self.split.is_some() || !composing(engine) {
            return None;
        }
        let text = self.before_rewrite.take()?;
        let reading = engine.take_reading();
        engine.note_commit(&reading, &text);
        self.note_recent(reading, text.clone());
        Some(
            EngineResult::consumed()
                .with_action(EngineAction::Commit(text))
                .with_action(EngineAction::HideCandidates)
                .with_action(EngineAction::HideAuxText),
        )
    }

    /// `ReadingDisplay::Ruby`: the last kanji run of the preedit — its span
    /// (chars) and reading — for the frontend to set the reading above.
    pub fn ruby(&self, engine: &InputMethodEngine) -> Option<(usize, usize, String)> {
        if self.reading_display != ReadingDisplay::Ruby
            || self.mode != PartMode::Part
            || !composing(engine)
        {
            return None;
        }
        let head = engine.preedit()?.text().to_string();
        inline_kanji(&engine.reading(), &head)
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
    /// earlier-committed words the reading completes to.
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

            // Ctrl+J inside a part: commit the reading as typed, as SKK's
            // C-j does in ▽ (and macOS's Ctrl+J, ひらがな). Ahead of `kana`,
            // which shares the key and keeps it everywhere else.
            if self.keys.hiragana.hit(key)
                && self.mode == PartMode::Part
                && !self.abbrev
                && !matches!(engine.state(), InputState::Empty)
            {
                return self.commit_kana(engine, false);
            }
            // Ctrl+J / かな: back to kana from anywhere, closing an open part.
            if self.keys.kana.hit(key) || key.keysym.is_mode_toggle_key() {
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
        // something is composing — or it is bound in `[keys]`.
        if m.control_key
            && matches!(key.keysym, Keysym::KEY_K | Keysym::KEY_K_UPPER)
            && !self.keys.binds(key)
        {
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
                _ => {
                    self.stepped = false;
                    self.part_key(engine, key)
                }
            },
        }
    }

    /// Focus loss / explicit commit: commit what is open, emitting the same
    /// cleanup actions as the engine's own commit. A part ends with it.
    pub fn flush(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        // A chain's window offers, nothing was typed: it goes uncommitted.
        if engine.chaining() {
            self.enter(engine, PartMode::Plain);
            return engine.end_chain();
        }
        let result = match (self.mode, engine.state()) {
            (_, InputState::Empty) => engine.commit_result(),
            // The part right of the caret goes too.
            (PartMode::Part, _) => self.close(engine),
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
        let mut result = self.close_head(engine);
        // The part right of the caret goes too, as shown.
        if let Some(split) = self.split.take() {
            engine.note_commit(&split.reading, &split.text);
            self.note_recent(split.reading, split.text.clone());
            result = result.with_action(EngineAction::Commit(split.text));
        }
        self.record_pieces(engine);
        result
    }

    /// The pieces of a part cut at the caret are all committed: recorded as
    /// one, which is also what a later one-char part joins.
    fn record_pieces(&mut self, engine: &mut InputMethodEngine) {
        if let Some((reading, text)) = engine.record_pieces() {
            self.recent = None;
            self.note_recent(reading, text);
        }
    }

    /// The reading right of the caret, taken for the part that goes on.
    fn take_tail(&mut self) -> Option<String> {
        self.split.take().map(|split| split.reading)
    }

    /// Commit the part's left side (all of it without a split), keeping
    /// what is right of the caret for the part that goes on.
    fn close_keeping_tail(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        let tail = self.take_tail();
        let result = self.close_head(engine);
        self.continue_with(engine, tail, result)
    }

    /// After the left side committed: the rest is the part now, or with no
    /// rest, back to plain kana.
    fn continue_with(
        &mut self,
        engine: &mut InputMethodEngine,
        tail: Option<String>,
        result: EngineResult,
    ) -> EngineResult {
        match tail {
            Some(tail) => {
                self.enter(engine, PartMode::Part);
                engine.reopen(&tail);
                // Word by word: the rest's first word is the next target,
                // particles it starts with going out with what was just
                // committed (今日|はいい天気 → 今日は, then いい).
                let surface = engine
                    .preedit()
                    .map(|p| p.text().to_string())
                    .unwrap_or_default();
                let chars: Vec<char> = tail.chars().collect();
                let after_content = result
                    .actions
                    .iter()
                    .rev()
                    .find_map(|a| match a {
                        EngineAction::Commit(text) => text.chars().last(),
                        _ => None,
                    })
                    .is_some_and(|c| matches!(c, '\u{3400}'..='\u{9FFF}' | 'ァ'..='ヺ'));
                let (lead, end) = next_word(&tail, &surface, after_content);
                let mut result = result;
                if lead > 0 {
                    let particles: String = chars[..lead].iter().collect();
                    engine.note_commit(&particles, &particles);
                    result = result.with_action(EngineAction::Commit(particles));
                }
                self.split = (end < chars.len())
                    .then(|| Split::converted(engine, chars[end..].iter().collect()));
                self.stepped = false;
                let reopened = engine.reopen(&chars[lead..end].iter().collect::<String>());
                merge(merge(result, reopened), engine.convert())
            }
            None => {
                self.enter(engine, PartMode::Plain);
                result
            }
        }
    }

    /// Ctrl+B / Ctrl+F (← →), Ctrl+A / Ctrl+E: move the caret, which is
    /// where the conversion stops, and open the left side's candidates. At
    /// the end, the whole part is the target. From here the part commits in
    /// pieces, which the engine holds until the last is in.
    fn move_split(
        &mut self,
        engine: &mut InputMethodEngine,
        to: impl Fn(usize, usize) -> usize,
    ) -> EngineResult {
        engine.settle_pending();
        engine.hold_pieces();
        let head: Vec<char> = engine.reading().chars().collect();
        let tail = self.take_tail().unwrap_or_default();
        let all: Vec<char> = head.iter().copied().chain(tail.chars()).collect();
        let cut = to(head.len(), all.len()).clamp(1, all.len().max(1));
        let (left, right) = all.split_at(cut.min(all.len()));
        self.split = (!right.is_empty()).then(|| Split::converted(engine, right.iter().collect()));
        self.stepped = false;
        let result = engine.reopen(&left.iter().collect::<String>());
        merge(result, engine.convert())
    }

    fn close_head(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        match (self.mode, engine.state()) {
            (_, InputState::Empty) => EngineResult::consumed(),
            (PartMode::Part, InputState::Composing { .. }) => {
                engine.settle_pending();
                self.note_part(engine);
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

    /// A part about to be committed as shown: nothing to learn (the model
    /// gives it again), but it is what a later one-char pick joins to.
    fn note_part(&mut self, engine: &mut InputMethodEngine) {
        let Some((reading, text)) = engine
            .composing_commit_preview()
            .filter(|(_, t)| !t.is_empty())
        else {
            return;
        };
        self.note_recent(reading, text);
    }

    /// `recent` after committing `text`: a one-char part (て, 中) joins what
    /// came just before (入力 + 中).
    fn note_recent(&mut self, reading: String, text: String) {
        let (reading, text) = match self.recent.take() {
            Some((r, t)) if text.chars().count() == 1 => (r + &reading, t + &text),
            _ => (reading, text),
        };
        self.recent = (text.chars().count() <= RECENT_MAX_CHARS).then_some((reading, text));
    }

    /// A candidate picked over the model's answer, which the engine learned
    /// — unless it is one char (中): no word of its own, it is learned
    /// joined to what was committed just before it (入力 + 中 →
    /// にゅうりょくちゅう→入力中).
    fn learn_pick(&mut self, engine: &mut InputMethodEngine) {
        let Some((reading, text)) = engine.take_chosen() else {
            return;
        };
        if text.chars().count() == 1
            && let Some((prev_reading, prev_text)) = self.recent.clone()
        {
            engine.learn_after(
                &format!("{prev_reading}{reading}"),
                &format!("{prev_text}{text}"),
                &prev_text,
            );
        }
        self.note_recent(reading, text);
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
            if self.keys.abbrev.hit(key) && matches!(engine.state(), InputState::Empty) {
                self.enter(engine, PartMode::Part);
                engine.set_live_conversion(false);
                engine.start_abbrev();
                self.abbrev = true;
                return EngineResult::consumed();
            }
            // A z sequence typed on its own is a symbol, not a word: outside
            // SKK style too, `z` waits in plain input, and only a key making
            // it kana (za → ざ) opens the part, `z` and all.
            let lone_z = !self.skk_style && engine.pending_romaji() == "z";
            if z_sequence(engine, ch) || (lone_z && Z_SEQUENCE_KEYS.contains(ch)) {
                let result = without_suggestions(engine.feed_romaji(ch));
                let result = merge(result, engine.commit_plain_result());
                self.note_plain_commits(&result);
                return result;
            }
            if lone_z {
                engine.take_reading();
                let result = self.start_part(engine, Some('z'));
                return merge(result, self.part_key(engine, key));
            }
            if !self.skk_style && ch == 'z' && matches!(engine.state(), InputState::Empty) {
                return without_suggestions(engine.process_key(key));
            }
            if let Some(first) = part_opener(key, ch) {
                return self.start_part(engine, first);
            }
            if self.keys.ascii.hit(key) && !l_is_a_letter(engine, ch) {
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
        if self.abbrev {
            return self.abbrev_key(engine, key);
        }
        let k = &self.keys;
        if (k.commit.hit(key) || k.close.hit(key) || k.end_commit.hit(key))
            && let Some(result) = self.guarded_commit(engine)
        {
            self.enter(engine, PartMode::Plain);
            return result;
        }
        let k = &self.keys;
        if k.convert.hit(key) {
            if matches!(engine.state(), InputState::Empty) {
                return EngineResult::consumed();
            }
            // The window already shows the first: the key steps on to the
            // second, as it does once the conversion is open.
            let previewed = engine.previewing();
            let result = engine.process_key(&press(Keysym::SPACE));
            if previewed && matches!(engine.state(), InputState::Conversion { .. }) {
                self.stepped = true;
                return merge(result, engine.process_key(&press(Keysym::TAB)));
            }
            return result;
        }
        if k.cancel.hit(key) {
            return self.cancel_part(engine);
        }
        if k.register.hit(key) {
            return self.start_registration(engine);
        }
        if k.complete.hit(key) {
            return engine.open_completions();
        }
        if self.at_end_commit(key) && composing(engine) {
            let result = self.close(engine);
            self.enter(engine, PartMode::Plain);
            return result;
        }
        if let Some(to) = self.split_move(key).filter(|_| composing(engine)) {
            return self.move_split(engine, to);
        }
        if self.keys.commit.hit(key) {
            let result = self.close(engine);
            self.enter(engine, PartMode::Plain);
            return result;
        }
        let Some(ch) = typed_char(key) else {
            // Bindings on Ctrl chords and named keys; a typed character's
            // wait for the z sequences and capitals below.
            if self.keys.ascii.hit(key) {
                return self.switch_to_ascii(engine);
            }
            if self.keys.katakana.hit(key) && !matches!(engine.state(), InputState::Empty) {
                return self.commit_kana(engine, true);
            }
            return match key.modifiers.control_key {
                true => engine.process_key(key),
                false => self.after_edit(engine, key),
            };
        };
        if z_sequence(engine, ch) {
            return engine.feed_romaji(ch);
        }
        if self.keys.close.hit(key) {
            return self.close_keeping_tail(engine);
        }
        if let Some(Some(first)) = part_opener(key, ch) {
            // Inside a part a capital opens no next part: Shift + a select
            // key picks from the window, any other capital reads on as its
            // lowercase (SandS rolling over, `s` + `E`, among them).
            if let Some(index) = self.keys.select_index(first) {
                let result = engine.select_candidate_on_page(index);
                if matches!(engine.state(), InputState::Empty) {
                    let tail = self.take_tail();
                    return self.continue_with(engine, tail, result);
                }
            }
            return engine.process_key(&press_char(first));
        }
        if self.keys.ascii.hit(key) && !l_is_a_letter(engine, ch) {
            return self.switch_to_ascii(engine);
        }
        if self.keys.katakana.hit(key) && !matches!(engine.state(), InputState::Empty) {
            return self.commit_kana(engine, true);
        }
        if matches!(ch, ',' | '.' | '?' | '!') {
            let result = self.close(engine);
            self.enter(engine, PartMode::Plain);
            return merge(result, self.plain_key(engine, key));
        }
        engine.process_key(key)
    }

    /// `[keys] end_commit` with the caret at the part's end (nothing split
    /// off to its right): the caret can't go further, so the key commits
    /// everything, as `commit` does.
    fn at_end_commit(&self, key: &KeyEvent) -> bool {
        self.split.is_none() && self.keys.end_commit.hit(key)
    }

    /// The caret move a key asks for (`[keys] left` / `right` / `home` /
    /// `end`), as the new cut given the cut and the reading's length.
    fn split_move(&self, key: &KeyEvent) -> Option<fn(usize, usize) -> usize> {
        let k = &self.keys;
        if k.left.hit(key) {
            Some(|cut, _| cut.saturating_sub(1))
        } else if k.right.hit(key) {
            Some(|cut, _| cut + 1)
        } else if k.home.hit(key) {
            Some(|_, _| 1)
        } else if k.end.hit(key) {
            Some(|_, len| len)
        } else {
            None
        }
    }

    /// Abbrev input: every printable key is a letter of it; Space or Ctrl+N
    /// converts, Enter commits it as typed (not learned: nothing was
    /// chosen), Ctrl+G / Escape cancels.
    fn abbrev_key(&mut self, engine: &mut InputMethodEngine, key: &KeyEvent) -> EngineResult {
        let empty = matches!(engine.state(), InputState::Empty);
        if self.keys.cancel.hit(key) {
            return self.cancel_part(engine);
        }
        if self.keys.convert.hit(key) || self.keys.close.hit(key) {
            return if empty {
                self.enter(engine, PartMode::Plain);
                EngineResult::consumed()
            } else {
                engine.convert()
            };
        }
        if self.keys.commit.hit(key) {
            let result = if empty {
                EngineResult::consumed()
            } else {
                engine.commit_unlearned_result()
            };
            self.enter(engine, PartMode::Plain);
            return result;
        }
        if typed_char(key).is_some() {
            return engine.process_key(key);
        }
        self.after_edit(engine, key)
    }

    /// Backspace and friends: leaving the part empty drops back to plain.
    fn after_edit(&mut self, engine: &mut InputMethodEngine, key: &KeyEvent) -> EngineResult {
        let result = engine.process_key(key);
        if matches!(engine.state(), InputState::Empty) {
            let tail = self.take_tail();
            return self.continue_with(engine, tail, result);
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
        if engine.chaining()
            && let Some(result) = self.chain_key(engine, key)
        {
            return result;
        }
        if self.keys.next.hit(key) {
            self.stepped = true;
            return engine.process_key(&press(Keysym::TAB));
        }
        if self.keys.prev.hit(key) {
            self.stepped = true;
            return engine.process_key(&KeyEvent::new(
                Keysym::TAB,
                KeyModifiers::new().with_shift(true),
                true,
            ));
        }
        // Cancel closes the window, back to the part's composition.
        if self.keys.cancel.hit(key) {
            return engine.process_key(&press(Keysym::ESCAPE));
        }
        if self.keys.register.hit(key) {
            return self.start_registration(engine);
        }
        if self.keys.commit.hit(key) || self.at_end_commit(key) {
            return self.commit_conversion_all(engine);
        }
        if let Some(to) = self.split_move(key) {
            return self.move_split(engine, to);
        }
        if self.keys.katakana.hit(key) {
            return self.commit_kana(engine, true);
        }
        let Some(ch) = typed_char(key) else {
            return engine.process_key(key);
        };
        // Picking by letter is for a window just opened; once the selection
        // was stepped to, the letter types on. Shift + a select key picks
        // either way, as it does while the part is typed.
        let shifted = key.modifiers.shift_key || ch.is_ascii_uppercase();
        if let Some(index) = self
            .keys
            .select_index(ch.to_ascii_lowercase())
            .filter(|_| shifted || !self.stepped)
        {
            let result = engine.select_candidate_on_page(index);
            if matches!(engine.state(), InputState::Empty) {
                let tail = self.take_tail();
                return self.continue_with(engine, tail, result);
            }
            return result;
        }
        if self.keys.close.hit(key) {
            return self.commit_conversion(engine);
        }
        // A capital opens no next part here either: it accepts the
        // selection and types on as its lowercase.
        if let Some(Some(first)) = part_opener(key, ch) {
            let result = self.commit_conversion_all(engine);
            return merge(result, self.process_key(engine, &press_char(first)));
        }
        // Any other key accepts the selection and keeps typing plain kana.
        let result = self.commit_conversion_all(engine);
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
        let abort = self.keys.cancel.hit(key);
        let save = self.keys.commit.hit(key);
        let reg = self.register.as_mut().expect("registering");
        if idle && self.mode != PartMode::Part {
            if abort {
                self.register = None;
                return EngineResult::consumed()
                    .with_action(EngineAction::UpdatePreedit(Preedit::new()));
            }
            if save {
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

    /// `q` / Ctrl+J: commit the part's reading in katakana or as typed, and
    /// learn it so the live conversion offers it next time (てすと → テスト).
    fn commit_kana(&mut self, engine: &mut InputMethodEngine, katakana: bool) -> EngineResult {
        engine.settle_pending();
        let reading = engine.take_reading();
        let text = match katakana {
            true => mimir_engine::hiragana_to_katakana(&reading),
            false => reading.clone(),
        };
        engine.learn(&reading, &text);
        self.recent = Some((reading, text.clone()));
        let result = EngineResult::consumed()
            .with_action(EngineAction::Commit(text))
            .with_action(EngineAction::HideCandidates)
            .with_action(EngineAction::HideAuxText);
        let tail = self.take_tail();
        self.continue_with(engine, tail, result)
    }

    /// Commit the selected candidate; what is right of the caret goes on
    /// as the part.
    fn commit_conversion(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        let tail = self.take_tail();
        let result = engine.process_key(&press(Keysym::RETURN));
        self.continue_with(engine, tail, result)
    }

    /// Commit the selected candidate and the rest of the part with it, as
    /// shown.
    fn commit_conversion_all(&mut self, engine: &mut InputMethodEngine) -> EngineResult {
        let result = engine.process_key(&press(Keysym::RETURN));
        let result = merge(result, self.close(engine));
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
/// What follows `z` in the z sequences (zh ← / zl → / z. … / z+Space …).
const Z_SEQUENCE_KEYS: &str = "hjkl ,./-";

/// Whether the ASCII key, a letter, is typed as a letter of the reading:
/// right after a consonant (`gl` of `google`, `pl` of `apple`), which no
/// romaji continues with `l`, so English typed as romaji reaches the word
/// it spells. After a vowel or a lone `n` (こん → ん) it switches as ever;
/// Shift+L types the letter there (`heLlo`, `Linux`).
fn l_is_a_letter(engine: &InputMethodEngine, ch: char) -> bool {
    let pending = engine.pending_romaji();
    ch.is_ascii_alphabetic() && !pending.is_empty() && pending != "n"
}

fn z_sequence(engine: &InputMethodEngine, ch: char) -> bool {
    matches!(ch, 'l' | ' ' | ',' | '.') && engine.pending_romaji() == "z"
}

/// `Some(first)` when the key opens a kanji part: a capital, fed lowercased.
fn part_opener(key: &KeyEvent, ch: char) -> Option<Option<char>> {
    (ch.is_ascii_uppercase() || (key.modifiers.shift_key && ch.is_ascii_lowercase()))
        .then(|| Some(ch.to_ascii_lowercase()))
}

/// The last kanji run of `surface`: its span in `surface` (chars) and its
/// reading.
fn last_kanji(reading: &str, surface: &str) -> Option<(usize, usize, String)> {
    let units = align(reading, surface)?;
    let k = units
        .iter()
        .rposition(|u| !u.kana && matches!(u.last, '\u{3400}'..='\u{9FFF}' | '々'))?;
    let (reading_start, surface_start) = match k {
        0 => (0, 0),
        _ => (units[k - 1].reading_end, units[k - 1].surface_end),
    };
    let text = reading
        .chars()
        .skip(reading_start)
        .take(units[k].reading_end - reading_start)
        .collect();
    Some((surface_start, units[k].surface_end, text))
}

/// `surface` with the last `n` chars of `reading` back in kana: converted
/// up to the piece holding the cut, the reading from there (a kanji run
/// the cut falls in goes back whole).
fn with_kana_tail(reading: &str, surface: &str, n: usize) -> Option<String> {
    let units = align(reading, surface)?;
    let r: Vec<char> = reading.chars().collect();
    let cut = r.len().saturating_sub(n);
    let k = units.iter().position(|u| u.reading_end > cut)?;
    let (reading_start, surface_start) = match k {
        0 => (0, 0),
        _ => (units[k - 1].reading_end, units[k - 1].surface_end),
    };
    let head: String = surface.chars().take(surface_start).collect();
    let tail: String = r[reading_start..].iter().collect();
    Some(head + &tail)
}

/// Whether Backspace would take a kana that stands on its own in the
/// preedit (the で of 規定で), not a piece of a kanji run's reading.
fn deletes_a_lone_kana(engine: &InputMethodEngine) -> bool {
    if engine.has_pending_romaji() {
        return true;
    }
    let reading = engine.reading();
    let Some(preedit) = engine.preedit() else {
        return false;
    };
    align(&reading, preedit.text()).is_some_and(|units| units.last().is_some_and(|u| u.kana))
}

/// [`last_kanji`] of a live preedit, unfired romaji left out of both.
fn inline_kanji(reading: &str, preedit: &str) -> Option<(usize, usize, String)> {
    let (reading, _) = split_pending(reading);
    let (preedit, _) = split_pending(preedit);
    last_kanji(reading, preedit)
}

/// The preedit with the typed tail back in kana ([`with_kana_tail`]), caret
/// at its end.
fn kana_tail_preedit(reading: &str, preedit: &str, n: usize) -> Option<Preedit> {
    let (reading, pending) = split_pending(reading);
    let (shown, _) = split_pending(preedit);
    let text = with_kana_tail(reading, shown, n)? + pending;
    Some(Preedit::with_text_underlined(text))
}

/// Unfired romaji at the end of the reading and the preedit (`か|k`): kept
/// out of the alignment, put back after.
fn split_pending(text: &str) -> (&str, &str) {
    let kana = text.trim_end_matches(|c: char| c.is_ascii_alphabetic());
    (kana, &text[kana.len()..])
}

fn composing(engine: &InputMethodEngine) -> bool {
    matches!(engine.state(), InputState::Composing { .. })
}

/// The reading held right of the caret, shown after the converted left
/// side. The left side, what converts, is highlighted (a thick underline)
/// and the rest gets a plain one: some applications draw the caret at the
/// end of the preedit whatever they are told, so the boundary must show
/// without it. The caret is still put there.
fn show_split(tail: &str, result: &mut EngineResult) {
    for action in &mut result.actions {
        if let EngineAction::UpdatePreedit(preedit) = action
            && !preedit.is_empty()
        {
            let head = preedit.text().chars().count();
            let mut shown = Preedit::with_text(format!("{}{tail}", preedit.text()));
            shown.set_caret(head);
            let end = shown.text().chars().count();
            shown.set_attributes(vec![
                PreeditAttribute::new(0, head, AttributeType::Highlight),
                PreeditAttribute::underline(head, end),
            ]);
            *preedit = shown;
        }
    }
}

/// SKK's ▽ in front of abbrev input — alone before its first letter.
const ABBREV_MARK: &str = "▽";

fn mark_abbrev(engine: &InputMethodEngine, result: &mut EngineResult) {
    let mut marked = false;
    for action in &mut result.actions {
        if let EngineAction::UpdatePreedit(preedit) = action {
            *preedit = prefixed(preedit, ABBREV_MARK);
            marked = true;
        }
    }
    if !marked && matches!(engine.state(), InputState::Empty) {
        result
            .actions
            .push(EngineAction::UpdatePreedit(Preedit::with_text(ABBREV_MARK)));
    }
}

/// `preedit` with `prefix` in front, caret and attributes moved along.
fn prefixed(preedit: &Preedit, prefix: &str) -> Preedit {
    let shift = prefix.chars().count();
    let mut out = Preedit::with_text(format!("{prefix}{}", preedit.text()));
    out.set_caret(preedit.caret() + shift);
    out.set_attributes(
        preedit
            .attributes()
            .iter()
            .map(|a| PreeditAttribute::new(a.start + shift, a.end + shift, a.attr_type))
            .collect(),
    );
    out
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
    use crate::config::settings::CandidateWindow;
    use crate::core::engine::EngineConfig;

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

    /// The window a key's result shows, as texts.
    fn window(result: &EngineResult) -> Option<Vec<String>> {
        result.actions.iter().find_map(|a| match a {
            EngineAction::ShowCandidates(list) => {
                Some(list.candidates().iter().map(|c| c.text.clone()).collect())
            }
            _ => None,
        })
    }

    /// A layer whose history holds 今日はいい天気ですね and 今日は寒いですね.
    fn history_setup() -> (PartLayer, InputMethodEngine) {
        let (layer, mut engine) = setup();
        engine_learning_on(&mut engine);
        engine.note_commit("きょうはいいてんきですね", "今日はいい天気ですね");
        engine.note_commit("きょうはさむいですね", "今日は寒いですね");
        (layer, engine)
    }

    /// Pick `text` from the window `shown` with Shift + its select key.
    fn pick(
        layer: &mut PartLayer,
        engine: &mut InputMethodEngine,
        shown: &[String],
        text: &str,
    ) -> EngineResult {
        let at = shown.iter().position(|t| t == text).expect("on offer");
        let key = layer.select_keys().chars().nth(at).unwrap();
        layer.process_key(
            engine,
            &KeyEvent::new(
                Keysym(key as u32),
                KeyModifiers::new().with_shift(true),
                true,
            ),
        )
    }

    #[test]
    fn the_history_completes_to_the_end_of_the_word_then_word_by_word() {
        let (mut layer, mut engine) = history_setup();
        feed(&mut layer, &mut engine, "Kyouh");
        let shown = window(&layer.process_key(&mut engine, &press_char('a'))).unwrap();
        // To each word end, shallow first: both entries' first word before
        // the first one's second.
        let at = |t: &str| {
            shown
                .iter()
                .position(|s| s == t)
                .unwrap_or_else(|| panic!("{t} in {shown:?}"))
        };
        assert!(at("今日はいい") < at("今日はいい天気ですね"));
        assert!(at("今日は寒いですね") < at("今日はいい天気ですね"));
        let result = pick(&mut layer, &mut engine, &shown, "今日はいい");
        assert_eq!(commits(&result), "今日はいい");
        // Nothing typed, the entry's next word on offer, named as the
        // history's.
        assert!(engine.chaining());
        assert_eq!(engine.window_source(), Some("履歴"));
        assert_eq!(window(&result).unwrap(), ["天気ですね"]);
        let result = pick(
            &mut layer,
            &mut engine,
            &["天気ですね".into()],
            "天気ですね",
        );
        assert_eq!(commits(&result), "天気ですね");
        // The entry is done.
        assert!(!engine.chaining());
        assert!(matches!(engine.state(), InputState::Empty));
        assert_eq!(layer.mode(), PartMode::Plain);
    }

    #[test]
    fn a_chain_steps_with_ctrl_n_and_other_keys_close_it_uncommitted() {
        let (mut layer, mut engine) = history_setup();
        engine.note_commit("きょうはいいひ", "今日はいい日");
        feed(&mut layer, &mut engine, "Kyouh");
        let shown = window(&layer.process_key(&mut engine, &press_char('a'))).unwrap();
        let result = pick(&mut layer, &mut engine, &shown, "今日はいい");
        let next = window(&result).unwrap();
        assert_eq!(next.len(), 2, "{next:?}");
        // Unstepped, Enter is not the window's: it closes it and goes on.
        let result = layer.process_key(&mut engine, &press(Keysym::RETURN));
        assert_eq!(commits(&result), "");
        assert!(!result.consumed);
        assert!(!engine.chaining());
        // Stepped with Ctrl+N (Ctrl+I too), Enter commits the selection.
        feed(&mut layer, &mut engine, "Kyouh");
        let shown = window(&layer.process_key(&mut engine, &press_char('a'))).unwrap();
        pick(&mut layer, &mut engine, &shown, "今日はいい");
        layer.process_key(&mut engine, &ctrl('n'));
        let result = layer.process_key(&mut engine, &press(Keysym::RETURN));
        assert_eq!(commits(&result), next[1]);
        // A letter closes it and types on (plain kana, in SKK style).
        feed(&mut layer, &mut engine, "Kyouh");
        let shown = window(&layer.process_key(&mut engine, &press_char('a'))).unwrap();
        pick(&mut layer, &mut engine, &shown, "今日はいい");
        let result = layer.process_key(&mut engine, &press_char('a'));
        assert_eq!(commits(&result), "あ");
        assert!(!engine.chaining());
    }

    #[test]
    fn tab_opens_the_history_by_name() {
        let (mut layer, mut engine) = history_setup();
        feed(&mut layer, &mut engine, "Kyouha");
        layer.process_key(&mut engine, &press(Keysym::TAB));
        assert_eq!(engine.window_source(), Some("履歴"));
        // Escape back to typing, then Ctrl+N: the full list, unnamed.
        layer.process_key(&mut engine, &press(Keysym::ESCAPE));
        layer.process_key(&mut engine, &ctrl('n'));
        assert!(matches!(engine.state(), InputState::Conversion { .. }));
        assert_eq!(engine.window_source(), None);
    }

    #[test]
    fn a_word_typed_in_full_starts_no_chain() {
        let (mut layer, mut engine) = history_setup();
        // 今日は committed as converted: no completion taken.
        let result = feed(&mut layer, &mut engine, "Kyouha ");
        assert!(!result.is_empty());
        assert!(!engine.chaining());
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
    fn z_sequences_alone_stay_out_of_a_part_without_skk_style() {
        let (layer, mut engine) = setup();
        let mut layer = layer.with_skk_style(false);
        for (keys, symbol) in [
            ("zl", "→"),
            ("zh", "←"),
            ("z.", "…"),
            ("z/", "・"),
            ("z ", "\u{3000}"),
        ] {
            assert_eq!(feed(&mut layer, &mut engine, keys), symbol, "{keys}");
            assert_eq!(layer.mode(), PartMode::Plain, "{keys}");
        }
        // A kana from z opens the part as usual.
        assert_eq!(feed(&mut layer, &mut engine, "za"), "");
        assert_eq!(layer.mode(), PartMode::Part);
        assert_eq!(engine.reading(), "ざ");
        assert_eq!(feed(&mut layer, &mut engine, "zl "), "ざ→");
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
    fn ctrl_j_commits_the_part_in_hiragana() {
        let (mut layer, mut engine) = setup();
        let committed = |result: EngineResult| -> String {
            result
                .actions
                .into_iter()
                .filter_map(|a| match a {
                    EngineAction::Commit(text) => Some(text),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(feed(&mut layer, &mut engine, "noKaita"), "の");
        let result = layer.process_key(&mut engine, &ctrl('j'));
        assert_eq!(committed(result), "かいた");
        assert_eq!(layer.mode(), PartMode::Plain);
        // From the candidate window too.
        feed(&mut layer, &mut engine, "Pan");
        layer.process_key(&mut engine, &ctrl('n'));
        assert_eq!(
            committed(layer.process_key(&mut engine, &ctrl('j'))),
            "ぱん"
        );
        // Outside a part it still comes back to kana.
        feed(&mut layer, &mut engine, "l");
        assert_eq!(layer.mode(), PartMode::Ascii);
        layer.process_key(&mut engine, &ctrl('j'));
        assert_eq!(layer.mode(), PartMode::Plain);
    }

    #[test]
    fn slash_opens_abbrev_input() {
        let (mut layer, mut engine) = setup();
        // Letters (l and q too) go in as typed; Enter commits them.
        assert_eq!(feed(&mut layer, &mut engine, "/Html5q"), "");
        let result = layer.process_key(&mut engine, &press(Keysym::RETURN));
        assert!(
            result
                .actions
                .iter()
                .any(|a| matches!(a, EngineAction::Commit(text) if text == "Html5q"))
        );
        // The ▽ mark leads the preedit.
        let result = layer.process_key(&mut engine, &press_char('/'));
        assert!(
            result
                .actions
                .iter()
                .any(|a| matches!(a, EngineAction::UpdatePreedit(p) if p.text() == "▽"))
        );
        let result = layer.process_key(&mut engine, &press_char('x'));
        assert!(result.actions.iter().any(
            |a| matches!(a, EngineAction::UpdatePreedit(p) if p.text() == "▽x" && p.caret() == 2)
        ));
        layer.process_key(&mut engine, &ctrl('g'));
        layer.process_key(&mut engine, &ctrl('g'));
        // Back to kana afterwards.
        assert_eq!(feed(&mut layer, &mut engine, "ka"), "か");
        // Space converts.
        feed(&mut layer, &mut engine, "/abc");
        layer.process_key(&mut engine, &press(Keysym::SPACE));
        assert!(matches!(engine.state(), InputState::Conversion { .. }));
        // Cancelled before a letter: kana again.
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "/");
        layer.process_key(&mut engine, &ctrl('g'));
        assert_eq!(feed(&mut layer, &mut engine, "ka"), "か");
    }

    #[test]
    fn after_stepping_letters_type_on() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Kaigi");
        // The window shows while typing: Ctrl+N steps on to the second.
        layer.process_key(&mut engine, &ctrl('n'));
        assert!(!layer.picks_by_letter());
        // `j` commits the selection (the second candidate, カイギ without
        // a model) and types on.
        assert_eq!(feed(&mut layer, &mut engine, "ja"), "カイギじゃ");
        // A window just opened (none while typing) still picks by letter.
        let mut engine = InputMethodEngine::with_config(EngineConfig {
            candidate_window: CandidateWindow::Conversion,
            ..EngineConfig::default()
        });
        let mut layer = PartLayer::new();
        layer.attach(&mut engine);
        feed(&mut layer, &mut engine, "Kaigi");
        layer.process_key(&mut engine, &ctrl('n'));
        assert!(layer.picks_by_letter());
        assert_eq!(feed(&mut layer, &mut engine, "j"), "かいぎ");
        assert!(matches!(engine.state(), InputState::Empty));
    }

    /// The preedit text and caret a key shows.
    fn shown(
        layer: &mut PartLayer,
        engine: &mut InputMethodEngine,
        key: &KeyEvent,
    ) -> Option<(String, usize)> {
        layer
            .process_key(engine, key)
            .actions
            .into_iter()
            .rev()
            .find_map(|a| match a {
                EngineAction::UpdatePreedit(p) => Some((p.text().to_string(), p.caret())),
                _ => None,
            })
    }

    #[test]
    fn the_caret_sets_where_the_conversion_stops() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Kyouhaiitenki");
        for _ in 0..4 {
            layer.process_key(&mut engine, &ctrl('b'));
        }
        // Left of the caret composes; the rest is shown after it.
        let back = shown(&mut layer, &mut engine, &ctrl('b'));
        assert_eq!(back, Some(("きょうはいいてんき".into(), 4)));
        assert_eq!(engine.reading(), "きょうは");
        // Space commits the left side; the rest goes on as the part.
        assert_eq!(feed(&mut layer, &mut engine, " "), "きょうは");
        assert_eq!(layer.mode(), PartMode::Part);
        assert_eq!(engine.reading(), "いいてんき");
        assert_eq!(feed(&mut layer, &mut engine, " "), "いいてんき");
        assert_eq!(layer.mode(), PartMode::Plain);
    }

    #[test]
    fn a_reading_cut_at_the_caret_predicts_nothing() {
        // The caret marks where the reading ends; the history's 今日は寒い
        // would complete past it.
        let (mut layer, mut engine) = history_setup();
        feed(&mut layer, &mut engine, "Kyouhaiitenki");
        for _ in 0..4 {
            layer.process_key(&mut engine, &ctrl('b'));
        }
        layer.process_key(&mut engine, &ctrl('b'));
        assert_eq!(engine.reading(), "きょうは");
        // The conversion's window, after the reopened composition's.
        let texts: Vec<String> = engine
            .state()
            .candidates()
            .unwrap()
            .candidates()
            .iter()
            .map(|c| c.text.clone())
            .collect();
        assert!(
            !texts
                .iter()
                .any(|t| t.starts_with("今日はいい") || t.starts_with("今日は寒")),
            "{texts:?}"
        );
    }

    /// A layer showing the typed tail in kana, with `timing`, and an engine
    /// that converts てんき(が) to 天気(が) after 昨日は without a model.
    /// No window while typing: with one, the preedit shows its head.
    fn tail_setup(tail_chars: usize, timing: Timing) -> (PartLayer, InputMethodEngine) {
        tail_setup_with(tail_chars, timing, CandidateWindow::Conversion)
    }

    fn tail_setup_with(
        tail_chars: usize,
        timing: Timing,
        window: CandidateWindow,
    ) -> (PartLayer, InputMethodEngine) {
        let mut engine = InputMethodEngine::with_config(EngineConfig {
            candidate_window: window,
            ..EngineConfig::default()
        });
        engine_learning_on(&mut engine);
        engine.set_surrounding_context("昨日は", "");
        engine.learn_after("てんき", "天気", "");
        engine.learn_after("てんきが", "天気が", "");
        let mut layer = PartLayer::new()
            .with_reading_display(ReadingDisplay::Tail, tail_chars, 250, 600)
            .with_timing(timing);
        layer.attach(&mut engine);
        (layer, engine)
    }

    fn refreshed(layer: &mut PartLayer, engine: &mut InputMethodEngine) -> Option<String> {
        layer
            .refresh(engine)
            .actions
            .into_iter()
            .find_map(|a| match a {
                EngineAction::UpdatePreedit(p) => Some(p.text().to_string()),
                _ => None,
            })
    }

    #[test]
    fn with_the_window_the_preedit_shows_its_head_and_ctrl_n_opens_on_it() {
        let (mut layer, mut engine) = tail_setup_with(1, Timing::OFF, CandidateWindow::Always);
        feed(&mut layer, &mut engine, "Tenk");
        let result = layer.process_key(&mut engine, &press_char('i'));
        let head = result.actions.iter().find_map(|a| match a {
            EngineAction::ShowCandidates(list) => Some(list.candidates()[0].text.clone()),
            _ => None,
        });
        // Converted to the end, not held back in kana: what the window heads.
        assert_eq!(head.as_deref(), Some("天気"));
        assert_eq!(engine.preedit().unwrap().text(), "天気");
        let shown = |r: &EngineResult| {
            r.actions.iter().find_map(|a| match a {
                EngineAction::ShowCandidates(list) => Some(
                    list.candidates()
                        .iter()
                        .map(|c| c.text.clone())
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
        };
        let before = shown(&result);
        // Ctrl+N opens on the window as it was, its first row and all, and
        // steps on to the second, the first being on screen already.
        let opened = layer.process_key(&mut engine, &ctrl('n'));
        assert_eq!(shown(&opened), before);
        match engine.state() {
            InputState::Conversion { candidates, .. } => assert_eq!(candidates.cursor(), 1),
            _ => panic!("converting"),
        }
        assert!(!layer.picks_by_letter());
    }

    #[test]
    fn a_commit_right_after_the_tail_converts_takes_what_showed_before() {
        let guarded = Timing {
            commit_guard_ms: 60_000,
            ..Timing::OFF
        };
        let (mut layer, mut engine) = tail_setup(1, guarded);
        feed(&mut layer, &mut engine, "Tenk");
        assert_eq!(
            shown(&mut layer, &mut engine, &press_char('i')),
            Some(("てんき".into(), 3))
        );
        assert_eq!(refreshed(&mut layer, &mut engine).as_deref(), Some("天気"));
        let result = layer.process_key(&mut engine, &press(Keysym::RETURN));
        assert_eq!(commits(&result), "てんき");
        assert_eq!(layer.mode(), PartMode::Plain);
        // Unguarded, what the rewrite shows commits.
        let (mut layer, mut engine) = tail_setup(1, Timing::OFF);
        feed(&mut layer, &mut engine, "Tenki");
        refreshed(&mut layer, &mut engine);
        let result = layer.process_key(&mut engine, &press(Keysym::RETURN));
        assert_eq!(commits(&result), "天気");
    }

    #[test]
    fn keys_right_after_the_tail_converts_keep_it_converted() {
        let held = Timing {
            rewrite_hold_ms: 60_000,
            ..Timing::OFF
        };
        let (mut layer, mut engine) = tail_setup(2, held);
        feed(&mut layer, &mut engine, "Tenki");
        assert_eq!(refreshed(&mut layer, &mut engine).as_deref(), Some("天気"));
        feed(&mut layer, &mut engine, "g");
        assert_eq!(
            shown(&mut layer, &mut engine, &press_char('a')),
            Some(("天気が".into(), 3))
        );
        // Without the hold, the last two typed go back to kana.
        let (mut layer, mut engine) = tail_setup(2, Timing::OFF);
        feed(&mut layer, &mut engine, "Tenki");
        refreshed(&mut layer, &mut engine);
        feed(&mut layer, &mut engine, "g");
        assert_eq!(
            shown(&mut layer, &mut engine, &press_char('a')),
            Some(("てんきが".into(), 4))
        );
    }

    #[test]
    fn a_part_left_alone_commits_on_its_own() {
        let quick = Timing {
            auto_commit_ms: 1,
            ..Timing::OFF
        };
        let (layer, mut engine) = setup();
        let mut layer = layer.with_timing(quick);
        feed(&mut layer, &mut engine, "Kaigi");
        assert!(layer.refresh_after_ms(&engine).is_some());
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(commits(&layer.refresh(&mut engine)), "かいぎ");
        assert_eq!(layer.mode(), PartMode::Plain);
        // Not with the candidate window open, nor on an unfired consonant.
        feed(&mut layer, &mut engine, "Kaigi");
        layer.process_key(&mut engine, &ctrl('n'));
        assert_eq!(layer.refresh_after_ms(&engine), None);
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(commits(&layer.refresh(&mut engine)), "");
        layer.process_key(&mut engine, &ctrl('g'));
        feed(&mut layer, &mut engine, "k");
        assert_eq!(layer.refresh_after_ms(&engine), None);
        // Off by default.
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Kaigi");
        assert_eq!(layer.refresh_after_ms(&engine), None);
    }

    #[test]
    fn ctrl_f_and_ctrl_e_at_the_end_commit_everything() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Kyouha");
        let result = layer.process_key(&mut engine, &ctrl('f'));
        assert_eq!(commits(&result), "きょうは");
        assert_eq!(layer.mode(), PartMode::Plain);
        // From the candidate window too, with what it selects (Ctrl+N
        // stepped on to the second, キョウハ without a model).
        feed(&mut layer, &mut engine, "Kyouha");
        layer.process_key(&mut engine, &ctrl('n'));
        let result = layer.process_key(&mut engine, &ctrl('e'));
        assert_eq!(commits(&result), "キョウハ");
        // Cut short, Ctrl+F moves the caret back first; at the end, it commits.
        feed(&mut layer, &mut engine, "Kyouha");
        layer.process_key(&mut engine, &ctrl('b'));
        let result = layer.process_key(&mut engine, &ctrl('f'));
        assert_eq!(commits(&result), "");
        assert_eq!(engine.reading(), "きょうは");
        let result = layer.process_key(&mut engine, &ctrl('f'));
        assert_eq!(commits(&result), "きょうは");
    }

    #[test]
    fn split_parts_commit_whole_on_enter_and_move_back_right() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Kyouha");
        layer.process_key(&mut engine, &press(Keysym::LEFT));
        layer.process_key(&mut engine, &press(Keysym::LEFT));
        assert_eq!(engine.reading(), "きょ");
        // → and Ctrl+F move it back; at the end the whole part converts.
        layer.process_key(&mut engine, &press(Keysym::RIGHT));
        layer.process_key(&mut engine, &ctrl('f'));
        assert_eq!(engine.reading(), "きょうは");
        layer.process_key(&mut engine, &ctrl('a'));
        assert_eq!(engine.reading(), "き");
        // Enter commits both sides.
        let result = layer.process_key(&mut engine, &press(Keysym::RETURN));
        let committed: String = result
            .actions
            .iter()
            .filter_map(|a| match a {
                EngineAction::Commit(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(committed, "きょうは");
        assert_eq!(layer.mode(), PartMode::Plain);
        // Moving the caret opens the left side's candidates; a pick goes on
        // with the rest, like Space.
        feed(&mut layer, &mut engine, "Kyouha");
        layer.process_key(&mut engine, &ctrl('b'));
        assert!(matches!(engine.state(), InputState::Conversion { .. }));
        assert_eq!(feed(&mut layer, &mut engine, "j"), "きょう");
        assert_eq!(engine.reading(), "は");
    }

    /// What a key's result commits, joined.
    fn commits(result: &EngineResult) -> String {
        result
            .actions
            .iter()
            .filter_map(|a| match a {
                EngineAction::Commit(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_rest_right_of_the_caret_shows_converted_on_its_own() {
        let (mut layer, mut engine) = setup();
        engine_learning_on(&mut engine);
        // No model: teach の天気 after 昨日は, which live conversion then
        // shows for のてんき typed there.
        engine.set_surrounding_context("昨日は", "");
        engine.learn_after("のてんき", "の天気", "");
        feed(&mut layer, &mut engine, "Kyounotenki");
        for _ in 0..3 {
            layer.process_key(&mut engine, &ctrl('b'));
        }
        let back = shown(&mut layer, &mut engine, &ctrl('b'));
        assert_eq!(back, Some(("きょうの天気".into(), 3)));
        assert_eq!(engine.reading(), "きょう");
        // Enter commits the rest as shown. Nothing was picked: the whole
        // goes to the history only.
        let result = layer.process_key(&mut engine, &press(Keysym::RETURN));
        assert_eq!(commits(&result), "きょうの天気");
        assert!(!learned(&engine).iter().any(|(r, _)| r == "きょうのてんき"));
        // Completed to each word end past きょう.
        feed(&mut layer, &mut engine, "Kyou");
        assert_eq!(engine.completions(3), ["きょうの", "きょうの天気"]);
    }

    #[test]
    fn a_part_cut_at_the_caret_is_recorded_whole_once_committed() {
        let (mut layer, mut engine) = setup();
        engine_learning_on(&mut engine);
        feed(&mut layer, &mut engine, "Kyounotenki");
        for _ in 0..4 {
            layer.process_key(&mut engine, &ctrl('b'));
        }
        // キョウ picked over きょう (の goes out with it): nothing is
        // recorded while the rest waits.
        assert_eq!(feed(&mut layer, &mut engine, "k"), "キョウの");
        assert!(learned(&engine).is_empty(), "{:?}", learned(&engine));
        assert!(engine.completions(3).is_empty());
        // The rest goes out; then the whole is learned, with what followed
        // the pick, and completes later input.
        assert_eq!(feed(&mut layer, &mut engine, " "), "てんき");
        let got = learned(&engine);
        assert_eq!(
            got,
            [("きょうのてんき".to_string(), "キョウのてんき".to_string())]
        );
        feed(&mut layer, &mut engine, "Kyou");
        assert_eq!(engine.completions(3), ["キョウの", "キョウのてんき"]);
    }

    #[test]
    fn next_word_splits_after_particles_and_before_content() {
        assert_eq!(
            next_word("きょうはいいてんき", "今日はいい天気", false),
            (0, 4)
        );
        assert_eq!(
            next_word("はいいてんきですね", "はいい天気ですね", true),
            (1, 3)
        );
        assert_eq!(
            next_word("はいいてんきですね", "はいい天気ですね", false),
            (0, 3)
        );
        assert_eq!(
            next_word("いいてんきですね", "いい天気ですね", true),
            (0, 2)
        );
        assert_eq!(next_word("てんきですね", "天気ですね", true), (0, 6));
        assert_eq!(
            next_word("とうきょうにはいきます", "東京には行きます", false),
            (0, 7)
        );
        assert_eq!(next_word("かいたてすと", "書いたテスト", false), (0, 3));
        assert_eq!(next_word("は", "は", true), (0, 1));
        assert_eq!(next_word("かいぎ", "懐疑", false), (0, 3));
        assert_eq!(
            next_word("おにぎりをたべる", "おにぎりを食べる", false),
            (0, 5)
        );
        assert_eq!(next_word("とてもいい", "とてもいい", true), (0, 5));
    }

    #[test]
    fn the_last_kanji_run_and_its_reading() {
        assert_eq!(
            last_kanji("きょうはいいてんき", "今日はいい天気"),
            Some((5, 7, "てんき".into()))
        );
        assert_eq!(
            last_kanji("かいたてすと", "書いたテスト"),
            Some((0, 1, "か".into()))
        );
        assert_eq!(last_kanji("いいね", "いいね"), None);
        assert_eq!(
            inline_kanji("てんきk", "天気k"),
            Some((0, 2, "てんき".into()))
        );
    }

    #[test]
    fn the_typed_tail_goes_back_to_kana() {
        // The cut in 天気 takes the whole run back.
        assert_eq!(
            with_kana_tail("きょうはいいてんき", "今日はいい天気", 2).as_deref(),
            Some("今日はいいてんき")
        );
        assert_eq!(
            with_kana_tail("きょうはいい", "今日はいい", 2).as_deref(),
            Some("今日はいい")
        );
        assert_eq!(
            with_kana_tail("きょうは", "今日は", 2).as_deref(),
            Some("きょうは")
        );
        let shown = kana_tail_preedit("きょうはk", "今日はk", 1).unwrap();
        assert_eq!(shown.text(), "今日はk");
    }

    #[test]
    fn keys_follow_the_keymap() {
        let mut settings = crate::config::Settings::default_keys();
        settings.katakana = "q C-o".into();
        settings.ascii = ";".into();
        settings.select = "asdfghjk".into();
        let (layer, mut engine) = setup();
        let mut layer = layer.with_keys(Keymap::parse(&settings).unwrap());
        assert_eq!(layer.select_keys(), "ASDFGHJK");
        // A Ctrl chord commits in katakana too.
        feed(&mut layer, &mut engine, "Tesuto");
        let result = layer.process_key(&mut engine, &ctrl('o'));
        assert!(
            result
                .actions
                .iter()
                .any(|a| matches!(a, EngineAction::Commit(t) if t == "テスト"))
        );
        // Candidates are picked by the new keys.
        feed(&mut layer, &mut engine, "Kaigi");
        assert_eq!(feed(&mut layer, &mut engine, "A"), "かいぎ");
        // ; switches to ASCII, l no longer does.
        feed(&mut layer, &mut engine, "l");
        assert_eq!(layer.mode(), PartMode::Plain);
        feed(&mut layer, &mut engine, ";");
        assert_eq!(layer.mode(), PartMode::Ascii);
    }

    #[test]
    fn a_capital_inside_a_part_reads_on() {
        let (mut layer, mut engine) = setup();
        assert_eq!(
            feed(&mut layer, &mut engine, "HonjituGaigi "),
            "ほんじつがいぎ"
        );
    }

    #[test]
    fn shift_and_a_select_key_pick_from_the_conversion_shown() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Kon");
        let shown = layer
            .process_key(&mut engine, &press_char('o'))
            .actions
            .into_iter()
            .find_map(|a| match a {
                EngineAction::ShowCandidates(list) => Some(list),
                _ => None,
            })
            .expect("the window shows while typing");
        // What Ctrl+N opens on, the katakana fallback among it.
        assert!(shown.candidates().iter().any(|c| c.text == "コノ"));
        assert_eq!(layer.select_keys(), "JKFDUIRE");
        let second = shown.candidates()[1].text.clone();
        assert_eq!(feed(&mut layer, &mut engine, "K"), second);
        assert_eq!(layer.mode(), PartMode::Plain);
    }

    #[test]
    fn shift_and_a_select_key_pick_after_stepping() {
        let (mut layer, mut engine) = setup();
        feed(&mut layer, &mut engine, "Kono");
        layer.process_key(&mut engine, &ctrl('n'));
        layer.process_key(&mut engine, &ctrl('n'));
        // Stepped: j types on, J still picks the first.
        let first = match engine.state() {
            InputState::Conversion { candidates, .. } => candidates.candidates()[0].text.clone(),
            _ => panic!("converting"),
        };
        assert_eq!(feed(&mut layer, &mut engine, "J"), first);
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
    fn l_mid_reading_is_a_letter() {
        let (layer, mut engine) = setup();
        let mut layer = layer.with_skk_style(false);
        // Right after a consonant, `l` is a letter.
        feed(&mut layer, &mut engine, "goog");
        feed(&mut layer, &mut engine, "le");
        assert_eq!(layer.mode(), PartMode::Part);
        assert_eq!(engine.reading(), "ごおgぇ");
        // After kana it switches, closing the part.
        feed(&mut layer, &mut engine, "l");
        assert_eq!(layer.mode(), PartMode::Ascii);

        // Shift+L is the letter there.
        let (layer, mut engine) = setup();
        let mut layer = layer.with_skk_style(false);
        feed(&mut layer, &mut engine, "heLlo");
        assert_eq!(layer.mode(), PartMode::Part);
        assert_eq!(engine.reading(), "へっぉ");
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
        // After a whole kana too, a capital reads on inside the part.
        assert_eq!(feed(&mut layer, &mut engine, "Gi "), "せぎ");
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
    fn only_a_pick_is_learned_and_one_char_joins_what_came_before() {
        let (mut layer, mut engine) = setup();
        engine_learning_on(&mut engine);
        // No model: parts commit their reading, which teaches nothing.
        feed(&mut layer, &mut engine, "Kaigi noTe");
        // Picking テ over て: learned, joined to かいぎ + の.
        assert_eq!(feed(&mut layer, &mut engine, "K"), "テ");
        let got = learned(&engine);
        assert!(!got.iter().any(|(r, _)| r == "かいぎ"), "{got:?}");
        assert!(
            got.contains(&("かいぎのて".into(), "かいぎのテ".into())),
            "{got:?}"
        );
        assert!(!got.iter().any(|(r, _)| r == "て"));
        // The window's own answer, committed, teaches nothing either.
        feed(&mut layer, &mut engine, "Kaigi");
        feed(&mut layer, &mut engine, "J");
        assert!(!learned(&engine).iter().any(|(r, _)| r == "かいぎ"));
    }

    #[test]
    fn commits_complete_later_input_without_being_learned() {
        let (mut layer, mut engine) = setup();
        engine_learning_on(&mut engine);
        // Committed as shown (no model: カイギ is picked by q, which learns).
        feed(&mut layer, &mut engine, "Kaigishitsuq");
        // A part committed as shown goes to the history only.
        feed(&mut layer, &mut engine, "Nyuuryoku");
        layer.process_key(&mut engine, &ctrl('n'));
        feed(&mut layer, &mut engine, "k");
        feed(&mut layer, &mut engine, "Kai");
        assert_eq!(engine.completions(3), vec!["カイギシツ".to_string()]);
        // Tab opens them as candidates.
        layer.process_key(&mut engine, &press(Keysym::TAB));
        assert!(matches!(engine.state(), InputState::Conversion { .. }));
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
