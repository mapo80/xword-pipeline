//! Scored wordlist (Crossword Nexus `WORD;score` format) with per-length
//! compatibility masks for bitset-based constraint propagation.
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

/// Overlay files that sit next to the wordlist, HIGHEST PRIORITY FIRST. Each
/// uses the same `WORD;SCORE` format as the main dict and is prepended to it,
/// so the first-occurrence-wins dedup in `from_str_filtered` resolves:
///
///   supplemental (hand edits, absolute — may raise OR lower)
///     > rescores-local (local; gitignored)
///     > rescores (usage-evidence floors from the xd corpus; generated)
///     > main dict (upstream snapshot, never edited)
///
/// The blocklist still kills a word regardless of any overlay. See
/// data/README.md for the full layering story.
const OVERLAYS: [&str; 3] = ["supplemental.txt", "rescores-local.txt", "rescores.txt"];

/// Raw text of an overlay file next to the wordlist (same `WORD;SCORE` format
/// as the main dict). Missing file → empty string.
fn overlay_beside(wordlist_path: &str, name: &str) -> String {
    let p = match Path::new(wordlist_path).parent() {
        Some(d) => d.join(name),
        None => Path::new(name).to_path_buf(),
    };
    fs::read_to_string(&p).unwrap_or_default()
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
        &self.compat[pos * 26 + c as usize]
    }

    #[inline]
    pub fn pos_count(&self, pos: usize, c: u8) -> u32 {
        self.pos_count[pos * 26 + c as usize]
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
}

impl Wordlist {
    /// Load the wordlist, automatically applying:
    ///   * a `blocklist.txt` next to the wordlist file (excludes regardless
    ///     of score — so a mis-scored junk word can't slip into a fill), and
    ///   * the `OVERLAYS` next to the wordlist (`supplemental.txt`,
    ///     `rescores-local.txt`, `rescores.txt`) — each adds entries and
    ///     overrides scores of anything below it in the layering.
    ///
    /// Blocklist still wins if a word appears in any overlay and the
    /// blocklist.
    pub fn load(path: &str, min_score: u8) -> std::io::Result<Wordlist> {
        let text = fs::read_to_string(path)?;
        let blocklist = blocklist_beside(path);
        if !blocklist.is_empty() {
            eprintln!("blocklist: excluding {} word(s)", blocklist.len());
        }
        // Overlays are PREPENDED highest-priority-first so the
        // first-occurrence-wins dedup in from_str_filtered gives each layer
        // priority over everything below it (see OVERLAYS).
        let mut parts: Vec<String> = Vec::new();
        for name in OVERLAYS {
            let overlay = overlay_beside(path, name);
            if overlay.is_empty() {
                continue;
            }
            let n_entries = overlay
                .lines()
                .filter(|l| {
                    let t = l.trim();
                    !t.is_empty() && !t.starts_with('#')
                })
                .count();
            eprintln!(
                "{}: {} entries loaded",
                name.trim_end_matches(".txt"),
                n_entries
            );
            parts.push(overlay);
        }
        parts.push(text);
        Ok(Self::from_str_filtered(
            &parts.join("\n"),
            min_score,
            &blocklist,
        ))
    }

    pub fn from_str(text: &str, min_score: u8) -> Wordlist {
        Self::from_str_filtered(text, min_score, &HashSet::new())
    }

    pub fn from_str_filtered(
        text: &str,
        min_score: u8,
        blocklist: &HashSet<Box<[u8]>>,
    ) -> Wordlist {
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
            // First occurrence RESOLVES the word's score (overlays are
            // prepended highest-priority-first), so register it in `seen`
            // before the min_score cut: a high-priority entry below the
            // cutoff must EXCLUDE the word, not defer to a lower layer
            // (e.g. a hand-lowered supplemental score suppresses a rescore
            // floor rather than being resurrected by it).
            if seen.contains(&boxed) {
                continue;
            }
            seen.insert(boxed.clone());
            if score < min_score {
                continue;
            }
            buckets[len].push((boxed, score));
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
            let n_masks = if len >= MIN_LEN { len * 26 } else { 0 };
            let mut compat: Vec<Bitset> = (0..n_masks).map(|_| Bitset::zeros(n.max(1))).collect();
            for (i, w) in letters.iter().enumerate() {
                for (p, &c) in w.iter().enumerate() {
                    compat[p * 26 + c as usize].set(i);
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

        Wordlist { by_len }
    }

    #[inline]
    pub fn len_data(&self, len: usize) -> &LenData {
        &self.by_len[len]
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
    fn tier_cutoff_works() {
        let wl = tiny();
        let l5 = wl.len_data(5);
        // length-5 words: ABBEY(90), EDGAR(80). (TOSSED is length 6.)
        assert_eq!(l5.tier_cutoff(85), 1); // only ABBEY >= 85
        assert_eq!(l5.tier_cutoff(80), 2); // ABBEY, EDGAR
        assert_eq!(l5.tier_cutoff(40), 2); // both
        assert_eq!(l5.tier_cutoff(95), 0);
    }

    /// Supplemental entries (modeled as text prepended to the main dict) are
    /// included in the loaded wordlist with their declared scores.
    #[test]
    fn supplemental_adds_new_entries() {
        // Supplemental adds WORDLE (length 6) that main doesn't have.
        let supplemental = "WORDLE;75\n";
        let main = "ABBEY;90\nCAT;75\n";
        let combined = format!("{}\n{}", supplemental, main);
        let wl = Wordlist::from_str(&combined, 40);
        let l6: Vec<String> = (0..wl.len_data(6).n)
            .map(|i| wl.len_data(6).word_string(i))
            .collect();
        assert!(
            l6.contains(&"WORDLE".to_string()),
            "supplemental WORDLE should be loaded"
        );
    }

    /// When both supplemental and main declare the same word with different
    /// scores, supplemental's score wins (it's prepended → seen first).
    #[test]
    fn supplemental_score_overrides_main() {
        // Both files declare CAT. Supplemental's score 95 should be the one kept.
        let supplemental = "CAT;95\n";
        let main = "CAT;75\nDOG;75\n";
        let combined = format!("{}\n{}", supplemental, main);
        let wl = Wordlist::from_str(&combined, 40);
        let l3 = wl.len_data(3);
        let i_cat = l3
            .index_of(&normalize_letters("cat"))
            .expect("CAT must be present");
        assert_eq!(
            l3.scores[i_cat], 95,
            "supplemental's CAT;95 must win over main's CAT;75"
        );
    }

    /// Blocklist still excludes entries even when supplemental tries to add
    /// them — blocklist is the strongest signal.
    #[test]
    fn blocklist_overrides_supplemental() {
        let supplemental = "BANNED;90\n";
        let main = "CAT;75\n";
        let combined = format!("{}\n{}", supplemental, main);
        let mut block: HashSet<Box<[u8]>> = HashSet::new();
        block.insert(normalize_letters("banned").into_boxed_slice());
        let wl = Wordlist::from_str_filtered(&combined, 40, &block);
        let l6: Vec<String> = (0..wl.len_data(6).n)
            .map(|i| wl.len_data(6).word_string(i))
            .collect();
        assert!(
            !l6.contains(&"BANNED".to_string()),
            "blocklist must exclude even supplemental entries"
        );
    }

    /// End-to-end `load()` layering through real beside-files:
    /// supplemental > rescores-local > rescores > dict, blocklist supreme,
    /// min_score applied to the RESOLVED score (so a floor makes a word
    /// loadable, and a hand-lowered supplemental score can exclude one).
    #[test]
    fn load_applies_overlay_layering() {
        let dir = std::env::temp_dir().join(format!("xfill-overlays-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let dict = dir.join("dict.txt");
        std::fs::write(&dict, "ISSA;44\nETUI;30\nOREO;90\nJUNK;80\n").unwrap();
        std::fs::write(dir.join("rescores.txt"), "# floors\nISSA;60\nETUI;65\n").unwrap();
        std::fs::write(dir.join("rescores-local.txt"), "ISSA;62\n").unwrap();
        std::fs::write(dir.join("supplemental.txt"), "ETUI;40\n").unwrap();
        std::fs::write(dir.join("blocklist.txt"), "JUNK\n").unwrap();

        let wl = Wordlist::load(dict.to_str().unwrap(), 50).unwrap();
        std::fs::remove_dir_all(&dir).ok();

        let l4 = wl.len_data(4);
        let words: Vec<String> = (0..l4.n).map(|i| l4.word_string(i)).collect();
        assert!(words.contains(&"OREO".to_string()), "dict word kept");
        assert!(
            words.contains(&"ISSA".to_string()),
            "rescore floor lifts ISSA above the min_score cutoff"
        );
        assert!(
            !words.contains(&"ETUI".to_string()),
            "hand-lowered supplemental score (40) beats the rescore floor (65)"
        );
        assert!(!words.contains(&"JUNK".to_string()), "blocklist supreme");
        let issa = l4.index_of(&normalize_letters("ISSA")).unwrap();
        assert_eq!(
            l4.scores[issa], 62,
            "rescores-local (62) outranks rescores (60) and dict (44)"
        );
    }
}
