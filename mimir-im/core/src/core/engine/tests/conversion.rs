use super::*;

#[test]
fn test_conversion_char_refines_reading() {
    let mut engine = InputMethodEngine::new();

    // Type "あい" and enter conversion
    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    // Typing during conversion must NOT commit — it drops back to the
    // composition and extends the reading (incremental-search feel).
    let result = engine.process_key(&press('k'));
    assert!(result.consumed);
    assert!(
        !result
            .actions
            .iter()
            .any(|a| matches!(a, EngineAction::Commit(_))),
        "typing must refine, not commit"
    );
    assert!(matches!(engine.state(), InputState::Composing { .. }));

    engine.process_key(&press('a'));
    assert_eq!(engine.input_buf.reading(), "あいか");

    // The refined reading converts and commits as one unit.
    engine.process_key(&press_key(Keysym::SPACE));
    assert!(matches!(engine.state(), InputState::Conversion { .. }));
    let result = engine.process_key(&press_key(Keysym::RETURN));
    assert!(
        result
            .actions
            .iter()
            .any(|a| matches!(a, EngineAction::Commit(_)))
    );
}

#[test]
fn test_alphabet_mode_space_inserts_literal_space() {
    let mut engine = InputMethodEngine::new();

    // Enter alphabet mode via Shift+N
    engine.process_key(&press_shift('N'));
    assert!(engine.mode.current() == InputMode::Alphabet);

    // Type "ew"
    engine.process_key(&press('e'));
    engine.process_key(&press('w'));
    assert_eq!(engine.preedit().unwrap().text(), "New");

    // Space → should insert literal space, NOT start conversion
    engine.process_key(&press_key(Keysym::SPACE));
    assert!(matches!(engine.state(), InputState::Composing { .. }));
    assert_eq!(engine.preedit().unwrap().text(), "New ");

    // Type "york"
    engine.process_key(&press('y'));
    engine.process_key(&press('o'));
    engine.process_key(&press('r'));
    engine.process_key(&press('k'));
    assert_eq!(engine.preedit().unwrap().text(), "New york");
}

#[test]
fn test_stray_keys_are_consumed_during_conversion() {
    // Unbound chords and special keys must be consumed as no-ops while the
    // conversion window is shown — leaking them would let the application
    // act on them (e.g. Ctrl+R reloading a browser page) mid-conversion.
    let mut engine = InputMethodEngine::new();
    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));
    assert!(matches!(engine.state(), InputState::Conversion { .. }));
    let cursor = engine.candidates().unwrap().cursor();

    for key in [
        press_ctrl(Keysym(0x0067)), // Ctrl+g (unbound)
        press_ctrl(Keysym(0x0077)), // Ctrl+w (unbound; closes a browser tab)
        press_key(Keysym(0xffc2)),  // F5
    ] {
        let result = engine.process_key(&key);
        assert!(result.consumed, "key must not leak to the application");
        assert!(matches!(engine.state(), InputState::Conversion { .. }));
        assert_eq!(engine.candidates().unwrap().cursor(), cursor);
    }
}

/// Text of the first Commit action in a result, if any.
fn committed(result: &EngineResult) -> Option<String> {
    result.actions.iter().find_map(|a| match a {
        EngineAction::Commit(text) => Some(text.clone()),
        _ => None,
    })
}

#[test]
fn test_bare_digit_during_conversion_refines_instead_of_selecting() {
    // Digits are plain text input everywhere: during conversion they extend
    // the reading like any printable char, never select a candidate.
    let mut engine = InputMethodEngine::new();
    engine.dicts.user = Some(dict_from_json(
        r#"[{"reading":"あい","candidates":[{"surface":"藍","score":1.0}]}]"#,
    ));

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    let result = engine.process_key(&press('2'));
    assert!(committed(&result).is_none(), "a digit must not commit");
    assert_eq!(engine.input_buf.reading(), "あい2");
}

#[test]
fn test_ctrl_digit_selects_candidate_during_conversion() {
    let mut engine = InputMethodEngine::new();
    engine.dicts.user = Some(dict_from_json(
        r#"[{"reading":"あい","candidates":[
            {"surface":"藍","score":2.0},
            {"surface":"愛","score":1.0}
        ]}]"#,
    ));

    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));
    let shown: Vec<String> = engine
        .candidates()
        .unwrap()
        .candidates()
        .iter()
        .map(|c| c.text.clone())
        .collect();

    let result = engine.process_key(&press_ctrl(Keysym::KEY_2));
    assert_eq!(committed(&result).as_deref(), Some(shown[1].as_str()));
    assert!(matches!(engine.state(), InputState::Empty));
    assert!(engine.input_buf.is_empty(), "buffer must be cleared");
}

#[test]
fn test_ctrl_digit_selects_candidate_while_composing() {
    // The suggestion window is on screen while composing, so Ctrl+digit
    // commits straight from it — no Space needed first.
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;
    engine.dicts.user = Some(dict_from_json(
        r#"[{"reading":"あい","candidates":[{"surface":"藍","score":1.0}]}]"#,
    ));

    engine.process_key(&press('a'));
    let result = engine.process_key(&press('i'));
    let shown = result
        .actions
        .iter()
        .find_map(|a| match a {
            EngineAction::ShowCandidates(list) => Some(list.candidates().to_vec()),
            _ => None,
        })
        .expect("suggestion window");
    // The dictionary entry's position depends on what else the suggestion
    // list holds (a loaded model contributes its own row), so select it by
    // the digit it is actually shown under.
    let digit = shown
        .iter()
        .position(|c| c.text == "藍")
        .expect("dictionary candidate in the suggestion window")
        + 1;
    assert!(matches!(engine.state(), InputState::Composing { .. }));

    let result = engine.process_key(&press_ctrl(Keysym(b'0' as u32 + digit as u32)));
    assert_eq!(committed(&result).as_deref(), Some("藍"));
    assert!(matches!(engine.state(), InputState::Empty));
}

#[test]
fn test_ctrl_digit_with_no_suggestion_is_consumed() {
    // Nothing to select: the chord must still be swallowed rather than
    // leaking to the application mid-composition.
    let mut engine = InputMethodEngine::new();
    engine.converters.kanji = None;

    engine.process_key(&press('a'));
    let result = engine.process_key(&press_ctrl(Keysym::KEY_9));
    assert!(result.consumed);
    assert!(committed(&result).is_none());
    assert!(matches!(engine.state(), InputState::Composing { .. }));
}

#[test]
fn test_emoji_digit_selection_does_not_pollute_learning() {
    // Committing an emoji by number must not record `:query` → 😀 into the
    // kana-keyed learning cache, and must leave emoji mode.
    let mut engine = engine_with_learned("あい", "愛");
    engine.process_key(&press(':'));
    assert_eq!(engine.mode.current(), InputMode::Emoji);
    for ch in "smile".chars() {
        engine.process_key(&press(ch));
    }

    let result = engine.process_key(&press_ctrl(Keysym::KEY_1));
    assert!(committed(&result).is_some(), "emoji must commit");
    assert_eq!(engine.mode.current(), InputMode::Hiragana);
    let learned = engine.learning.as_ref().unwrap();
    assert!(
        learned.lookup(":smile").is_empty(),
        "emoji query must not enter the learning cache"
    );
}

#[test]
fn test_arrow_in_conversion_returns_to_composing_and_moves_caret() {
    // Matching live conversion: a caret key dissolves the conversion and
    // moves the caret in the raw composition.
    let mut engine = InputMethodEngine::new();
    for ch in "kyou".chars() {
        engine.process_key(&press(ch));
    }
    assert_eq!(engine.input_buf.cursor(), 3); // き ょ う
    engine.process_key(&press_key(Keysym::SPACE));
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    let result = engine.process_key(&press_key(Keysym::LEFT));
    assert!(result.consumed);
    assert!(matches!(engine.state(), InputState::Composing { .. }));
    assert_eq!(engine.input_buf.cursor(), 2, "caret must move left");

    engine.process_key(&press_key(Keysym::END));
    assert_eq!(engine.input_buf.cursor(), 3);
}

#[test]
fn test_arrow_in_source_view_dissolves_the_filter() {
    // From the Ctrl+I model view: the caret key exits to editing and the
    // filter dies with the conversion state.
    let mut engine = InputMethodEngine::new();
    for ch in "kyou".chars() {
        engine.process_key(&press(ch));
    }
    engine.process_key(&press_ctrl(Keysym::KEY_I));
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    let result = engine.process_key(&press_key(Keysym::LEFT));
    assert!(result.consumed);
    assert!(matches!(engine.state(), InputState::Composing { .. }));
    assert_eq!(engine.input_buf.cursor(), 2);
    assert!(engine.state().filter().is_none());
}

#[test]
fn test_ctrl_b_in_conversion_moves_caret_like_left() {
    let mut engine = InputMethodEngine::new();
    for ch in "kyou".chars() {
        engine.process_key(&press(ch));
    }
    engine.process_key(&press_key(Keysym::SPACE));
    assert!(matches!(engine.state(), InputState::Conversion { .. }));

    let result = engine.process_key(&press_ctrl(Keysym::KEY_B));
    assert!(result.consumed);
    assert!(matches!(engine.state(), InputState::Composing { .. }));
    assert_eq!(engine.input_buf.cursor(), 2);
}

#[test]
fn test_shift_space_steps_back_a_candidate() {
    // Shift+Space is prev-candidate, the mirror of Space, like Shift+Tab.
    let mut engine = InputMethodEngine::new();
    engine.dicts.user = Some(dict_from_json(
        r#"[{"reading":"あい","candidates":[
            {"surface":"藍","score":2.0},
            {"surface":"愛","score":1.0}
        ]}]"#,
    ));
    engine.process_key(&press('a'));
    engine.process_key(&press('i'));
    engine.process_key(&press_key(Keysym::SPACE));
    let before = engine.candidates().unwrap().cursor();
    engine.process_key(&press_key(Keysym::SPACE));
    assert_eq!(engine.candidates().unwrap().cursor(), before + 1);

    let result = engine.process_key(&press_shift_key(Keysym::SPACE));
    assert!(result.consumed);
    assert_eq!(engine.candidates().unwrap().cursor(), before);
    assert!(matches!(engine.state(), InputState::Conversion { .. }));
    assert!(committed(&result).is_none(), "Shift+Space must not commit");
}

#[test]
fn romaji_stranded_between_kana_is_retyped_one_keystroke_away() {
    let engine = InputMethodEngine::new();
    let romaji = &engine.converters.romaji;
    assert_eq!(
        super::super::conversion::retypings("ございmす", romaji),
        [
            "ございます",
            "ございみす",
            "ございむす",
            "ございめす",
            "ございもす",
            "ございす"
        ]
    );
    // Still being typed, or letters meant as letters: nothing.
    assert!(super::super::conversion::retypings("ございm", romaji).is_empty());
    assert!(super::super::conversion::retypings("ございます", romaji).is_empty());
}

#[test]
fn a_typo_offers_the_retyped_reading_next_to_what_was_typed() {
    let mut engine = InputMethodEngine::new();
    for ch in "gozaimsu".chars() {
        engine.process_key(&press(ch));
    }
    assert_eq!(engine.input_buf.reading(), "ございmす");
    engine.process_key(&press_key(Keysym::SPACE));
    let candidates = engine.state().candidates().unwrap().candidates().to_vec();
    // What was typed stays first; the retyping follows, marked.
    assert_eq!(candidates[0].text, "ございmす");
    assert_eq!(candidates[1].text, "ございます");
    assert_eq!(
        candidates[1].description.as_deref(),
        Some("打ち直し: ございます")
    );
    assert_eq!(candidates[1].reading.as_deref(), Some("ございます"));
}

#[test]
fn english_typed_as_romaji_is_offered_spelled_out() {
    let mut engine = InputMethodEngine::new();
    engine.dicts.system = Some(dict_from_json(
        r#"[{"reading":"やふー","candidates":[{"surface":"YAHOO","score":2.0},{"surface":"Yahoo","score":1.0}]},
            {"reading":"すし","candidates":[{"surface":"Sushi","score":1.0}]}]"#,
    ));
    engine.init_english_words();
    // The best-scored spelling; romaji that reads as Japanese is left out.
    assert_eq!(
        engine.dicts.english.get("やほお").map(String::as_str),
        Some("Yahoo")
    );
    assert!(!engine.dicts.english.contains_key("すし"));

    for ch in "yahoochiebukuro".chars() {
        engine.process_key(&press(ch));
    }
    engine.process_key(&press_key(Keysym::SPACE));
    let candidates = engine.state().candidates().unwrap().candidates().to_vec();
    assert_eq!(candidates[0].text, "やほおちえぶくろ");
    assert_eq!(candidates[1].text, "Yahooちえぶくろ");
    assert_eq!(
        candidates[1].description.as_deref(),
        Some("英字: Yahooちえぶくろ")
    );

    // Kana the conversion made Japanese of is not English.
    let spell = super::super::conversion::spell_english;
    let words = &engine.dicts.english;
    assert_eq!(
        spell("やほおで", "やほおで", words).as_deref(),
        Some("Yahooで")
    );
    assert_eq!(spell("やほおで", "夜頬で", words), None);
}

#[test]
fn english_stranding_a_letter_converts_live_as_its_own_chunk() {
    let mut engine = InputMethodEngine::new();
    engine.dicts.system = Some(dict_from_json(
        r#"[{"reading":"あっぷる","candidates":[{"surface":"Apple","score":1.0}]}]"#,
    ));
    engine.init_english_words();
    // あっ | p | ぇに would reach the model as Japanese; Apple is one chunk.
    let chars: Vec<char> = "あっpぇに".chars().collect();
    assert_eq!(engine.split_chunks(&chars), ["あっpぇ", "に"]);
    assert_eq!(engine.convert_on_chunk_grid(&chars, ""), "Appleに");
}

#[test]
fn a_single_n_is_read_again_where_it_makes_a_common_word() {
    use super::super::conversion::n_retypings;
    let costs = [
        ("ほんやく", 6508.0),
        ("はんにち", 6250.0),
        ("こんや", 5405.0),
        ("がんない", 7138.0), // 眼内: too rare to read ねこがない as
        ("しんあい", 6518.0),
        ("しない", 7000.0),
        ("きにょう", f32::INFINITY), // kana-only junk hides nothing
        ("きんよう", 5826.0),
        ("えき", 3000.0),
        ("きんいつ", 2605.0), // 均一: common, but えき + に is 駅に
    ];
    let cost = |w: &str| costs.iter().find(|(r, _)| *r == w).map(|(_, c)| *c);
    let readings = |r: &str| -> Vec<(String, bool)> {
        n_retypings(r, cost)
            .into_iter()
            .map(|n| (n.reading, n.ny))
            .collect()
    };
    assert_eq!(readings("ほにゃく"), [("ほんやく".into(), true)]);
    assert_eq!(readings("はにちで"), [("はんにちで".into(), false)]);
    assert_eq!(readings("こにゃ"), [("こんや".into(), true)]);
    assert_eq!(readings("きにょうび"), [("きんようび".into(), true)]);
    // Rare words, and spots the reading as typed already makes a word of.
    assert!(readings("ねこがない").is_empty());
    assert!(readings("これはしない").is_empty());
    // A particle right after a word stays a particle.
    assert!(readings("えきにつく").is_empty());
}
