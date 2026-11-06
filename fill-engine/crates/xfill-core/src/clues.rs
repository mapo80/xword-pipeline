//! Word + clue database in `WORD,clue text` CSV form (one clue per line, a
//! word may repeat with different clues) — the format of the Italian dataset.
//!
//! The file carries no quality scores, so a word's score is derived from how
//! many distinct clues it has: words clued many times are the ones setters
//! actually use. 1 clue → 50, 3 → 63, 10 → 78, capped at 90.

use crate::wordlist::{blocklist_beside, Wordlist};
use std::collections::HashMap;
use std::fs;

/// Clues per normalized answer (A–Z), in file order, deduplicated.
pub struct ClueDb {
    clues: HashMap<String, Vec<String>>,
}

impl ClueDb {
    pub fn get(&self, answer: &str) -> &[String] {
        self.clues.get(answer).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn len(&self) -> usize {
        self.clues.len()
    }

    pub fn is_empty(&self) -> bool {
        self.clues.is_empty()
    }
}

impl ClueDb {
    /// Pick a clue for `answer`, avoiding clues that contain the answer
    /// itself (for answers of 3+ letters). `pick` chooses among the usable
    /// clues (called with their count, must return an index below it).
    /// Accents typed as apostrophes are restored (see [`fix_accents`]).
    pub fn pick_clue(&self, answer: &str, pick: impl FnOnce(usize) -> usize) -> Option<String> {
        let all = self.get(answer);
        let no_answer =
            |c: &&String| answer.len() < 3 || !normalize_letters_lossy(c).contains(answer);
        // Preference tiers: clean and answer-free → answer-free → anything.
        let tiers: [Vec<&String>; 3] = [
            all.iter()
                .filter(no_answer)
                .filter(|c| !looks_truncated(c))
                .collect(),
            all.iter().filter(no_answer).collect(),
            all.iter().collect(),
        ];
        let pool = tiers.into_iter().find(|t| !t.is_empty())?;
        let chosen = fix_accents(pool[pick(pool.len()) % pool.len()]);
        Some(capitalize_first(&chosen))
    }
}

/// Heuristic for clues cut off in the source data ("ABBA,Un",
/// "Introducono molti ann", "Erano duecento n"): very short, or ending in a
/// lone lowercase letter.
fn looks_truncated(clue: &str) -> bool {
    let t = clue.trim();
    if t.chars().count() < 4 {
        return true;
    }
    match t.split_whitespace().last() {
        Some(w) => w.chars().count() == 1 && w.chars().all(|c| c.is_lowercase()),
        None => true,
    }
}

fn capitalize_first(s: &str) -> String {
    let mut cs = s.chars();
    match cs.next() {
        Some(f) => f.to_uppercase().chain(cs).collect(),
        None => String::new(),
    }
}

/// Uppercase A–Z letters of `s` (accents folded, everything else dropped).
fn normalize_letters_lossy(s: &str) -> String {
    s.split(|c: char| !c.is_alphabetic())
        .filter_map(normalize_answer)
        .collect()
}

/// Restore Italian accents typed as a trailing apostrophe: `citta'` → `città`,
/// `perche'` → `perché`, `e'` → `è`, `Ne'` → `Né`. Leaves elisions
/// (`dell'aula`), `po'`, and single-quoted titles (`'Barbie girl'`) alone.
pub fn fix_accents(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut in_quote = false;
    let mut word_start = 0usize;
    for (i, &c) in chars.iter().enumerate() {
        if c.is_alphabetic() {
            if i == 0 || !chars[i - 1].is_alphabetic() {
                word_start = i;
            }
            out.push(c);
            continue;
        }
        if c != '\'' {
            out.push(c);
            continue;
        }
        let prev_letter = i > 0 && chars[i - 1].is_alphabetic();
        let next_letter = chars.get(i + 1).is_some_and(|n| n.is_alphabetic());
        if !prev_letter {
            // opening quote ('Giselle') or stray apostrophe
            if next_letter {
                in_quote = true;
            }
            out.push(c);
            continue;
        }
        if next_letter {
            out.push(c); // elision: dell'aula
            continue;
        }
        if in_quote {
            in_quote = false; // closing quote
            out.push(c);
            continue;
        }
        let word: String = chars[word_start..i]
            .iter()
            .collect::<String>()
            .to_lowercase();
        let last = chars[i - 1];
        let acute = word.ends_with("che")
            || word == "ne"
            || word == "se"
            || (word.ends_with("tre") && word.len() > 3);
        let accented = match (last, acute) {
            ('a', _) => 'à',
            ('e', false) => 'è',
            ('e', true) => 'é',
            ('i', _) => 'ì',
            ('o', _) => 'ò',
            ('u', _) => 'ù',
            ('A', _) => 'À',
            ('E', false) => 'È',
            ('E', true) => 'É',
            ('I', _) => 'Ì',
            ('O', _) => 'Ò',
            ('U', _) => 'Ù',
            _ => {
                out.push(c);
                continue;
            }
        };
        if word == "po" {
            out.push(c);
            continue;
        }
        out.pop();
        out.push(accented);
    }
    out
}

/// Score from the number of distinct clues a word has.
pub fn score_from_clue_count(n: usize) -> u8 {
    if n == 0 {
        return 50;
    }
    (50.0 + 12.0 * (n as f64).ln()).round().min(90.0) as u8
}

/// Map a solution word to grid letters: uppercase, accents folded
/// (È→E, Ç→C…), spaces/apostrophes/hyphens/dots dropped. `None` if any other
/// character remains.
pub fn normalize_answer(word: &str) -> Option<String> {
    let mut out = String::with_capacity(word.len());
    for ch in word.chars() {
        let base = match ch {
            'à' | 'á' | 'â' | 'ä' | 'ã' | 'å' | 'À' | 'Á' | 'Â' | 'Ä' | 'Ã' | 'Å' => {
                'A'
            }
            'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => 'E',
            'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
            'ò' | 'ó' | 'ô' | 'ö' | 'õ' | 'Ò' | 'Ó' | 'Ô' | 'Ö' | 'Õ' => 'O',
            'ù' | 'ú' | 'û' | 'ü' | 'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
            'ç' | 'Ç' => 'C',
            'ñ' | 'Ñ' => 'N',
            ' ' | '\'' | '’' | '-' | '.' => continue,
            c if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
            _ => return None,
        };
        out.push(base);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Decode one line: UTF-8 when valid, else Latin-1 (the dataset mixes both).
fn decode_line(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_string(),
        Err(_) => raw.iter().map(|&b| b as char).collect(),
    }
}

/// Parse `WORD,clue` CSV text into (answer → clues). Lines without a comma,
/// with an empty/unusable word, or `#` comments are skipped.
pub fn parse_clue_csv(bytes: &[u8]) -> HashMap<String, Vec<String>> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let mut map: HashMap<String, Vec<String>> = HashMap::new();
    for raw in bytes.split(|&b| b == b'\n') {
        let line = decode_line(raw);
        let line = line.trim_end_matches('\r');
        if line.trim_start().starts_with('#') {
            continue;
        }
        let Some((word, clue)) = line.split_once(',') else {
            continue;
        };
        let Some(answer) = normalize_answer(word.trim()) else {
            continue;
        };
        let clue = clue.trim().trim_matches('"').trim().to_string();
        let list = map.entry(answer).or_default();
        if !clue.is_empty() && !list.contains(&clue) {
            list.push(clue);
        }
    }
    map
}

/// Load a clue CSV as both the fill wordlist (scores derived from clue
/// counts) and the clue database used to clue the finished grid. A
/// `blocklist.txt` next to the CSV is honored, as with `Wordlist::load`.
pub fn load_clue_csv(path: &str, min_score: u8) -> std::io::Result<(Wordlist, ClueDb)> {
    let bytes = fs::read(path)?;
    let clues = parse_clue_csv(&bytes);
    let blocklist = blocklist_beside(path);
    if !blocklist.is_empty() {
        eprintln!("blocklist: excluding {} word(s)", blocklist.len());
    }
    let mut dict = String::with_capacity(clues.len() * 12);
    let mut answers: Vec<&String> = clues.keys().collect();
    answers.sort();
    for a in answers {
        dict.push_str(a);
        dict.push(';');
        dict.push_str(&score_from_clue_count(clues[a].len()).to_string());
        dict.push('\n');
    }
    let wl = Wordlist::from_str_filtered(&dict, min_score, &blocklist);
    Ok((wl, ClueDb { clues }))
}

/// Load any supported word source: a `.csv` clue file (wordlist + clue
/// database) or a scored `WORD;score` dict (wordlist only, no clues).
pub fn load_wordlist(path: &str, min_score: u8) -> std::io::Result<(Wordlist, Option<ClueDb>)> {
    if path.to_ascii_lowercase().ends_with(".csv") {
        let (wl, db) = load_clue_csv(path, min_score)?;
        Ok((wl, Some(db)))
    } else {
        Ok((Wordlist::load(path, min_score)?, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_accents_and_punctuation() {
        assert_eq!(normalize_answer("città").as_deref(), Some("CITTA"));
        assert_eq!(normalize_answer("Tom Dick").as_deref(), Some("TOMDICK"));
        assert_eq!(normalize_answer("L'Aquila").as_deref(), Some("LAQUILA"));
        assert_eq!(normalize_answer("90°"), None);
        assert_eq!(normalize_answer(""), None);
    }

    #[test]
    fn parses_csv_with_bom_crlf_latin1_and_dups() {
        let mut bytes =
            b"\xEF\xBB\xBFAA,Alto Adige\r\nAA,Alto Adige\r\nAA,Ai lati dell'aula\r\n".to_vec();
        bytes.extend_from_slice(b"ETA,Et\xe0\r\n"); // Latin-1 'à'
        bytes.extend_from_slice(b"senza virgola\r\n,clue senza parola\r\n");
        let m = parse_clue_csv(&bytes);
        assert_eq!(m["AA"], vec!["Alto Adige", "Ai lati dell'aula"]);
        assert_eq!(m["ETA"], vec!["Età"]);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn restores_apostrophe_accents() {
        assert_eq!(fix_accents("Citta' della Norvegia"), "Città della Norvegia");
        assert_eq!(
            fix_accents("Lo e' colei che... da' la colpa"),
            "Lo è colei che... dà la colpa"
        );
        assert_eq!(
            fix_accents("Ne' marittima ne' terrestre"),
            "Né marittima né terrestre"
        );
        assert_eq!(fix_accents("Perche' no?"), "Perché no?");
        assert_eq!(
            fix_accents("Musico' il Mefistofele"),
            "Musicò il Mefistofele"
        );
        assert_eq!(
            fix_accents("Un po' di sale, nell'aula"),
            "Un po' di sale, nell'aula"
        );
        assert_eq!(
            fix_accents("Il compositore di 'Giselle'"),
            "Il compositore di 'Giselle'"
        );
        assert_eq!(fix_accents("Ventitre'"), "Ventitré");
    }

    #[test]
    fn pick_clue_avoids_answer_in_clue() {
        let db = ClueDb {
            clues: parse_clue_csv(b"MARE,Il mare blu\nMARE,Distesa salata\n"),
        };
        for k in 0..4 {
            assert_eq!(
                db.pick_clue("MARE", |_| k).as_deref(),
                Some("Distesa salata")
            );
        }
        assert_eq!(db.pick_clue("ZZZ", |_| 0), None);
    }

    #[test]
    fn pick_clue_prefers_complete_clues_and_capitalizes() {
        let db = ClueDb {
            clues: parse_clue_csv(b"ABBA,Un\nABBA,Erano duecento n\nABBA,e' un gruppo svedese\n"),
        };
        for k in 0..3 {
            assert_eq!(
                db.pick_clue("ABBA", |_| k).as_deref(),
                Some("È un gruppo svedese")
            );
        }
        // only truncated clues available → still returns one
        let db = ClueDb {
            clues: parse_clue_csv(b"ACA,Un\n"),
        };
        assert_eq!(db.pick_clue("ACA", |_| 0).as_deref(), Some("Un"));
    }

    #[test]
    fn score_grows_with_clue_count() {
        assert_eq!(score_from_clue_count(1), 50);
        assert!(score_from_clue_count(3) > score_from_clue_count(2));
        assert_eq!(score_from_clue_count(1000), 90);
    }

    #[test]
    fn two_letter_words_are_loadable() {
        let m = parse_clue_csv(b"TI,Pronome\nCASA,Abitazione\n");
        assert_eq!(m.len(), 2);
        let mut dict = String::new();
        for (w, c) in &m {
            dict.push_str(&format!("{};{}\n", w, score_from_clue_count(c.len())));
        }
        let wl = Wordlist::from_str(&dict, 0);
        assert_eq!(wl.len_data(2).n, 1);
        assert_eq!(wl.len_data(4).n, 1);
    }
}
