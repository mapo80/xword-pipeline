//! Compile a 2D template into a geometry-agnostic `Puzzle`.
//!
//! Template chars: `#` block, `.` empty white, `A-Z` pre-placed letter (themers
//! / rebus seeds). Rows separated by newlines; spaces within a row ignored.
//!
//! Following crossword-composer's insight, the solver operates only on the
//! incidence structure (entries ↔ cells); 2D geometry is kept solely for I/O.

use crate::wordlist::MIN_LEN;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Across,
    Down,
}

pub struct Entry {
    pub cells: Vec<usize>,
    pub len: usize,
    pub dir: Dir,
    pub row: usize,
    pub col: usize,
}

pub struct Puzzle {
    pub rows: usize,
    pub cols: usize,
    pub n_cells: usize,
    pub entries: Vec<Entry>,
    /// cell -> [(entry_id, position within that entry)]
    pub cell_entries: Vec<Vec<(usize, usize)>>,
    /// cell -> Some(letter 0..25) if pre-placed
    pub prefilled: Vec<Option<u8>>,
    pub cell_rc: Vec<(usize, usize)>,
}

impl Puzzle {
    pub fn from_template(text: &str) -> Puzzle {
        let rows_raw: Vec<Vec<char>> = text
            .lines()
            .map(|l| l.trim().replace(' ', ""))
            .filter(|l| !l.is_empty())
            .map(|l| l.chars().collect())
            .collect();
        let rows = rows_raw.len();
        let cols = rows_raw[0].len();
        assert!(rows_raw.iter().all(|r| r.len() == cols), "ragged template");

        let is_block = |r: usize, c: usize| rows_raw[r][c] == '#';

        // Assign cell ids to white cells.
        let mut cell_id = vec![vec![usize::MAX; cols]; rows];
        let mut cell_rc = Vec::new();
        let mut prefilled = Vec::new();
        for r in 0..rows {
            for c in 0..cols {
                if !is_block(r, c) {
                    let id = cell_rc.len();
                    cell_id[r][c] = id;
                    cell_rc.push((r, c));
                    let ch = rows_raw[r][c];
                    prefilled.push(if ch.is_ascii_uppercase() {
                        Some(ch as u8 - b'A')
                    } else {
                        None
                    });
                }
            }
        }
        let n_cells = cell_rc.len();
        let mut entries: Vec<Entry> = Vec::new();

        // Across runs
        for (r, row_cells) in cell_id.iter().enumerate() {
            let mut c = 0;
            while c < cols {
                if is_block(r, c) {
                    c += 1;
                    continue;
                }
                let start = c;
                let mut cells = Vec::new();
                while c < cols && !is_block(r, c) {
                    cells.push(row_cells[c]);
                    c += 1;
                }
                if cells.len() >= MIN_LEN {
                    entries.push(Entry {
                        len: cells.len(),
                        cells,
                        dir: Dir::Across,
                        row: r,
                        col: start,
                    });
                }
            }
        }
        // Down runs — column-major scan over a row-major grid; an enumerate()
        // rewrite would require a per-cell transpose, so the range loop is
        // genuinely clearer here.
        #[allow(clippy::needless_range_loop)]
        for c in 0..cols {
            let mut r = 0;
            while r < rows {
                if is_block(r, c) {
                    r += 1;
                    continue;
                }
                let start = r;
                let mut cells = Vec::new();
                while r < rows && !is_block(r, c) {
                    cells.push(cell_id[r][c]);
                    r += 1;
                }
                if cells.len() >= MIN_LEN {
                    entries.push(Entry {
                        len: cells.len(),
                        cells,
                        dir: Dir::Down,
                        row: start,
                        col: c,
                    });
                }
            }
        }

        let mut cell_entries: Vec<Vec<(usize, usize)>> = vec![Vec::new(); n_cells];
        for (ei, e) in entries.iter().enumerate() {
            for (pos, &cid) in e.cells.iter().enumerate() {
                cell_entries[cid].push((ei, pos));
            }
        }

        Puzzle {
            rows,
            cols,
            n_cells,
            entries,
            cell_entries,
            prefilled,
            cell_rc,
        }
    }

    pub fn block_count(&self) -> usize {
        self.rows * self.cols - self.n_cells
    }

    /// Find the entry starting at (row, col) going in `dir`, if any.
    pub fn find_entry(&self, row: usize, col: usize, dir: Dir) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.dir == dir && e.row == row && e.col == col)
    }

    /// Standard crossword clue numbers, one per entry (entries sharing a start
    /// cell — an across and a down — get the same number). Numbered in reading
    /// order, which matches cell-id order since cells are assigned row-major.
    pub fn number_entries(&self) -> Vec<u32> {
        use std::collections::HashMap;
        let mut starts: HashMap<usize, Vec<usize>> = HashMap::new();
        for (ei, e) in self.entries.iter().enumerate() {
            starts.entry(e.cells[0]).or_default().push(ei);
        }
        let mut numbers = vec![0u32; self.entries.len()];
        let mut num = 0u32;
        for cid in 0..self.n_cells {
            if let Some(eis) = starts.get(&cid) {
                num += 1;
                for &ei in eis {
                    numbers[ei] = num;
                }
            }
        }
        numbers
    }

    /// Cells that belong to no entry (unfillable) — should be empty for valid grids.
    pub fn orphan_cells(&self) -> usize {
        self.cell_entries.iter().filter(|v| v.is_empty()).count()
    }

    /// Render with an optional assignment (cell -> letter 0..25).
    pub fn render(&self, letters: Option<&[Option<u8>]>) -> String {
        let mut out = vec![vec!['#'; self.cols]; self.rows];
        for cid in 0..self.n_cells {
            let (r, c) = self.cell_rc[cid];
            let ch = match letters.and_then(|l| l[cid]) {
                Some(b) => (b'A' + b) as char,
                None => match self.prefilled[cid] {
                    Some(b) => (b'A' + b) as char,
                    None => '.',
                },
            };
            out[r][c] = ch;
        }
        out.iter()
            .map(|row| row.iter().collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Char content per cell: a letter, ' ' for an unfilled white cell, or
    /// '\0' to signal a black square.
    fn cell_chars(&self, letters: Option<&[Option<u8>]>) -> Vec<Vec<char>> {
        let mut grid = vec![vec!['\0'; self.cols]; self.rows];
        for cid in 0..self.n_cells {
            let (r, c) = self.cell_rc[cid];
            grid[r][c] = match letters.and_then(|l| l[cid]) {
                Some(b) => (b'A' + b) as char,
                None => match self.prefilled[cid] {
                    Some(b) => (b'A' + b) as char,
                    None => ' ',
                },
            };
        }
        grid
    }

    /// Box-drawn grid with cell borders. Black squares render as a solid block.
    /// Much easier to read than the bare letter dump.
    pub fn render_boxed(&self, letters: Option<&[Option<u8>]>) -> String {
        let grid = self.cell_chars(letters);
        let cols = self.cols;
        let mut s = String::new();

        let rule = |s: &mut String, left: char, mid: char, right: char| {
            s.push(left);
            for c in 0..cols {
                s.push('─');
                s.push('─');
                s.push('─');
                s.push(if c + 1 < cols { mid } else { right });
            }
            s.push('\n');
        };

        rule(&mut s, '┌', '┬', '┐');
        for (r, row) in grid.iter().enumerate() {
            s.push('│');
            for &ch in row {
                if ch == '\0' {
                    s.push_str("███"); // black square
                } else {
                    s.push(' ');
                    s.push(ch);
                    s.push(' ');
                }
                s.push('│');
            }
            s.push('\n');
            if r + 1 < self.rows {
                rule(&mut s, '├', '┼', '┤');
            } else {
                rule(&mut s, '└', '┴', '┘');
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_5x5_open() {
        let p = Puzzle::from_template(".....\n.....\n.....\n.....\n.....\n");
        assert_eq!(p.n_cells, 25);
        assert_eq!(p.entries.len(), 10); // 5 across + 5 down
        assert_eq!(p.block_count(), 0);
        assert_eq!(p.orphan_cells(), 0);
    }

    #[test]
    fn handles_blocks_and_min_run() {
        // MIN_LEN is 2 (Italian): a 2-cell run IS an entry, a 1-cell run is not.
        let p = Puzzle::from_template("..#..\n.#...\n.....\n.....\n.....\n");
        let across = |row: usize| {
            p.entries
                .iter()
                .filter(|e| e.dir == Dir::Across && e.row == row)
                .count()
        };
        assert_eq!(across(0), 2, "row 0: runs of 2 and 2");
        assert_eq!(across(1), 1, "row 1: run of 1 (unclued) + run of 3");
        assert_eq!(
            p.orphan_cells(),
            0,
            "r1c0 is still covered by its down entry"
        );
    }

    #[test]
    fn prefilled_letters() {
        let p = Puzzle::from_template("CAT..\n.....\n.....\n.....\n.....\n");
        // first three cells prefilled C,A,T
        assert_eq!(p.prefilled[0], Some(2)); // C
        assert_eq!(p.prefilled[1], Some(0)); // A
        assert_eq!(p.prefilled[2], Some(19)); // T
        assert_eq!(p.prefilled[3], None);
    }

    #[test]
    fn crossings_recorded() {
        let p = Puzzle::from_template(".....\n.....\n.....\n.....\n.....\n");
        // top-left cell (0) is in one across and one down entry
        assert_eq!(p.cell_entries[0].len(), 2);
    }

    #[test]
    fn clue_numbering() {
        let p = Puzzle::from_template("...\n...\n...\n");
        let nums = p.number_entries();
        // entries: [acrossR0, acrossR1, acrossR2, downC0, downC1, downC2]
        assert_eq!(nums[0], 1); // across row 0 starts at cell 0
        assert_eq!(nums[3], 1); // down col 0 also starts at cell 0 → same number
        assert_eq!(nums[4], 2); // down col 1 → number 2
        assert_eq!(nums[1], 4); // across row 1 starts at cell 3 → number 4
    }

    #[test]
    fn boxed_render_has_borders_and_blocks() {
        let p = Puzzle::from_template("CAT\n#..\n...\n");
        let s = p.render_boxed(None);
        assert!(s.contains('┌') && s.contains('┘') && s.contains('┼'));
        assert!(s.contains('█'), "block square rendered as solid");
        assert!(s.contains(" C ") && s.contains(" A ") && s.contains(" T "));
        // every line should be the same display width
        let widths: std::collections::HashSet<usize> =
            s.lines().map(|l| l.chars().count()).collect();
        assert_eq!(widths.len(), 1, "all rows equal width");
    }
}
