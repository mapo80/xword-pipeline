//! Word + clue database in `WORD,clue text` CSV form (one clue per line, a
//! word may repeat with different clues) — the format of the Italian dataset.
//!
//! The file carries no quality scores, so a word's score is derived from how
//! many distinct clues it has: words clued many times are the ones setters
//! actually use. 1 clue → 50, 3 → 63, 10 → 78, capped at 90.

use crate::wordlist::{blocklist_beside, Wordlist, MAX_LEN};
use std::fs;

/// Clues per normalized answer (A–Z), in file order, deduplicated.
///
/// Stored compactly (a handful of allocations whatever the size): the
/// answers sorted and concatenated, the clue texts concatenated answer by
/// answer, and u32 offsets into both.
#[derive(Default)]
pub struct ClueDb {
    /// answer i = `answers[aoff[i]..aoff[i + 1]]`, sorted
    answers: String,
    aoff: Vec<u32>,
    /// clues of answer i = clue indices `first[i]..first[i + 1]`
    first: Vec<u32>,
    /// clue j = `text[coff[j]..coff[j + 1]]`
    text: String,
    coff: Vec<u32>,
}

impl ClueDb {
    /// Parse `WORD,clue` CSV bytes (UTF-8 or Latin-1 per line, optional
    /// BOM, CRLF). Lines without a comma, with an empty/unusable word, or
    /// `#` comments are skipped; an answer whose lines carry no clue text is
    /// kept with no clues.
    pub fn from_csv_bytes(bytes: &[u8]) -> ClueDb {
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        // Every buffer is sized exactly by a first pass, so loading never
        // holds more than the final database plus a 12-byte index per line
        // and the normalized answers.
        let mut line_buf = String::new();
        let mut word_buf = String::new();
        let (mut n_lines, mut answer_bytes, mut clue_bytes) = (0usize, 0usize, 0usize);
        for raw in bytes.split(|&b| b == b'\n') {
            let Some((word, clue)) = split_line(raw, &mut line_buf) else {
                continue;
            };
            word_buf.clear();
            if normalize_answer_into(word.trim(), &mut word_buf) {
                n_lines += 1;
                answer_bytes += word_buf.len();
                clue_bytes += clue.trim().trim_matches('"').trim().len();
            }
        }
        // pass 1: normalized answers (concatenated) and the line of each:
        // (answer offset, answer length, line start)
        let mut abuf = String::with_capacity(answer_bytes);
        let mut idx: Vec<(u32, u32, u32)> = Vec::with_capacity(n_lines);
        let mut at = 0usize;
        for raw in bytes.split(|&b| b == b'\n') {
            let start = at;
            at += raw.len() + 1;
            let Some((word, _)) = split_line(raw, &mut line_buf) else {
                continue;
            };
            let a0 = abuf.len();
            if !normalize_answer_into(word.trim(), &mut abuf) {
                abuf.truncate(a0);
                continue;
            }
            idx.push((a0 as u32, (abuf.len() - a0) as u32, start as u32));
        }
        let ans = |e: &(u32, u32, u32)| &abuf[e.0 as usize..(e.0 + e.1) as usize];
        // group by answer, file order within a group
        idx.sort_unstable_by(|a, b| ans(a).cmp(ans(b)).then(a.2.cmp(&b.2)));
        let (mut n_answers, mut unique_bytes) = (0usize, 0usize);
        for (k, e) in idx.iter().enumerate() {
            if k == 0 || ans(&idx[k - 1]) != ans(e) {
                n_answers += 1;
                unique_bytes += e.1 as usize;
            }
        }
        // pass 2: answers, then their clues decoded straight into place
        let mut db = ClueDb {
            answers: String::with_capacity(unique_bytes),
            aoff: Vec::with_capacity(n_answers + 1),
            first: Vec::with_capacity(n_answers + 1),
            text: String::with_capacity(clue_bytes),
            coff: Vec::with_capacity(n_lines + 1),
        };
        db.coff.push(0);
        db.aoff.push(0);
        let mut i = 0;
        while i < idx.len() {
            let a = ans(&idx[i]);
            db.answers.push_str(a);
            db.aoff.push(db.answers.len() as u32);
            db.first.push((db.coff.len() - 1) as u32);
            let group_start = db.coff.len() - 1;
            while i < idx.len() && ans(&idx[i]) == a {
                let ls = idx[i].2 as usize;
                i += 1;
                let le = bytes[ls..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |n| ls + n);
                let Some((_, clue)) = split_line(&bytes[ls..le], &mut line_buf) else {
                    continue;
                };
                let clue = clue.trim().trim_matches('"').trim();
                let dup = (group_start..db.coff.len() - 1)
                    .any(|j| &db.text[db.coff[j] as usize..db.coff[j + 1] as usize] == clue);
                if !clue.is_empty() && !dup {
                    db.text.push_str(clue);
                    db.coff.push(db.text.len() as u32);
                }
            }
        }
        db.first.push((db.coff.len() - 1) as u32);
        drop(idx);
        drop(abuf);
        // only repeated clues leave slack
        db.text.shrink_to_fit();
        db.coff.shrink_to_fit();
        db
    }

    fn answer(&self, i: usize) -> &str {
        &self.answers[self.aoff[i] as usize..self.aoff[i + 1] as usize]
    }

    fn clues_of(&self, i: usize) -> impl ExactSizeIterator<Item = &str> + Clone + '_ {
        (self.first[i] as usize..self.first[i + 1] as usize)
            .map(move |j| &self.text[self.coff[j] as usize..self.coff[j + 1] as usize])
    }

    /// The clues of `answer` (normalized A–Z), in file order.
    pub fn get(&self, answer: &str) -> impl ExactSizeIterator<Item = &str> + Clone + '_ {
        let (mut lo, mut hi) = (0, self.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.answer(mid) < answer {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let r = if lo < self.len() && self.answer(lo) == answer {
            self.first[lo] as usize..self.first[lo + 1] as usize
        } else {
            0..0
        };
        r.map(move |j| &self.text[self.coff[j] as usize..self.coff[j + 1] as usize])
    }

    /// Answers in order, with their number of clues.
    pub fn answers(&self) -> impl Iterator<Item = (&str, usize)> + '_ {
        (0..self.len()).map(move |i| (self.answer(i), self.clues_of(i).len()))
    }

    /// Number of answers.
    pub fn len(&self) -> usize {
        self.first.len().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Heap memory held by the clues (bytes).
    pub fn heap_bytes(&self) -> usize {
        self.answers.capacity()
            + self.text.capacity()
            + 4 * (self.aoff.capacity() + self.first.capacity() + self.coff.capacity())
    }
}

/// Decode one CSV line (UTF-8 when valid, else Latin-1 — the dataset mixes
/// both) into `buf` and split it at the first comma: (word, clue). `None`
/// for comments and lines without a comma.
fn split_line<'b>(raw: &[u8], buf: &'b mut String) -> Option<(&'b str, &'b str)> {
    buf.clear();
    match std::str::from_utf8(raw) {
        Ok(s) => buf.push_str(s),
        Err(_) => buf.extend(raw.iter().map(|&b| b as char)),
    }
    let line = buf.trim_end_matches('\r');
    if line.trim_start().starts_with('#') {
        return None;
    }
    line.split_once(',')
}

impl ClueDb {
    /// Pick a clue for `answer`, avoiding clues that contain the answer
    /// itself (for answers of 3+ letters). `pick` chooses among the usable
    /// clues (called with their count, must return an index below it).
    /// Accents typed as apostrophes are restored (see [`fix_accents`]).
    pub fn pick_clue(&self, answer: &str, pick: impl FnOnce(usize) -> usize) -> Option<String> {
        let all = self.get(answer);
        let no_answer = |c: &&str| answer.len() < 3 || !normalize_letters_lossy(c).contains(answer);
        // Preference tiers: clean and answer-free → answer-free → anything.
        let tiers: [Vec<&str>; 3] = [
            all.clone()
                .filter(no_answer)
                .filter(|c| !looks_truncated(c))
                .collect(),
            all.clone().filter(no_answer).collect(),
            all.collect(),
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
pub(crate) fn normalize_letters_lossy(s: &str) -> String {
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
    normalize_answer_into(word, &mut out).then_some(out)
}

/// [`normalize_answer`] appending to `out`; false (with `out` partly
/// written) when the word is unusable or empty.
fn normalize_answer_into(word: &str, out: &mut String) -> bool {
    let start = out.len();
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
            _ => return false,
        };
        out.push(base);
    }
    out.len() > start
}

/// Load a clue CSV as both the fill wordlist (scores derived from clue
/// counts) and the clue database used to clue the finished grid. A
/// `blocklist.txt` next to the CSV is honored, as with `Wordlist::load`.
pub fn load_clue_csv(path: &str, min_score: u8) -> std::io::Result<(Wordlist, ClueDb)> {
    load_clue_csv_rebus(path, min_score, &[])
}

/// Like `load_clue_csv`, adding rebus variants for these strings (see
/// `Wordlist::from_str_rebus`).
pub fn load_clue_csv_rebus(
    path: &str,
    min_score: u8,
    rebus: &[String],
) -> std::io::Result<(Wordlist, ClueDb)> {
    let bytes = fs::read(path)?;
    let blocklist = blocklist_beside(path);
    if !blocklist.is_empty() {
        eprintln!("blocklist: excluding {} word(s)", blocklist.len());
    }
    Ok(dictionary_from_csv(&bytes, min_score, &blocklist, rebus))
}

/// Letters 0..25 of an answer, inline.
struct Code([u8; MAX_LEN + 1], usize);

impl AsRef<[u8]> for Code {
    fn as_ref(&self) -> &[u8] {
        &self.0[..self.1]
    }
}

/// Build the fill wordlist and the clue database from `WORD,clue` CSV
/// bytes already in memory (see [`load_clue_csv`]).
pub fn dictionary_from_csv(
    bytes: &[u8],
    min_score: u8,
    blocklist: &std::collections::HashSet<Box<[u8]>>,
    rebus: &[String],
) -> (Wordlist, ClueDb) {
    let clues = ClueDb::from_csv_bytes(bytes);
    let entries = clues.answers().map(|(a, n)| {
        // letters on the stack (longer answers are cut to MAX_LEN + 1 and
        // so still skipped by the list)
        let mut code = Code([0; MAX_LEN + 1], 0);
        for (i, b) in a.bytes().take(MAX_LEN + 1).enumerate() {
            code.0[i] = b - b'A';
            code.1 = i + 1;
        }
        (code, score_from_clue_count(n))
    });
    let wl = Wordlist::from_scored(entries, min_score, blocklist, rebus);
    (wl, clues)
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
        let m = ClueDb::from_csv_bytes(&bytes);
        let get = |a: &str| m.get(a).collect::<Vec<_>>();
        assert_eq!(get("AA"), vec!["Alto Adige", "Ai lati dell'aula"]);
        assert_eq!(get("ETA"), vec!["Età"]);
        assert!(get("ZZ").is_empty());
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
        let db = ClueDb::from_csv_bytes(b"MARE,Il mare blu\nMARE,Distesa salata\n");
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
        let db =
            ClueDb::from_csv_bytes(b"ABBA,Un\nABBA,Erano duecento n\nABBA,e' un gruppo svedese\n");
        for k in 0..3 {
            assert_eq!(
                db.pick_clue("ABBA", |_| k).as_deref(),
                Some("È un gruppo svedese")
            );
        }
        // only truncated clues available → still returns one
        let db = ClueDb::from_csv_bytes(b"ACA,Un\n");
        assert_eq!(db.pick_clue("ACA", |_| 0).as_deref(), Some("Un"));
    }

    #[test]
    fn load_clue_csv_scores_and_honors_blocklist() {
        let dir = std::env::temp_dir().join(format!("xfill-clues-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let csv = dir.join("db.csv");
        std::fs::write(
            &csv,
            "MARE,Distesa\nMARE,Acqua salata\nREMO,Pagaia\nBRUTTA,Esclusa\n",
        )
        .unwrap();
        std::fs::write(dir.join("blocklist.txt"), "# no\nbrutta\n").unwrap();
        let (wl, db) = load_clue_csv(csv.to_str().unwrap(), 0).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let l4 = wl.len_data(4);
        let mare = l4.index_of(&[12, 0, 17, 4]).expect("MARE loaded");
        assert_eq!(l4.scores[mare], score_from_clue_count(2));
        assert_eq!(
            wl.len_data(6).n,
            0,
            "blocklisted word excluded from the fill"
        );
        assert_eq!(db.len(), 3, "clue database keeps every answer");
        assert!(!db.is_empty());
        assert!(load_clue_csv("/nonexistent/x.csv", 0).is_err());
    }

    #[test]
    fn score_grows_with_clue_count() {
        assert_eq!(score_from_clue_count(1), 50);
        assert!(score_from_clue_count(3) > score_from_clue_count(2));
        assert_eq!(score_from_clue_count(1000), 90);
    }

    #[test]
    fn two_letter_words_are_loadable() {
        let m = ClueDb::from_csv_bytes(b"TI,Pronome\nCASA,Abitazione\n");
        assert_eq!(m.len(), 2);
        let mut dict = String::new();
        for (w, n) in m.answers() {
            dict.push_str(&format!("{};{}\n", w, score_from_clue_count(n)));
        }
        let wl = Wordlist::from_str(&dict, 0);
        assert_eq!(wl.len_data(2).n, 1);
        assert_eq!(wl.len_data(4).n, 1);
    }
}
