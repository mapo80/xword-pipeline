//! Word tools on the dictionary.
//!
//! Usage:
//!   parole pattern PATTERN [--wordlist PATH] [--limit N] [--clues]
//!   parole anagram LETTERS [--words N] [--wordlist PATH] [--limit N]
//!
//! Pattern: `?` any letter, `*` any run, `@` vowel, `#` consonant,
//! `[ABC]` one of, `[^ABC]` none of. Anagram letters may include `?`.

use xfill_core::clues::load_clue_csv;
use xfill_core::words::{anagrams, find_pattern};

fn arg<T: std::str::FromStr>(name: &str, default: T) -> T {
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        if a == name {
            if let Some(v) = it.next() {
                if let Ok(p) = v.parse() {
                    return p;
                }
            }
        }
    }
    default
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(cmd), Some(what)) = (args.first(), args.get(1)) else {
        eprintln!(
            "usage: parole pattern PATTERN | anagram LETTERS [--words N] [--clues] [--limit N]"
        );
        std::process::exit(2);
    };
    if cmd != "pattern" && cmd != "anagram" {
        eprintln!("error: unknown command '{cmd}' (use pattern or anagram)");
        std::process::exit(2);
    }
    let wordlist: String = arg("--wordlist", "../../dataset/Db.csv".to_string());
    let limit: usize = arg("--limit", 50);
    let show_clues = args.iter().any(|a| a == "--clues");
    let (wl, clues) = match load_clue_csv(&wordlist, 0) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("error: cannot load wordlist {wordlist}: {e}");
            std::process::exit(1);
        }
    };
    if cmd == "pattern" {
        let found = match find_pattern(&wl, what) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("error: {e}");
                std::process::exit(2);
            }
        };
        for (w, score) in found.iter().take(limit) {
            match clues.get(w).first().filter(|_| show_clues) {
                Some(c) => println!("{w:<16} {score:>3}  {c}"),
                None => println!("{w:<16} {score:>3}"),
            }
        }
        eprintln!("{} word(s)", found.len());
    } else {
        let words: usize = arg("--words", 1);
        let found = anagrams(&wl, what, words, limit);
        for a in &found {
            println!("{}", a.join(" "));
        }
        eprintln!("{} anagram(s)", found.len());
    }
}
