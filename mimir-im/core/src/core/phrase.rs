//! Where a conversion breaks into words (文節): its surface lined up with
//! its reading, engine-free. The part layer cuts a part at them; the engine
//! completes a history entry one of them at a time.

/// Particles that end a word (今日は, 東京には). で and か are left out: they
/// start です and かも.
pub(crate) const PARTICLES: &str = "はがをにともへのや";
/// The ones also taken as left over from the word before at the start of
/// the rest — not と も や, which start とても, もう, やはり.
pub(crate) const LEAD_PARTICLES: &str = "はがをにへの";

/// The next word of `reading`, given `surface`, its conversion: `(lead,
/// end)` in reading chars, where `..lead` are particles belonging to the
/// word before (left over by where the last one was cut) and `lead..end`
/// is the word. Leading particles count only `after_content`: when what was
/// committed before ends in kanji or katakana. A word runs until kanji or
/// katakana follow hiragana, or until the hiragana after its particles
/// (今日は|いい|天気ですね). The whole rest when the two can't be aligned.
pub(crate) fn next_word(reading: &str, surface: &str, after_content: bool) -> (usize, usize) {
    let r: Vec<char> = reading.chars().collect();
    let particle = |c: char| PARTICLES.contains(c);
    let Some(units) = align(reading, surface) else {
        return (0, r.len());
    };
    let units: Vec<(usize, bool, char)> = units
        .iter()
        .map(|u| (u.reading_end, u.kana, u.last))
        .collect();
    // Leading particles, when more follows.
    let lead_units = units
        .iter()
        .take_while(|&&(_, h, c)| after_content && h && LEAD_PARTICLES.contains(c))
        .count();
    let lead_units = if lead_units == units.len() {
        0
    } else {
        lead_units
    };
    let lead = if lead_units == 0 {
        0
    } else {
        units[lead_units - 1].0
    };
    // Then the word: content, then the particles right after a kanji or
    // katakana run (not inside a hiragana word: おにぎり, とても).
    let mut end = r.len();
    let mut in_particles = false;
    for k in lead_units + 1..units.len() {
        let (_, is_hira, c) = units[k];
        let (prev_end, prev_hira, _) = units[k - 1];
        let next = if is_hira {
            in_particles && !particle(c)
        } else {
            prev_hira
        };
        if next {
            end = prev_end;
            break;
        }
        in_particles = is_hira && particle(c) && (!prev_hira || in_particles);
    }
    (lead, end)
}

/// One piece of a conversion lined up with its reading: a kana standing for
/// itself, or a kanji/katakana run standing for the reading up to the kana
/// that follows it. Ends are in chars.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Unit {
    pub reading_end: usize,
    pub surface_end: usize,
    pub kana: bool,
    pub last: char,
}

pub(crate) fn is_hiragana(c: char) -> bool {
    matches!(c, 'ぁ'..='ゖ' | 'ー')
}

/// Line `surface` up with `reading`, `None` when they don't match.
pub(crate) fn align(reading: &str, surface: &str) -> Option<Vec<Unit>> {
    let r: Vec<char> = reading.chars().collect();
    let s: Vec<char> = surface.chars().collect();
    let mut units = Vec::new();
    let (mut i, mut j) = (0, 0);
    while j < s.len() {
        if i < r.len() && is_hiragana(s[j]) && s[j] == r[i] {
            i += 1;
            j += 1;
            units.push(Unit {
                reading_end: i,
                surface_end: j,
                kana: true,
                last: s[j - 1],
            });
            continue;
        }
        let mut j2 = j + 1;
        while j2 < s.len() && !is_hiragana(s[j2]) {
            j2 += 1;
        }
        if i >= r.len() {
            return None;
        }
        i = if j2 == s.len() {
            r.len()
        } else {
            (i + 1..r.len()).find(|&k| r[k] == s[j2])?
        };
        j = j2;
        units.push(Unit {
            reading_end: i,
            surface_end: j,
            kana: false,
            last: s[j - 1],
        });
    }
    (i == r.len()).then_some(units)
}

/// The ends of each word of `surface`, `(reading_end, surface_end)` in
/// chars, the last at the ends of both: [`next_word`] taken again from
/// where the last one ended, particles it leaves over joined to the word
/// before (今日は|いい|天気ですね). One word when the two can't be aligned.
pub(crate) fn word_ends(reading: &str, surface: &str) -> Vec<(usize, usize)> {
    let r: Vec<char> = reading.chars().collect();
    let whole = vec![(r.len(), surface.chars().count())];
    let Some(units) = align(reading, surface) else {
        return whole;
    };
    // Reading position → surface position, at the unit ends.
    let surface_at = |at: usize| {
        (at == 0).then_some(0).or_else(|| {
            units
                .iter()
                .find(|u| u.reading_end == at)
                .map(|u| u.surface_end)
        })
    };
    let s: Vec<char> = surface.chars().collect();
    let mut ends: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    while at < r.len() {
        let Some(from) = surface_at(at) else {
            return whole;
        };
        let rest_r: String = r[at..].iter().collect();
        let rest_s: String = s[from..].iter().collect();
        let after_content = from > 0 && !is_hiragana(s[from - 1]);
        let (lead, end) = next_word(&rest_r, &rest_s, after_content);
        if lead > 0
            && let Some(last) = ends.last_mut()
            && let Some(to) = surface_at(at + lead)
        {
            *last = (at + lead, to);
        }
        let Some(to) = surface_at(at + end) else {
            return whole;
        };
        if end > lead {
            ends.push((at + end, to));
        }
        at += end.max(1);
    }
    ends
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sentence_breaks_into_its_words() {
        let words = |r: &str, s: &str| {
            let chars: Vec<char> = s.chars().collect();
            let mut from = 0;
            word_ends(r, s)
                .into_iter()
                .map(|(_, to)| {
                    let w: String = chars[from..to].iter().collect();
                    from = to;
                    w
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            words("きょうはいいてんきですね", "今日はいい天気ですね"),
            ["今日は", "いい", "天気ですね"]
        );
        assert_eq!(
            words("かいぎょうするべき", "改行するべき"),
            ["改行するべき"]
        );
        assert_eq!(
            words("とうきょうにはいきます", "東京には行きます"),
            ["東京には", "行きます"]
        );
        // Not aligned: one word.
        assert_eq!(words("あ", "漢字"), ["漢字"]);
    }
}
