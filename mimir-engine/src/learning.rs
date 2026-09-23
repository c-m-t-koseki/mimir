//! Learning cache for remembering user-selected conversion results.
//!
//! Records which surface forms the user chose for each reading, and
//! boosts those candidates on subsequent conversions. Persisted as a
//! simple TSV file (`reading\tsurface\tfrequency\tlast_access[\tcontexts]`).

use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// A single learned conversion entry.
#[derive(Debug, Clone)]
pub struct LearningEntry {
    /// Surface form (e.g. "今日")
    pub surface: String,
    /// Number of times this surface was selected
    pub frequency: u32,
    /// Last selection time as Unix timestamp (seconds)
    pub last_access: u64,
    /// Tails of the left context this surface was committed after, most
    /// recent first (at most [`MAX_CONTEXTS`], each [`CONTEXT_CHARS`] long).
    pub contexts: Vec<String>,
}

/// Contexts kept per entry.
const MAX_CONTEXTS: usize = 4;
/// Left-context chars kept per recorded context.
const CONTEXT_CHARS: usize = 8;
/// Score per char of left context shared with a recorded one. One shared
/// char already outweighs any recency/frequency gap (at most ~10 + ln f).
const CONTEXT_WEIGHT: f64 = 10.0;
/// Separates contexts in the TSV column (never typed as text).
const CONTEXT_SEP: char = '\u{1f}';

/// A reading with latin letters still holds keystrokes that never became
/// kana (`s`, `でざいn`): a commit cut mid-syllable, never a word to learn.
fn has_unfired_romaji(reading: &str) -> bool {
    reading.chars().any(|c| c.is_ascii_alphabetic())
}

fn context_tail(lctx: &str) -> String {
    let chars: Vec<char> = lctx
        .chars()
        .filter(|c| *c != '\t' && *c != '\n' && *c != CONTEXT_SEP)
        .collect();
    chars[chars.len().saturating_sub(CONTEXT_CHARS)..]
        .iter()
        .collect()
}

/// The text after the last clause break (punctuation, brackets, spaces).
fn last_clause(ctx: &str) -> &str {
    let is_break = |c: char| "、。，．,.！？!?「」『』（）()[] 　\n".contains(c);
    match ctx.char_indices().rfind(|&(_, c)| is_break(c)) {
        Some((i, c)) => &ctx[i + c.len_utf8()..],
        None => ctx,
    }
}

/// Chars `a` and `b` share at their ends.
fn common_suffix_len(a: &str, b: &str) -> usize {
    a.chars()
        .rev()
        .zip(b.chars().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

impl LearningEntry {
    /// Bonus for the recorded context closest to `lctx`.
    fn context_bonus(&self, lctx: &str) -> f64 {
        let best = self
            .contexts
            .iter()
            .map(|c| common_suffix_len(c, lctx))
            .max()
            .unwrap_or(0);
        best as f64 * CONTEXT_WEIGHT
    }
}

/// Size limits for a [`LearningCache`].
///
/// Passed whole at construction ([`LearningCache::new`] /
/// [`LearningCache::load`]) so a caller can't apply one limit and forget the
/// other — there is no post-construction setter to miss.
#[derive(Debug, Clone, Copy)]
pub struct LearningConfig {
    /// Maximum number of total entries across all readings; lowest-score
    /// entries are evicted on save when over this limit.
    pub max_entries: usize,
    /// Maximum surface length (Unicode chars) that [`LearningCache::record`]
    /// accepts. Keeps whole-sentence live-conversion commits — one-off text
    /// that never matches again — out of the cache.
    pub max_surface_chars: usize,
    /// Minimum surface length recorded. 1 records everything; 2 keeps a
    /// lone て or 中 — no word on its own — out.
    pub min_surface_chars: usize,
}

impl LearningConfig {
    /// Default for [`max_entries`](Self::max_entries).
    pub const DEFAULT_MAX_ENTRIES: usize = 10_000;
    /// Default for [`max_surface_chars`](Self::max_surface_chars).
    pub const DEFAULT_MAX_SURFACE_CHARS: usize = 50;
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            max_entries: Self::DEFAULT_MAX_ENTRIES,
            max_surface_chars: Self::DEFAULT_MAX_SURFACE_CHARS,
            min_surface_chars: 1,
        }
    }
}

/// In-memory cache of user learning data.
///
/// Keyed by reading (hiragana). Each reading maps to a list of surface
/// entries with frequency and recency metadata.
#[derive(Debug)]
pub struct LearningCache {
    entries: HashMap<String, Vec<LearningEntry>>,
    max_entries: usize,
    max_surface_chars: usize,
    min_surface_chars: usize,
    dirty: bool,
}

impl LearningCache {
    /// Create an empty cache with the given limits.
    pub fn new(config: LearningConfig) -> Self {
        Self {
            entries: HashMap::new(),
            max_entries: config.max_entries,
            max_surface_chars: config.max_surface_chars,
            min_surface_chars: config.min_surface_chars,
            dirty: false,
        }
    }

    /// Record a user selection. Increments frequency and updates last_access.
    ///
    /// Surfaces longer than `max_surface_chars` are skipped; see
    /// [`LearningConfig::max_surface_chars`] for why.
    pub fn record(&mut self, reading: &str, surface: &str) {
        self.record_with_context(reading, surface, "");
    }

    /// [`Self::record`], remembering the left context the surface was
    /// committed after, so [`Self::lookup_with_context`] can prefer it there.
    pub fn record_with_context(&mut self, reading: &str, surface: &str, lctx: &str) {
        let len = surface.chars().count();
        if len > self.max_surface_chars
            || len < self.min_surface_chars
            || has_unfired_romaji(reading)
        {
            return;
        }
        let now = now_unix();
        let entries = self.entries.entry(reading.to_string()).or_default();

        let entry = match entries.iter().position(|e| e.surface == surface) {
            Some(i) => {
                let entry = &mut entries[i];
                entry.frequency += 1;
                entry.last_access = now;
                entry
            }
            None => {
                entries.push(LearningEntry {
                    surface: surface.to_string(),
                    frequency: 1,
                    last_access: now,
                    contexts: Vec::new(),
                });
                entries.last_mut().expect("just pushed")
            }
        };
        let ctx = context_tail(lctx);
        if !ctx.is_empty() {
            entry.contexts.retain(|c| *c != ctx);
            entry.contexts.insert(0, ctx);
            entry.contexts.truncate(MAX_CONTEXTS);
        }
        self.dirty = true;
    }

    /// Remove every learned entry that would resurface `surface` for input
    /// `reading`: the exact-reading entry plus every longer reading with
    /// `reading` as a prefix (the [`prefix_lookup`](Self::prefix_lookup)
    /// fan-out) — an exact-only delete would leave a twin that pops back on
    /// the next conversion. Returns whether anything was removed; persisted
    /// at the next `save`.
    pub fn remove_suggestion(&mut self, reading: &str, surface: &str) -> bool {
        let mut removed = false;
        self.entries.retain(|r, entries| {
            if !r.starts_with(reading) {
                return true;
            }
            let before = entries.len();
            entries.retain(|e| e.surface != surface);
            removed |= entries.len() != before;
            !entries.is_empty()
        });
        if removed {
            self.dirty = true;
        }
        removed
    }

    /// Every entry as `(reading, entry)`, readings sorted — for editing the
    /// history in a UI.
    pub fn entries(&self) -> Vec<(&str, &LearningEntry)> {
        let mut all: Vec<_> = self
            .entries
            .iter()
            .flat_map(|(r, es)| es.iter().map(move |e| (r.as_str(), e)))
            .collect();
        all.sort_by(|a, b| a.0.cmp(b.0).then(b.1.last_access.cmp(&a.1.last_access)));
        all
    }

    /// Forget exactly `reading → surface` (no prefix fan-out, unlike
    /// [`Self::remove_suggestion`]). Returns whether it existed.
    pub fn remove(&mut self, reading: &str, surface: &str) -> bool {
        let Some(entries) = self.entries.get_mut(reading) else {
            return false;
        };
        let before = entries.len();
        entries.retain(|e| e.surface != surface);
        let removed = entries.len() != before;
        if entries.is_empty() {
            self.entries.remove(reading);
        }
        self.dirty |= removed;
        removed
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.dirty = true;
    }

    /// Exact-match lookup: returns `(surface, score)` pairs sorted by score descending.
    pub fn lookup(&self, reading: &str) -> Vec<(String, f64)> {
        self.lookup_with_context(reading, "")
    }

    /// [`Self::lookup`], boosting surfaces committed after a left context
    /// that ends like `lctx`.
    pub fn lookup_with_context(&self, reading: &str, lctx: &str) -> Vec<(String, f64)> {
        let now = now_unix();
        let Some(entries) = self.entries.get(reading) else {
            return Vec::new();
        };
        let mut scored: Vec<(String, f64)> = entries
            .iter()
            .map(|e| (e.surface.clone(), score(e, now) + e.context_bonus(lctx)))
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        scored
    }

    /// The surface for `reading` last committed right after the same clause
    /// that ends `lctx` (the text since the last punctuation, at most
    /// [`CONTEXT_CHARS`]): the phrase *with what came before it* was seen
    /// and chosen, so it can override a context-aware model. A clause never
    /// seen with this reading gives `None` — trust the model there.
    pub fn lookup_after_clause(&self, reading: &str, lctx: &str) -> Option<String> {
        let tail = context_tail(lctx);
        let clause = last_clause(&tail);
        if clause.is_empty() {
            return None;
        }
        let now = now_unix();
        self.entries
            .get(reading)?
            .iter()
            .filter(|e| e.contexts.iter().any(|c| last_clause(c) == clause))
            .max_by(|a, b| score(a, now).total_cmp(&score(b, now)))
            .map(|e| e.surface.clone())
    }

    /// Prefix-match lookup: returns `(reading, surface, score)` triples
    /// for all readings that start with `prefix`, sorted by score descending.
    pub fn prefix_lookup(&self, prefix: &str) -> Vec<(String, String, f64)> {
        let now = now_unix();
        let mut results: Vec<(String, String, f64)> = Vec::new();
        for (reading, entries) in &self.entries {
            if reading.starts_with(prefix) {
                for entry in entries {
                    results.push((reading.clone(), entry.surface.clone(), score(entry, now)));
                }
            }
        }
        results.sort_by(|a, b| b.2.total_cmp(&a.2));
        results
    }

    /// Load a learning cache from a TSV file.
    ///
    /// Format: `reading\tsurface\tfrequency\tlast_access`
    /// Lines starting with `#` are comments.
    pub fn load(path: &Path, config: LearningConfig) -> anyhow::Result<Self> {
        let file = std::fs::File::open(path)?;
        let reader = std::io::BufReader::new(file);
        let mut cache = Self::new(config);

        for line in reader.lines() {
            let line = line?;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() < 4 {
                continue;
            }
            let reading = parts[0];
            let surface = parts[1];
            if has_unfired_romaji(reading) || surface.chars().count() < cache.min_surface_chars {
                continue;
            }
            let frequency: u32 = match parts[2].parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let last_access: u64 = match parts[3].parse() {
                Ok(v) => v,
                Err(_) => continue,
            };

            cache
                .entries
                .entry(reading.to_string())
                .or_default()
                .push(LearningEntry {
                    surface: surface.to_string(),
                    frequency,
                    last_access,
                    contexts: parts
                        .get(4)
                        .map(|c| {
                            c.split(CONTEXT_SEP)
                                .filter(|c| !c.is_empty())
                                .map(str::to_string)
                                .collect()
                        })
                        .unwrap_or_default(),
                });
        }

        // Not dirty — just loaded from disk
        cache.dirty = false;
        Ok(cache)
    }

    /// Save the cache to a TSV file, evicting low-score entries if over capacity.
    pub fn save(&mut self, path: &Path) -> anyhow::Result<()> {
        self.evict();

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = std::fs::File::create(path)?;
        let mut writer = std::io::BufWriter::new(file);
        writeln!(writer, "# mimir learning cache v1")?;

        // Sort readings for deterministic output
        let mut readings: Vec<&String> = self.entries.keys().collect();
        readings.sort();

        for reading in readings {
            if let Some(entries) = self.entries.get(reading) {
                for entry in entries {
                    write!(
                        writer,
                        "{}\t{}\t{}\t{}",
                        reading, entry.surface, entry.frequency, entry.last_access
                    )?;
                    if !entry.contexts.is_empty() {
                        let sep = CONTEXT_SEP.to_string();
                        write!(writer, "\t{}", entry.contexts.join(&sep))?;
                    }
                    writeln!(writer)?;
                }
            }
        }

        writer.flush()?;
        self.dirty = false;
        Ok(())
    }

    /// Whether there are unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Total number of (reading, surface) pairs across all readings.
    pub fn entry_count(&self) -> usize {
        self.entries.values().map(|v| v.len()).sum()
    }

    /// Evict lowest-score entries until total count is within `max_entries`.
    fn evict(&mut self) {
        let total = self.entry_count();
        if total <= self.max_entries {
            return;
        }

        let now = now_unix();
        // Collect all entries with their (reading, index, score)
        let mut all: Vec<(String, usize, f64)> = Vec::with_capacity(total);
        for (reading, entries) in &self.entries {
            for (i, entry) in entries.iter().enumerate() {
                all.push((reading.clone(), i, score(entry, now)));
            }
        }
        // Sort by score ascending (lowest first = eviction candidates)
        all.sort_by(|a, b| a.2.total_cmp(&b.2));

        let to_remove = total - self.max_entries;
        // Collect indices to remove, grouped by reading
        let mut remove_set: HashMap<String, Vec<usize>> = HashMap::new();
        for &(ref reading, idx, _) in all.iter().take(to_remove) {
            remove_set.entry(reading.clone()).or_default().push(idx);
        }

        // Remove entries in reverse index order to preserve indices
        for (reading, indices) in &mut remove_set {
            indices.sort_unstable();
            indices.reverse();
            if let Some(entries) = self.entries.get_mut(reading) {
                for &idx in indices.iter() {
                    if idx < entries.len() {
                        entries.remove(idx);
                    }
                }
                if entries.is_empty() {
                    self.entries.remove(reading);
                }
            }
        }
    }
}

/// Compute a candidate score: recency-weighted with frequency bonus.
///
/// Inspired by mozc's UserHistoryPredictor: recent selections rank higher,
/// with a logarithmic frequency term to reward repeated use.
fn score(entry: &LearningEntry, now: u64) -> f64 {
    let age_days = if now > entry.last_access {
        (now - entry.last_access) / 86400
    } else {
        0
    };
    let recency = 1.0 / (1.0 + age_days as f64);
    let freq = (entry.frequency as f64).ln_1p();
    recency * 10.0 + freq
}

/// Current time as Unix timestamp in seconds.
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Config/cache with a custom entry limit and the default surface cap.
    fn config_with(max_entries: usize) -> LearningConfig {
        LearningConfig {
            max_entries,
            ..LearningConfig::default()
        }
    }

    fn cache_with(max_entries: usize) -> LearningCache {
        LearningCache::new(config_with(max_entries))
    }
    use tempfile::NamedTempFile;

    #[test]
    fn test_record_and_lookup() {
        let mut cache = cache_with(100);

        cache.record("きょう", "今日");
        cache.record("きょう", "京");
        cache.record("きょう", "今日"); // frequency bump

        let results = cache.lookup("きょう");
        assert_eq!(results.len(), 2);
        // "今日" should have higher score (frequency 2 vs 1)
        assert_eq!(results[0].0, "今日");
        assert_eq!(results[1].0, "京");
    }

    #[test]
    fn test_lookup_empty() {
        let cache = cache_with(100);
        let results = cache.lookup("きょう");
        assert!(results.is_empty());
    }

    #[test]
    fn test_prefix_lookup() {
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");
        cache.record("きょうと", "京都");
        cache.record("あした", "明日");

        let results = cache.prefix_lookup("きょう");
        assert_eq!(results.len(), 2);
        // Both "きょう" and "きょうと" should match
        let readings: Vec<&str> = results.iter().map(|(r, _, _)| r.as_str()).collect();
        assert!(readings.contains(&"きょう"));
        assert!(readings.contains(&"きょうと"));
    }

    #[test]
    fn test_prefix_lookup_no_match() {
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");
        let results = cache.prefix_lookup("あ");
        assert!(results.is_empty());
    }

    #[test]
    fn test_save_and_load() {
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");
        cache.record("きょう", "今日");
        cache.record("きょう", "京");
        cache.record("あした", "明日");

        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();

        cache.save(&path).unwrap();
        assert!(!cache.is_dirty());

        let loaded = LearningCache::load(&path, config_with(100)).unwrap();
        assert!(!loaded.is_dirty());
        assert_eq!(loaded.entry_count(), 3);

        let results = loaded.lookup("きょう");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "今日"); // frequency 2
    }

    #[test]
    fn test_dirty_flag() {
        let mut cache = cache_with(100);
        assert!(!cache.is_dirty());

        cache.record("きょう", "今日");
        assert!(cache.is_dirty());

        let file = NamedTempFile::new().unwrap();
        cache.save(file.path()).unwrap();
        assert!(!cache.is_dirty());
    }

    #[test]
    fn test_eviction() {
        let mut cache = cache_with(3);

        // Add 5 entries
        cache.record("a", "A");
        cache.record("b", "B");
        cache.record("c", "C");
        cache.record("d", "D");
        cache.record("e", "E");

        // Boost some to give them higher scores
        cache.record("a", "A");
        cache.record("a", "A");
        cache.record("c", "C");

        let file = NamedTempFile::new().unwrap();
        cache.save(file.path()).unwrap();

        // After eviction, should be at most 3 entries
        assert!(cache.entry_count() <= 3);
    }

    #[test]
    fn test_score_recency() {
        let now = now_unix();
        let recent = LearningEntry {
            surface: "A".to_string(),
            frequency: 1,
            last_access: now,
            contexts: Vec::new(),
        };
        let old = LearningEntry {
            surface: "B".to_string(),
            frequency: 1,
            last_access: now.saturating_sub(30 * 86400), // 30 days ago
            contexts: Vec::new(),
        };
        assert!(score(&recent, now) > score(&old, now));
    }

    #[test]
    fn test_score_frequency() {
        let now = now_unix();
        let high_freq = LearningEntry {
            surface: "A".to_string(),
            frequency: 100,
            last_access: now,
            contexts: Vec::new(),
        };
        let low_freq = LearningEntry {
            surface: "B".to_string(),
            frequency: 1,
            last_access: now,
            contexts: Vec::new(),
        };
        assert!(score(&high_freq, now) > score(&low_freq, now));
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = LearningCache::load(Path::new("/nonexistent/path"), config_with(100));
        assert!(result.is_err());
    }

    #[test]
    fn test_tsv_format() {
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");

        let file = NamedTempFile::new().unwrap();
        cache.save(file.path()).unwrap();

        let content = std::fs::read_to_string(file.path()).unwrap();
        assert!(content.starts_with("# mimir learning cache v1"));
        assert!(content.contains("きょう\t今日\t1\t"));
    }

    #[test]
    fn test_tsv_comments_and_blanks_ignored() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "# comment\n\nきょう\t今日\t5\t1700000000\n# another comment\n",
        )
        .unwrap();

        let cache = LearningCache::load(file.path(), config_with(100)).unwrap();
        assert_eq!(cache.entry_count(), 1);
        let results = cache.lookup("きょう");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "今日");
    }

    #[test]
    fn test_remove_suggestion_last_surface_drops_reading() {
        let mut cache = cache_with(100);
        cache.record("きょう", "今日");

        assert!(cache.remove_suggestion("きょう", "今日"));
        assert_eq!(cache.entry_count(), 0);
        assert!(cache.lookup("きょう").is_empty());
        assert!(cache.prefix_lookup("き").is_empty());
    }

    #[test]
    fn test_remove_suggestion_clears_exact_and_prefix_twins() {
        let mut cache = cache_with(100);
        cache.record("あい", "藍");
        cache.record("あいさ", "藍"); // same surface, longer reading (a twin)
        cache.record("あい", "愛"); // different surface under the same reading
        cache.record("うみ", "藍"); // same surface, unrelated reading

        let file = NamedTempFile::new().unwrap();
        cache.save(file.path()).unwrap();
        assert!(!cache.is_dirty());

        assert!(cache.remove_suggestion("あい", "藍"));
        assert!(cache.is_dirty(), "removal must mark the cache dirty");

        // Both the exact entry and the prefix twin are gone...
        assert!(cache.lookup("あい").iter().all(|(s, _)| s != "藍"));
        assert!(cache.lookup("あいさ").is_empty());
        // ...but a different surface under the same reading survives...
        assert!(cache.lookup("あい").iter().any(|(s, _)| s == "愛"));
        // ...and the same surface under an unrelated reading is untouched.
        assert!(cache.lookup("うみ").iter().any(|(s, _)| s == "藍"));
    }

    #[test]
    fn test_remove_suggestion_nonexistent_is_noop() {
        let mut cache = cache_with(100);
        cache.record("あい", "藍");

        let file = NamedTempFile::new().unwrap();
        cache.save(file.path()).unwrap();

        assert!(!cache.remove_suggestion("あい", "愛"));
        assert!(!cache.remove_suggestion("かき", "柿"));
        assert!(!cache.is_dirty(), "no-op removal must not mark dirty");
    }

    #[test]
    fn test_record_skips_long_surface() {
        let mut cache = LearningCache::new(LearningConfig {
            max_entries: 100,
            max_surface_chars: 5,
            min_surface_chars: 1,
        });

        cache.record("あ", &"漢".repeat(6));
        assert_eq!(cache.entry_count(), 0);
        assert!(!cache.is_dirty());

        // Boundary: exactly max_surface_chars is accepted.
        cache.record("あ", &"漢".repeat(5));
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn test_record_ignores_reading_length() {
        let mut cache = LearningCache::new(LearningConfig {
            max_entries: 100,
            max_surface_chars: 5,
            min_surface_chars: 1,
        });

        // Only the surface is capped; a long reading with a short surface
        // is fine (e.g. a long kana reading converting to a short word).
        cache.record(&"あ".repeat(30), "短い");
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn test_default_max_surface_chars() {
        let mut cache = cache_with(100);

        cache.record(
            "よみ",
            &"あ".repeat(LearningConfig::DEFAULT_MAX_SURFACE_CHARS + 1),
        );
        assert_eq!(cache.entry_count(), 0);

        cache.record(
            "よみ",
            &"あ".repeat(LearningConfig::DEFAULT_MAX_SURFACE_CHARS),
        );
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn test_tsv_malformed_lines_skipped() {
        let file = NamedTempFile::new().unwrap();
        std::fs::write(
            file.path(),
            "きょう\t今日\t5\t1700000000\nmalformed_line\nきょう\t京\tbad\t1700000000\n",
        )
        .unwrap();

        let cache = LearningCache::load(file.path(), config_with(100)).unwrap();
        // Only the first valid line should be loaded
        assert_eq!(cache.entry_count(), 1);
    }

    #[test]
    fn test_context_outranks_recency() {
        let mut cache = LearningCache::new(LearningConfig::default());
        cache.record_with_context("かいぎ", "会議", "本日の");
        cache.record_with_context("かいぎ", "懐疑", "その説に");
        // Most recent wins without context, the matching context wins with it.
        assert_eq!(cache.lookup_with_context("かいぎ", "明日の")[0].0, "会議");
        assert_eq!(cache.lookup_with_context("かいぎ", "理論に")[0].0, "懐疑");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("learning.tsv");
        cache.save(&path).unwrap();
        let loaded = LearningCache::load(&path, LearningConfig::default()).unwrap();
        assert_eq!(loaded.lookup_with_context("かいぎ", "本日の")[0].0, "会議");
    }

    #[test]
    fn test_unfired_romaji_and_short_surfaces_are_never_learned() {
        let mut cache = LearningCache::new(LearningConfig {
            min_surface_chars: 2,
            ..Default::default()
        });
        cache.record("s", "s");
        cache.record("て", "て");
        cache.record("ちゅう", "中");
        cache.record("でざいn", "デザイn");
        assert_eq!(cache.entry_count(), 0);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("learning.tsv");
        std::fs::write(&path, "でざいn\tデザイn\t3\t0\nでざいん\tデザイン\t1\t0\n").unwrap();
        let loaded = LearningCache::load(&path, LearningConfig::default()).unwrap();
        assert_eq!(loaded.entry_count(), 1);
    }

    #[test]
    fn test_remove_is_exact_and_clear_empties() {
        let mut cache = LearningCache::new(LearningConfig::default());
        cache.record("きょう", "今日");
        cache.record("きょうと", "今日と");
        assert!(cache.remove("きょう", "今日"));
        assert!(!cache.remove("きょう", "今日"));
        assert_eq!(cache.entries().len(), 1);
        cache.clear();
        assert_eq!(cache.entry_count(), 0);
    }

    #[test]
    fn test_lookup_after_clause_needs_the_same_preceding_clause() {
        let mut cache = LearningCache::new(LearningConfig::default());
        cache.record_with_context("はいしゃ", "歯医者", "昨日は、歯が痛いので");
        assert_eq!(
            cache
                .lookup_after_clause("はいしゃ", "今日も歯が痛いので")
                .as_deref(),
            None,
            "clause 今日も歯が痛いので differs"
        );
        assert_eq!(
            cache
                .lookup_after_clause("はいしゃ", "朝から、歯が痛いので")
                .as_deref(),
            Some("歯医者")
        );
        // A clause never seen with this reading: leave it to the model.
        assert_eq!(
            cache.lookup_after_clause("はいしゃ", "車が壊れたので"),
            None
        );
        assert_eq!(cache.lookup_after_clause("はいしゃ", "。"), None);
    }
}
