//! The user dictionaries: the Mozc TSV files in `user_dicts/`, edited in
//! place (as the macOS settings do). Binary dictionaries are listed by name
//! only — they are built from a TSV, not edited.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the settings window adds words.
const REGISTERED: &str = "registered.tsv";

/// One word: where it is and what it says.
#[derive(Debug, Serialize, PartialEq)]
pub struct Entry {
    pub file: String,
    /// Index among the file's non-empty lines.
    pub line: usize,
    pub reading: String,
    pub word: String,
}

/// A word to remove, as [`Entry`] placed it.
#[derive(Debug, Deserialize)]
pub struct Place {
    pub file: String,
    pub line: usize,
}

#[derive(Debug, Serialize)]
pub struct Listing {
    pub entries: Vec<Entry>,
    pub binaries: Vec<String>,
}

pub fn list(dir: &Path) -> Listing {
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|f| f.file_name().into_string().ok())
        .collect();
    files.sort();
    let binaries = files
        .iter()
        .filter(|f| f.ends_with(".bin"))
        .cloned()
        .collect();
    let entries = files
        .iter()
        .filter(|f| f.ends_with(".tsv"))
        .flat_map(|file| {
            lines(&dir.join(file))
                .into_iter()
                .enumerate()
                .filter(|(_, line)| !line.starts_with('#'))
                .filter_map(|(line, text)| {
                    let mut cols = text.split('\t');
                    let reading = cols.next()?.to_string();
                    let word = cols.next()?.to_string();
                    Some(Entry {
                        file: file.clone(),
                        line,
                        reading,
                        word,
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect();
    Listing { entries, binaries }
}

/// Add `reading → word` to the settings window's own file.
pub fn add(dir: &Path, reading: &str, word: &str) -> io::Result<()> {
    if [reading, word]
        .iter()
        .any(|s| s.is_empty() || s.contains(['\t', '\n', '\r']))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "よみと単語に、タブと改行は使えません",
        ));
    }
    let path = dir.join(REGISTERED);
    let mut kept = lines(&path);
    kept.push(format!("{reading}\t{word}\t名詞\tmimir登録"));
    write(&path, &kept)
}

pub fn remove(dir: &Path, doomed: &[Place]) -> io::Result<()> {
    let mut files: Vec<&str> = doomed.iter().map(|p| p.file.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    for file in files {
        let path = file_in(dir, file)?;
        let kept: Vec<String> = lines(&path)
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !doomed.iter().any(|p| p.file == file && p.line == *i))
            .map(|(_, line)| line)
            .collect();
        write(&path, &kept)?;
    }
    Ok(())
}

/// `dir/file`, refusing anything but a plain file name in `dir`.
fn file_in(dir: &Path, file: &str) -> io::Result<PathBuf> {
    if Path::new(file).file_name().and_then(|f| f.to_str()) != Some(file) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{file:?}"),
        ));
    }
    Ok(dir.join(file))
}

/// Non-empty lines, indexed the way [`Entry::line`] counts them.
fn lines(path: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// Write through a temporary file, so an engine starting meanwhile reads
/// the old file or the new one, never half of it.
fn write(path: &Path, lines: &[String]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut text = lines.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    let tmp = path.with_extension("tsv.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_list_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let dicts = dir.path().join("user_dicts");
        std::fs::create_dir_all(&dicts).unwrap();
        std::fs::write(
            dicts.join("mine.tsv"),
            "# comment\nあ\t亜\t名詞\t\n\nい\t伊\t名詞\t\n",
        )
        .unwrap();
        std::fs::write(dicts.join("big.bin"), "").unwrap();
        add(&dicts, "みみる", "Mimir").unwrap();

        let listing = list(&dicts);
        assert_eq!(listing.binaries, ["big.bin"]);
        let words: Vec<_> = listing.entries.iter().map(|e| e.word.as_str()).collect();
        assert_eq!(words, ["亜", "伊", "Mimir"]);

        let place = |e: &Entry| Place {
            file: e.file.clone(),
            line: e.line,
        };
        remove(
            &dicts,
            &[place(&listing.entries[0]), place(&listing.entries[2])],
        )
        .unwrap();
        let words: Vec<_> = list(&dicts).entries.into_iter().map(|e| e.word).collect();
        assert_eq!(words, ["伊"]);
        let mine = std::fs::read_to_string(dicts.join("mine.tsv")).unwrap();
        assert!(mine.starts_with("# comment\n"), "comments stay: {mine:?}");
    }

    #[test]
    fn refuses_what_would_break_the_file_or_leave_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(add(dir.path(), "あ\tい", "亜").is_err());
        assert!(add(dir.path(), "", "亜").is_err());
        let outside = Place {
            file: "../config.toml".into(),
            line: 0,
        };
        assert!(remove(dir.path(), &[outside]).is_err());
    }
}
