//! Print-ready A4 PDF of a clued crossword: title + empty numbered grid and
//! the clues (ORIZZONTALI / VERTICALI, two-column flow) on page 1 onward, the
//! solution grid on the last page.
//!
//! A minimal hand-written PDF (rectangles + text with the built-in Helvetica
//! fonts, WinAnsiEncoding so Italian accents print) — no dependencies. The
//! approach and the Helvetica width table come from the upstream
//! xword-pipeline `clue-writer/src/pdf.ts` (MIT).

use std::io::Write;

const PAGE_W: f64 = 595.28;
const PAGE_H: f64 = 841.89;
const MARGIN: f64 = 42.0;
const COL_GAP: f64 = 14.0;

/// Helvetica advance widths (1/1000 em) for ASCII 32..126.
#[rustfmt::skip]
const HELV_W: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

pub struct PdfClue {
    pub num: u32,
    pub text: String,
}

pub struct PdfPuzzle {
    pub title: String,
    pub subtitle: String,
    /// Rows of the grid; `#` = black square. Letters are used for the solution.
    pub fill: Vec<String>,
    /// (row, col, number) of every numbered cell.
    pub numbers: Vec<(usize, usize, u32)>,
    pub across: Vec<PdfClue>,
    pub down: Vec<PdfClue>,
}

/// Map a char to its WinAnsi (cp1252) byte; unknown → '?'.
fn winansi(c: char) -> u8 {
    match c as u32 {
        0x20..=0x7E | 0xA0..=0xFF => c as u32 as u8,
        _ => match c {
            '€' => 0x80,
            '‚' => 0x82,
            '„' => 0x84,
            '…' => 0x85,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '•' => 0x95,
            '–' => 0x96,
            '—' => 0x97,
            '\t' => b' ',
            _ => b'?',
        },
    }
}

fn char_width(c: char) -> f64 {
    let base = match c {
        'à' | 'á' | 'â' | 'ä' => 'a',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' => 'i',
        'ò' | 'ó' | 'ô' | 'ö' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'À' | 'Á' | 'Â' | 'Ä' => 'A',
        'È' | 'É' | 'Ê' | 'Ë' => 'E',
        'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
        'Ò' | 'Ó' | 'Ô' | 'Ö' => 'O',
        'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
        'ñ' => 'n',
        'ç' => 'c',
        '’' | '‘' => '\'',
        '“' | '”' => '"',
        '–' => '-',
        c => c,
    };
    let code = base as u32;
    let w = if (32..=126).contains(&code) {
        HELV_W[(code - 32) as usize]
    } else {
        556
    };
    w as f64 / 1000.0
}

fn measure(s: &str, size: f64) -> f64 {
    s.chars().map(char_width).sum::<f64>() * size
}

/// Greedy word wrap; a single word wider than the column is hard-broken.
fn wrap(text: &str, size: f64, max_w: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let trial = if line.is_empty() {
            word.to_string()
        } else {
            format!("{line} {word}")
        };
        if measure(&trial, size) <= max_w {
            line = trial;
            continue;
        }
        if !line.is_empty() {
            lines.push(std::mem::take(&mut line));
        }
        if measure(word, size) <= max_w {
            line = word.to_string();
        } else {
            let mut chunk = String::new();
            for ch in word.chars() {
                chunk.push(ch);
                if measure(&chunk, size) > max_w {
                    chunk.pop();
                    lines.push(std::mem::take(&mut chunk));
                    chunk.push(ch);
                }
            }
            line = chunk;
        }
    }
    if !line.is_empty() || lines.is_empty() {
        lines.push(line);
    }
    lines
}

#[derive(Default)]
struct Page {
    ops: Vec<u8>,
}

impl Page {
    /// Text with its baseline at `base_top` points from the top of the page.
    fn text(&mut self, x: f64, base_top: f64, size: f64, bold: bool, s: &str) {
        let font = if bold { "F2" } else { "F1" };
        let _ = write!(
            self.ops,
            "BT /{font} {size:.2} Tf {x:.2} {:.2} Td (",
            PAGE_H - base_top
        );
        for c in s.chars() {
            let b = winansi(c);
            if matches!(b, b'(' | b')' | b'\\') {
                self.ops.push(b'\\');
            }
            self.ops.push(b);
        }
        self.ops.extend_from_slice(b") Tj ET\n");
    }

    fn text_centered(&mut self, cx: f64, base_top: f64, size: f64, bold: bool, s: &str) {
        self.text(cx - measure(s, size) / 2.0, base_top, size, bold, s);
    }

    fn rect_fill(&mut self, x: f64, top: f64, w: f64, h: f64) {
        let _ = writeln!(
            self.ops,
            "0 g {x:.2} {:.2} {w:.2} {h:.2} re f",
            PAGE_H - top - h
        );
    }

    fn rect_stroke(&mut self, x: f64, top: f64, w: f64, h: f64, lw: f64) {
        let _ = writeln!(
            self.ops,
            "{lw:.2} w 0 G {x:.2} {:.2} {w:.2} {h:.2} re S",
            PAGE_H - top - h
        );
    }
}

fn draw_grid(page: &mut Page, pz: &PdfPuzzle, x0: f64, y0: f64, cell: f64, letters: bool) {
    let grid: Vec<Vec<String>> = pz.fill.iter().map(|r| crate::grid::row_cells(r)).collect();
    let rows = grid.len();
    let cols = grid.first().map_or(0, |r| r.len());
    let num_at = |r: usize, c: usize| {
        pz.numbers
            .iter()
            .find(|&&(nr, nc, _)| nr == r && nc == c)
            .map(|&(_, _, n)| n)
    };
    for (r, row) in grid.iter().enumerate() {
        for (c, txt) in row.iter().enumerate() {
            let x = x0 + c as f64 * cell;
            let y = y0 + r as f64 * cell;
            if txt == "#" {
                page.rect_fill(x, y, cell, cell);
                continue;
            }
            page.rect_stroke(x, y, cell, cell, 0.6);
            if let Some(n) = num_at(r, c) {
                page.text(x + 1.5, y + cell * 0.3, cell * 0.27, false, &n.to_string());
            }
            // a letter, or a rebus string (shrunk to fit the square)
            let shown = txt.trim_start_matches('[').trim_end_matches(']');
            if letters && shown.chars().all(char::is_alphabetic) && !shown.is_empty() {
                let size = cell * 0.55 / (shown.chars().count() as f64).sqrt();
                page.text_centered(x + cell / 2.0, y + cell * 0.8, size, true, shown);
            }
        }
    }
    page.rect_stroke(x0, y0, cols as f64 * cell, rows as f64 * cell, 1.6);
}

/// Clue-flow parameters for one layout attempt.
#[derive(Clone, Copy)]
struct Flow {
    n_cols: usize,
    size: f64,
}

impl Flow {
    fn line_h(&self) -> f64 {
        self.size * 1.22
    }
    fn head_size(&self) -> f64 {
        self.size + 1.5
    }
    fn head_h(&self) -> f64 {
        self.head_size() * 1.7
    }
    fn indent(&self) -> f64 {
        self.size * 2.0
    }
    fn col_w(&self) -> f64 {
        let avail_w = PAGE_W - 2.0 * MARGIN;
        (avail_w - (self.n_cols - 1) as f64 * COL_GAP) / self.n_cols as f64
    }
}

/// Flow both clue lists through `flow.n_cols` columns, starting at
/// `first_top` on the first page and continuing at the top of new pages.
/// Returns the pages holding the clue text (page 0 = the grid page).
fn flow_clues(pz: &PdfPuzzle, flow: Flow, first_top: f64) -> Vec<Page> {
    let bottom = PAGE_H - MARGIN;
    let col_w = flow.col_w();
    let col_x = |c: usize| MARGIN + c as f64 * (col_w + COL_GAP);
    let mut pages = vec![Page::default()];
    let (mut col, mut y) = (0usize, first_top);
    let top_of = |pages: &Vec<Page>| if pages.len() == 1 { first_top } else { MARGIN };
    let next_column = |pages: &mut Vec<Page>, col: &mut usize, y: &mut f64| {
        *col += 1;
        if *col == flow.n_cols {
            *col = 0;
            pages.push(Page::default());
        }
        *y = top_of(pages);
    };

    for (k, (label, list)) in [("ORIZZONTALI", &pz.across), ("VERTICALI", &pz.down)]
        .into_iter()
        .enumerate()
    {
        if k > 0 {
            y += flow.size * 0.9; // small gap before the second list
        }
        // keep the heading with at least its first clue
        if y + flow.head_h() + flow.line_h() > bottom {
            next_column(&mut pages, &mut col, &mut y);
        }
        let cur = pages.len() - 1;
        pages[cur].text(
            col_x(col),
            y + flow.head_size(),
            flow.head_size(),
            true,
            label,
        );
        y += flow.head_h();
        for cl in list {
            let lines = wrap(&cl.text, flow.size, col_w - flow.indent());
            let h = lines.len() as f64 * flow.line_h() + flow.size * 0.25;
            if y + h > bottom {
                next_column(&mut pages, &mut col, &mut y);
            }
            let cur = pages.len() - 1;
            let x = col_x(col);
            pages[cur].text(x, y + flow.size, flow.size, true, &format!("{}.", cl.num));
            for (i, ln) in lines.iter().enumerate() {
                let base = y + flow.size + i as f64 * flow.line_h();
                pages[cur].text(x + flow.indent(), base, flow.size, false, ln);
            }
            y += h;
        }
    }
    pages
}

const HEADER_H: f64 = 44.0;
const GRID_CLUE_GAP: f64 = 16.0;
const MAX_CELL: f64 = 30.0;
const MIN_CELL: f64 = 15.0;

/// Choose the largest clue font, then the largest grid cell, that put the
/// grid and every clue on ONE page. `None` if no combination fits.
fn one_page_layout(pz: &PdfPuzzle, rows: usize, cols: usize) -> Option<(f64, Flow)> {
    let avail_w = PAGE_W - 2.0 * MARGIN;
    let max_cell = (avail_w / cols as f64).min(MAX_CELL);
    for size10 in (65..=95).rev().step_by(5) {
        let flow = Flow {
            n_cols: 3,
            size: size10 as f64 / 10.0,
        };
        let mut cell = max_cell;
        while cell >= MIN_CELL.min(max_cell) {
            let clues_top = MARGIN + HEADER_H + cell * rows as f64 + GRID_CLUE_GAP;
            if clues_top < PAGE_H - MARGIN && flow_clues(pz, flow, clues_top).len() == 1 {
                return Some((cell, flow));
            }
            cell -= 0.5;
        }
    }
    None
}

/// Columns of the grid (a rebus `[RE]` is one cell).
fn grid_cols(pz: &PdfPuzzle) -> usize {
    pz.fill
        .first()
        .map_or(1, |r| crate::grid::row_cells(r).len())
        .max(1)
}

/// Render the puzzle to PDF bytes: grid + clues (on one page whenever a
/// readable layout exists), then a solution page.
pub fn render_pdf(pz: &PdfPuzzle) -> Vec<u8> {
    let rows = pz.fill.len().max(1);
    let cols = grid_cols(pz);
    let avail_w = PAGE_W - 2.0 * MARGIN;

    let (cell, flow) = one_page_layout(pz, rows, cols).unwrap_or_else(|| {
        // Too much to fit: big grid on page 1, clues continue on more pages.
        let cell = (avail_w / cols as f64)
            .min(PAGE_H * 0.5 / rows as f64)
            .min(MAX_CELL);
        (
            cell,
            Flow {
                n_cols: 3,
                size: 8.0,
            },
        )
    });

    let grid_top = MARGIN + HEADER_H;
    let mut first = Page::default();
    first.text(MARGIN, MARGIN + 14.0, 18.0, true, &pz.title);
    first.text(MARGIN, MARGIN + 30.0, 9.5, false, &pz.subtitle);
    let grid_x = MARGIN + (avail_w - cell * cols as f64) / 2.0;
    draw_grid(&mut first, pz, grid_x, grid_top, cell, false);

    let clues_top = grid_top + cell * rows as f64 + GRID_CLUE_GAP;
    let mut pages = flow_clues(pz, flow, clues_top);
    first.ops.extend_from_slice(&pages[0].ops);
    pages[0] = first;

    // ---- solution page ----
    let mut sol = Page::default();
    sol.text(MARGIN, MARGIN + 14.0, 16.0, true, "Soluzione");
    sol.text(MARGIN, MARGIN + 30.0, 9.5, false, &pz.title);
    let scell = (avail_w / cols as f64)
        .min((PAGE_H - 2.0 * MARGIN - 60.0) / rows as f64)
        .min(26.0);
    let sx = MARGIN + (avail_w - scell * cols as f64) / 2.0;
    draw_grid(&mut sol, pz, sx, MARGIN + 44.0, scell, true);
    pages.push(sol);

    assemble(&pages)
}

fn assemble(pages: &[Page]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    let n_pages = pages.len();
    let page_obj = |i: usize| 5 + 2 * i;
    let obj = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]| {
        offsets.push(out.len());
        let id = offsets.len();
        let _ = writeln!(out, "{id} 0 obj");
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    let kids: Vec<String> = (0..n_pages)
        .map(|i| format!("{} 0 R", page_obj(i)))
        .collect();
    obj(
        &mut out,
        &mut offsets,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {n_pages} >>",
            kids.join(" ")
        )
        .as_bytes(),
    );
    obj(
        &mut out,
        &mut offsets,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    );
    obj(
        &mut out,
        &mut offsets,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>",
    );
    for (i, p) in pages.iter().enumerate() {
        obj(
            &mut out,
            &mut offsets,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W} {PAGE_H}] \
                 /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents {} 0 R >>",
                page_obj(i) + 1
            )
            .as_bytes(),
        );
        let mut body = format!("<< /Length {} >>\nstream\n", p.ops.len()).into_bytes();
        body.extend_from_slice(&p.ops);
        body.extend_from_slice(b"\nendstream");
        obj(&mut out, &mut offsets, &body);
    }
    let xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1);
    for o in &offsets {
        let _ = writeln!(out, "{o:010} 00000 n ");
    }
    let _ = write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        offsets.len() + 1
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PdfPuzzle {
        PdfPuzzle {
            title: "Prova".into(),
            subtitle: "3×3".into(),
            fill: vec!["CA#".into(), "ASO".into(), "#OK".into()],
            numbers: vec![(0, 0, 1), (0, 1, 2), (1, 0, 3), (1, 2, 4), (2, 1, 5)],
            across: vec![PdfClue {
                num: 1,
                text: "Città (con accento) e \\ barra".into(),
            }],
            down: vec![PdfClue {
                num: 1,
                text: "Verticale".into(),
            }],
        }
    }

    #[test]
    fn produces_wellformed_pdf() {
        let pdf = render_pdf(&sample());
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        // accented letter encoded as a single WinAnsi byte; parens escaped
        assert!(pdf.windows(5).any(|w| w == b"Citt\xE0"));
        assert!(pdf.windows(2).any(|w| w == b"\\("));
        // two pages: puzzle + solution
        assert!(String::from_utf8_lossy(&pdf).contains("/Count 2"));
    }

    #[test]
    fn xref_offsets_point_at_objects() {
        let pdf = render_pdf(&sample());
        let s = String::from_utf8_lossy(&pdf).to_string();
        let xref_at: usize = s
            .rsplit("startxref\n")
            .next()
            .unwrap()
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        // the xref table is pure ASCII, so slice the raw bytes (the lossy
        // string above has shifted offsets after the binary header bytes)
        let table = String::from_utf8(pdf[xref_at..].to_vec()).unwrap();
        assert!(table.starts_with("xref"));
        for (i, line) in table.lines().skip(3).take(4).enumerate() {
            let off: usize = line[..10].parse().unwrap();
            assert!(pdf[off..].starts_with(format!("{} 0 obj", i + 1).as_bytes()));
        }
    }

    #[test]
    fn overflowing_clues_continue_on_extra_pages() {
        let mut pz = sample();
        let long = "una definizione molto lunga che occupa parecchie righe ".repeat(6);
        pz.across = (1..=150)
            .map(|n| PdfClue {
                num: n,
                text: long.clone(),
            })
            .collect();
        let s = String::from_utf8_lossy(&render_pdf(&pz)).to_string();
        let count: usize = s
            .split("/Count ")
            .nth(1)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(count > 2, "expected overflow pages, got {count}");
        assert!(s.contains("(150.)"), "last clue rendered");
    }

    #[test]
    fn rebus_cells_render_as_one_square() {
        let pz = PdfPuzzle {
            title: "R".into(),
            subtitle: String::new(),
            fill: vec!["C[RE]DO".into(), "#X##".into()],
            numbers: vec![(0, 0, 1), (0, 1, 2)],
            across: vec![PdfClue {
                num: 1,
                text: "Credo".into(),
            }],
            down: vec![PdfClue {
                num: 2,
                text: "Re".into(),
            }],
        };
        let pdf = String::from_utf8_lossy(&render_pdf(&pz)).to_string();
        assert!(
            pdf.contains("(RE) Tj"),
            "the rebus string is drawn in its cell"
        );
        assert!(!pdf.contains("([) Tj"), "brackets are not cells");
        // 4 columns: grid width = 4 cells, so no cell is drawn past column 4
        assert_eq!(grid_cols(&pz), 4);
    }

    #[test]
    fn wrap_respects_width() {
        let lines = wrap(
            "una definizione abbastanza lunga da andare a capo",
            10.0,
            80.0,
        );
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| measure(l, 10.0) <= 80.0));
    }
}
