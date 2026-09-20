//! Generate VALID rotationally-symmetric grids: 180° symmetry, every run >= 3,
//! single connected white component, target block count. Random patterns are
//! mostly ugly-to-fill; pairing this with the solver as a screener (fill many,
//! keep the clean ones) is the product path.

use crate::util::Rng;

pub fn generate(n: usize, target_blocks: usize, rng: &mut Rng, max_outer: usize) -> Option<String> {
    for _ in 0..max_outer {
        let mut block = vec![vec![false; n]; n];
        let mut placed = 0usize;
        let center = (n / 2, n / 2);
        let mut attempts = 0;
        while placed < target_blocks && attempts < 3000 {
            attempts += 1;
            let r = rng.below(n);
            let c = rng.below(n);
            let (r2, c2) = (n - 1 - r, n - 1 - c);
            if block[r][c] {
                continue;
            }
            // A self-symmetric center cell only exists on ODD-sized grids;
            // on even sizes every cell has a distinct 180° partner.
            let is_center = n % 2 == 1 && (r, c) == center;
            if is_center && target_blocks - placed < 1 {
                continue;
            }
            if !is_center && target_blocks - placed < 2 {
                continue;
            }
            block[r][c] = true;
            if !is_center {
                block[r2][c2] = true;
            }
            let ok =
                runs_ok_after(&block, r, c, n) && (is_center || runs_ok_after(&block, r2, c2, n));
            if ok {
                placed += if is_center { 1 } else { 2 };
            } else {
                block[r][c] = false;
                if !is_center {
                    block[r2][c2] = false;
                }
            }
        }
        if placed + 1 >= target_blocks && all_runs_ok(&block, n) && connected(&block, n) {
            return Some(render(&block, n));
        }
    }
    None
}

/// Choose across-entry placements for theme answers of the given `lengths`.
/// Returns (row, col, len) per theme, or None if the count isn't supported.
///
/// Themes go on spread-out rows in the TOP half (their 180° mirror rows stay
/// theme-free), which keeps the block pattern symmetric for ANY mix of theme
/// lengths without theme-vs-theme conflicts. Columns are chosen close to
/// centered while keeping the side gaps legal (0 or >= 3 cells).
pub fn place_themes(n: usize, lengths: &[usize]) -> Option<Vec<(usize, usize, usize)>> {
    let k = lengths.len();
    // Rows are chosen to avoid mirror-row pairs (which would force conflicting
    // bounding blocks for unequal lengths). Each theme sits in the top half (or
    // the self-symmetric center row 7); its 180° mirror becomes an equal-length
    // NON-theme slot in the bottom half — so the grid stays symmetric and looks
    // balanced (long entries top and bottom) while theme answers fill cleanly.
    let rows: &[usize] = match k {
        1 => &[7],
        2 => &[3, 5],
        3 => &[2, 4, 6],
        4 => &[1, 3, 5, 7],
        _ => return None,
    };
    // Reject mirror-row pairs unless equal length (they'd force conflicting
    // bounding blocks). We pick row tables that only pair mirror rows when we
    // can also require equal lengths; to stay simple, only pair them when the
    // lengths actually match, else fail and let the caller adjust.
    let mut out = Vec::with_capacity(k);
    for (i, &len) in lengths.iter().enumerate() {
        let r = rows[i];
        if len > n {
            return None;
        }
        let col = find_start(n, len)?;
        out.push((r, col, len));
    }
    // Validate mirror-pair length compatibility: if row r and n-1-r are both
    // theme rows, their lengths must match (centered bounds mirror to equal len).
    for a in 0..k {
        for b in (a + 1)..k {
            if rows[a] + rows[b] == n - 1 && lengths[a] != lengths[b] {
                return None;
            }
        }
    }
    Some(out)
}

/// Every legal start column for a length-`len` across entry: the side gaps
/// (between the bounding blocks and the grid edges) must be 0 or >= 3 cells.
pub fn legal_starts(n: usize, len: usize) -> Vec<usize> {
    if len > n {
        return Vec::new();
    }
    (0..=(n - len))
        .filter(|&start| {
            let left_ok = start == 0 || start == 1 || start >= 4;
            let rend = start + len;
            left_ok && (rend == n || rend == n - 1 || rend + 4 <= n)
        })
        .collect()
}

/// Closest-to-centered start column for a length-`len` across entry such that
/// the side gaps (between the bounding blocks and the grid edges) are legal.
fn find_start(n: usize, len: usize) -> Option<usize> {
    let centered = (n - len) / 2;
    legal_starts(n, len)
        .into_iter()
        .min_by_key(|&start| (start as isize - centered as isize).unsigned_abs())
}

/// Sample a random placement variant for the theme lengths: a random
/// theme→row assignment (permutation of the row table) and a random legal
/// start column per theme.
///
/// Why: `place_themes` is deterministic — one row order, one (centered)
/// column per length — so every generated candidate shares the exact same
/// theme-letter alignments. If those alignments happen to create a weak
/// crossing pattern, ALL candidates inherit it and the fill fails wholesale.
/// Sampling variants de-correlates candidates; the screener keeps whichever
/// alignment fills best.
///
/// Index i of the result is theme i's (row, col, len), matching the input
/// order (locks are built by zipping themes with placements).
pub fn sample_placement(
    n: usize,
    lengths: &[usize],
    rng: &mut Rng,
) -> Option<Vec<(usize, usize, usize)>> {
    let k = lengths.len();
    let rows: &[usize] = match k {
        1 => &[7],
        2 => &[3, 5],
        3 => &[2, 4, 6],
        4 => &[1, 3, 5, 7],
        _ => return None,
    };
    // Random theme→row assignment (Fisher-Yates on a copy of the row table).
    let mut perm: Vec<usize> = rows.to_vec();
    for i in (1..perm.len()).rev() {
        let j = rng.below(i + 1);
        perm.swap(i, j);
    }
    let mut out = Vec::with_capacity(k);
    for (i, &len) in lengths.iter().enumerate() {
        let starts = legal_starts(n, len);
        if starts.is_empty() {
            return None;
        }
        let col = starts[rng.below(starts.len())];
        out.push((perm[i], col, len));
    }
    // Mirror-pair rows must carry equal lengths (their centered bounds mirror
    // onto each other). Current row tables have no mirror pairs, but keep the
    // guard so a future table change can't silently produce broken seeds.
    for a in 0..k {
        for b in (a + 1)..k {
            if out[a].0 + out[b].0 == n - 1 && lengths[a] != lengths[b] {
                return None;
            }
        }
    }
    Some(out)
}

/// Repair a seeded block pattern: any white run of length 1-2 that is bounded
/// by seed blocks / grid edges can NEVER become legal — later placement only
/// ADDS blocks, so short runs can only shrink, never grow to >= 3. Block such
/// runs out now (symmetrically), cascading until no short run remains.
///
/// Without this, bounding blocks near an edge deadlock the random placer: a
/// theme on row 2 seeds blocks at (2,c), leaving a 2-cell run above in that
/// column; fixing it needs BOTH (0,c) and (1,c) blocked, but each rescue cell
/// is individually illegal under `runs_ok_after`, so the outer loop fails
/// forever regardless of --blocks.
///
/// Returns None if a doomed run overlaps a protected (theme) cell — that
/// placement geometry is infeasible.
fn repair_seed(
    mut block: Vec<Vec<bool>>,
    protected: &[Vec<bool>],
    n: usize,
) -> Option<Vec<Vec<bool>>> {
    loop {
        let mut to_block: Vec<(usize, usize)> = Vec::new();
        // Like all_runs_ok: range loops keep the `== n` sentinel (closes the
        // final run) readable; iterator forms obscure it.
        #[allow(clippy::needless_range_loop)]
        for r in 0..n {
            let mut start = 0usize;
            for c in 0..=n {
                if c == n || block[r][c] {
                    let run = c - start;
                    if run > 0 && run < 3 {
                        for cc in start..c {
                            to_block.push((r, cc));
                        }
                    }
                    start = c + 1;
                }
            }
        }
        #[allow(clippy::needless_range_loop)]
        for c in 0..n {
            let mut start = 0usize;
            for r in 0..=n {
                if r == n || block[r][c] {
                    let run = r - start;
                    if run > 0 && run < 3 {
                        for rr in start..r {
                            to_block.push((rr, c));
                        }
                    }
                    start = r + 1;
                }
            }
        }
        if to_block.is_empty() {
            return Some(block);
        }
        for (r, c) in to_block {
            if protected[r][c] || protected[n - 1 - r][n - 1 - c] {
                return None;
            }
            block[r][c] = true;
            block[n - 1 - r][n - 1 - c] = true;
        }
    }
}

fn themes_intact(block: &[Vec<bool>], placements: &[(usize, usize, usize)], n: usize) -> bool {
    for &(r, c, len) in placements {
        if c > 0 && !block[r][c - 1] {
            return false; // missing left bound
        }
        if c + len < n && !block[r][c + len] {
            return false; // missing right bound
        }
        for j in 0..len {
            if block[r][c + j] {
                return false; // interior block
            }
        }
    }
    true
}

/// Generate a valid symmetric grid containing the given across theme slots.
/// `placements` are (row, col, len); the answers themselves are locked in later.
pub fn generate_themed(
    n: usize,
    placements: &[(usize, usize, usize)],
    target_blocks: usize,
    rng: &mut Rng,
    max_outer: usize,
) -> Option<String> {
    // Cells that must stay white: theme cells and their 180° partners.
    let mut protected = vec![vec![false; n]; n];
    for &(r, c, len) in placements {
        if r >= n || c + len > n {
            return None;
        }
        for j in 0..len {
            protected[r][c + j] = true;
            protected[n - 1 - r][n - 1 - (c + j)] = true;
        }
    }
    // Seed the theme bounding blocks (and their symmetric partners).
    let mut seed = vec![vec![false; n]; n];
    let place = |block: &mut Vec<Vec<bool>>, r: usize, c: usize| -> bool {
        let (r2, c2) = (n - 1 - r, n - 1 - c);
        if protected[r][c] || protected[r2][c2] {
            return false;
        }
        block[r][c] = true;
        block[r2][c2] = true;
        true
    };
    for &(r, c, len) in placements {
        if c > 0 && !place(&mut seed, r, c - 1) {
            return None;
        }
        if c + len < n && !place(&mut seed, r, c + len) {
            return None;
        }
    }
    // Block out runs the bounding blocks have already doomed (see repair_seed).
    let seed = repair_seed(seed, &protected, n)?;
    let seed_blocks: usize = seed.iter().flatten().filter(|&&b| b).count();

    for _ in 0..max_outer {
        let mut block = seed.clone();
        let mut placed = seed_blocks;
        let center = (n / 2, n / 2);
        let mut attempts = 0;
        while placed < target_blocks && attempts < 4000 {
            attempts += 1;
            let r = rng.below(n);
            let c = rng.below(n);
            let (r2, c2) = (n - 1 - r, n - 1 - c);
            if block[r][c] || protected[r][c] || protected[r2][c2] {
                continue;
            }
            // A self-symmetric center cell only exists on ODD-sized grids;
            // on even sizes every cell has a distinct 180° partner.
            let is_center = n % 2 == 1 && (r, c) == center;
            if is_center && target_blocks - placed < 1 {
                continue;
            }
            if !is_center && target_blocks - placed < 2 {
                continue;
            }
            block[r][c] = true;
            if !is_center {
                block[r2][c2] = true;
            }
            let ok =
                runs_ok_after(&block, r, c, n) && (is_center || runs_ok_after(&block, r2, c2, n));
            if ok {
                placed += if is_center { 1 } else { 2 };
            } else {
                block[r][c] = false;
                if !is_center {
                    block[r2][c2] = false;
                }
            }
        }
        if placed + 1 >= target_blocks
            && all_runs_ok(&block, n)
            && connected(&block, n)
            && themes_intact(&block, placements, n)
        {
            return Some(render(&block, n));
        }
    }
    None
}

fn runs_ok_after(block: &[Vec<bool>], r: usize, c: usize, n: usize) -> bool {
    // measure white segments adjacent to (r,c) in its row and column
    let mut left = 0;
    let mut cc = c as isize - 1;
    while cc >= 0 && !block[r][cc as usize] {
        left += 1;
        cc -= 1;
    }
    let mut right = 0;
    let mut cc = c + 1;
    while cc < n && !block[r][cc] {
        right += 1;
        cc += 1;
    }
    let mut up = 0;
    let mut rr = r as isize - 1;
    while rr >= 0 && !block[rr as usize][c] {
        up += 1;
        rr -= 1;
    }
    let mut down = 0;
    let mut rr = r + 1;
    while rr < n && !block[rr][c] {
        down += 1;
        rr += 1;
    }
    for seg in [left, right, up, down] {
        if seg > 0 && seg < 3 {
            return false;
        }
    }
    true
}

fn all_runs_ok(block: &[Vec<bool>], n: usize) -> bool {
    for row in block.iter().take(n) {
        let mut run = 0;
        // Range loop is the cleanest expression: 0..=n includes the sentinel
        // index `c == n` that closes any open run at the end of the row.
        #[allow(clippy::needless_range_loop)]
        for c in 0..=n {
            let b = c == n || row[c];
            if !b {
                run += 1;
            } else {
                if run > 0 && run < 3 {
                    return false;
                }
                run = 0;
            }
        }
    }
    for c in 0..n {
        let mut run = 0;
        for row in block.iter().take(n) {
            let b = row[c];
            if !b {
                run += 1;
            } else {
                if run > 0 && run < 3 {
                    return false;
                }
                run = 0;
            }
        }
        // sentinel pass equivalent to r == n in the original loop
        if run > 0 && run < 3 {
            return false;
        }
    }
    true
}

fn connected(block: &[Vec<bool>], n: usize) -> bool {
    let mut start = None;
    let mut total = 0;
    for (r, row) in block.iter().take(n).enumerate() {
        for (c, &cell) in row.iter().take(n).enumerate() {
            if !cell {
                total += 1;
                if start.is_none() {
                    start = Some((r, c));
                }
            }
        }
    }
    let Some(start) = start else { return false };
    let mut seen = vec![vec![false; n]; n];
    seen[start.0][start.1] = true;
    let mut stack = vec![start];
    let mut count = 0;
    while let Some((r, c)) = stack.pop() {
        count += 1;
        let mut nbrs = Vec::new();
        if r > 0 {
            nbrs.push((r - 1, c));
        }
        if r + 1 < n {
            nbrs.push((r + 1, c));
        }
        if c > 0 {
            nbrs.push((r, c - 1));
        }
        if c + 1 < n {
            nbrs.push((r, c + 1));
        }
        for (nr, nc) in nbrs {
            if !block[nr][nc] && !seen[nr][nc] {
                seen[nr][nc] = true;
                stack.push((nr, nc));
            }
        }
    }
    count == total
}

fn render(block: &[Vec<bool>], n: usize) -> String {
    let mut s = String::with_capacity(n * (n + 1));
    for row in block.iter().take(n) {
        for &cell in row.iter().take(n) {
            s.push(if cell { '#' } else { '.' });
        }
        s.push('\n');
    }
    s
}

/// Size-aware default for candidate-to-candidate block-count jitter: ±2 on
/// full-size grids, ±1 on minis/midis (where ±2 is a big relative swing).
pub fn default_block_jitter(n: usize) -> usize {
    if n >= 13 {
        2
    } else {
        1
    }
}

/// Per-candidate block count: uniform in `base ..= base + jitter` (UP-only).
/// Blockier grids fill cleaner and more often at every quality gate, so
/// spreading candidates upward widens the pool where it pays. The original
/// symmetric ±jitter design measurably hurt at strict gates (15×15, 300
/// candidates, mean>=78/iffy 0: symmetric kept 1 vs 2-3 for each of 36/37/38
/// pinned) because below-target templates rarely pass yet burn fill budget —
/// deliberately open builds should pin a lower exact count instead.
pub fn jittered_blocks(base: usize, jitter: usize, rng: &mut Rng) -> usize {
    if jitter == 0 {
        return base;
    }
    base + (rng.next_u64() as usize) % (jitter + 1)
}

/// Calibrated word-count floor for candidate templates (themeless and
/// seeded). At a fixed block count the block ARRANGEMENT still swings the
/// word count widely, and word count is a near-step-function predictor of
/// clean fillability: a template study (1000 patterns x 8 fills, 15x15 @ 38
/// blocks) found shapes with <=68 words almost never fill clean (~1%/fill)
/// while >=72-word shapes fill clean 45-82% of the time — so rejecting
/// sub-floor templates at generation time roughly triples the clean-keeper
/// rate for free.
///
/// The same step shows up at every midi size (measured 2026-09, publication
/// gates, auto blocks, share of keepers with no weak entry by word count):
///
/// ```text
///  7×7: 14w 0-8%   16w 57-80%  20w+ 100%      → floor 16
///  8×8: 20w 6%     22w 79%     24w+ 100%      → floor 22
///  9×9: 26w 0%     28w 53%     30w  95%       → floor 28
/// 10×10: 32w 0%    34w 27%     36w  100%      → floor 34
/// 11×11: 38w 0%    40w 45%     46w+ 100%      → floor 40
/// 12×12: 46-48w 0% 50w 50%     52w  100%      → floor 50
/// ```
///
/// Each floor sits at the start of its jump (as 72 does at 15×15) and keeps
/// roughly a quarter to two thirds of generated shapes; the floors track
/// ~0.33-0.35 words per cell. Minis (≤6) have essentially one word count per
/// shape, so they get NO floor. Callers apply this at generation (themeless
/// and seeded) and should relax it if it ever starves generation (a very
/// open, low-block pinned build) — it must never hard-fail a run.
pub fn word_floor(size: usize) -> usize {
    match size {
        13.. => 72,
        12 => 50,
        11 => 40,
        10 => 34,
        9 => 28,
        8 => 22,
        7 => 16,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Puzzle;

    #[test]
    fn generates_valid_grid() {
        let mut rng = Rng::new(1);
        let t = generate(15, 36, &mut rng, 4000).expect("should generate");
        let p = Puzzle::from_template(&t);
        assert_eq!(p.orphan_cells(), 0, "no orphan cells");
        // block count near target
        let b = p.block_count();
        assert!((34..=38).contains(&b), "block count {b} near 36");
    }

    #[test]
    fn find_start_avoids_short_edge_runs() {
        // len 11 can't be centered (start 2 leaves a 1-cell edge run); a legal
        // start must exist.
        let s = find_start(15, 11).unwrap();
        let left_ok = s == 0 || s == 1 || s >= 4;
        let rend = s + 11;
        assert!(left_ok && (rend == 15 || rend == 14 || rend + 4 <= 15));
        // full-width and clean lengths center fine
        assert_eq!(find_start(15, 15), Some(0));
        assert_eq!(find_start(15, 13), Some(1));
    }

    #[test]
    fn legal_starts_all_legal() {
        for len in 3..=15usize {
            if len == 12 {
                assert!(legal_starts(15, len).is_empty(), "12 is unplaceable");
                continue;
            }
            let starts = legal_starts(15, len);
            assert!(!starts.is_empty(), "len {len} must have a legal start");
            for s in starts {
                let left_ok = s == 0 || s == 1 || s >= 4;
                let rend = s + len;
                assert!(left_ok && (rend == 15 || rend == 14 || rend + 4 <= 15));
            }
        }
    }

    #[test]
    fn sample_placement_valid_and_diverse() {
        let lengths = [7usize, 8, 8];
        let mut rng = Rng::new(11);
        let mut variants = std::collections::HashSet::new();
        for _ in 0..50 {
            let p = sample_placement(15, &lengths, &mut rng).expect("placeable");
            assert_eq!(p.len(), 3);
            let mut rows_seen = std::collections::HashSet::new();
            for (i, &(r, c, len)) in p.iter().enumerate() {
                assert_eq!(len, lengths[i], "placement {i} keeps input order");
                assert!([2usize, 4, 6].contains(&r), "row from the k=3 table");
                assert!(rows_seen.insert(r), "rows distinct");
                assert!(legal_starts(15, len).contains(&c), "column legal");
            }
            variants.insert(format!("{p:?}"));
        }
        assert!(
            variants.len() > 5,
            "sampling must produce variety, got {} distinct",
            variants.len()
        );
    }

    /// Regression: a 3-theme set with a sub-15 first answer puts bounding
    /// blocks on row 2, dooming the 2-cell runs above them in those columns.
    /// Before repair_seed, the one-block-at-a-time placer could never rescue
    /// them (each rescue cell is individually illegal), so NO template could
    /// be generated at ANY block count — themed mode was unusable for the
    /// documented 3-theme workflow unless the first theme was 15 letters.
    #[test]
    fn generates_themed_grid_short_themes_row2() {
        use crate::grid::Dir;
        let lengths = [7usize, 8, 8]; // e.g. HOMERUN, HATTRICK, SLAMDUNK
        let placements = place_themes(15, &lengths).expect("placeable");
        let mut rng = Rng::new(3);
        let mut got = None;
        for _ in 0..40 {
            if let Some(t) = generate_themed(15, &placements, 44, &mut rng, 300) {
                got = Some(t);
                break;
            }
        }
        let t = got.expect("repair_seed must unlock 7/8/8 themed generation");
        let p = Puzzle::from_template(&t);
        assert_eq!(p.orphan_cells(), 0);
        for &(r, c, len) in &placements {
            let ei = p
                .find_entry(r, c, Dir::Across)
                .expect("theme slot is an entry");
            assert_eq!(p.entries[ei].len, len);
        }
    }

    #[test]
    fn generates_themed_grid_with_intact_slots() {
        use crate::grid::Dir;
        let lengths = [15usize, 13, 7];
        let placements = place_themes(15, &lengths).expect("placeable");
        let mut rng = Rng::new(7);
        let mut got = None;
        for _ in 0..40 {
            if let Some(t) = generate_themed(15, &placements, 40, &mut rng, 300) {
                got = Some(t);
                break;
            }
        }
        let t = got.expect("should generate a themed grid");
        let p = Puzzle::from_template(&t);
        assert_eq!(p.orphan_cells(), 0);
        // every theme placement must be a real across entry of that exact length
        for &(r, c, len) in &placements {
            let ei = p
                .find_entry(r, c, Dir::Across)
                .expect("theme slot is an entry");
            assert_eq!(p.entries[ei].len, len, "theme slot has expected length");
        }
    }

    /// The floor table: off for minis, the calibrated midi steps, 72 from
    /// full size up, and monotone in size (a bigger grid never floors lower).
    #[test]
    fn word_floor_table_is_calibrated_and_monotone() {
        assert_eq!(word_floor(5), 0);
        assert_eq!(word_floor(6), 0);
        assert_eq!(word_floor(7), 16);
        assert_eq!(word_floor(9), 28);
        assert_eq!(word_floor(13), 72);
        assert_eq!(word_floor(15), 72);
        assert_eq!(word_floor(21), 72);
        for size in 4..21 {
            assert!(
                word_floor(size) <= word_floor(size + 1),
                "floor must not drop from {size} to {}",
                size + 1
            );
        }
    }

    #[test]
    fn jittered_blocks_bounds_spread_and_defaults() {
        let mut rng = Rng::new(42);
        assert_eq!(jittered_blocks(36, 0, &mut rng), 36, "jitter 0 is exact");
        let draws: Vec<usize> = (0..200).map(|_| jittered_blocks(36, 2, &mut rng)).collect();
        assert!(
            draws.iter().all(|&b| (36..=38).contains(&b)),
            "up-only: within base ..= base + jitter"
        );
        let distinct: std::collections::HashSet<_> = draws.iter().collect();
        assert_eq!(distinct.len(), 3, "jitter should cover the whole up-range");
        assert_eq!(default_block_jitter(15), 2);
        assert_eq!(default_block_jitter(12), 1);
        assert_eq!(default_block_jitter(5), 1);
    }
}
