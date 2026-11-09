//! Word + clue database in `WORD,clue text` CSV form (one clue per line, a
//! word may repeat with different clues) — the format of the Italian dataset.
//!
//! The file carries no quality scores, so a word's score is derived from how
//! many distinct clues it has: words clued many times are the ones setters
//! actually use. 1 clue → 50, 3 → 63, 10 → 78, capped at 90.

use crate::mapped::Buf;
use crate::wordlist::{blocklist_beside, Wordlist, MAX_LEN};
use std::fs;

/// Separator after each clue of an answer: never a byte of UTF-8 text.
const SEP: u8 = 0xFF;
/// Byte-pair codes are learned on one clue in this many.
const BPE_SAMPLE: usize = 32;

/// Clues per normalized answer (A–Z), in file order, deduplicated.
///
/// Stored compactly, in a handful of allocations: the answers sorted by
/// length and letters (each length group has fixed-size records, so no
/// offsets), one offset per answer into the clue text, and the clue text
/// itself compressed with byte-pair codes (the bytes the text never uses
/// stand for frequent pairs, recursively) — about 0.6 of its size on the
/// Italian dataset. A clue is decoded only when asked for.
pub struct ClueDb {
    /// answers sorted by (length, letters), concatenated
    pub(crate) answers: Buf<u8>,
    /// per length L: index of the first answer of length L, and the byte
    /// where its group starts in `answers` (both with one entry past the end)
    pub(crate) by_len_first: Buf<u32>,
    pub(crate) by_len_byte: Buf<u32>,
    /// clues of answer i = `text[off[i]..off[i + 1]]`, each followed by SEP
    pub(crate) off: Buf<u32>,
    pub(crate) text: Buf<u8>,
    /// byte-pair codes: `pair[b]` is what byte `b` stands for, when
    /// `is_code[b]`
    pub(crate) pair: [[u8; 2]; 256],
    pub(crate) is_code: [bool; 256],
}

/// [`ClueDb`] while it is built.
struct ClueVecs {
    answers: Vec<u8>,
    by_len_first: Vec<u32>,
    by_len_byte: Vec<u32>,
    off: Vec<u32>,
    text: Vec<u8>,
    pair: [[u8; 2]; 256],
    is_code: [bool; 256],
}

impl ClueDb {
    /// Parse `WORD,clue` CSV bytes (UTF-8 or Latin-1 per line, optional
    /// BOM, CRLF). Lines without a comma, with an empty/unusable word, or
    /// `#` comments are skipped; an answer whose lines carry no clue text is
    /// kept with no clues.
    pub fn from_csv_bytes(bytes: &[u8]) -> ClueDb {
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        // Pass 0, over the raw bytes: sizes (upper bounds) and the bytes the
        // clue text uses (those it never uses become byte-pair codes).
        let mut used = [false; 256];
        let (mut n_lines, mut answer_bytes, mut clue_bytes) = (0usize, 0usize, 0usize);
        for raw in bytes.split(|&b| b == b'\n') {
            let Some(comma) = raw.iter().position(|&b| b == b',') else {
                continue;
            };
            let clue = &raw[comma + 1..];
            n_lines += 1;
            answer_bytes += comma;
            if std::str::from_utf8(raw).is_ok() {
                clue_bytes += clue.len();
                for &b in clue {
                    used[b as usize] = true;
                }
            } else {
                // Latin-1: a byte from 0x80 becomes two in UTF-8
                for &b in clue {
                    if b < 0x80 {
                        clue_bytes += 1;
                        used[b as usize] = true;
                    } else {
                        clue_bytes += 2;
                        used[0xC0 | (b >> 6) as usize] = true;
                        used[0x80 | (b & 0x3F) as usize] = true;
                    }
                }
            }
        }
        // Pass 1: normalized answers (concatenated), the line of each
        // (answer offset << 8 | answer length, line start), and a sample of
        // the clues to learn the codes on.
        let mut line_buf = String::new();
        let mut abuf = String::with_capacity(answer_bytes);
        let mut idx: Vec<(u32, u32)> = Vec::with_capacity(n_lines);
        let mut sample: Vec<Vec<u8>> = Vec::new();
        let mut at = 0usize;
        for raw in bytes.split(|&b| b == b'\n') {
            let start = at;
            at += raw.len() + 1;
            let Some((word, clue)) = split_line(raw, &mut line_buf) else {
                continue;
            };
            let a0 = abuf.len();
            if !normalize_answer_into(word.trim(), &mut abuf) || abuf.len() - a0 > 255 {
                abuf.truncate(a0);
                continue;
            }
            if idx.len().is_multiple_of(BPE_SAMPLE) {
                let clue = clue.trim().trim_matches('"').trim();
                if !clue.is_empty() {
                    sample.push(clue.as_bytes().to_vec());
                }
            }
            idx.push(((a0 as u32) << 8 | (abuf.len() - a0) as u32, start as u32));
        }
        let bpe = Bpe::learn(sample, &used);
        let ans = |e: &(u32, u32)| {
            let (off, len) = ((e.0 >> 8) as usize, (e.0 & 255) as usize);
            &abuf.as_bytes()[off..off + len]
        };
        // group by answer (length first), file order within a group
        idx.sort_unstable_by(|a, b| {
            (a.0 & 255, ans(a))
                .cmp(&(b.0 & 255, ans(b)))
                .then(a.1.cmp(&b.1))
        });
        let (mut n_answers, mut unique_bytes, mut longest) = (0usize, 0usize, 0usize);
        for (k, e) in idx.iter().enumerate() {
            if k == 0 || ans(&idx[k - 1]) != ans(e) {
                let len = (e.0 & 255) as usize;
                n_answers += 1;
                unique_bytes += len;
                longest = longest.max(len);
            }
        }
        // Pass 2: answers, and their clues coded straight into place (the
        // raw text is never held whole). Text reserved for the sample's ratio
        // plus a margin.
        let estimate = (clue_bytes as f64 * bpe.ratio * 1.05) as usize + n_lines + 64;
        let mut db = ClueVecs {
            answers: Vec::with_capacity(unique_bytes),
            by_len_first: vec![0; longest + 2],
            by_len_byte: vec![0; longest + 2],
            off: Vec::with_capacity(n_answers + 1),
            text: Vec::with_capacity(estimate.min(clue_bytes + n_lines)),
            pair: bpe.pair,
            is_code: bpe.is_code,
        };
        let mut counts = vec![0u32; longest + 1];
        let mut coded: Vec<u8> = Vec::new();
        let mut i = 0;
        while i < idx.len() {
            let a = ans(&idx[i]);
            counts[a.len()] += 1;
            db.answers.extend_from_slice(a);
            db.off.push(db.text.len() as u32);
            let group = db.text.len();
            while i < idx.len() && ans(&idx[i]) == a {
                let ls = idx[i].1 as usize;
                i += 1;
                let le = bytes[ls..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(bytes.len(), |n| ls + n);
                let Some((_, clue)) = split_line(&bytes[ls..le], &mut line_buf) else {
                    continue;
                };
                let clue = clue.trim().trim_matches('"').trim().as_bytes();
                if clue.is_empty() {
                    continue;
                }
                coded.clear();
                bpe.encode(clue, &mut coded);
                // equal clues have equal codes
                let dup = db.text[group..]
                    .split(|&b| b == SEP)
                    .any(|c| c == coded.as_slice());
                if !dup {
                    db.text.extend_from_slice(&coded);
                    db.text.push(SEP);
                }
            }
        }
        db.off.push(db.text.len() as u32);
        for (len, &n) in counts.iter().enumerate() {
            db.by_len_first[len + 1] = db.by_len_first[len] + n;
            db.by_len_byte[len + 1] = db.by_len_byte[len] + n * len as u32;
        }
        drop(idx);
        drop(abuf);
        drop(bpe);
        db.text.shrink_to_fit();
        ClueDb {
            answers: db.answers.into(),
            by_len_first: db.by_len_first.into(),
            by_len_byte: db.by_len_byte.into(),
            off: db.off.into(),
            text: db.text.into(),
            pair: db.pair,
            is_code: db.is_code,
        }
    }

    /// A database from its arrays (a dictionary file), checked so that no
    /// later lookup can go out of bounds or expand a code forever.
    pub(crate) fn from_parts(
        answers: Buf<u8>,
        by_len_first: Buf<u32>,
        by_len_byte: Buf<u32>,
        off: Buf<u32>,
        text: Buf<u8>,
        pair: [[u8; 2]; 256],
        is_code: [bool; 256],
    ) -> Result<ClueDb, String> {
        let bad = |what: &str| Err(format!("clues: {what}"));
        let n = off.len().saturating_sub(1);
        if by_len_first.len() != by_len_byte.len() || by_len_first.is_empty() || off.is_empty() {
            return bad("index sizes");
        }
        if by_len_first[0] != 0
            || by_len_byte[0] != 0
            || *by_len_first.last().unwrap() as usize != n
        {
            return bad("length groups");
        }
        for len in 0..by_len_first.len() - 1 {
            let (a, b) = (by_len_first[len], by_len_first[len + 1]);
            let bytes = by_len_byte[len] as usize + (b.saturating_sub(a) as usize) * len;
            if b < a || by_len_byte[len + 1] as usize != bytes {
                return bad("length groups");
            }
        }
        if *by_len_byte.last().unwrap() as usize != answers.len() {
            return bad("answers");
        }
        if off.windows(2).any(|w| w[0] > w[1]) || *off.last().unwrap() as usize > text.len() {
            return bad("clue offsets");
        }
        // every code expands to a finite string: codes only refer to bytes
        // already shown to be finite
        let mut finite = is_code.map(|c| !c);
        loop {
            let before = finite.iter().filter(|&&f| f).count();
            for c in 0..256 {
                let [a, b] = pair[c];
                if !finite[c] && finite[a as usize] && finite[b as usize] {
                    finite[c] = true;
                }
            }
            let after = finite.iter().filter(|&&f| f).count();
            if after == 256 {
                break;
            }
            if after == before {
                return bad("byte-pair codes");
            }
        }
        if is_code[SEP as usize] {
            return bad("separator");
        }
        Ok(ClueDb {
            answers,
            by_len_first,
            by_len_byte,
            off,
            text,
            pair,
            is_code,
        })
    }

    fn answer(&self, i: usize) -> &[u8] {
        // the length group of answer i
        let len = self.by_len_first.partition_point(|&f| f as usize <= i) - 1;
        let at = self.by_len_byte[len] as usize + (i - self.by_len_first[len] as usize) * len;
        &self.answers[at..at + len]
    }

    /// Index of `answer`, if present.
    fn find(&self, answer: &str) -> Option<usize> {
        let len = answer.len();
        if len + 1 >= self.by_len_first.len() {
            return None;
        }
        let (first, n) = (
            self.by_len_first[len] as usize,
            (self.by_len_first[len + 1] - self.by_len_first[len]) as usize,
        );
        let base = self.by_len_byte[len] as usize;
        let rec = |k: usize| &self.answers[base + k * len..base + (k + 1) * len];
        let (mut lo, mut hi) = (0, n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if rec(mid) < answer.as_bytes() {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        (lo < n && rec(lo) == answer.as_bytes()).then_some(first + lo)
    }

    /// The coded clues of answer `i`.
    fn group(&self, i: usize) -> &[u8] {
        &self.text[self.off[i] as usize..self.off[i + 1] as usize]
    }

    /// Expand coded bytes into `out` (raw bytes: a single code may stand
    /// for part of a UTF-8 character).
    fn expand_into(&self, coded: &[u8], out: &mut Vec<u8>) {
        let mut stack: Vec<u8> = Vec::new();
        for &c in coded {
            stack.push(c);
            while let Some(x) = stack.pop() {
                if self.is_code[x as usize] {
                    let [a, b] = self.pair[x as usize];
                    stack.push(b);
                    stack.push(a);
                } else {
                    out.push(x);
                }
            }
        }
    }

    fn decode(&self, coded: &[u8]) -> String {
        let mut out: Vec<u8> = Vec::with_capacity(coded.len() * 2);
        self.expand_into(coded, &mut out);
        String::from_utf8(out).expect("clues are stored as UTF-8")
    }

    /// The clues of `answer` (normalized A–Z), in file order.
    pub fn get(&self, answer: &str) -> std::vec::IntoIter<String> {
        let list: Vec<String> = match self.find(answer) {
            Some(i) => self
                .group(i)
                .split(|&b| b == SEP)
                .filter(|c| !c.is_empty())
                .map(|c| self.decode(c))
                .collect(),
            None => Vec::new(),
        };
        list.into_iter()
    }

    /// Answers in order (by length, then letters), with their number of
    /// clues.
    pub fn answers(&self) -> impl Iterator<Item = (&str, usize)> + '_ {
        (0..self.len()).map(move |i| {
            let a = std::str::from_utf8(self.answer(i)).expect("A-Z");
            (a, self.group(i).iter().filter(|&&b| b == SEP).count())
        })
    }

    /// Number of answers.
    pub fn len(&self) -> usize {
        self.off.len().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Heap memory held by the clues (bytes).
    pub fn heap_bytes(&self) -> usize {
        self.answers.heap_bytes()
            + self.text.heap_bytes()
            + self.by_len_first.heap_bytes()
            + self.by_len_byte.heap_bytes()
            + self.off.heap_bytes()
    }

    /// Bytes read in place from a dictionary file.
    pub fn mapped_bytes(&self) -> usize {
        self.answers.mapped_bytes()
            + self.text.mapped_bytes()
            + self.by_len_first.mapped_bytes()
            + self.by_len_byte.mapped_bytes()
            + self.off.mapped_bytes()
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

/// Byte-pair codes learned on a sample of clues, and the tables to encode
/// with them (dropped after loading).
struct Bpe {
    pair: [[u8; 2]; 256],
    is_code: [bool; 256],
    /// expansion of each code (bytes, possibly part of a UTF-8 character)
    expansion: Vec<Vec<u8>>,
    /// codes sorted by the first two bytes of their expansion (every
    /// expansion has at least two), longest first: `codes[start[p]..start[p + 1]]`
    codes: Vec<u8>,
    start: Vec<u16>,
    /// coded / raw size on the sample
    ratio: f64,
}

impl Bpe {
    /// Learn codes (bytes not in `used`, nor SEP) on `sample`: repeatedly
    /// the most frequent adjacent pair (ties: the smallest) becomes a code,
    /// while some pair occurs at least twice.
    fn learn(mut sample: Vec<Vec<u8>>, used: &[bool; 256]) -> Bpe {
        let raw: usize = sample.iter().map(Vec::len).sum();
        let key = |x: u8, y: u8| (x as usize) << 8 | y as usize;
        let mut bpe = Bpe {
            pair: [[0; 2]; 256],
            is_code: [false; 256],
            expansion: vec![Vec::new(); 256],
            codes: Vec::new(),
            start: Vec::new(),
            ratio: 1.0,
        };
        let mut count = vec![0u32; 1 << 16];
        let tally = |count: &mut [u32], c: &[u8], add: bool| {
            for p in c.windows(2) {
                let k = key(p[0], p[1]);
                if add {
                    count[k] += 1;
                } else {
                    count[k] -= 1;
                }
            }
        };
        for c in &sample {
            tally(&mut count, c, true);
        }
        for code in (0..=255u8).filter(|&b| !used[b as usize] && b != SEP) {
            let n = count.iter().copied().max().unwrap_or(0);
            if n < 2 {
                break;
            }
            let best = count.iter().position(|&k| k == n).expect("max");
            let (a, b) = ((best >> 8) as u8, (best & 255) as u8);
            bpe.pair[code as usize] = [a, b];
            bpe.is_code[code as usize] = true;
            // only the clues holding the pair change: recount just those
            for c in sample.iter_mut() {
                if c.windows(2).any(|p| p[0] == a && p[1] == b) {
                    tally(&mut count, c, false);
                    merge_pair(c, a, b, code);
                    tally(&mut count, c, true);
                }
            }
        }
        let coded: usize = sample.iter().map(Vec::len).sum();
        bpe.ratio = if raw > 0 {
            coded as f64 / raw as f64
        } else {
            1.0
        };
        drop(count);
        drop(sample);
        for code in 0..=255u8 {
            if bpe.is_code[code as usize] {
                let mut e = Vec::new();
                let mut stack = vec![code];
                while let Some(x) = stack.pop() {
                    if bpe.is_code[x as usize] {
                        let [a, b] = bpe.pair[x as usize];
                        stack.push(b);
                        stack.push(a);
                    } else {
                        e.push(x);
                    }
                }
                bpe.expansion[code as usize] = e;
            }
        }
        let mut codes: Vec<u8> = (0..=255u8).filter(|&c| bpe.is_code[c as usize]).collect();
        let prefix = |c: u8| {
            let e = &bpe.expansion[c as usize];
            key(e[0], e[1])
        };
        codes.sort_by_key(|&c| {
            (
                prefix(c),
                std::cmp::Reverse(bpe.expansion[c as usize].len()),
                c,
            )
        });
        let mut start = vec![0u16; (1 << 16) + 1];
        for &c in &codes {
            start[prefix(c) + 1] += 1;
        }
        for k in 0..1 << 16 {
            start[k + 1] += start[k];
        }
        bpe.codes = codes;
        bpe.start = start;
        bpe
    }

    /// Append the coding of `raw` to `out`: at each position the code with
    /// the longest expansion that matches, else the byte itself.
    fn encode(&self, raw: &[u8], out: &mut Vec<u8>) {
        let mut r = 0;
        while r < raw.len() {
            let rest = &raw[r..];
            let hit = match rest.get(1) {
                Some(&b1) => {
                    let p = (rest[0] as usize) << 8 | b1 as usize;
                    self.codes[self.start[p] as usize..self.start[p + 1] as usize]
                        .iter()
                        .find(|&&c| rest.starts_with(&self.expansion[c as usize]))
                }
                None => None,
            };
            match hit {
                Some(&c) => {
                    out.push(c);
                    r += self.expansion[c as usize].len();
                }
                None => {
                    out.push(rest[0]);
                    r += 1;
                }
            }
        }
    }
}

/// Replace every non-overlapping `a b` (left to right) with `code`, in place.
fn merge_pair(buf: &mut Vec<u8>, a: u8, b: u8, code: u8) {
    let (mut r, mut w) = (0, 0);
    while r < buf.len() {
        if r + 1 < buf.len() && buf[r] == a && buf[r + 1] == b {
            buf[w] = code;
            r += 2;
        } else {
            buf[w] = buf[r];
            r += 1;
        }
        w += 1;
    }
    buf.truncate(w);
}

impl ClueDb {
    /// Pick a clue for `answer`, avoiding clues that contain the answer
    /// itself (for answers of 3+ letters). `pick` chooses among the usable
    /// clues (called with their count, must return an index below it).
    /// Accents typed as apostrophes are restored (see [`fix_accents`]).
    pub fn pick_clue(&self, answer: &str, pick: impl FnOnce(usize) -> usize) -> Option<String> {
        let all: Vec<String> = self.get(answer).collect();
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
    fn clues_are_stored_compressed_and_come_back_intact() {
        // 3,000 clues in the style of the dataset (repeated words), some
        // with accents, quotes and every byte of Latin-1
        let words = [
            "Il",
            "la",
            "capitale",
            "della",
            "città",
            "fiume",
            "dell'Asia",
            "è",
            "un",
            "nel",
        ];
        let mut csv = String::new();
        let mut expect: Vec<(String, Vec<String>)> = Vec::new();
        for i in 0..1_000usize {
            let answer: String = (0..3 + i % 5)
                .map(|k| (b'A' + ((i * 7 + k * 3) % 26) as u8) as char)
                .collect();
            let mut list: Vec<String> = Vec::new();
            for j in 0..3 {
                let clue: String = (0..4 + (i + j) % 4)
                    .map(|k| words[(i * 3 + j * 5 + k) % words.len()])
                    .collect::<Vec<_>>()
                    .join(" ")
                    + &format!(" {i}-{j} «ü»");
                csv.push_str(&format!("{answer},{clue}\n"));
                if !list.contains(&clue) {
                    list.push(clue);
                }
            }
            match expect.iter_mut().find(|(a, _)| *a == answer) {
                Some((_, l)) => l.extend(
                    list.into_iter()
                        .filter(|c| !l.contains(c))
                        .collect::<Vec<_>>(),
                ),
                None => expect.push((answer, list)),
            }
        }
        let db = ClueDb::from_csv_bytes(csv.as_bytes());
        let text: usize = expect
            .iter()
            .flat_map(|(_, l)| l.iter())
            .map(|c| c.len())
            .sum();
        for (a, list) in &expect {
            let got: Vec<String> = db.get(a).map(|c| c.to_string()).collect();
            assert_eq!(&got, list, "clues of {a}");
        }
        assert_eq!(db.len(), expect.len());
        assert!(
            db.heap_bytes() * 10 < text * 8,
            "{} bytes held for {} bytes of clue text",
            db.heap_bytes(),
            text
        );
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
