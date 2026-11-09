//! Scored wordlist with per-length compatibility masks for bitset-based
//! constraint propagation. Built from `WORD;score` lines (the clue CSV loader
//! in `clues` produces them in memory).
//!
//! Within each length, words are sorted by score DESCENDING, so word index 0 is
//! the highest-quality entry. This makes score-ordered enumeration (iterate set
//! bits low→high) and quality tiers (an index cutoff) fall out for free.

use crate::mapped::Buf;
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
/// Words kept per length (the best ones): the solver counts candidates per
/// (position, letter) in 16 bits, which halves its search memory.
pub const MAX_WORDS_PER_LEN: usize = u16::MAX as usize;
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
    /// Letters, contiguous: word `w` is `flat[w * len..(w + 1) * len]`
    /// (symbols 0..25 = A..Z, then rebus symbols).
    pub flat: Buf<u8>,
    pub scores: Buf<u8>,
    /// Compatibility masks, one block run per (position, symbol) with at
    /// least one word: bitset of the words with symbol c at pos. Runs of
    /// `blocks_per_mask` u64 each; offset 0 is an all-zero run shared by
    /// every absent (position, symbol).
    pub(crate) compat_blocks: Buf<u64>,
    /// compat_off[pos * ALPHA + c] = start of its run in `compat_blocks`.
    pub(crate) compat_off: Buf<u32>,
    pub(crate) blocks_per_mask: usize,
    /// pos_count[pos * ALPHA + c] = number of words with letter c at pos
    /// (cheap O(1) least-constraining-value signal).
    pub pos_count: Buf<u32>,
}

impl LenData {
    /// Words with symbol `c` at `pos`, as 64-bit blocks (bit w = word w).
    #[inline]
    pub fn compat(&self, pos: usize, c: u8) -> &[u64] {
        let off = self.compat_off[pos * ALPHA + c as usize] as usize;
        &self.compat_blocks[off..off + self.blocks_per_mask]
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

    /// Letters of word `i`.
    #[inline]
    pub fn word_letters(&self, i: usize) -> &[u8] {
        &self.flat[i * self.len..(i + 1) * self.len]
    }

    pub fn word_string(&self, i: usize) -> String {
        self.word_letters(i)
            .iter()
            .map(|&b| (b'A' + b) as char)
            .collect()
    }

    /// Index of an exact word (letters 0..25) in this length bucket, if present.
    /// Linear scan — intended for the handful of theme answers, not hot paths.
    pub fn index_of(&self, letters: &[u8]) -> Option<usize> {
        if letters.len() != self.len || self.len == 0 {
            return None;
        }
        self.flat.chunks_exact(self.len).position(|w| w == letters)
    }

    /// Words sorted by score (descending, then letters) → per-length data.
    fn build(len: usize, flat: Vec<u8>, scores: Vec<u8>) -> LenData {
        let n = scores.len();
        let nb = n.div_ceil(64).max(1);
        let n_masks = if len >= MIN_LEN { len * ALPHA } else { 0 };
        let mut pos_count = vec![0u32; n_masks];
        for (p_c, &c) in flat.iter().enumerate() {
            pos_count[(p_c % len) * ALPHA + c as usize] += 1;
        }
        let mut compat_off = vec![0u32; n_masks];
        let mut next = nb; // run 0 = the shared zeros
        for (i, &k) in pos_count.iter().enumerate() {
            if k > 0 {
                compat_off[i] = next as u32;
                next += nb;
            }
        }
        let mut compat_blocks = vec![0u64; next];
        for w in 0..n {
            for p in 0..len {
                let c = flat[w * len + p] as usize;
                let off = compat_off[p * ALPHA + c] as usize;
                compat_blocks[off + (w >> 6)] |= 1u64 << (w & 63);
            }
        }
        LenData {
            len,
            n,
            flat: flat.into(),
            scores: scores.into(),
            compat_blocks: compat_blocks.into(),
            compat_off: compat_off.into(),
            blocks_per_mask: nb,
            pos_count: pos_count.into(),
        }
    }

    /// Per-length data from its arrays (a dictionary file), checked so that
    /// no later access can go out of bounds.
    pub(crate) fn from_parts(
        len: usize,
        flat: Buf<u8>,
        scores: Buf<u8>,
        compat_blocks: Buf<u64>,
        compat_off: Buf<u32>,
        pos_count: Buf<u32>,
        symbols: usize,
    ) -> Result<LenData, String> {
        let n = scores.len();
        let nb = n.div_ceil(64).max(1);
        let n_masks = if len >= MIN_LEN { len * ALPHA } else { 0 };
        let bad = |what: &str| Err(format!("length {len}: {what}"));
        if flat.len() != n * len || compat_off.len() != n_masks || pos_count.len() != n_masks {
            return bad("array sizes");
        }
        if n > MAX_WORDS_PER_LEN
            || compat_blocks.len() < nb
            || !compat_blocks.len().is_multiple_of(nb)
        {
            return bad("mask sizes");
        }
        if compat_off
            .iter()
            .any(|&o| !(o as usize).is_multiple_of(nb) || o as usize + nb > compat_blocks.len())
        {
            return bad("mask offsets");
        }
        if flat.iter().any(|&c| c as usize >= symbols) {
            return bad("letters");
        }
        Ok(LenData {
            len,
            n,
            flat,
            scores,
            compat_blocks,
            compat_off,
            blocks_per_mask: nb,
            pos_count,
        })
    }

    /// Heap memory (bytes).
    fn heap_bytes(&self) -> usize {
        self.flat.heap_bytes()
            + self.scores.heap_bytes()
            + self.compat_blocks.heap_bytes()
            + self.compat_off.heap_bytes()
            + self.pos_count.heap_bytes()
    }

    /// Bytes read in place from a dictionary file.
    fn mapped_bytes(&self) -> usize {
        self.flat.mapped_bytes()
            + self.scores.mapped_bytes()
            + self.compat_blocks.mapped_bytes()
            + self.compat_off.mapped_bytes()
            + self.pos_count.mapped_bytes()
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

    /// `WORD;score` lines (other lines and non-letter words are skipped);
    /// see [`Wordlist::from_scored`].
    pub fn from_str_rebus(
        text: &str,
        min_score: u8,
        blocklist: &HashSet<Box<[u8]>>,
        rebus: &[String],
    ) -> Wordlist {
        let entries = text.lines().filter_map(|line| {
            let (word, score_s) = line.trim().split_once(';')?;
            let score: u8 = match score_s.trim().parse::<i32>() {
                Ok(s) if s >= 0 => s.min(100) as u8,
                _ => return None,
            };
            let mut letters: Vec<u8> = Vec::with_capacity(word.len());
            for ch in word.chars() {
                let up = ch.to_ascii_uppercase();
                if !up.is_ascii_uppercase() {
                    return None;
                }
                letters.push(up as u8 - b'A');
            }
            Some((letters, score))
        });
        Self::from_scored(entries, min_score, blocklist, rebus)
    }

    /// Build the list from (letters 0..25, score) entries. Words outside
    /// MIN_LEN..=MAX_LEN or in `blocklist` are skipped; of repeated words the
    /// first occurrence wins (a later duplicate never overrides it), then
    /// words scoring below `min_score` are dropped. For each rebus string R
    /// (at most `ALPHA - 26`) every kept word containing R also enters as a
    /// variant with that occurrence replaced by R's single-cell symbol (same
    /// score); `word()` spells variants back with the full string.
    pub fn from_scored<I, A>(
        entries: I,
        min_score: u8,
        blocklist: &HashSet<Box<[u8]>>,
        rebus: &[String],
    ) -> Wordlist
    where
        I: IntoIterator<Item = (A, u8)>,
        A: AsRef<[u8]>,
    {
        let rebus: Vec<String> = rebus
            .iter()
            .map(|r| r.to_ascii_uppercase())
            .take(ALPHA - REBUS_BASE as usize)
            .collect();
        // per length: letters (contiguous) and (score, input order)
        let mut flat: Vec<Vec<u8>> = (0..=MAX_LEN).map(|_| Vec::new()).collect();
        let mut meta: Vec<Vec<(u8, u32)>> = (0..=MAX_LEN).map(|_| Vec::new()).collect();
        for (order, (w, score)) in entries.into_iter().enumerate() {
            let w = w.as_ref();
            let len = w.len();
            if !(MIN_LEN..=MAX_LEN).contains(&len) || blocklist.contains(w) {
                continue;
            }
            flat[len].extend_from_slice(w);
            meta[len].push((score, order as u32));
        }
        // dedup (first occurrence wins), then the score floor
        let mut kept: Vec<Vec<(u32, u8)>> = (0..=MAX_LEN).map(|_| Vec::new()).collect();
        for len in MIN_LEN..=MAX_LEN {
            let f = &flat[len];
            let word = |i: u32| &f[i as usize * len..(i as usize + 1) * len];
            let mut idx: Vec<u32> = (0..meta[len].len() as u32).collect();
            idx.sort_unstable_by(|&a, &b| {
                word(a)
                    .cmp(word(b))
                    .then(meta[len][a as usize].1.cmp(&meta[len][b as usize].1))
            });
            idx.dedup_by(|b, a| word(*a) == word(*b));
            kept[len] = idx
                .into_iter()
                .map(|i| (i, meta[len][i as usize].0))
                .filter(|&(_, sc)| sc >= min_score)
                .collect();
            meta[len] = Vec::new();
        }
        // rebus variants of the kept words (a variant holds a rebus symbol,
        // so it can't repeat a word; one per occurrence of the string)
        for (k, r) in rebus.iter().enumerate() {
            let rl = normalize_letters(r);
            if rl.len() < 2 {
                continue;
            }
            let sym = REBUS_BASE + k as u8;
            for len in MIN_LEN..=MAX_LEN {
                if len < rl.len() {
                    continue;
                }
                let vlen = len + 1 - rl.len();
                for j in 0..kept[len].len() {
                    let (i, sc) = kept[len][j];
                    let w = flat[len][i as usize * len..(i as usize + 1) * len].to_vec();
                    if w.iter().any(|&c| c >= REBUS_BASE) {
                        continue;
                    }
                    for at in 0..=len - rl.len() {
                        if w[at..at + rl.len()] != rl[..] || vlen < MIN_LEN {
                            continue;
                        }
                        let vi = (flat[vlen].len() / vlen) as u32;
                        flat[vlen].extend_from_slice(&w[..at]);
                        flat[vlen].push(sym);
                        flat[vlen].extend_from_slice(&w[at + rl.len()..]);
                        kept[vlen].push((vi, sc));
                    }
                }
            }
        }

        let mut by_len: Vec<LenData> = Vec::with_capacity(MAX_LEN + 1);
        for len in 0..=MAX_LEN {
            let f = std::mem::take(&mut flat[len]);
            let mut k = std::mem::take(&mut kept[len]);
            let word = |i: u32| &f[i as usize * len..(i as usize + 1) * len];
            // score desc, tie-break by letters asc for determinism
            k.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| word(a.0).cmp(word(b.0))));
            k.truncate(MAX_WORDS_PER_LEN);
            let mut out = Vec::with_capacity(k.len() * len);
            for &(i, _) in &k {
                out.extend_from_slice(word(i));
            }
            let scores = k.iter().map(|&(_, sc)| sc).collect();
            drop(f);
            by_len.push(LenData::build(len, out, scores));
        }

        Wordlist { by_len, rebus }
    }

    /// Word `i` of length `len`, with rebus symbols spelled out.
    pub fn word(&self, len: usize, i: usize) -> String {
        let mut out = String::with_capacity(len);
        for &c in self.len_data(len).word_letters(i) {
            if c < REBUS_BASE {
                out.push((b'A' + c) as char);
            } else {
                out.push_str(&self.rebus[(c - REBUS_BASE) as usize]);
            }
        }
        out
    }

    #[inline]
    pub fn len_data(&self, len: usize) -> &LenData {
        &self.by_len[len]
    }

    /// Number of words (all lengths).
    pub fn word_count(&self) -> usize {
        self.by_len.iter().map(|l| l.n).sum()
    }

    /// Bytes of the list read in place from a dictionary file.
    pub fn mapped_bytes(&self) -> usize {
        self.by_len.iter().map(LenData::mapped_bytes).sum()
    }

    /// Heap memory held by the list and its masks (bytes).
    pub fn heap_bytes(&self) -> usize {
        self.by_len.iter().map(LenData::heap_bytes).sum::<usize>()
            + self.by_len.capacity() * std::mem::size_of::<LenData>()
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
        let hits: Vec<String> = (0..l3.n)
            .filter(|&i| (mask[i >> 6] >> (i & 63)) & 1 == 1)
            .map(|i| l3.word_string(i))
            .collect();
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
    fn a_length_keeps_at_most_max_words_the_best_ones() {
        // 70,000 distinct 4-letter words; the solver counts supports in u16
        let mut entries: Vec<(Vec<u8>, u8)> = Vec::new();
        for i in 0..70_000u32 {
            let w = vec![
                (i % 26) as u8,
                (i / 26 % 26) as u8,
                (i / 676 % 26) as u8,
                (i / 17_576) as u8,
            ];
            // the last 4,465 words score lowest
            entries.push((w, if i < MAX_WORDS_PER_LEN as u32 { 60 } else { 10 }));
        }
        let wl = Wordlist::from_scored(entries, 0, &HashSet::new(), &[]);
        let l4 = wl.len_data(4);
        assert_eq!(l4.n, MAX_WORDS_PER_LEN);
        assert!(l4.scores.iter().all(|&s| s == 60));
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
