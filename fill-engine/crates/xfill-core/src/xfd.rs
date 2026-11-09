//! Dictionary files (`.xfd`): the arrays of a built dictionary — words,
//! scores, compatibility masks, compressed clues — written as they are in
//! memory, so that [`open`] maps the file and uses them in place: no
//! parsing, no copy, no peak while loading, and file-backed pages the system
//! can reclaim and share.
//!
//! Layout (integers little-endian, sections aligned to 8 bytes):
//!
//! ```text
//! 0   magic "XFDICT01"
//! 8   format version (u32), byte-order mark 0x01020304 (u32)
//! 16  file length (u64)
//! 24  checksum of bytes 64.. (u64)
//! 32  section count (u32), reserved
//! 64  section table: (offset u64, bytes u64) per section
//! ..  sections
//! ```
//!
//! Sections, in order: meta (u64: rebus count, then per length
//! `len, n`); per length 0..=MAX_LEN its letters, scores, mask blocks,
//! mask offsets and position counts; the rebus strings (NUL-separated); the
//! clues' answers, length-group indices, offsets, coded text, byte-pair
//! table and code flags. A file of another format version, corrupted or
//! inconsistent, is refused with an error (rebuild it from the CSV).

use crate::api::Dictionary;
use crate::clues::ClueDb;
use crate::mapped::{Buf, Mapping, Pod};
use crate::wordlist::{LenData, Wordlist, ALPHA, MAX_LEN, REBUS_BASE};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

const MAGIC: &[u8; 8] = b"XFDICT01";
/// Version of the layout: bump on any change (old files are refused).
pub const FORMAT: u32 = 1;
const BOM: u32 = 0x0102_0304;
const HEADER: usize = 64;

/// A fast 64-bit checksum (8 bytes per step).
fn checksum(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325 ^ bytes.len() as u64;
    let (words, rest) = bytes.as_chunks::<8>();
    for w in words {
        h = (h ^ u64::from_le_bytes(*w))
            .wrapping_mul(0x0000_0100_0000_01B3)
            .rotate_left(29);
    }
    for &b in rest {
        h = (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01B3);
    }
    h ^ (h >> 31)
}

fn le_bytes<T: Pod>(v: &[T]) -> &[u8] {
    // SAFETY: plain integers; every target of the engine is little-endian
    // (checked by the byte-order mark when reading)
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
}

/// The bytes of a dictionary file for `dict`.
pub fn to_bytes(dict: &Dictionary) -> Vec<u8> {
    let wl = &dict.words;
    let c = &dict.clues;
    let mut meta: Vec<u64> = vec![wl.rebus.len() as u64];
    for ld in &wl.by_len {
        meta.extend([ld.len as u64, ld.n as u64]);
    }
    let rebus: Vec<u8> = wl.rebus.join("\0").into_bytes();
    let pair: Vec<u8> = c.pair.iter().flatten().copied().collect();
    let is_code: Vec<u8> = c.is_code.iter().map(|&b| b as u8).collect();
    let mut sections: Vec<&[u8]> = vec![le_bytes(&meta)];
    for ld in &wl.by_len {
        sections.extend([
            le_bytes(&ld.flat),
            le_bytes(&ld.scores),
            le_bytes(&ld.compat_blocks),
            le_bytes(&ld.compat_off),
            le_bytes(&ld.pos_count),
        ]);
    }
    sections.extend([
        &rebus[..],
        le_bytes(&c.answers),
        le_bytes(&c.by_len_first),
        le_bytes(&c.by_len_byte),
        le_bytes(&c.off),
        le_bytes(&c.text),
        &pair[..],
        &is_code[..],
    ]);
    let table = sections.len() * 16;
    let mut out = vec![0u8; HEADER + table];
    let mut entries: Vec<(u64, u64)> = Vec::with_capacity(sections.len());
    for s in &sections {
        while !out.len().is_multiple_of(8) {
            out.push(0);
        }
        entries.push((out.len() as u64, s.len() as u64));
        out.extend_from_slice(s);
    }
    while !out.len().is_multiple_of(8) {
        out.push(0);
    }
    for (k, (off, len)) in entries.iter().enumerate() {
        let at = HEADER + k * 16;
        out[at..at + 8].copy_from_slice(&off.to_le_bytes());
        out[at + 8..at + 16].copy_from_slice(&len.to_le_bytes());
    }
    out[0..8].copy_from_slice(MAGIC);
    out[8..12].copy_from_slice(&FORMAT.to_le_bytes());
    out[12..16].copy_from_slice(&BOM.to_le_bytes());
    let total = out.len() as u64;
    out[16..24].copy_from_slice(&total.to_le_bytes());
    out[32..36].copy_from_slice(&(sections.len() as u32).to_le_bytes());
    let sum = checksum(&out[HEADER..]);
    out[24..32].copy_from_slice(&sum.to_le_bytes());
    out
}

/// Write `dict` to `path` atomically (a temporary file renamed over it: a
/// reader never sees a partial file).
pub fn write(dict: &Dictionary, path: &Path) -> std::io::Result<()> {
    let bytes = to_bytes(dict);
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    result
}

/// Why a dictionary file can't be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenError {
    /// The file can't be read (missing, permissions, empty).
    Io(String),
    /// The content isn't a valid dictionary of this format (rebuild it).
    Invalid(String),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Io(m) | OpenError::Invalid(m) => f.write_str(m),
        }
    }
}

impl OpenError {
    /// The message.
    pub fn contains(&self, part: &str) -> bool {
        self.to_string().contains(part)
    }
}

/// Map the dictionary file at `path` and use it in place.
pub fn open(path: &Path) -> Result<Dictionary, OpenError> {
    let map = Mapping::open(path)
        .map_err(|e| OpenError::Io(format!("cannot open {}: {e}", path.display())))?;
    from_mapping(&map).map_err(|e| OpenError::Invalid(format!("{}: {e}", path.display())))
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

fn from_mapping(map: &Arc<Mapping>) -> Result<Dictionary, String> {
    let b = map.bytes();
    if b.len() < HEADER || &b[0..8] != MAGIC {
        return Err("not a dictionary file".into());
    }
    if u32_at(b, 12) != BOM {
        return Err("byte order of another machine".into());
    }
    if u32_at(b, 8) != FORMAT {
        return Err(format!(
            "dictionary format {} (this engine reads {FORMAT}): rebuild it",
            u32_at(b, 8)
        ));
    }
    if u64_at(b, 16) != b.len() as u64 {
        return Err("truncated file".into());
    }
    if u64_at(b, 24) != checksum(&b[HEADER..]) {
        return Err("checksum mismatch (corrupted file)".into());
    }
    let count = u32_at(b, 32) as usize;
    let lengths = MAX_LEN + 1;
    if count != 1 + lengths * 5 + 8 || HEADER + count * 16 > b.len() {
        return Err("unexpected section table".into());
    }
    let mut next = 0usize;
    let mut section = || {
        let at = HEADER + next * 16;
        next += 1;
        (u64_at(b, at) as usize, u64_at(b, at + 8) as usize)
    };
    fn buf<T: Pod>(map: &Arc<Mapping>, (off, len): (usize, usize)) -> Result<Buf<T>, String> {
        Buf::mapped(map, off, len)
    }
    let meta: Buf<u64> = buf(map, section())?;
    if meta.len() != 1 + 2 * lengths {
        return Err("meta".into());
    }
    let n_rebus = meta[0] as usize;
    if n_rebus > ALPHA - REBUS_BASE as usize {
        return Err("rebus count".into());
    }
    let symbols = REBUS_BASE as usize + n_rebus;
    let mut by_len = Vec::with_capacity(lengths);
    for len in 0..lengths {
        if meta[1 + 2 * len] as usize != len {
            return Err("meta".into());
        }
        let ld = LenData::from_parts(
            len,
            buf(map, section())?,
            buf(map, section())?,
            buf(map, section())?,
            buf(map, section())?,
            buf(map, section())?,
            symbols,
        )?;
        if ld.n as u64 != meta[2 + 2 * len] {
            return Err(format!("length {len}: word count"));
        }
        by_len.push(ld);
    }
    let rebus_bytes: Buf<u8> = buf(map, section())?;
    let rebus: Vec<String> = if n_rebus == 0 {
        Vec::new()
    } else {
        std::str::from_utf8(&rebus_bytes)
            .map_err(|_| "rebus strings".to_string())?
            .split('\0')
            .map(str::to_string)
            .collect()
    };
    if rebus.len() != n_rebus {
        return Err("rebus strings".into());
    }
    let answers = buf(map, section())?;
    let by_len_first = buf(map, section())?;
    let by_len_byte = buf(map, section())?;
    let off = buf(map, section())?;
    let text = buf(map, section())?;
    let pair_bytes: Buf<u8> = buf(map, section())?;
    let code_bytes: Buf<u8> = buf(map, section())?;
    if pair_bytes.len() != 512 || code_bytes.len() != 256 || code_bytes.iter().any(|&c| c > 1) {
        return Err("clues: byte-pair table".into());
    }
    let mut pair = [[0u8; 2]; 256];
    let mut is_code = [false; 256];
    for c in 0..256 {
        pair[c] = [pair_bytes[2 * c], pair_bytes[2 * c + 1]];
        is_code[c] = code_bytes[c] == 1;
    }
    let clues = ClueDb::from_parts(answers, by_len_first, by_len_byte, off, text, pair, is_code)?;
    Ok(Dictionary {
        words: Wordlist { by_len, rebus },
        clues,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{cancel_flag, generate, GenerateRequest};
    use std::path::PathBuf;

    fn csv() -> String {
        let alpha = ['A', 'E', 'I', 'O', 'R', 'S', 'T', 'N'];
        let mut words: Vec<String> = alpha.iter().map(|c| c.to_string()).collect();
        let mut out = String::new();
        for len in 2..=5 {
            words = words
                .iter()
                .flat_map(|w| alpha.iter().map(move |c| format!("{w}{c}")))
                .collect();
            for (i, w) in words.iter().enumerate() {
                if len == 5 && i % 3 != 0 {
                    continue;
                }
                out.push_str(&format!("{w},Voce {len}-{i} città\n"));
                if i % 4 == 0 {
                    out.push_str(&format!("{w},Altra voce {len}-{i}\n"));
                }
            }
        }
        out
    }

    fn temp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("xfill-xfd-{}-{name}.xfd", std::process::id()))
    }

    #[test]
    fn a_saved_dictionary_opens_identical_and_in_place() {
        let built = Dictionary::from_csv_bytes(csv().as_bytes()).unwrap();
        let path = temp("same");
        built.save(&path).unwrap();
        let opened = Dictionary::open(&path).unwrap();
        // every array, every clue
        for (a, b) in built.words.by_len.iter().zip(&opened.words.by_len) {
            assert_eq!(
                (a.len, a.n, a.blocks_per_mask),
                (b.len, b.n, b.blocks_per_mask)
            );
            assert_eq!(&a.flat[..], &b.flat[..]);
            assert_eq!(&a.scores[..], &b.scores[..]);
            assert_eq!(&a.compat_blocks[..], &b.compat_blocks[..]);
            assert_eq!(&a.compat_off[..], &b.compat_off[..]);
            assert_eq!(&a.pos_count[..], &b.pos_count[..]);
        }
        assert_eq!(built.clues.len(), opened.clues.len());
        for (answer, n) in built.clues.answers() {
            let a: Vec<String> = built.clues.get(answer).collect();
            let b: Vec<String> = opened.clues.get(answer).collect();
            assert_eq!((a.len(), &a), (n, &b), "{answer}");
        }
        // the same grid, clues included
        let req = GenerateRequest {
            rows: 5,
            cols: 5,
            blocks: Some(2),
            seed: 4,
            threads: 1,
            ..Default::default()
        };
        let (x, y) = (
            generate(&built, &req, &cancel_flag()),
            generate(&opened, &req, &cancel_flag()),
        );
        assert_eq!(
            serde_json::to_string(&x.entries).unwrap(),
            serde_json::to_string(&y.entries).unwrap()
        );
        assert_eq!(x.grid, y.grid);
        // in place: nothing copied
        // (only the small per-length headers are owned)
        assert!(
            opened.heap_bytes() < 16 * 1024,
            "{} bytes copied",
            opened.heap_bytes()
        );
        assert!(opened.mapped_bytes() >= built.heap_bytes() * 9 / 10);
        drop(opened);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn rebus_strings_survive_the_file() {
        let (wl, clues) = crate::clues::dictionary_from_csv(
            b"CREDO,Fede\nREDO,X\nCARE,Y\n",
            0,
            &Default::default(),
            &["RE".to_string()],
        );
        let built = Dictionary { words: wl, clues };
        let path = temp("rebus");
        built.save(&path).unwrap();
        let opened = Dictionary::open(&path).unwrap();
        assert_eq!(opened.words.rebus, vec!["RE".to_string()]);
        let l4 = opened.words.len_data(4);
        let v = l4.index_of(&[2, REBUS_BASE, 3, 14]).expect("C<RE>DO");
        assert_eq!(opened.words.word(4, v), "CREDO");
        std::fs::remove_file(&path).ok();
    }

    /// Rewrite the checksum after editing the body (to reach the checks
    /// behind it).
    fn reseal(b: &mut [u8]) {
        let sum = checksum(&b[HEADER..]);
        b[24..32].copy_from_slice(&sum.to_le_bytes());
    }

    fn section(b: &[u8], k: usize) -> (usize, usize) {
        (
            u64_at(b, HEADER + k * 16) as usize,
            u64_at(b, HEADER + k * 16 + 8) as usize,
        )
    }

    #[test]
    fn damaged_or_foreign_files_are_refused_without_panicking() {
        let built = Dictionary::from_csv_bytes(csv().as_bytes()).unwrap();
        let good = to_bytes(&built);
        let path = temp("bad");
        let refuse = |bytes: &[u8], why: &str| {
            std::fs::write(&path, bytes).unwrap();
            let err = Dictionary::open(&path)
                .err()
                .unwrap_or_else(|| panic!("accepted: {why}"));
            assert!(matches!(err, OpenError::Invalid(_)), "{err:?}");
            assert!(err.contains(why), "{err} (expected {why})");
        };
        let mut b = good.clone();
        b[HEADER + 100] ^= 1;
        refuse(&b, "checksum");
        refuse(&good[..good.len() - 8], "truncated");
        refuse(
            b"hello, this is not a dictionary file at all, really: just text....",
            "not a dictionary",
        );
        let mut b = good.clone();
        b[8..12].copy_from_slice(&(FORMAT + 1).to_le_bytes());
        refuse(&b, "format");
        let mut b = good.clone();
        b[12..16].copy_from_slice(&0x0403_0201u32.to_le_bytes());
        refuse(&b, "byte order");
        // consistent checksum, inconsistent content
        let lengths = MAX_LEN + 1;
        let mut b = good.clone();
        let (off, _) = section(&b, 1 + 3 * 5 + 3); // compat_off of length 3
        b[off..off + 4].copy_from_slice(&u32::MAX.to_le_bytes());
        reseal(&mut b);
        refuse(&b, "mask offsets");
        let mut b = good.clone();
        let (off, _) = section(&b, 1 + 4 * 5); // letters of length 4
        b[off] = 31;
        reseal(&mut b);
        refuse(&b, "letters");
        let mut b = good.clone();
        let pair_k = 1 + lengths * 5 + 6;
        let code_k = pair_k + 1;
        let (pair_off, _) = section(&b, pair_k);
        let (code_off, _) = section(&b, code_k);
        // two unused bytes standing for each other: an endless expansion
        let free: Vec<usize> = (1..255).filter(|&c| b[code_off + c] == 0).take(2).collect();
        let (x, y) = (free[0], free[1]);
        b[code_off + x] = 1;
        b[code_off + y] = 1;
        b[pair_off + 2 * x..pair_off + 2 * x + 2].copy_from_slice(&[y as u8, y as u8]);
        b[pair_off + 2 * y..pair_off + 2 * y + 2].copy_from_slice(&[x as u8, x as u8]);
        reseal(&mut b);
        refuse(&b, "byte-pair codes");
        let mut b = good.clone();
        let (off, len) = section(&b, pair_k - 2); // clue offsets
        b[off + len - 4..off + len].copy_from_slice(&u32::MAX.to_le_bytes());
        reseal(&mut b);
        refuse(&b, "clue offsets");
        std::fs::remove_file(&path).ok();
        let missing = Dictionary::open(&path).err().unwrap();
        assert!(matches!(missing, OpenError::Io(_)) && missing.contains("cannot open"));
    }
}
