//! xfill-core: quality-aware crossword fill engine (Italian fork).
//!
//! - [`bitset`]: fixed-size bitset (constraint-propagation primitive)
//! - [`wordlist`]: scored wordlist + compatibility masks
//! - [`clues`]: `WORD,clue` CSV loader (wordlist + clue database)
//! - [`grid`]: 2D template → geometry-agnostic [`grid::Puzzle`]
//! - [`gen_it`]: Italian-style block-pattern generator
//! - [`solver`]: DFS solver (MRV + score order + restarts)
//! - [`library`]: keeper selection + JSON artifact

pub mod bench;
pub mod bitset;
pub mod clues;
pub mod gen_it;
pub mod grid;
pub mod library;
pub mod pdf;
pub mod solver;
pub mod util;
pub mod wordlist;

pub use grid::Puzzle;
pub use solver::{solve_portfolio, Analysis, Lock, SolveConfig, SolveResult, Solver};
pub use wordlist::Wordlist;
