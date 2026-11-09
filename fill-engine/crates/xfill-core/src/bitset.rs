//! Fixed-size bitset over word indices. Hot path of the solver: `and_assign`
//! for constraint propagation, `count_ones` for MRV, `iter_ones` for
//! score-ordered candidate enumeration.

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Bitset {
    words: Box<[u64]>,
    nbits: usize,
}

impl AsRef<[u64]> for Bitset {
    #[inline]
    fn as_ref(&self) -> &[u64] {
        &self.words
    }
}

impl Bitset {
    pub fn zeros(nbits: usize) -> Self {
        let nwords = nbits.div_ceil(64);
        Bitset {
            words: vec![0u64; nwords].into_boxed_slice(),
            nbits,
        }
    }

    pub fn ones(nbits: usize) -> Self {
        let nwords = nbits.div_ceil(64);
        let mut words = vec![u64::MAX; nwords].into_boxed_slice();
        // clear the tail bits beyond nbits in the last word
        let rem = nbits % 64;
        if rem != 0 {
            words[nwords - 1] = (1u64 << rem) - 1;
        }
        Bitset { words, nbits }
    }

    /// The 64-bit blocks (bit i of the set is bit i%64 of block i/64).
    #[inline]
    pub fn blocks(&self) -> &[u64] {
        &self.words
    }

    /// Mutable 64-bit blocks; callers keep the bits past `nbits` clear.
    #[inline]
    pub fn blocks_mut(&mut self) -> &mut [u64] {
        &mut self.words
    }

    /// Clear every bit (no reallocation).
    #[inline]
    pub fn clear_all(&mut self) {
        self.words.fill(0);
    }

    /// Heap memory of the bit storage (bytes).
    pub fn heap_bytes(&self) -> usize {
        self.words.len() * std::mem::size_of::<u64>()
    }

    #[inline]
    pub fn nbits(&self) -> usize {
        self.nbits
    }

    #[inline]
    pub fn get(&self, i: usize) -> bool {
        (self.words[i >> 6] >> (i & 63)) & 1 == 1
    }

    #[inline]
    pub fn set(&mut self, i: usize) {
        self.words[i >> 6] |= 1u64 << (i & 63);
    }

    #[inline]
    pub fn clear(&mut self, i: usize) {
        self.words[i >> 6] &= !(1u64 << (i & 63));
    }

    /// self &= other
    #[inline]
    pub fn and_assign<B: AsRef<[u64]> + ?Sized>(&mut self, other: &B) {
        for (a, b) in self.words.iter_mut().zip(other.as_ref().iter()) {
            *a &= *b;
        }
    }

    /// self &= !other
    #[inline]
    pub fn andnot_assign<B: AsRef<[u64]> + ?Sized>(&mut self, other: &B) {
        for (a, b) in self.words.iter_mut().zip(other.as_ref().iter()) {
            *a &= !*b;
        }
    }

    #[inline]
    /// Do `self` and `other` share a set bit below index `limit`?
    pub fn intersects_below<B: AsRef<[u64]> + ?Sized>(&self, other: &B, limit: usize) -> bool {
        let full = limit / 64;
        for i in 0..full.min(self.words.len()) {
            if self.words[i] & other.as_ref()[i] != 0 {
                return true;
            }
        }
        let rem = limit % 64;
        if rem > 0 && full < self.words.len() {
            let mask = (1u64 << rem) - 1;
            return self.words[full] & other.as_ref()[full] & mask != 0;
        }
        false
    }

    /// Lowest index set in both `self` and `other`, if below `limit`.
    pub fn first_and_below<B: AsRef<[u64]> + ?Sized>(
        &self,
        other: &B,
        limit: usize,
    ) -> Option<usize> {
        for (i, (a, b)) in self.words.iter().zip(other.as_ref().iter()).enumerate() {
            let x = a & b;
            if x != 0 {
                let idx = i * 64 + x.trailing_zeros() as usize;
                return (idx < limit).then_some(idx);
            }
        }
        None
    }

    /// Number of bits set in both `self` and `other` below index `limit`.
    pub fn count_and_below<B: AsRef<[u64]> + ?Sized>(&self, other: &B, limit: usize) -> u32 {
        let full = (limit / 64).min(self.words.len());
        let mut n: u32 = self.words[..full]
            .iter()
            .zip(other.as_ref()[..full].iter())
            .map(|(a, b)| (a & b).count_ones())
            .sum();
        let rem = limit % 64;
        if rem > 0 && full < self.words.len() {
            let mask = (1u64 << rem) - 1;
            n += (self.words[full] & other.as_ref()[full] & mask).count_ones();
        }
        n
    }

    pub fn or_assign<B: AsRef<[u64]> + ?Sized>(&mut self, other: &B) {
        for (a, b) in self.words.iter_mut().zip(other.as_ref().iter()) {
            *a |= *b;
        }
    }

    pub fn count_ones(&self) -> u32 {
        self.words.iter().map(|w| w.count_ones()).sum()
    }

    /// Number of set bits whose index is < `limit` (for score-tier counting).
    pub fn count_ones_below(&self, limit: usize) -> u32 {
        if limit >= self.nbits {
            return self.count_ones();
        }
        let full = limit >> 6;
        let mut c = 0u32;
        for w in &self.words[..full] {
            c += w.count_ones();
        }
        let rem = limit & 63;
        if rem != 0 {
            let mask = (1u64 << rem) - 1;
            c += (self.words[full] & mask).count_ones();
        }
        c
    }

    #[inline]
    pub fn copy_from(&mut self, other: &Bitset) {
        debug_assert_eq!(self.words.len(), other.words.len());
        self.words.copy_from_slice(&other.words);
    }

    /// Iterate set-bit indices in ascending order (i.e. by score, since words
    /// are sorted by score descending → index 0 is best).
    pub fn iter_ones(&self) -> OnesIter<'_> {
        OnesIter {
            words: &self.words,
            word_idx: 0,
            cur: if self.words.is_empty() {
                0
            } else {
                self.words[0]
            },
        }
    }
}

pub struct OnesIter<'a> {
    words: &'a [u64],
    word_idx: usize,
    cur: u64,
}

impl Iterator for OnesIter<'_> {
    type Item = usize;
    #[inline]
    fn next(&mut self) -> Option<usize> {
        loop {
            if self.cur != 0 {
                let bit = self.cur.trailing_zeros() as usize;
                self.cur &= self.cur - 1; // clear lowest set bit
                return Some(self.word_idx * 64 + bit);
            }
            self.word_idx += 1;
            if self.word_idx >= self.words.len() {
                return None;
            }
            self.cur = self.words[self.word_idx];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersects_below_respects_limit() {
        let mut a = Bitset::zeros(200);
        let mut b = Bitset::zeros(200);
        a.set(130);
        b.set(130);
        assert!(a.intersects_below(&b, 200));
        assert!(a.intersects_below(&b, 131));
        assert!(!a.intersects_below(&b, 130), "bit 130 is not below 130");
        assert!(!a.intersects_below(&b, 64));
        a.set(5);
        b.set(5);
        assert!(a.intersects_below(&b, 6));
        assert!(!Bitset::zeros(10).intersects_below(&Bitset::ones(10), 10));
    }

    #[test]
    fn count_and_below_counts_common_bits_under_limit() {
        let mut a = Bitset::zeros(200);
        let mut b = Bitset::zeros(200);
        for i in [3, 70, 150, 199] {
            a.set(i);
            b.set(i);
        }
        a.set(10);
        assert_eq!(a.count_and_below(&b, 200), 4);
        assert_eq!(a.count_and_below(&b, 150), 2);
        assert_eq!(a.count_and_below(&b, 151), 3);
        assert_eq!(a.count_and_below(&b, 0), 0);
    }

    #[test]
    fn first_and_below_finds_lowest_common_bit() {
        let mut a = Bitset::zeros(200);
        let mut b = Bitset::zeros(200);
        a.set(150);
        b.set(150);
        a.set(10);
        assert_eq!(a.first_and_below(&b, 200), Some(150));
        assert_eq!(a.first_and_below(&b, 150), None);
        b.set(10);
        assert_eq!(a.first_and_below(&b, 200), Some(10));
        assert_eq!(
            Bitset::zeros(70).first_and_below(&Bitset::ones(70), 70),
            None
        );
    }

    #[test]
    fn or_assign_unions() {
        let mut a = Bitset::zeros(130);
        let mut b = Bitset::zeros(130);
        a.set(1);
        b.set(129);
        a.or_assign(&b);
        assert!(a.get(1) && a.get(129));
        assert_eq!(a.count_ones(), 2);
    }

    #[test]
    fn ones_zeros_count() {
        assert_eq!(Bitset::zeros(100).count_ones(), 0);
        assert_eq!(Bitset::ones(100).count_ones(), 100);
        assert_eq!(Bitset::ones(64).count_ones(), 64);
        assert_eq!(Bitset::ones(65).count_ones(), 65);
    }

    #[test]
    fn set_get_clear() {
        let mut b = Bitset::zeros(200);
        b.set(0);
        b.set(63);
        b.set(64);
        b.set(199);
        assert!(b.get(0) && b.get(63) && b.get(64) && b.get(199));
        assert!(!b.get(1));
        assert_eq!(b.count_ones(), 4);
        b.clear(63);
        assert!(!b.get(63));
        assert_eq!(b.count_ones(), 3);
    }

    #[test]
    fn and_andnot() {
        let mut a = Bitset::ones(128);
        let mut mask = Bitset::zeros(128);
        mask.set(1);
        mask.set(100);
        a.and_assign(&mask);
        assert_eq!(a.count_ones(), 2);
        assert!(a.get(1) && a.get(100));

        let mut c = Bitset::ones(128);
        c.andnot_assign(&mask);
        assert_eq!(c.count_ones(), 126);
        assert!(!c.get(1) && !c.get(100));
    }

    #[test]
    fn iter_in_order() {
        let mut b = Bitset::zeros(200);
        for i in [3usize, 64, 65, 130, 199] {
            b.set(i);
        }
        let got: Vec<usize> = b.iter_ones().collect();
        assert_eq!(got, vec![3, 64, 65, 130, 199]);
    }

    #[test]
    fn count_below() {
        let mut b = Bitset::ones(200);
        assert_eq!(b.count_ones_below(50), 50);
        assert_eq!(b.count_ones_below(64), 64);
        assert_eq!(b.count_ones_below(200), 200);
        b.clear(10);
        assert_eq!(b.count_ones_below(50), 49);
    }
}
