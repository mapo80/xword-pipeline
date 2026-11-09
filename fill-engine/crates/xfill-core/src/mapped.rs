//! Read-only file mappings, and buffers that either own their elements or
//! point into a mapping. A dictionary opened from a file (see [`crate::xfd`])
//! uses its arrays in place: no parsing and no copy, and the pages are
//! file-backed — the system can drop them under memory pressure and
//! processes mapping the same file share them.

use std::fs::File;
use std::io;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

/// A whole file mapped read-only; unmapped when the last user goes.
pub struct Mapping {
    ptr: *const u8,
    len: usize,
    #[cfg(windows)]
    map: *mut std::ffi::c_void,
}

// SAFETY: the mapping is read-only and never changes while it lives
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    /// Map `path` read-only (an empty file is an error).
    pub fn open(path: &Path) -> io::Result<Arc<Mapping>> {
        let file = File::open(path)?;
        let len = usize::try_from(file.metadata()?.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "file too large"))?;
        if len == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "empty file"));
        }
        sys::map(&file, len).map(Arc::new)
    }

    /// The mapped bytes.
    pub fn bytes(&self) -> &[u8] {
        // SAFETY: `ptr` maps `len` readable bytes for the mapping's lifetime
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        sys::unmap(self);
    }
}

#[cfg(unix)]
mod sys {
    use super::Mapping;
    use std::ffi::{c_int, c_void};
    use std::io;
    use std::os::fd::AsRawFd;

    const PROT_READ: c_int = 1;
    const MAP_PRIVATE: c_int = 2;

    extern "C" {
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: c_int,
            flags: c_int,
            fd: c_int,
            off: i64,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> c_int;
    }

    pub fn map(file: &std::fs::File, len: usize) -> io::Result<Mapping> {
        // SAFETY: a fresh read-only private mapping of an open file
        let p = unsafe {
            mmap(
                std::ptr::null_mut(),
                len,
                PROT_READ,
                MAP_PRIVATE,
                file.as_raw_fd(),
                0,
            )
        };
        if p as isize == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(Mapping {
            ptr: p as *const u8,
            len,
        })
    }

    pub fn unmap(m: &mut Mapping) {
        // SAFETY: unmapping what `map` mapped, once
        unsafe { munmap(m.ptr as *mut c_void, m.len) };
    }
}

#[cfg(windows)]
mod sys {
    use super::Mapping;
    use std::ffi::c_void;
    use std::io;
    use std::os::windows::io::AsRawHandle;

    const PAGE_READONLY: u32 = 0x02;
    const FILE_MAP_READ: u32 = 0x04;

    extern "system" {
        fn CreateFileMappingW(
            file: *mut c_void,
            attributes: *mut c_void,
            protect: u32,
            size_high: u32,
            size_low: u32,
            name: *const u16,
        ) -> *mut c_void;
        fn MapViewOfFile(
            map: *mut c_void,
            access: u32,
            off_high: u32,
            off_low: u32,
            len: usize,
        ) -> *mut c_void;
        fn UnmapViewOfFile(base: *const c_void) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    pub fn map(file: &std::fs::File, len: usize) -> io::Result<Mapping> {
        // SAFETY: Win32 calls on an open file handle; failures are checked
        unsafe {
            let map = CreateFileMappingW(
                file.as_raw_handle() as *mut c_void,
                std::ptr::null_mut(),
                PAGE_READONLY,
                0,
                0,
                std::ptr::null(),
            );
            if map.is_null() {
                return Err(io::Error::last_os_error());
            }
            let view = MapViewOfFile(map, FILE_MAP_READ, 0, 0, 0);
            if view.is_null() {
                let e = io::Error::last_os_error();
                CloseHandle(map);
                return Err(e);
            }
            Ok(Mapping {
                ptr: view as *const u8,
                len,
                map,
            })
        }
    }

    pub fn unmap(m: &mut Mapping) {
        // SAFETY: releasing what `map` created, once
        unsafe {
            UnmapViewOfFile(m.ptr as *const c_void);
            CloseHandle(m.map);
        }
    }
}

/// Elements that can be read in place from a mapping (plain little-endian
/// integers, any bit pattern valid).
pub trait Pod: Copy + 'static {}
impl Pod for u8 {}
impl Pod for u32 {}
impl Pod for u64 {}

/// An immutable array: owned, or inside a [`Mapping`] it keeps alive.
pub struct Buf<T: Pod> {
    ptr: *const T,
    len: usize,
    owner: Owner<T>,
}

enum Owner<T> {
    Vec(Vec<T>),
    /// held only to keep the mapping alive
    #[allow(dead_code)]
    Map(Arc<Mapping>),
}

// SAFETY: read-only after construction; the owner keeps the memory alive
unsafe impl<T: Pod + Send> Send for Buf<T> {}
unsafe impl<T: Pod + Sync> Sync for Buf<T> {}

impl<T: Pod> Buf<T> {
    /// The elements `[offset, offset + bytes)` of `map` (offset aligned for
    /// `T`, length a multiple of its size).
    pub fn mapped(map: &Arc<Mapping>, offset: usize, bytes: usize) -> Result<Buf<T>, String> {
        let size = std::mem::size_of::<T>();
        let all = map.bytes();
        if offset.checked_add(bytes).is_none_or(|end| end > all.len()) {
            return Err("section outside the file".into());
        }
        if !bytes.is_multiple_of(size) || !offset.is_multiple_of(std::mem::align_of::<T>()) {
            return Err("misaligned section".into());
        }
        Ok(Buf {
            // SAFETY: in bounds and aligned (checked above)
            ptr: unsafe { all.as_ptr().add(offset) } as *const T,
            len: bytes / size,
            owner: Owner::Map(Arc::clone(map)),
        })
    }

    /// Heap memory owned by this buffer (0 when mapped).
    pub fn heap_bytes(&self) -> usize {
        match &self.owner {
            Owner::Vec(v) => v.capacity() * std::mem::size_of::<T>(),
            Owner::Map(_) => 0,
        }
    }

    /// Bytes read in place from a mapping (0 when owned).
    pub fn mapped_bytes(&self) -> usize {
        match &self.owner {
            Owner::Vec(_) => 0,
            Owner::Map(_) => self.len * std::mem::size_of::<T>(),
        }
    }
}

impl<T: Pod> From<Vec<T>> for Buf<T> {
    fn from(v: Vec<T>) -> Buf<T> {
        Buf {
            ptr: v.as_ptr(),
            len: v.len(),
            owner: Owner::Vec(v),
        }
    }
}

impl<T: Pod> Default for Buf<T> {
    fn default() -> Buf<T> {
        Vec::new().into()
    }
}

impl<T: Pod> Deref for Buf<T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        // SAFETY: `ptr`/`len` describe memory owned by `owner` (a Vec never
        // mutated after construction, or a live mapping)
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl<T: Pod + std::fmt::Debug> std::fmt::Debug for Buf<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mapped_file_is_read_in_place() {
        let path = std::env::temp_dir().join(format!("xfill-map-{}.bin", std::process::id()));
        let mut data = vec![0u8; 24];
        data[8..16].copy_from_slice(&0x0102_0304_0506_0708u64.to_le_bytes());
        data[16..20].copy_from_slice(&7u32.to_le_bytes());
        std::fs::write(&path, &data).unwrap();
        let map = Mapping::open(&path).unwrap();
        let words: Buf<u64> = Buf::mapped(&map, 8, 8).unwrap();
        assert_eq!(&words[..], &[0x0102_0304_0506_0708]);
        assert_eq!(words.mapped_bytes(), 8);
        assert_eq!(words.heap_bytes(), 0);
        let small: Buf<u32> = Buf::mapped(&map, 16, 4).unwrap();
        assert_eq!(small[0], 7);
        assert!(Buf::<u64>::mapped(&map, 4, 8).is_err(), "misaligned");
        assert!(Buf::<u8>::mapped(&map, 20, 8).is_err(), "past the end");
        assert!(Buf::<u32>::mapped(&map, 16, 3).is_err(), "partial element");
        drop(map); // the buffers keep the mapping alive
        assert_eq!(small[0], 7);
        drop((words, small));
        std::fs::write(&path, b"").unwrap();
        assert!(Mapping::open(&path).is_err(), "empty");
        std::fs::remove_file(&path).ok();
        assert!(Mapping::open(&path).is_err(), "missing");
        let owned: Buf<u32> = vec![1, 2, 3].into();
        assert_eq!(owned.len(), 3);
        assert_eq!(owned.heap_bytes(), 12);
        assert_eq!(owned.mapped_bytes(), 0);
        assert!(Buf::<u8>::default().is_empty());
        assert_eq!(format!("{owned:?}"), "[1, 2, 3]");
    }
}
