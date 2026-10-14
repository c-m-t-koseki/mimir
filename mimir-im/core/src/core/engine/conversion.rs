//! Conversion state handling: building the mixed candidate list, key
//! handling, and commit. Model dispatch lives in the sibling `model`
//! module, the Ctrl+R source views in `filter`, live chunking in `chunk`.

use std::collections::{HashMap, HashSet};

use tracing::debug;

use super::filter::source_for_key;
use super::*;

/// Maximum number of learning candidates to show
const MAX_LEARNING_CANDIDATES: usize = 3;

/// Max predictive (prefix-extending) dictionary candidates in the
/// composing suggestion list. The conversion list is uncapped — the full
/// ranked set goes into the paged candidate window.
const MAX_PREDICTIVE_SUGGESTIONS: usize = 3;

/// Min typed characters before predictive dictionary lookup kicks in — a
/// single key would flood the list from a large dictionary
const MIN_PREDICTIVE_PREFIX_CHARS: usize = 2;

/// How the unresolved romaji tail constrains the predictive lookup.
enum TailConstraint {
    /// No tail: prediction is unconstrained
    Unconstrained,
    /// The tail can still become these kana: narrow to them (`d` → だ/で…)
    Narrow(Vec<String>),
    /// The tail can no longer become kana (`yk`): no reading extends it
    Dead,
}

/// Mozc-style width/script annotation for a pure-kana candidate, or `None`
/// if the text mixes scripts or contains kanji/punctuation. Used to label
/// `あ` / `ア` / `ｱ` candidates in the conversion list.
pub(super) fn width_annotation(text: &str) -> Option<&'static str> {
    if mimir_engine::is_pure_hiragana(text) {
        Some("[全]ひらがな")
    } else if mimir_engine::is_pure_full_katakana(text) {
        Some("[全]カタカナ")
    } else {
        None
    }
}

/// Helper for building a deduplicated list of conversion candidates.
///
/// Two push paths exist: [`push`] dedups by text (skips duplicates), and
/// [`push_force`] always inserts (used for learning candidates that should
/// appear at the top even if a later source re-emits the same text).
struct CandidateBuilder {
    candidates: Vec<AnnotatedCandidate>,
    seen: HashSet<String>,
}

impl CandidateBuilder {
    fn new() -> Self {
        Self {
            candidates: Vec::new(),
            seen: HashSet::new(),
        }
    }

    /// Push a candidate if its text hasn't been seen yet.
    fn push(&mut self, ac: AnnotatedCandidate) {
        if self.seen.insert(ac.text.clone()) {
            self.candidates.push(ac);
        }
    }

    /// Push a candidate unconditionally, marking its text as seen so later
    /// dedup'd inserts skip it. Use only for sources that should win over
    /// duplicates from later steps (e.g. learning cache).
    fn push_force(&mut self, ac: AnnotatedCandidate) {
        self.seen.insert(ac.text.clone());
        self.candidates.push(ac);
    }

    fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }

    fn into_candidates(self) -> Vec<AnnotatedCandidate> {
        self.candidates
    }
}

impl InputMethodEngine {
    /// Start kanji conversion for the current buffer (Space/Down/Tab).
    pub(super) fn start_conversion(&mut self, learning: LearningLookup) -> EngineResult {
        self.history_window = false;
        // The window already shows what to open on: open on it as shown.
        if learning == LearningLookup::Use
            && let Some(list) = self.preview().cloned()
        {
            self.live.shown = false;
            let reading = self.input_buf.settled_reading(&self.converters.romaji);
            return self.enter_conversion_state(&reading, list);
        }
        // Snapshot the live-conversion text before clearing it, so the
        // displayed candidate survives even if re-inference diverges.
        let prev_suggest_text = self.live_text_with_pending();
        self.live.shown = false;
        let Some((reading, candidates)) = self.conversion_candidates(prev_suggest_text, learning)
        else {
            // No candidates: stay composing, untouched (emoji queries with
            // no match land here)
            if self
                .input_buf
                .settled_reading(&self.converters.romaji)
                .is_empty()
            {
                return EngineResult::consumed();
            }
            let preedit = self.set_composing_state();
            return EngineResult::consumed().with_action(EngineAction::UpdatePreedit(preedit));
        };
        self.enter_conversion_state(&reading, candidates)
    }

    /// The window a conversion would open on, shown while the part is
    /// still typed (`[display] candidate_window = "always"`): the list the
    /// composing window shows and a pick by select key commits from. The
    /// composition stays as it is.
    pub fn preview_conversion(&mut self) -> Option<CandidateList> {
        if !matches!(self.state, InputState::Composing { .. }) {
            return None;
        }
        let (reading, candidates) =
            self.conversion_candidates(self.live_text_with_pending(), LearningLookup::Use)?;
        // A pick of what the window opened on is no choice to learn, as in
        // a conversion.
        self.conversion_origin = candidates.selected_text().unwrap_or_default().to_string();
        self.shown_suggestions = candidates.clone();
        self.previewed = Some((reading, candidates.clone()));
        Some(candidates)
    }

    /// The reading and candidates a conversion opens on, `shown` (the
    /// live-conversion text) first; `None` without any.
    fn conversion_candidates(
        &mut self,
        prev_suggest_text: String,
        learning: LearningLookup,
    ) -> Option<(String, CandidateList)> {
        // Resolve the reading without touching the composition, so Esc
        // returns to an editable buffer with the romaji tail still live.
        let reading = self.input_buf.settled_reading(&self.converters.romaji);
        // The unresolved tail keeps narrowing the predictive dictionary
        // lookup (わせd → 早稲田 stays selectable).
        let base = self.input_buf.reading();
        let pending = self.input_buf.pending();

        if reading.is_empty() {
            return None;
        }

        // Get candidates from kanji converter (use full num_candidates for explicit conversion)
        let mut candidates = self.build_conversion_candidates(
            &reading,
            &base,
            &pending,
            self.config.num_candidates,
            learning,
        );

        // What the preedit shows heads the list, so the window opens on it
        // (moved up when another source ranked it lower).
        if !prev_suggest_text.is_empty() {
            let shown = match candidates.iter().position(|c| c.text == prev_suggest_text) {
                Some(at) => candidates.remove(at),
                // Live text the learning cache answered is the learning's.
                None if self.live.learned => {
                    AnnotatedCandidate::new(prev_suggest_text, CandidateSource::Learning)
                }
                None => AnnotatedCandidate::new(prev_suggest_text, CandidateSource::Model),
            };
            candidates.insert(0, shown);
        }

        // A reading with romaji stranded between kana (ございmす) is a typo:
        // what one keystroke more or less makes of it, right after the first
        // candidate — offered, never put in place of what was typed.
        // Stray letters that spell an English word (あっpぇ) are no typo.
        let typo = spell_english(&reading, "", &self.dicts.english).is_none();
        let seen: HashSet<String> = candidates.iter().map(|c| c.text.clone()).collect();
        let retyped = if typo {
            self.retyped_candidates(&reading)
        } else {
            Vec::new()
        };
        let retyped: Vec<AnnotatedCandidate> = retyped
            .into_iter()
            .filter(|c| !seen.contains(&c.text))
            .collect();
        let at = candidates.len().min(1);
        candidates.splice(at..at, retyped);

        // A single `n` read the other way (ほにゃく → ほんやく): next to the
        // head, recorded under the typed reading like the English spelling.
        for retyping in self.n_retyped(&reading) {
            let text = self
                .model_candidates(&retyping.reading, 1)
                .into_iter()
                .next()
                .unwrap_or_else(|| retyping.reading.clone());
            if !candidates.iter().any(|c| c.text == text) {
                let at = candidates.len().min(1);
                candidates.insert(
                    at,
                    AnnotatedCandidate::new(text, CandidateSource::Model)
                        .with_description(Some(format!("n 補正: {}", retyping.reading))),
                );
            }
        }

        // English typed as romaji (yahoo → やほお): the same reading with
        // the word spelled out, next to the head. Recorded under the typed
        // reading, so once picked it heads the list itself.
        let head = candidates
            .first()
            .map_or(reading.as_str(), |c| c.text.as_str());
        if let Some(spelled) = spell_english(&reading, head, &self.dicts.english) {
            let text = self
                .model_candidates(&spelled, 1)
                .into_iter()
                .next()
                .unwrap_or_else(|| spelled.clone());
            if !candidates.iter().any(|c| c.text == text) {
                let at = candidates.len().min(1);
                candidates.insert(
                    at,
                    AnnotatedCandidate::new(text, CandidateSource::Model)
                        .with_description(Some(format!("英字: {spelled}"))),
                );
            }
        }

        if candidates.is_empty() {
            return None;
        }
        let candidate_list = self.to_conversion_candidate_list(candidates, &reading);
        Some((reading, candidate_list))
    }

    /// Map builder output to the public [`CandidateList`] shown in the
    /// conversion window, settled at the configured width.
    fn to_conversion_candidate_list(
        &self,
        candidates: Vec<AnnotatedCandidate>,
        reading: &str,
    ) -> CandidateList {
        self.settle_candidates(
            candidates
                .into_iter()
                .map(|ac| ac.into_candidate(reading))
                .collect(),
        )
    }

    /// Transition to Conversion state with the given reading and candidate list.
    ///
    /// Sets up the preedit (highlighted selected text), updates the state, and
    /// returns an EngineResult with preedit, candidates, and aux text actions.
    pub(super) fn enter_conversion_state(
        &mut self,
        reading: &str,
        candidates: CandidateList,
    ) -> EngineResult {
        let selected_text = candidates.selected_text().unwrap_or(reading).to_string();
        // A fresh window (not a source filter over an open one) opens on the
        // answer that needs no learning.
        if !matches!(self.state, InputState::Conversion { .. }) {
            self.conversion_origin = selected_text.clone();
        }

        let preedit = Preedit::with_text_highlighted(&selected_text);

        self.state = InputState::Conversion {
            preedit: preedit.clone(),
            candidates: candidates.clone(),
            reading: reading.to_string(),
            // A fresh conversion always starts unfiltered
            filter: None,
        };

        // After the state assignment: the aux header reads the active filter.
        let aux = self.format_aux_conversion_with_page(reading, Some(&candidates));

        EngineResult::consumed()
            .with_action(EngineAction::UpdatePreedit(preedit))
            .with_action(EngineAction::ShowCandidates(candidates))
            .with_action(EngineAction::UpdateAuxText(aux))
    }

    /// Dictionary candidates for a reading: user dict first, then system,
    /// exact matches then predictive (prefix-extending) ones, deduped.
    ///
    /// `pending` narrows the predictive lookup to readings the romaji tail
    /// can still become (わせ + `d` keeps わせだ…, drops わせり…);
    /// `predictive_limit` caps those results. `only` restricts search and
    /// dedup to one dictionary, so shared surfaces stay visible per view.
    pub(super) fn search_dictionaries(
        &self,
        reading: &str,
        pending: &str,
        limit: usize,
        predictive_limit: usize,
        min_prefix_chars: usize,
        only: Option<CandidateSource>,
    ) -> Vec<AnnotatedCandidate> {
        let dicts = [
            (self.dicts.user.as_ref(), CandidateSource::UserDictionary),
            (self.dicts.system.as_ref(), CandidateSource::Dictionary),
        ]
        .into_iter()
        .filter(|(_, source)| only.is_none_or(|o| o == *source))
        .collect::<Vec<_>>();
        let mut candidates = Vec::new();
        let mut seen = HashSet::new();

        // Exact matches, user dictionary first — only when no romaji tail
        // is pending (an exact hit on the base would ignore the typed
        // tail). Candidates are sorted by score at build/load time
        for &(dict, source) in &dicts {
            if !pending.is_empty() {
                break;
            }
            let Some(result) = dict.and_then(|d| d.exact_match_search(reading)) else {
                continue;
            };
            for cand in result.candidates {
                if candidates.len() >= limit {
                    break;
                }
                if seen.insert(cand.surface.clone()) {
                    candidates.push(AnnotatedCandidate::new(cand.surface.clone(), source));
                }
            }
        }

        // Predictive: dictionary readings extending the typed prefix,
        // mirroring the learning cache's prefix lookup. The full reading
        // rides on the candidate so selecting it commits and records under
        // the right key.
        let constraint = self.tail_constraint(pending);
        if self.config.predictive
            && !self.bounded
            && reading.chars().count() >= min_prefix_chars
            && !matches!(constraint, TailConstraint::Dead)
        {
            let mut budget = predictive_limit;
            for &(dict, source) in &dicts {
                if budget == 0 {
                    break;
                }
                let Some(dict) = dict else { continue };
                let matches = match &constraint {
                    TailConstraint::Unconstrained => dict.predictive_search(reading, budget),
                    TailConstraint::Narrow(expansions) => {
                        dict.predictive_search_expanded(reading, expansions, budget)
                    }
                    TailConstraint::Dead => unreachable!("checked above"),
                };
                for m in matches {
                    if budget == 0 || candidates.len() >= limit {
                        break;
                    }
                    if seen.insert(m.candidate.surface.clone()) {
                        budget -= 1;
                        candidates.push(
                            AnnotatedCandidate::new(m.candidate.surface.clone(), source)
                                .with_reading(Some(m.reading.to_string())),
                        );
                    }
                }
            }
        }

        candidates
    }

    /// Classify the unresolved romaji tail for predictive lookup.
    fn tail_constraint(&self, pending: &str) -> TailConstraint {
        if pending.is_empty() {
            return TailConstraint::Unconstrained;
        }
        let expansions = self.converters.romaji.pending_expansions(pending);
        if expansions.is_empty() {
            TailConstraint::Dead
        } else {
            TailConstraint::Narrow(expansions)
        }
    }

    /// Build the mixed candidate list, deduped in priority order:
    /// Learning → User Dictionary → Model → System Dictionary → Fallback.
    ///
    /// `base`/`pending` split the reading for the dictionary lookup (the
    /// unresolved romaji tail narrows prediction).
    pub(super) fn build_conversion_candidates(
        &mut self,
        reading: &str,
        base: &str,
        pending: &str,
        num_candidates: usize,
        learning: LearningLookup,
    ) -> Vec<AnnotatedCandidate> {
        // No converter (still loading in the background, or loading failed)
        // just means no model candidates: symbol-only and early keystrokes
        // still get dictionary/rewriter/fallback candidates. Loading here
        // synchronously would block the key-event thread on the download.
        let candidates = self.model_candidates(reading, num_candidates);
        self.model_head = candidates.first().map(|text| self.settle_text(text));

        let hiragana = reading.to_string();
        let katakana = mimir_engine::hiragana_to_katakana(reading);

        // Priority: Learning → User Dictionary → Model → System Dictionary → Fallback
        let mut builder = CandidateBuilder::new();

        // 1. Learning cache candidates (highest priority).
        //    Force-inserted so they win against duplicate text from later sources.
        //    Skipped when the caller asks for a learning-free conversion (Tab key).
        if learning == LearningLookup::Use {
            for c in self.lookup_learning_candidates(reading) {
                // Exact matches have reading == input reading; use None to avoid redundancy
                let cand_reading = c.reading.filter(|r| r != reading);
                builder.push_force(
                    AnnotatedCandidate::new(c.text, CandidateSource::Learning)
                        .with_reading(cand_reading),
                );
            }
        }

        // 2. User dictionary candidates (system dictionary follows the model
        //    in step 4, so the two are split here).
        let (user_dict, system_dict): (Vec<_>, Vec<_>) = self
            .search_dictionaries(
                base,
                pending,
                usize::MAX,
                usize::MAX,
                MIN_PREDICTIVE_PREFIX_CHARS,
                None,
            )
            .into_iter()
            .partition(|ac| ac.source == CandidateSource::UserDictionary);
        for ac in user_dict {
            builder.push(ac);
        }

        // 3. Model inference results
        if candidates.is_empty() {
            // No literal fallback in emoji mode: `:smile` must not outrank
            // the 😄 surfaced by the rewriter step below.
            if builder.is_empty() && self.mode.current() != InputMode::Emoji {
                builder.push(AnnotatedCandidate::new(
                    hiragana.clone(),
                    CandidateSource::Fallback,
                ));
            }
        } else {
            for text in candidates {
                builder.push(AnnotatedCandidate::new(text, CandidateSource::Model));
            }
        }

        // 3b. Earlier commits extending the reading, to each word end (history
        //     completion), a page at most: after the model's answers, which
        //     the live head already leads with. What is offered is
        //     remembered: taking one goes on to the entry's next words
        //     (`continue_chain`). Apart from `predictive`, which is the
        //     dictionary's guessing: these were all typed before.
        if learning == LearningLookup::Use && pending.is_empty() {
            let completions: Vec<Candidate> = self
                .history_completions(reading)
                .into_iter()
                .take(CandidateList::DEFAULT_PAGE_SIZE)
                .collect();
            self.offer_completions(&completions);
            for c in completions {
                builder.push(
                    AnnotatedCandidate::new(c.text, CandidateSource::Learning)
                        .with_reading(c.reading),
                );
            }
        }

        // 4. System dictionary candidates
        for ac in system_dict {
            builder.push(ac);
        }

        // 5/6. Hiragana/katakana fallback + rewriter variants. Emoji mode
        // shows rewriter (emoji) candidates only — no kana pair, like an
        // emoji picker; Enter in Composing still commits the literal query.
        if self.mode.current() != InputMode::Emoji {
            builder.push(AnnotatedCandidate::new(hiragana, CandidateSource::Fallback));
            builder.push(AnnotatedCandidate::new(katakana, CandidateSource::Fallback));
        }
        // Date/time candidates sit above the width/kana variants; like the
        // rewriters they derive from the typed reading alone.
        for (variant, description) in self.date_variants(reading) {
            builder.push(
                AnnotatedCandidate::new(variant, CandidateSource::Date)
                    .with_description(description),
            );
        }
        // Rewriters run on the typed reading only; running them on other
        // sources' candidates would emit variants nobody asked for.
        for (variant, description) in self.rewriter_variants(reading) {
            builder.push(
                AnnotatedCandidate::new(variant, CandidateSource::Rewriter)
                    .with_description(description),
            );
        }

        // 7. Back-fill descriptions. Symbol names are Fallback-only —
        //    model/dict/learning candidates must not inherit labels like
        //    「金 = 部首」 — while width annotations (`[全]カタカナ`) apply to
        //    any pure-kana candidate that still has none.
        for c in &mut builder.candidates {
            if c.description.is_some() {
                continue;
            }
            let symbol = (c.source == CandidateSource::Fallback)
                .then(|| mimir_engine::symbol_description(&c.text))
                .flatten();
            c.description = symbol
                .or_else(|| width_annotation(&c.text))
                .map(str::to_string);
        }

        builder.into_candidates()
    }

    /// Learning cache candidates for a reading (at most 3), for the
    /// composing window and the mixed conversion list.
    pub(super) fn lookup_learning_candidates(&self, reading: &str) -> Vec<Candidate> {
        self.lookup_learning(reading, "", MAX_LEARNING_CANDIDATES)
    }

    /// Every learned pick for `reading` (uncapped), for the learning view.
    pub(super) fn lookup_learning_history(&self, reading: &str, pending: &str) -> Vec<Candidate> {
        self.lookup_learning(reading, pending, usize::MAX)
    }

    /// Learned picks for `reading`, the exact reading only: learning holds
    /// what the model can't give, not what to complete to — completing is
    /// the history's (`history_completions`). Nothing while a romaji tail
    /// is pending: an exact hit on the base would ignore the typed tail.
    fn lookup_learning(&self, reading: &str, pending: &str, max: usize) -> Vec<Candidate> {
        let Some(cache) = &self.learning else {
            return vec![];
        };
        if !pending.is_empty() {
            return vec![];
        }
        let lctx = self.truncate_context_for_api();
        let mut seen = HashSet::new();
        cache
            .lookup_with_context(reading, &lctx)
            .into_iter()
            .filter(|(surface, _)| seen.insert(surface.clone()))
            .take(max)
            .map(|(surface, _)| Candidate {
                text: surface,
                reading: Some(reading.to_string()),
                source: Some(CandidateSource::Learning),
                description: None,
            })
            .collect()
    }

    /// Dictionary candidates for the composing suggestion list (one page).
    pub(super) fn lookup_dict_candidates(&self, reading: &str) -> Vec<Candidate> {
        let pending = self.input_buf.pending();
        self.search_dictionaries(
            reading,
            &pending,
            CandidateList::DEFAULT_PAGE_SIZE,
            MAX_PREDICTIVE_SUGGESTIONS,
            MIN_PREDICTIVE_PREFIX_CHARS,
            None,
        )
        .into_iter()
        .map(|ac| ac.into_candidate(reading))
        .collect()
    }

    /// Rewriter variants for `reading`, as `(text, description)` pairs.
    ///
    /// In emoji mode only the emoji rewriter runs: `:smile` is a query, and
    /// another rewriter's width variant (`：ｓｍｉｌｅ`) would head the
    /// picker and be what Enter commits.
    pub(super) fn rewriter_variants(&self, reading: &str) -> Vec<RewriteOutput> {
        if self.mode.current() == InputMode::Emoji {
            return EmojiRewriter.rewrite(reading);
        }
        self.converters
            .rewriters
            .rewrite_all(&[reading.to_string()])
    }

    /// Date/time candidates for `reading` (`[date]` phrases). None in emoji
    /// mode — the picker shows emojis only.
    pub(super) fn date_variants(&self, reading: &str) -> Vec<RewriteOutput> {
        if self.mode.current() == InputMode::Emoji {
            return Vec::new();
        }
        self.converters.date.rewrite(reading)
    }

    /// Build rule-based rewriter variants for the reading itself (e.g. for
    /// symbol input `「` → `『`, `【`, `（`, ...), date/time candidates first.
    /// Used in the auto-suggest path so users see mozc-style symbol variants
    /// without pressing Space first, and as the body of the Ctrl+R view.
    pub(super) fn lookup_rewriter_variants(&self, reading: &str) -> Vec<Candidate> {
        let as_candidate = |source: CandidateSource| {
            move |(text, description): RewriteOutput| Candidate {
                text,
                reading: Some(reading.to_string()),
                source: Some(source),
                description,
            }
        };
        self.date_variants(reading)
            .into_iter()
            .map(as_candidate(CandidateSource::Date))
            .chain(
                self.rewriter_variants(reading)
                    .into_iter()
                    .map(as_candidate(CandidateSource::Rewriter)),
            )
            .collect()
    }

    pub(super) fn process_key_conversion(&mut self, key: &KeyEvent) -> EngineResult {
        // Alt chords pass through before any binding matches: Alt+Tab must
        // navigate and Alt+Return must not commit.
        if key.modifiers.alt_key {
            return EngineResult::not_consumed();
        }
        match key.keysym {
            Keysym::RETURN => self.commit_conversion(),
            Keysym::ESCAPE => self.cancel_conversion(),
            // Shift+Tab (ISO_Left_Tab on X11) and Shift+Space step back the
            // way Tab and Space step forward: mozc-compatible muscle memory.
            Keysym::ISO_LEFT_TAB => self.prev_candidate(),
            Keysym::TAB | Keysym::SPACE if key.modifiers.shift_key => self.prev_candidate(),
            Keysym::SPACE | Keysym::DOWN | Keysym::TAB => self.next_candidate(),
            Keysym::UP => self.prev_candidate(),
            Keysym::PAGE_DOWN => self.next_candidate_page(),
            Keysym::PAGE_UP => self.prev_candidate_page(),
            // Ctrl+Backspace / Ctrl+Delete: delete the selected learning
            // candidate (the Mac "delete" key is Backspace). A non-learning
            // selection consumes the chord as a no-op.
            Keysym::DELETE | Keysym::BACKSPACE if key.modifiers.control_key => {
                if self.selected_is_deletable() {
                    self.delete_selected_candidate_from_history()
                } else {
                    EngineResult::consumed()
                }
            }
            // Inside a narrowed view Backspace shrinks the reading and
            // stays in the view — the mirror of typing-refine, so the list
            // re-expands as the query shrinks. Without a filter it returns
            // to the composition as before.
            Keysym::BACKSPACE if self.state.filter().is_some() => {
                self.refine_through_composing(key)
            }
            // Backspace cancels back to the composition, like Escape.
            Keysym::BACKSPACE => self.cancel_conversion(),
            // Caret keys drop back to editing, the same way a caret move
            // ends the live-conversion display while composing: the
            // conversion (and its source filter) dissolves and the raw
            // reading gets the caret. Delegated to the composing handler so
            // the two states cannot drift apart.
            Keysym::LEFT | Keysym::RIGHT | Keysym::HOME | Keysym::END => {
                self.in_composing(false, |e| e.process_key_composing(key))
            }
            _ => {
                // Ctrl+N / Ctrl+P: emacs-style candidate navigation
                if key.modifiers.control_key {
                    match key.keysym {
                        Keysym::KEY_N | Keysym::KEY_N_UPPER => return self.next_candidate(),
                        Keysym::KEY_P | Keysym::KEY_P_UPPER => return self.prev_candidate(),
                        // Ctrl+R / Ctrl+T: cycle the source filter. Both
                        // keysym cases — some environments fold Shift into
                        // an uppercase keysym; direction must not change.
                        Keysym::KEY_R | Keysym::KEY_R_UPPER => {
                            return self.cycle_candidate_filter(FilterDirection::Backward);
                        }
                        Keysym::KEY_T | Keysym::KEY_T_UPPER => {
                            return self.cycle_candidate_filter(FilterDirection::Forward);
                        }
                        // Ctrl+J: split at the caret and rebuild, so the
                        // alternatives cover only the text after the break.
                        Keysym::KEY_J | Keysym::KEY_J_UPPER => {
                            return self.rebreak_conversion();
                        }
                        // Ctrl+A/B/E/F: the same caret moves as while
                        // composing, dropping back to editing like the
                        // arrow keys above.
                        Keysym::KEY_A
                        | Keysym::KEY_A_UPPER
                        | Keysym::KEY_B
                        | Keysym::KEY_B_UPPER
                        | Keysym::KEY_E
                        | Keysym::KEY_E_UPPER
                        | Keysym::KEY_F
                        | Keysym::KEY_F_UPPER => {
                            return self.in_composing(false, |e| e.process_key_composing(key));
                        }
                        _ => {}
                    }

                    // Ctrl+Y/U/I/O: jump straight to one source's view.
                    if let Some(source) = source_for_key(key.keysym) {
                        return self.jump_to_source(source);
                    }

                    // Ctrl+1..9: select and commit that candidate. Bare
                    // digits refine below like any printable character, so
                    // typing numbers never conflicts with selection.
                    if let Some(digit) = key.keysym.digit_value() {
                        return self.select_shown_candidate(digit);
                    }
                }

                // A printable character refines instead of committing:
                // the reading grows and the suggestion rewrites in place,
                // keeping any active source filter.
                if key.to_char().is_some() && !key.modifiers.control_key {
                    return self.refine_through_composing(key);
                }

                // Everything else is consumed as a no-op — leaked chords
                // would let the app act on them mid-conversion (e.g. a
                // browser reloading on Ctrl+R).
                EngineResult::consumed()
            }
        }
    }

    /// Feed a refining keystroke (printable char, Backspace) through the
    /// composing path, then re-enter the conversion with the previous
    /// source filter if one was active. With a filter the composing render
    /// is discarded, so its auto-suggest inference is suppressed.
    fn refine_through_composing(&mut self, key: &KeyEvent) -> EngineResult {
        let filter = self.state.filter();
        let result =
            self.in_composing(filter.is_some(), |engine| engine.process_key_composing(key));
        if let Some(source) = filter
            && matches!(self.state, InputState::Composing { .. })
        {
            return self.start_conversion_with_filter(source);
        }
        result
    }

    /// Insert a chunk break at the caret without leaving the conversion.
    /// The span is the last chunk, so breaking narrows what the beam
    /// covers; the rebuilt list keeps the active source filter. The
    /// intermediate composing render is discarded, so its auto-suggest
    /// inference is suppressed.
    fn rebreak_conversion(&mut self) -> EngineResult {
        let filter = self.state.filter();
        self.in_composing(true, |engine| engine.insert_chunk_break());
        match filter {
            Some(source) => self.start_conversion_with_filter(source),
            None => self.start_conversion(LearningLookup::Use),
        }
    }

    /// Drop back to the untouched composition and run `edit` there. Set
    /// `discard_render` when the caller rebuilds the conversion afterwards:
    /// the composing render is thrown away, so its auto-suggest inference
    /// would be pure waste. The flag lives and dies inside this call, so no
    /// other path can inherit it.
    fn in_composing<R>(&mut self, discard_render: bool, edit: impl FnOnce(&mut Self) -> R) -> R {
        self.set_composing_state();
        self.suppress_suggest = discard_render;
        let out = edit(self);
        self.suppress_suggest = false;
        out
    }

    /// Get selected text and reading from conversion state, or None if not in conversion
    pub(super) fn selected_conversion_info(&self) -> Option<(String, Option<String>)> {
        match &self.state {
            InputState::Conversion {
                candidates,
                reading,
                ..
            } => {
                // An empty (source-filtered) view displays the raw reading
                // as its preedit, so that is what committing produces —
                // never an empty commit that would eat the composition.
                let text = candidates.selected_text().unwrap_or(reading).to_string();
                // The reading rides along solely for the learning record;
                // a non-learnable source (a date is stale tomorrow) yields
                // None so nothing is recorded.
                let reading = candidates
                    .selected()
                    .filter(|c| c.source.is_none_or(|s| s.is_learnable()))
                    .and_then(|c| c.reading.clone());
                Some((text, reading))
            }
            _ => None,
        }
    }

    /// Record a selection in the learning cache. No-op in emoji mode — the
    /// buffer is a `:query`, not a kana reading, and would corrupt the
    /// kana-keyed cache.
    pub(super) fn record_learning(&mut self, reading: &str, surface: &str) {
        if let Some(pieces) = &mut self.pieces {
            pieces.picked = true;
            return;
        }
        if self.mode.current() == InputMode::Emoji {
            return;
        }
        let lctx = self.truncate_context_for_api();
        if let Some(cache) = &mut self.learning {
            cache.record_with_context(reading, surface, &lctx);
        }
    }

    /// Record the committed conversion in the learning cache — when it is
    /// not what the window opened on — and end the composition.
    pub(super) fn finish_conversion(&mut self, text: &str, reading: &Option<String>) {
        if self.follow_chain(text, reading) {
            // A word of an entry the history already holds: nothing to note.
            self.end_composition();
            return;
        }
        let picked = reading.is_some() && text != self.conversion_origin;
        if let Some(pieces) = &mut self.pieces {
            pieces.push(reading.as_deref(), text, picked);
            self.end_composition();
            return;
        }
        if let Some(reading) = reading {
            self.record_history(reading, text);
        }
        if let Some(reading) = reading
            && text != self.conversion_origin
        {
            self.record_learning(reading, text);
            self.chosen = Some((reading.clone(), text.to_string()));
        }
        // Learning is a scaffold for what the model can't yet give: taken as
        // the window opened, and the model's own answer too, the lone learned
        // entry for the reading has done its job.
        if let Some(reading) = reading
            && text == self.conversion_origin
            && self.model_head.as_deref() == Some(text)
        {
            self.retire_learning(reading, text);
        }
        self.end_composition();
    }

    /// Forget `reading → surface` when it is all the learning cache holds
    /// for the reading (more than one, and it is what ranks it above the
    /// others).
    fn retire_learning(&mut self, reading: &str, surface: &str) {
        if let Some(cache) = &mut self.learning
            && let [(learned, _)] = cache.lookup(reading).as_slice()
            && learned == surface
        {
            cache.remove(reading, surface);
        }
    }

    /// [`n_retypings`] of `reading`, looked up in both dictionaries.
    pub(super) fn n_retyped(&self, reading: &str) -> Vec<NRetyping> {
        let dicts = [&self.dicts.user, &self.dicts.system];
        n_retypings(reading, |word| {
            let entries: Vec<_> = dicts
                .iter()
                .filter_map(|d| d.as_ref()?.exact_match_search(word))
                .collect();
            if entries.is_empty() {
                return None;
            }
            let kanji = |surface: &str| {
                surface
                    .chars()
                    .any(|c| !c.is_ascii() && !mimir_engine::contains_kana(&c.to_string()))
            };
            Some(
                entries
                    .iter()
                    .flat_map(|e| e.candidates)
                    .filter(|c| kanji(&c.surface))
                    .map(|c| c.score)
                    .fold(f32::INFINITY, f32::min),
            )
        })
    }

    /// [`retypings`] of `reading`, each converted as the model would (or
    /// as kana without it), marked with the reading it stands for.
    fn retyped_candidates(&mut self, reading: &str) -> Vec<AnnotatedCandidate> {
        retypings(reading, &self.converters.romaji)
            .into_iter()
            .map(|retyped| {
                let text = self
                    .model_candidates(&retyped, 1)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| retyped.clone());
                AnnotatedCandidate::new(text, CandidateSource::Model)
                    .with_description(Some(format!("打ち直し: {retyped}")))
                    .with_reading(Some(retyped))
            })
            .collect()
    }

    /// Commit the current conversion
    fn commit_conversion(&mut self) -> EngineResult {
        let Some((text, reading)) = self.selected_conversion_info() else {
            return EngineResult::not_consumed();
        };

        if text.is_empty() {
            return EngineResult::consumed();
        }

        self.finish_conversion(&text, &reading);

        EngineResult::consumed()
            .with_action(EngineAction::HideCandidates)
            .with_action(EngineAction::HideAuxText)
            .with_action(EngineAction::Commit(text))
    }

    /// Whether the selected candidate can be removed from the learning
    /// history. False when nothing is selected, so the delete chord stays
    /// inert outside the case it is meant for.
    fn selected_is_deletable(&self) -> bool {
        self.state
            .candidates()
            .and_then(|c| c.selected())
            .is_some_and(Candidate::is_deletable)
    }

    /// Delete the selected learning candidate and its prefix twins from the
    /// history, then rebuild the conversion in place — dedup hid any other
    /// source's copy of the surface, and only a rebuild brings it back.
    /// The caller guards deletability ([`Self::selected_is_deletable`]).
    fn delete_selected_candidate_from_history(&mut self) -> EngineResult {
        let Some(surface) = self
            .state
            .candidates()
            .and_then(|c| c.selected())
            .map(|c| c.text.clone())
        else {
            return EngineResult::consumed();
        };
        // Remove by the typed reading: every entry surfacing this row has
        // it as a prefix, so the row and its twins clear together.
        let Some(reading) = self.state.reading().map(str::to_string) else {
            return EngineResult::consumed();
        };
        let removed = self
            .learning
            .as_mut()
            .is_some_and(|cache| cache.remove_suggestion(&reading, &surface));
        // A history completion shares the Learning source but comes from
        // `history`, cut short of its entry; only a row actually offered
        // as one is cleared there, so deleting a plain learned 今日 leaves
        // 今日はいい天気 alone.
        let completed = self
            .history_completions(&reading)
            .into_iter()
            .any(|c| c.text == surface);
        let removed = removed
            | (completed
                && self
                    .history
                    .as_mut()
                    .is_some_and(|cache| cache.remove_completion(&reading, &surface)));
        if !removed {
            return EngineResult::consumed();
        }
        debug!("deleted learning entry: {} -> {}", reading, surface);

        // Keep the filter and cursor so consecutive deletes stay in the
        // narrowed view and chew through the list top-down.
        let prev_filter = self.state.filter();
        let prev_cursor = self.state.candidates().map(|c| c.cursor()).unwrap_or(0);

        let candidates = self.build_conversion_candidates(
            &reading,
            &reading,
            "",
            self.config.num_candidates,
            LearningLookup::Use,
        );
        if candidates.is_empty() {
            return self.cancel_conversion();
        }
        let candidate_list = self.to_conversion_candidate_list(candidates, &reading);
        let mut result = self.enter_conversion_state(&reading, candidate_list);

        if let Some(source) = prev_filter {
            result = self.apply_candidate_filter(source);
        }
        if self.state.candidates().is_some_and(|c| !c.is_empty()) {
            return self.navigate_candidate(|c| {
                c.set_cursor(prev_cursor);
                true
            });
        }
        result
    }
    pub(super) fn cancel_conversion(&mut self) -> EngineResult {
        if !matches!(self.state, InputState::Conversion { .. }) {
            return EngineResult::not_consumed();
        }
        self.history_window = false;

        if self.input_buf.is_empty() {
            self.state = InputState::Empty;
            return EngineResult::consumed()
                .with_action(EngineAction::UpdatePreedit(Preedit::new()))
                .with_action(EngineAction::HideCandidates)
                .with_action(EngineAction::HideAuxText);
        }

        // The composition was left untouched when the conversion started:
        // just come back to it, pending romaji still live
        let preedit = self.set_composing_state();

        EngineResult::consumed()
            .with_action(EngineAction::UpdatePreedit(preedit))
            .with_action(EngineAction::HideCandidates)
            .with_action(EngineAction::UpdateAuxText(self.format_aux_composing()))
    }

    /// Navigate candidates with the given operation, then update preedit
    fn navigate_candidate(&mut self, op: impl FnOnce(&mut CandidateList) -> bool) -> EngineResult {
        let (selected_text, candidates) = {
            let Some(candidates) = self.state.candidates_mut() else {
                return EngineResult::not_consumed();
            };
            // Nothing to navigate in an empty (source-filtered) view; keep
            // the reading preedit instead of blanking it.
            if candidates.is_empty() {
                return EngineResult::consumed();
            }
            op(candidates);
            let text = candidates.selected_text().unwrap_or("").to_string();
            (text, candidates.clone())
        };
        self.update_conversion_preedit(&selected_text, candidates)
    }

    /// Select next candidate
    fn next_candidate(&mut self) -> EngineResult {
        self.navigate_candidate(CandidateList::move_next)
    }

    /// Select previous candidate
    fn prev_candidate(&mut self) -> EngineResult {
        self.navigate_candidate(CandidateList::move_prev)
    }

    /// Go to next candidate page
    fn next_candidate_page(&mut self) -> EngineResult {
        self.navigate_candidate(CandidateList::next_page)
    }

    /// Go to previous candidate page
    fn prev_candidate_page(&mut self) -> EngineResult {
        self.navigate_candidate(CandidateList::prev_page)
    }

    /// Select and commit the candidate at `page_index` (0-based) within the
    /// current page, like pressing the digit key `page_index + 1`. Not
    /// consumed unless a candidate list is active (Conversion state).
    pub fn select_candidate_on_page(&mut self, page_index: usize) -> EngineResult {
        let start = std::time::Instant::now();
        self.metrics.conversion_ms = 0;
        let result = self.select_shown_candidate(page_index + 1);
        self.metrics.process_key_ms = start.elapsed().as_millis() as u64;
        result
    }

    /// Update preedit after candidate selection change
    fn update_conversion_preedit(
        &mut self,
        selected_text: &str,
        candidates: CandidateList,
    ) -> EngineResult {
        let preedit = Preedit::with_text_highlighted(selected_text);

        if let Some(p) = self.state.preedit_mut() {
            *p = preedit.clone();
        }

        let reading = candidates
            .selected()
            .and_then(|c| c.reading.clone())
            .unwrap_or_default();
        let aux = self.format_aux_conversion_with_page(&reading, Some(&candidates));

        EngineResult::consumed()
            .with_action(EngineAction::UpdatePreedit(preedit))
            .with_action(EngineAction::ShowCandidates(candidates))
            .with_action(EngineAction::UpdateAuxText(aux))
    }
}

/// Readings one keystroke away from `reading` where it strands lowercase
/// romaji between kana (ございmす: masu typed msu): the stray letters with a
/// vowel after them, or without them. Only for a single stray run with kana
/// after it (a run at the end is still being typed); each result is all
/// kana.
pub(super) fn retypings(reading: &str, romaji: &mimir_engine::RomajiConverter) -> Vec<String> {
    let chars: Vec<char> = reading.chars().collect();
    let stray = |c: &char| c.is_ascii_lowercase();
    let Some(start) = chars.iter().position(stray) else {
        return Vec::new();
    };
    let end = start + chars[start..].iter().take_while(|c| stray(c)).count();
    if end == chars.len() || chars[end..].iter().any(stray) {
        return Vec::new();
    }
    let head: String = chars[..start].iter().collect();
    let letters: String = chars[start..end].iter().collect();
    let tail: String = chars[end..].iter().collect();
    let mut out: Vec<String> = Vec::new();
    for vowel in ['a', 'i', 'u', 'e', 'o'] {
        let converted = romaji.convert(&format!("{letters}{vowel}"));
        if converted.pending.is_empty() && !converted.text.chars().any(|c| c.is_ascii_alphabetic())
        {
            out.push(format!("{head}{}{tail}", converted.text));
        }
    }
    out.push(format!("{head}{tail}"));
    out.dedup();
    out
}

/// Readings a single `n` may have meant where it took the next key along
/// (ほにゃく for `honyaku`): ny+vowel read ん + y (ほんやく), n+vowel read
/// ん + vowel (げにん → げんいん), or the `nn` of ん + な行 typed once
/// (はにち → はんにち). Each is kept only where it makes a long, common
/// dictionary word across the ん and the reading as typed makes none across
/// that spot, best first, at most [`MAX_N_RETYPINGS`].
///
/// `word_cost` is a reading's lowest cost written in kanji: `None` for no
/// word, infinite for a word written in kana alone.
pub(super) fn n_retypings(
    reading: &str,
    word_cost: impl Fn(&str) -> Option<f32>,
) -> Vec<NRetyping> {
    let chars: Vec<char> = reading.chars().collect();
    // Longest word in `chars` costing at most `max` that runs from at or
    // before `from` to at or after `to` (inclusive char indices).
    let covering = |chars: &[char], from: usize, to: usize, max: f32| {
        let mut best = 0;
        for start in from.saturating_sub(MAX_WORD_CHARS - 1)..=from {
            for end in to + 1..=chars.len().min(start + MAX_WORD_CHARS) {
                let word: String = chars[start..end].iter().collect();
                if end - start > best && word_cost(&word).is_some_and(|c| c <= max) {
                    best = end - start;
                }
            }
        }
        best
    };
    let mut found: Vec<(usize, NRetyping)> = Vec::new();
    for i in 1..chars.len() {
        let prev = chars[i - 1];
        if !matches!(prev, 'ぁ'..='ゖ') || matches!(prev, 'ん' | 'っ') {
            continue;
        }
        let small_y = chars.get(i + 1).and_then(|&c| match c {
            'ゃ' => Some('や'),
            'ゅ' => Some('ゆ'),
            'ょ' => Some('よ'),
            _ => None,
        });
        let vowel = match chars[i] {
            'な' => 'あ',
            'に' => 'い',
            'ぬ' => 'う',
            'ね' => 'え',
            'の' => 'お',
            _ => continue,
        };
        // (replacement for chars[i..i + taken], ny or not)
        let mut options = vec![(vec!['ん', chars[i]], 1, false)];
        match small_y {
            Some(y) if chars[i] == 'に' => options.push((vec!['ん', y], 2, true)),
            _ => options.push((vec!['ん', vowel], 1, false)),
        }
        // A particle right after a word (駅 + に of えきにつく) is just that,
        // however common the word the ん would make (均一).
        let after_word = (i.saturating_sub(MAX_WORD_CHARS)..i - 1).any(|start| {
            let word: String = chars[start..i].iter().collect();
            word_cost(&word).is_some_and(|c| c < f32::MAX)
        });
        if after_word && matches!(chars[i], 'な' | 'に' | 'ね' | 'の') {
            continue;
        }
        // Words written in kana alone are left out: the dictionary carries
        // junk like きにょう that would hide きんよう.
        let typed = covering(&chars, i - 1, i, f32::MAX);
        for (with, taken, ny) in options {
            let mut variant = chars[..i].to_vec();
            variant.extend(&with);
            variant.extend(&chars[i + taken..]);
            // にゃ/にゅ/にょ are rare enough to take a shorter word (今夜).
            let min = if ny {
                MIN_N_WORD_CHARS - 1
            } else {
                MIN_N_WORD_CHARS
            };
            let word = covering(&variant, i - 1, i + 1, MAX_N_WORD_COST);
            if word >= min && typed < min - 1 {
                let gain = word - typed;
                let reading = variant.into_iter().collect();
                found.push((gain, NRetyping { reading, at: i, ny }));
            }
        }
    }
    found.sort_by_key(|a| std::cmp::Reverse(a.0));
    found.truncate(MAX_N_RETYPINGS);
    found.into_iter().map(|(_, r)| r).collect()
}

/// One reading [`n_retypings`] offers.
#[derive(Debug, PartialEq)]
pub(super) struct NRetyping {
    pub reading: String,
    /// Char index of the kana the single `n` made (に of ほにゃく)
    pub at: usize,
    /// ny + vowel read as ん + y: a にゃ/にゅ/にょ few words have, so one the
    /// conversion left as kana is taken as ん + y in live conversion too.
    pub ny: bool,
}

/// At most this many [`n_retypings`] are offered, each costing an inference.
const MAX_N_RETYPINGS: usize = 2;

/// Shortest word a [`n_retypings`] reading must make across the ん (ほんやく),
/// where the reading as typed makes at most one char less than that minus
/// one (ほに): short words are everywhere, and これはしない must not turn
/// into 親愛 for its しな.
const MIN_N_WORD_CHARS: usize = 4;

/// Costliest word a [`n_retypings`] reading may make: 翻訳 / 原因 / 半日
/// are 6250–6520, while rarities a particle runs into (ねこが + ない →
/// 眼内 7138, なにも + ない → 門内 7404) are not.
const MAX_N_WORD_COST: f32 = 6600.0;

/// Longest dictionary reading [`n_retypings`] looks for across a spot.
const MAX_WORD_CHARS: usize = 8;

/// Longest reading an English word is looked up under (`kubernetes` →
/// くべrねてs is 8).
const MAX_ENGLISH_CHARS: usize = 16;

/// `reading` with every English word typed as romaji spelled out
/// (やほおちえぶくろ → Yahooちえぶくろ); `None` when there is none.
pub(super) fn spell_english(
    reading: &str,
    head: &str,
    words: &HashMap<String, String>,
) -> Option<String> {
    let chars: Vec<char> = reading.chars().collect();
    let spans = english_spans(&chars, head, words);
    if spans.is_empty() {
        return None;
    }
    let mut out = String::new();
    let mut at = 0;
    for (start, end, word) in spans {
        out.extend(&chars[at..start]);
        out.push_str(word);
        at = end;
    }
    out.extend(&chars[at..]);
    Some(out)
}

/// The English words typed as romaji in `chars`, as `(start, end, word)`,
/// taking the longest word at each position, left to right.
///
/// A word whose romaji is all kana counts only where `head` (what the
/// conversion would show) left that kana as typed: うにいく is a word, but
/// not inside ほっかいどうにいく, which the model made 北海道に行く. One that
/// strands a letter (ぎてゅb) is English wherever it appears; an empty
/// `head` finds those alone.
// ponytail: a word typed right before a vowel merges with it as romaji
// (`githubapi` → ぎてゅばぴ) and is not found; split on raw keystrokes if
// that turns out to matter.
pub(super) fn english_spans<'a>(
    chars: &[char],
    head: &str,
    words: &'a HashMap<String, String>,
) -> Vec<(usize, usize, &'a str)> {
    let mut spans = Vec::new();
    if words.is_empty() {
        return spans;
    }
    let mut i = 0;
    while i < chars.len() {
        let word = (1..=MAX_ENGLISH_CHARS.min(chars.len() - i))
            .rev()
            .find_map(|n| {
                let key: String = chars[i..i + n].iter().collect();
                let word = words.get(&key)?;
                (key.chars().any(|c| c.is_ascii()) || head.contains(&key))
                    .then_some((n, word.as_str()))
            });
        match word {
            Some((n, w)) => {
                spans.push((i, i + n, w));
                i += n;
            }
            None => i += 1,
        }
    }
    spans
}
