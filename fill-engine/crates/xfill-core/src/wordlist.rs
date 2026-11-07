//! Scored wordlist with per-length compatibility masks for bitset-based
//! constraint propagation. Built from `WORD;score` lines (the clue CSV loader
//! in `clues` produces them in memory).
//!
//! Within each length, words are sorted by score DESCENDING, so word index 0 is
//! the highest-quality entry. This makes score-ordered enumeration (iterate set
//! bits low→high) and quality tiers (an index cutoff) fall out for free.

use crate::bitset::Bitset;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

/// Longest entry the engine can index. Raised from 15 (NYT) to 24 for Italian
/// grids, whose word lists and rectangular layouts carry longer answers.
pub const MAX_LEN: usize = 24;
/// Shortest run that counts as an entry. Italian crosswords use 2-letter
/// answers; a 1-letter run is an unclued cell (it belongs only to the
/// perpendicular entry), which `grid::Puzzle` already models.
pub const MIN_LEN: usize = 2;
/// Symbols per cell: 26 letters plus up to 6 rebus strings.
pub const ALPHA: usize = 32;
/// Symbol of the first rebus string (letters are 0..26).
pub const REBUS_BASE: u8 = 26;

/// Letters of `s` as 0..25, ignoring any non-alphabetic characters.
fn normalize_letters(s: &str) -> Vec<u8> {
    s.chars()
        .filter_map(|c| {
            let up = c.to_ascii_uppercase();
            if up.is_ascii_uppercase() {
                Some(up as u8 - b'A')
            } else {
                None
            }
        })
        .collect()
}

/// Read a blocklist file (one word per line; `#` comments and blank lines
/// ignored). Returns the normalized words to exclude. Missing file → empty.
fn read_blocklist(path: &Path) -> HashSet<Box<[u8]>> {
    let mut set = HashSet::new();
    if let Ok(text) = fs::read_to_string(path) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let letters = normalize_letters(line);
            if !letters.is_empty() {
                set.insert(letters.into_boxed_slice());
            }
        }
    }
    set
}

/// The blocklist that sits next to the wordlist file (`<dir>/blocklist.txt`).
pub(crate) fn blocklist_beside(wordlist_path: &str) -> HashSet<Box<[u8]>> {
    let bl = match Path::new(wordlist_path).parent() {
        Some(d) => d.join("blocklist.txt"),
        None => Path::new("blocklist.txt").to_path_buf(),
    };
    read_blocklist(&bl)
}

pub struct LenData {
    pub len: usize,
    pub n: usize,
    pub letters: Vec<Box<[u8]>>, // [word][pos] = 0..25
    pub scores: Vec<u8>,
    /// compat[pos * 26 + c] = bitset of words with letter c at pos.
    pub compat: Vec<Bitset>,
    /// pos_count[pos * 26 + c] = number of words with letter c at pos
    /// (cheap O(1) least-constraining-value signal).
    pub pos_count: Vec<u32>,
}

impl LenData {
    #[inline]
    pub fn compat(&self, pos: usize, c: u8) -> &Bitset {
        &self.compat[pos * ALPHA + c as usize]
    }

    #[inline]
    pub fn pos_count(&self, pos: usize, c: u8) -> u32 {
        self.pos_count[pos * ALPHA + c as usize]
    }

    /// First word index whose score is < `floor` (cutoff for a quality tier).
    /// Words are score-descending, so [0, cutoff) are all >= floor.
    pub fn tier_cutoff(&self, floor: u8) -> usize {
        // binary search on descending scores
        let mut lo = 0usize;
        let mut hi = self.n;
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.scores[mid] >= floor {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        lo
    }

    pub fn word_string(&self, i: usize) -> String {
        self.letters[i]
            .iter()
            .map(|&b| (b'A' + b) as char)
            .collect()
    }

    /// Index of an exact word (letters 0..25) in this length bucket, if present.
    /// Linear scan — intended for the handful of theme answers, not hot paths.
    pub fn index_of(&self, letters: &[u8]) -> Option<usize> {
        self.letters.iter().position(|w| w.as_ref() == letters)
    }
}

pub struct Wordlist {
    /// Indexed by length; entries for len < MIN_LEN are empty placeholders.
    pub by_len: Vec<LenData>,
    /// Rebus strings: symbol `REBUS_BASE + k` stands for `rebus[k]`.
    pub rebus: Vec<String>,
}

impl Wordlist {
    pub fn from_str(text: &str, min_score: u8) -> Wordlist {
        Self::from_str_filtered(text, min_score, &HashSet::new())
    }

    pub fn from_str_filtered(
        text: &str,
        min_score: u8,
        blocklist: &HashSet<Box<[u8]>>,
    ) -> Wordlist {
        Self::from_str_rebus(text, min_score, blocklist, &[])
    }

    /// Like `from_str_filtered`, plus rebus variants: for each rebus string
    /// R (at most `ALPHA - 26`) every word containing R also enters as a
    /// variant with that occurrence replaced by R's single-cell symbol (same
    /// score). `word()` spells variants back with the full string.
    pub fn from_str_rebus(
        text: &str,
        min_score: u8,
        blocklist: &HashSet<Box<[u8]>>,
        rebus: &[String],
    ) -> Wordlist {
        let rebus: Vec<String> = rebus
            .iter()
            .map(|r| r.to_ascii_uppercase())
            .take(ALPHA - REBUS_BASE as usize)
            .collect();
        // collect (letters, score) per length
        let mut buckets: Vec<Vec<(Box<[u8]>, u8)>> = (0..=MAX_LEN).map(|_| Vec::new()).collect();
        let mut seen: HashSet<Box<[u8]>> = HashSet::new();

        for line in text.lines() {
            let line = line.trim();
            let Some((word, score_s)) = line.split_once(';') else {
                continue;
            };
            let score: u8 = match score_s.trim().parse::<i32>() {
                Ok(s) if s >= 0 => s.min(100) as u8,
                _ => continue,
            };
            let mut letters: Vec<u8> = Vec::with_capacity(word.len());
            let mut ok = true;
            for ch in word.chars() {
                let up = ch.to_ascii_uppercase();
                if up.is_ascii_uppercase() {
                    letters.push(up as u8 - b'A');
                } else {
                    ok = false;
                    break;
                }
            }
            if !ok {
                continue;
            }
            let len = letters.len();
            if !(MIN_LEN..=MAX_LEN).contains(&len) {
                continue;
            }
            let boxed: Box<[u8]> = letters.into_boxed_slice();
            if blocklist.contains(&boxed) {
                continue;
            }
            // First occurrence wins; a later duplicate never overrides it.
            if seen.contains(&boxed) {
                continue;
            }
            seen.insert(boxed.clone());
            if score < min_score {
                continue;
            }
            buckets[len].push((boxed, score));
        }

        // rebus variants
        for (k, r) in rebus.iter().enumerate() {
            let rl = normalize_letters(r);
            if rl.len() < 2 {
                continue;
            }
            let sym = REBUS_BASE + k as u8;
            let originals: Vec<(Box<[u8]>, u8)> = buckets.iter().flatten().cloned().collect();
            for (w, sc) in originals {
                if w.iter().any(|&c| c >= REBUS_BASE) {
                    continue;
                }
                for i in 0..=w.len().saturating_sub(rl.len()) {
                    if w.len() < rl.len() || w[i..i + rl.len()] != rl[..] {
                        continue;
                    }
                    let v: Vec<u8> = w[..i]
                        .iter()
                        .copied()
                        .chain(std::iter::once(sym))
                        .chain(w[i + rl.len()..].iter().copied())
                        .collect();
                    let boxed: Box<[u8]> = v.into_boxed_slice();
                    if (MIN_LEN..=MAX_LEN).contains(&boxed.len()) && seen.insert(boxed.clone()) {
                        buckets[boxed.len()].push((boxed, sc));
                    }
                }
            }
        }

        let mut by_len: Vec<LenData> = Vec::with_capacity(MAX_LEN + 1);
        for (len, mut entries) in buckets.into_iter().enumerate() {
            // score desc, tie-break by letters asc for determinism
            entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            let n = entries.len();
            let mut letters = Vec::with_capacity(n);
            let mut scores = Vec::with_capacity(n);
            for (w, s) in entries {
                letters.push(w);
                scores.push(s);
            }
            // build compat masks
            let n_masks = if len >= MIN_LEN { len * ALPHA } else { 0 };
            let mut compat: Vec<Bitset> = (0..n_masks).map(|_| Bitset::zeros(n.max(1))).collect();
            for (i, w) in letters.iter().enumerate() {
                for (p, &c) in w.iter().enumerate() {
                    compat[p * ALPHA + c as usize].set(i);
                }
            }
            let pos_count: Vec<u32> = compat.iter().map(|b| b.count_ones()).collect();
            by_len.push(LenData {
                len,
                n,
                letters,
                scores,
                compat,
                pos_count,
            });
        }

        Wordlist { by_len, rebus }
    }

    /// Word `i` of length `len`, with rebus symbols spelled out.
    pub fn word(&self, len: usize, i: usize) -> String {
        self.len_data(len).letters[i]
            .iter()
            .map(|&c| {
                if c < REBUS_BASE {
                    ((b'A' + c) as char).to_string()
                } else {
                    self.rebus[(c - REBUS_BASE) as usize].clone()
                }
            })
            .collect()
    }

    #[inline]
    pub fn len_data(&self, len: usize) -> &LenData {
        &self.by_len[len]
    }

    /// Number of words (all lengths).
    pub fn word_count(&self) -> usize {
        self.by_len.iter().map(|l| l.n).sum()
    }

    /// Approximate heap memory held by the list and its masks (bytes).
    pub fn heap_bytes(&self) -> usize {
        self.by_len
            .iter()
            .map(|l| {
                l.letters.iter().map(|w| w.len()).sum::<usize>()
                    + l.letters.capacity() * std::mem::size_of::<Box<[u8]>>()
                    + l.scores.capacity()
                    + l.compat
                        .iter()
                        .map(|b| b.heap_bytes() + std::mem::size_of::<crate::bitset::Bitset>())
                        .sum::<usize>()
                    + l.pos_count.capacity() * std::mem::size_of::<u32>()
            })
            .sum::<usize>()
            + self.rebus.iter().map(String::capacity).sum::<usize>()
    }

    /// Length of the longest word in the list (0 if empty).
    pub fn longest(&self) -> usize {
        self.by_len.iter().rposition(|l| l.n > 0).unwrap_or(0)
    }

    /// Entry lengths used by `p` for which the list has no words, with the
    /// number of entries of each such length (ascending by length).
    pub fn missing_lengths(&self, p: &crate::grid::Puzzle) -> Vec<(usize, usize)> {
        let mut out: Vec<(usize, usize)> = Vec::new();
        for e in &p.entries {
            if e.len < self.by_len.len() && self.by_len[e.len].n > 0 {
                continue;
            }
            match out.iter_mut().find(|(l, _)| *l == e.len) {
                Some((_, c)) => *c += 1,
                None => out.push((e.len, 1)),
            }
        }
        out.sort_unstable();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Wordlist {
        // scores chosen to exercise sorting + tiers
        let src = "\
ABBEY;90
EDGAR;80
TOSSED;40
CAT;75
DOG;75
RAT;30
FOO;25
";
        Wordlist::from_str(src, 40)
    }

    #[test]
    fn loads_and_filters_min_score() {
        let wl = tiny();
        let l3 = wl.len_data(3);
        // CAT, DOG (75), then... RAT(30) and FOO(25) are below min_score 40 → excluded
        let words: Vec<String> = (0..l3.n).map(|i| l3.word_string(i)).collect();
        assert!(words.contains(&"CAT".to_string()));
        assert!(words.contains(&"DOG".to_string()));
        assert!(!words.contains(&"RAT".to_string()));
        assert!(!words.contains(&"FOO".to_string()));
    }

    #[test]
    fn sorted_score_desc() {
        let wl = tiny();
        let l5 = wl.len_data(5);
        // ABBEY(90) before EDGAR(80)
        assert_eq!(l5.word_string(0), "ABBEY");
        assert_eq!(l5.word_string(1), "EDGAR");
        assert!(l5.scores[0] >= l5.scores[1]);
    }

    #[test]
    fn compat_masks_correct() {
        let wl = tiny();
        let l3 = wl.len_data(3);
        // words with 'A' (c=0) at pos 1: CAT, RAT(excluded) → just CAT among kept
        let mask = l3.compat(1, 0);
        let hits: Vec<String> = mask.iter_ones().map(|i| l3.word_string(i)).collect();
        assert!(hits.contains(&"CAT".to_string()));
        assert!(!hits.contains(&"DOG".to_string()));
    }

    #[test]
    fn blocklist_excludes_regardless_of_score() {
        // ABBEY is scored 90; block it anyway. CAT (75) stays.
        let src = "ABBEY;90\nCAT;75\nDOG;75\n";
        let mut block: HashSet<Box<[u8]>> = HashSet::new();
        block.insert(normalize_letters("abbey").into_boxed_slice()); // case-insensitive
        let wl = Wordlist::from_str_filtered(src, 40, &block);
        let l5: Vec<String> = (0..wl.len_data(5).n)
            .map(|i| wl.len_data(5).word_string(i))
            .collect();
        assert!(
            !l5.contains(&"ABBEY".to_string()),
            "blocked word must be gone"
        );
        let l3: Vec<String> = (0..wl.len_data(3).n)
            .map(|i| wl.len_data(3).word_string(i))
            .collect();
        assert!(l3.contains(&"CAT".to_string()));
    }

    #[test]
    fn longest_and_missing_lengths() {
        let wl = tiny(); // kept lengths: 3, 5, 6
        assert_eq!(wl.longest(), 6);
        let p = crate::grid::Puzzle::from_template("....#...\n");
        // slots: 4 (missing) and 3 (present)
        assert_eq!(wl.missing_lengths(&p), vec![(4, 1)]);
        let p = crate::grid::Puzzle::from_template("...\n");
        assert!(wl.missing_lengths(&p).is_empty());
        assert_eq!(Wordlist::from_str("", 0).longest(), 0);
    }

    #[test]
    fn rebus_variants_replace_the_string_with_a_symbol() {
        let wl = Wordlist::from_str_rebus(
            "CREDO;80\nREDO;70\nCARE;60\nSOLE;50\n",
            0,
            &HashSet::new(),
            &["RE".to_string()],
        );
        assert_eq!(wl.rebus, vec!["RE".to_string()]);
        let sym = REBUS_BASE;
        // C·RE·DO (4 cells), RE·DO → ·RE·DO (3 cells), CA·RE (3 cells)
        let l4 = wl.len_data(4);
        let v = l4.index_of(&[2, sym, 3, 14]).expect("C<RE>DO");
        assert_eq!(wl.word(4, v), "CREDO");
        assert_eq!(l4.scores[v], 80);
        assert!(wl.len_data(3).index_of(&[sym, 3, 14]).is_some());
        assert!(wl.len_data(3).index_of(&[2, 0, sym]).is_some());
        // originals stay; words without RE get no variant
        assert!(l4.index_of(&normalize_letters("REDO")).is_some());
        assert!(wl.len_data(3).index_of(&normalize_letters("SOL")).is_none());
    }

    #[test]
    fn tier_cutoff_works() {
        let wl = tiny();
        let l5 = wl.len_data(5);
        // length-5 words: ABBEY(90), EDGAR(80). (TOSSED is length 6.)
        assert_eq!(l5.tier_cutoff(85), 1); // only ABBEY >= 85
        assert_eq!(l5.tier_cutoff(80), 2); // ABBEY, EDGAR
        assert_eq!(l5.tier_cutoff(40), 2); // both
        assert_eq!(l5.tier_cutoff(95), 0);
    }
}
