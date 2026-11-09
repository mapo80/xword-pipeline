//! Word tools on a word list: pattern search and anagrams.

use crate::wordlist::{Wordlist, REBUS_BASE};

const VOWELS: u32 = 1 << 0 | 1 << 4 | 1 << 8 | 1 << 14 | 1 << 20; // A E I O U
const ALL: u32 = (1 << 26) - 1;

#[derive(Debug, Clone, Copy)]
enum Tok {
    /// one letter from the set (bit c = letter c)
    One(u32),
    /// any run of letters, possibly empty
    Star,
}

/// `?` any letter, `*` any run, `@` vowel, `#` consonant, `[ABC]` one of,
/// `[^ABC]` none of, letters literal (case-insensitive).
fn parse_pattern(pattern: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        out.push(match c {
            '?' => Tok::One(ALL),
            '*' => Tok::Star,
            '@' => Tok::One(VOWELS),
            '#' => Tok::One(ALL & !VOWELS),
            '[' => {
                let negate = chars.peek() == Some(&'^');
                if negate {
                    chars.next();
                }
                let mut set = 0u32;
                let mut closed = false;
                for d in chars.by_ref() {
                    if d == ']' {
                        closed = true;
                        break;
                    }
                    let u = d.to_ascii_uppercase();
                    if u.is_ascii_uppercase() {
                        set |= 1 << (u as u8 - b'A');
                    }
                }
                if !closed {
                    return Err(format!("unclosed [ in pattern '{pattern}'"));
                }
                Tok::One(if negate { ALL & !set } else { set })
            }
            c if c.is_ascii_alphabetic() => Tok::One(1 << (c.to_ascii_uppercase() as u8 - b'A')),
            other => return Err(format!("bad character '{other}' in pattern")),
        });
    }
    Ok(out)
}

fn matches(toks: &[Tok], word: &[u8]) -> bool {
    match toks.first() {
        None => word.is_empty(),
        Some(Tok::One(set)) => {
            !word.is_empty() && set & (1 << word[0]) != 0 && matches(&toks[1..], &word[1..])
        }
        Some(Tok::Star) => (0..=word.len()).any(|k| matches(&toks[1..], &word[k..])),
    }
}

/// Words matching `pattern`, best score first (then alphabetical).
pub fn find_pattern(wl: &Wordlist, pattern: &str) -> Result<Vec<(String, u8)>, String> {
    let toks = parse_pattern(pattern)?;
    let fixed = !toks.iter().any(|t| matches!(t, Tok::Star));
    let mut out = Vec::new();
    for (len, ld) in wl.by_len.iter().enumerate() {
        if fixed && len != toks.len() {
            continue;
        }
        for (i, w) in ld.flat.chunks_exact(ld.len.max(1)).take(ld.n).enumerate() {
            if w.iter().all(|&c| c < REBUS_BASE) && matches(&toks, w) {
                out.push((wl.word(len, i), ld.scores[i]));
            }
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Ok(out)
}

struct Anagrams<'a> {
    cands: &'a [(&'a [u8], String, u8)],
    max_words: usize,
    cap: usize,
    stack: Vec<usize>,
    results: Vec<(u32, Vec<String>)>,
}

impl Anagrams<'_> {
    fn dfs(&mut self, start: usize, counts: &mut [i32; 26], wild: i32, left: i32) {
        if left == 0 {
            let score = self.stack.iter().map(|&k| self.cands[k].2 as u32).sum();
            let mut words: Vec<String> = self
                .stack
                .iter()
                .map(|&k| self.cands[k].1.clone())
                .collect();
            words.sort(); // stable presentation, whatever the search order
            self.results.push((score, words));
            return;
        }
        if self.stack.len() == self.max_words || self.results.len() >= self.cap {
            return;
        }
        for k in start..self.cands.len() {
            let w = self.cands[k].0;
            if w.len() as i32 > left {
                continue;
            }
            // take the letters; missing ones come from the wildcards
            let mut used_wild = 0;
            for &c in w {
                counts[c as usize] -= 1;
                if counts[c as usize] < 0 {
                    used_wild += 1;
                }
            }
            if used_wild <= wild {
                self.stack.push(k);
                self.dfs(k, counts, wild - used_wild, left - w.len() as i32);
                self.stack.pop();
            }
            for &c in w {
                counts[c as usize] += 1;
            }
            if self.results.len() >= self.cap {
                return;
            }
        }
    }
}

/// Anagrams of `letters` (`?` = any letter) as one word or up to
/// `max_words` words using every letter exactly once; at most `limit`
/// results, best total score first.
pub fn anagrams(wl: &Wordlist, letters: &str, max_words: usize, limit: usize) -> Vec<Vec<String>> {
    let mut counts = [0i32; 26];
    let mut wild = 0i32;
    for c in letters.chars() {
        let u = c.to_ascii_uppercase();
        if u == '?' {
            wild += 1;
        } else if u.is_ascii_uppercase() {
            counts[(u as u8 - b'A') as usize] += 1;
        }
    }
    let total: i32 = counts.iter().sum::<i32>() + wild;
    // candidate words that fit in the available letters
    let mut cands: Vec<(&[u8], String, u8)> = Vec::new();
    for (len, ld) in wl.by_len.iter().enumerate() {
        if len as i32 > total {
            continue;
        }
        for (i, w) in ld.flat.chunks_exact(ld.len.max(1)).take(ld.n).enumerate() {
            if w.iter().any(|&c| c >= REBUS_BASE) {
                continue;
            }
            let mut need = [0i32; 26];
            for &c in w.iter() {
                need[c as usize] += 1;
            }
            let extra: i32 = (0..26).map(|c| (need[c] - counts[c]).max(0)).sum();
            if extra <= wild {
                cands.push((w, wl.word(len, i), ld.scores[i]));
            }
        }
    }
    let mut search = Anagrams {
        cands: &cands,
        max_words: max_words.max(1),
        cap: limit.saturating_mul(4).max(limit),
        stack: Vec::new(),
        results: Vec::new(),
    };
    // wildcards used show up as negative counts while a branch is open
    search.dfs(0, &mut counts, wild, total);
    let mut results = search.results;
    results.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    results.truncate(limit);
    results.into_iter().map(|(_, w)| w).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wordlist::Wordlist;

    fn wl() -> Wordlist {
        Wordlist::from_str(
            "CASA;80\nCOSA;70\nCASALE;60\nMASA;50\nROMA;90\nAMOR;60\nMORA;65\nORMA;55\nRAMO;75\nROSA;85\nAIA;40\nEIA;40\nBATA;30\n",
            0,
        )
    }

    fn words(v: &[(String, u8)]) -> Vec<&str> {
        let mut w: Vec<&str> = v.iter().map(|(s, _)| s.as_str()).collect();
        w.sort();
        w
    }

    #[test]
    fn pattern_syntax() {
        let wl = wl();
        let m = |p: &str| words(&find_pattern(&wl, p).unwrap()).join(",");
        assert_eq!(m("C?SA"), "CASA,COSA");
        assert_eq!(m("ca*"), "CASA,CASALE", "case-insensitive, * any run");
        assert_eq!(m("*SA"), "CASA,COSA,MASA,ROSA");
        assert_eq!(m("[CM]ASA"), "CASA,MASA");
        assert_eq!(m("[^C]ASA"), "MASA");
        assert_eq!(m("@#@"), "", "no V-C-V word here");
        assert_eq!(m("#@#@"), "BATA,CASA,COSA,MASA,MORA,RAMO,ROMA,ROSA");
        assert_eq!(m("@@@"), "AIA,EIA", "all vowels");
        assert!(find_pattern(&wl, "[AB").is_err(), "unclosed class");
        let best = find_pattern(&wl, "R???").unwrap();
        assert_eq!(best[0].0, "ROMA", "best score first");
    }

    #[test]
    fn anagrams_single_and_multi_word() {
        let wl = wl();
        let one = anagrams(&wl, "roma", 1, 100);
        let set: Vec<String> = one.iter().map(|v| v.join(" ")).collect();
        for w in ["AMOR", "MORA", "ORMA", "RAMO", "ROMA"] {
            assert!(set.contains(&w.to_string()), "{set:?}");
        }
        assert_eq!(set.len(), 5);
        let two = anagrams(&wl, "CASAROMA", 2, 100);
        assert!(two
            .iter()
            .any(|v| v == &vec!["CASA".to_string(), "ROMA".to_string()]));
        assert!(two.iter().all(|v| v.len() == 2), "no single 8-letter word");
        let wild = anagrams(&wl, "RO?A", 1, 100);
        let ws: Vec<String> = wild.iter().map(|v| v.join("")).collect();
        assert!(ws.contains(&"ROSA".to_string()) && ws.contains(&"ROMA".to_string()));
    }
}
