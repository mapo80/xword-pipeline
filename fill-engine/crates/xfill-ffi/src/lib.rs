//! C interface of the crossword engine. Requests and results are UTF-8 JSON
//! (the serde form of `xfill_core::api`); strings returned by the library
//! are freed with [`xfill_string_free`], dictionaries with
//! [`xfill_wordlist_free`]. No panic crosses the boundary: every function
//! catches them and returns [`XFILL_ERR_PANIC`].
//!
//! A loaded dictionary is immutable: one handle can serve any number of
//! concurrent [`xfill_generate`] calls from different threads.

#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, c_int, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use xfill_core::api::{self, Dictionary, GenerateRequest, Status, ValidateInput};
use xfill_core::xfd::OpenError;

/// Success.
pub const XFILL_OK: c_int = 0;
/// A required pointer is null or a string is not valid UTF-8.
pub const XFILL_ERR_ARG: c_int = -1;
/// The JSON text is malformed or doesn't match the expected shape.
pub const XFILL_ERR_JSON: c_int = -2;
/// The request is well-formed but its values are invalid (see `err`).
pub const XFILL_ERR_REQUEST: c_int = -3;
/// The engine panicked; the call had no effect.
pub const XFILL_ERR_PANIC: c_int = -4;
/// The generation was cancelled through the cancellation flag.
pub const XFILL_CANCELLED: c_int = -5;
/// No usable word in the CSV, or a dictionary file corrupted or of
/// another format version.
pub const XFILL_ERR_DATA: c_int = -6;
/// A file can't be read or written.
pub const XFILL_ERR_IO: c_int = -7;

/// Opaque handle of a loaded dictionary (words, scores, clues).
pub struct XfillWordlist {
    dict: Dictionary,
}

/// Cancellation flag shared with the caller: any non-zero value cancels.
pub type XfillCancelFlag = i32;

/// Engine version, a static NUL-terminated string (never free it).
#[no_mangle]
pub extern "C" fn xfill_version() -> *const c_char {
    static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION.as_ptr().cast()
}

/// Build a dictionary file at `path` (UTF-8, NUL-terminated) from a
/// `WORD,clue` CSV in memory (UTF-8 or Latin-1). The file is written
/// atomically (never seen half-written) and holds the dictionary ready for
/// [`xfill_dict_open`]. On success, if `stats_json` is not null, writes
/// `{"words","bytes"}` there. Errors: [`XFILL_ERR_DATA`] (no usable word),
/// [`XFILL_ERR_IO`] (the file can't be written), [`XFILL_ERR_ARG`], with a
/// message in `*err` when `err` is not null.
#[no_mangle]
pub unsafe extern "C" fn xfill_dict_build(
    csv: *const u8,
    len: usize,
    path: *const c_char,
    stats_json: *mut *mut c_char,
    err: *mut *mut c_char,
) -> c_int {
    guard(err, || {
        if path.is_null() || (csv.is_null() && len > 0) {
            set_err(err, "null pointer argument");
            return XFILL_ERR_ARG;
        }
        // SAFETY: a NUL-terminated string from the caller
        let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
            set_err(err, "path is not valid UTF-8");
            return XFILL_ERR_ARG;
        };
        let bytes: &[u8] = if len == 0 {
            &[]
        } else {
            // SAFETY: the caller passes `len` readable bytes at `csv`
            unsafe { std::slice::from_raw_parts(csv, len) }
        };
        let dict = match Dictionary::from_csv_bytes(bytes) {
            Ok(d) => d,
            Err(e) => {
                set_err(err, &e);
                return XFILL_ERR_DATA;
            }
        };
        if let Err(e) = dict.save(std::path::Path::new(path)) {
            set_err(err, &format!("cannot write {path}: {e}"));
            return XFILL_ERR_IO;
        }
        if !stats_json.is_null() {
            // SAFETY: checked non-null
            unsafe { *stats_json = to_c_string(stats(&dict)) };
        }
        XFILL_OK
    })
}

/// Open the dictionary file at `path` (written by [`xfill_dict_build`]):
/// it is mapped and used in place — no parsing, no copy. On success writes
/// the handle to `*out`. Errors: [`XFILL_ERR_IO`] (missing or unreadable),
/// [`XFILL_ERR_DATA`] (corrupted, or another format version: rebuild it),
/// [`XFILL_ERR_ARG`], with a message in `*err` when `err` is not null.
#[no_mangle]
pub unsafe extern "C" fn xfill_dict_open(
    path: *const c_char,
    out: *mut *mut XfillWordlist,
    err: *mut *mut c_char,
) -> c_int {
    guard(err, || {
        if path.is_null() || out.is_null() {
            set_err(err, "null pointer argument");
            return XFILL_ERR_ARG;
        }
        // SAFETY: a NUL-terminated string from the caller
        let Ok(path) = unsafe { CStr::from_ptr(path) }.to_str() else {
            set_err(err, "path is not valid UTF-8");
            return XFILL_ERR_ARG;
        };
        match Dictionary::open(std::path::Path::new(path)) {
            Ok(dict) => {
                // SAFETY: `out` is a valid, writable pointer (checked non-null)
                unsafe { *out = Box::into_raw(Box::new(XfillWordlist { dict })) };
                XFILL_OK
            }
            Err(OpenError::Io(e)) => {
                set_err(err, &e);
                XFILL_ERR_IO
            }
            Err(OpenError::Invalid(e)) => {
                set_err(err, &e);
                XFILL_ERR_DATA
            }
        }
    })
}

/// `{"words", "bytes", "mapped"}`: words, size of the dictionary, and the
/// part of it read in place from its file.
fn stats(dict: &Dictionary) -> String {
    serde_json::json!({
        "words": dict.word_count(),
        "bytes": dict.heap_bytes() + dict.mapped_bytes(),
        "mapped": dict.mapped_bytes(),
    })
    .to_string()
}

/// Free a dictionary (null is ignored).
#[no_mangle]
pub unsafe extern "C" fn xfill_wordlist_free(wordlist: *mut XfillWordlist) {
    if !wordlist.is_null() {
        // SAFETY: the handle came from `xfill_dict_open` and is
        // freed once
        guard_void(|| drop(unsafe { Box::from_raw(wordlist) }));
    }
}

/// Statistics of a dictionary as JSON: `{"words": N, "bytes": B}` (bytes =
/// approximate heap memory). Free the string with [`xfill_string_free`].
#[no_mangle]
pub unsafe extern "C" fn xfill_wordlist_stats(
    wordlist: *const XfillWordlist,
    stats_json: *mut *mut c_char,
) -> c_int {
    guard(std::ptr::null_mut(), || {
        if wordlist.is_null() || stats_json.is_null() {
            return XFILL_ERR_ARG;
        }
        // SAFETY: a live handle from `xfill_dict_open`
        let dict = unsafe { &(*wordlist).dict };
        // SAFETY: checked non-null
        unsafe { *stats_json = to_c_string(stats(dict)) };
        XFILL_OK
    })
}

/// Generate a crossword. `request_json` is a `GenerateRequest`; `cancel`
/// (may be null) points to an int the caller sets to non-zero to cancel —
/// it must stay valid until the call returns. Returns [`XFILL_OK`] with
/// the `GenerateResult` in `*result_json` (status `ok` or `no_grid`),
/// [`XFILL_CANCELLED`] or [`XFILL_ERR_REQUEST`] with the result too (status
/// `cancelled` / `invalid_request`), or another error code with a message
/// in `*err` (if `err` is not null).
#[no_mangle]
pub unsafe extern "C" fn xfill_generate(
    wordlist: *const XfillWordlist,
    request_json: *const c_char,
    cancel: *const XfillCancelFlag,
    result_json: *mut *mut c_char,
    err: *mut *mut c_char,
) -> c_int {
    guard(err, || {
        if wordlist.is_null() || request_json.is_null() || result_json.is_null() {
            set_err(err, "null pointer argument");
            return XFILL_ERR_ARG;
        }
        // SAFETY: a NUL-terminated string from the caller
        let Ok(text) = unsafe { CStr::from_ptr(request_json) }.to_str() else {
            set_err(err, "request is not valid UTF-8");
            return XFILL_ERR_ARG;
        };
        let req: GenerateRequest = match serde_json::from_str(text) {
            Ok(r) => r,
            Err(e) => {
                set_err(err, &format!("bad request JSON: {e}"));
                return XFILL_ERR_JSON;
            }
        };
        // SAFETY: a live handle from `xfill_dict_open`
        let dict = unsafe { &(*wordlist).dict };
        let flag = api::cancel_flag();
        let result = with_cancel_watch(cancel, &flag, || api::generate(dict, &req, &flag));
        let rc = match result.status {
            Status::Ok | Status::NoGrid => XFILL_OK,
            Status::Cancelled => XFILL_CANCELLED,
            Status::InvalidRequest => {
                set_err(err, result.error.as_deref().unwrap_or("invalid request"));
                XFILL_ERR_REQUEST
            }
        };
        let json = serde_json::to_string(&result).expect("result serializes");
        // SAFETY: checked non-null
        unsafe { *result_json = to_c_string(json) };
        rc
    })
}

/// Run `f` while a watcher thread copies the caller's int (`cancel`, may
/// be null) into `flag`. The watcher polls every millisecond and ends as
/// soon as `f` returns.
fn with_cancel_watch<T>(
    cancel: *const XfillCancelFlag,
    flag: &api::CancelFlag,
    f: impl FnOnce() -> T,
) -> T {
    use std::sync::atomic::{AtomicI32, Ordering};
    if cancel.is_null() {
        return f();
    }
    // SAFETY: the caller keeps the int alive and aligned for the whole call
    // and only writes it (atomically or volatile) from other threads
    let caller = unsafe { AtomicI32::from_ptr(cancel.cast_mut()) };
    let read = || caller.load(Ordering::SeqCst) != 0;
    if read() {
        flag.store(true, Ordering::SeqCst);
        return f();
    }
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|s| {
        s.spawn(move || loop {
            match done_rx.recv_timeout(std::time::Duration::from_millis(1)) {
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if read() {
                        flag.store(true, Ordering::SeqCst);
                        return;
                    }
                }
                _ => return,
            }
        });
        let out = f();
        drop(done_tx);
        out
    })
}

/// Check a finished grid (`ValidateInput` JSON; a generation result can be
/// passed as is). Returns [`XFILL_OK`] with a `ValidationReport` in
/// `*report_json` — `valid` tells whether the grid passed — or
/// [`XFILL_ERR_JSON`] with a report holding one `bad_json` error, or
/// [`XFILL_ERR_ARG`].
#[no_mangle]
pub unsafe extern "C" fn xfill_validate(
    grid_json: *const c_char,
    report_json: *mut *mut c_char,
) -> c_int {
    guard(std::ptr::null_mut(), || {
        if grid_json.is_null() || report_json.is_null() {
            return XFILL_ERR_ARG;
        }
        // SAFETY: a NUL-terminated string from the caller
        let parsed = unsafe { CStr::from_ptr(grid_json) }
            .to_str()
            .map_err(|e| e.to_string())
            .and_then(|t| serde_json::from_str::<ValidateInput>(t).map_err(|e| e.to_string()));
        let (rc, report) = match parsed {
            Ok(input) => (XFILL_OK, api::validate(&input)),
            Err(e) => (
                XFILL_ERR_JSON,
                api::ValidationReport {
                    valid: false,
                    errors: vec![api::Issue {
                        code: "bad_json".into(),
                        message: e,
                        number: None,
                        direction: None,
                        row: None,
                        col: None,
                    }],
                    warnings: Vec::new(),
                    slots: 0,
                },
            ),
        };
        let json = serde_json::to_string(&report).expect("report serializes");
        // SAFETY: checked non-null
        unsafe { *report_json = to_c_string(json) };
        rc
    })
}

/// Free a string returned by the library (null is ignored).
#[no_mangle]
pub unsafe extern "C" fn xfill_string_free(s: *mut c_char) {
    if !s.is_null() {
        // SAFETY: the string came from `to_c_string` and is freed once
        guard_void(|| drop(unsafe { CString::from_raw(s) }));
    }
}

/// Run `f`, turning a panic into [`XFILL_ERR_PANIC`] (message in `*err`
/// when `err` is not null).
fn guard(err: *mut *mut c_char, f: impl FnOnce() -> c_int) -> c_int {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(rc) => rc,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string());
            set_err(err, &format!("internal error (panic): {msg}"));
            XFILL_ERR_PANIC
        }
    }
}

/// Run `f`, swallowing a panic (for functions without a return code).
fn guard_void(f: impl FnOnce()) {
    let _ = catch_unwind(AssertUnwindSafe(f));
}

/// Write a message to `*err` when `err` is not null.
fn set_err(err: *mut *mut c_char, msg: &str) {
    if !err.is_null() {
        // SAFETY: the caller passes a writable pointer or null
        unsafe { *err = to_c_string(msg.to_string()) };
    }
}

/// A C string owned by the caller (interior NULs become spaces).
fn to_c_string(s: String) -> *mut c_char {
    let s = if s.contains('\0') {
        s.replace('\0', " ")
    } else {
        s
    };
    CString::new(s).expect("no interior NUL").into_raw()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::{null, null_mut};
    use std::sync::atomic::{AtomicI32, Ordering};
    use std::time::{Duration, Instant};

    fn csv() -> Vec<u8> {
        let alpha = ['A', 'E', 'I', 'O', 'R', 'S', 'T', 'N'];
        let mut words: Vec<String> = alpha.iter().map(|c| c.to_string()).collect();
        let mut out = String::new();
        for len in 2..=4 {
            words = words
                .iter()
                .flat_map(|w| alpha.iter().map(move |c| format!("{w}{c}")))
                .collect();
            for (i, w) in words.iter().enumerate() {
                out.push_str(&format!("{w},Voce {len}-{i}\n"));
            }
        }
        out.into_bytes()
    }

    fn temp(name: &str) -> CString {
        let p = std::env::temp_dir().join(format!("xfill-ffi-{}-{name}.xfd", std::process::id()));
        CString::new(p.to_str().unwrap()).unwrap()
    }

    /// Build the test dictionary file and open it.
    fn load() -> *mut XfillWordlist {
        let bytes = csv();
        let path = temp(&format!("{:?}", std::thread::current().id()));
        let mut stats = null_mut();
        let rc = unsafe {
            xfill_dict_build(
                bytes.as_ptr(),
                bytes.len(),
                path.as_ptr(),
                &mut stats,
                null_mut(),
            )
        };
        assert_eq!(rc, XFILL_OK);
        assert!(take(stats).contains("\"words\""));
        let mut wl = null_mut();
        let rc = unsafe { xfill_dict_open(path.as_ptr(), &mut wl, null_mut()) };
        assert_eq!(rc, XFILL_OK);
        assert!(!wl.is_null());
        // the open dictionary keeps the mapping: the file can go
        std::fs::remove_file(path.to_str().unwrap()).ok();
        wl
    }

    /// Take ownership of a library string.
    fn take(s: *mut c_char) -> String {
        assert!(!s.is_null());
        let text = unsafe { CStr::from_ptr(s) }.to_str().unwrap().to_string();
        unsafe { xfill_string_free(s) };
        text
    }

    fn generate(
        wl: *const XfillWordlist,
        request: &str,
        cancel: *const XfillCancelFlag,
    ) -> (c_int, Option<serde_json::Value>, Option<String>) {
        let req = CString::new(request).unwrap();
        let (mut out, mut err) = (null_mut(), null_mut());
        let rc = unsafe { xfill_generate(wl, req.as_ptr(), cancel, &mut out, &mut err) };
        let out = (!out.is_null()).then(|| serde_json::from_str(&take(out)).unwrap());
        let err = (!err.is_null()).then(|| take(err));
        (rc, out, err)
    }

    #[test]
    fn version_is_the_engine_version() {
        let v = unsafe { CStr::from_ptr(xfill_version()) };
        assert_eq!(v.to_str().unwrap(), api::ENGINE_VERSION);
    }

    #[test]
    fn load_stats_and_free() {
        let wl = load();
        let mut stats = null_mut();
        assert_eq!(unsafe { xfill_wordlist_stats(wl, &mut stats) }, XFILL_OK);
        let v: serde_json::Value = serde_json::from_str(&take(stats)).unwrap();
        assert_eq!(v["words"], 8 * 8 + 8 * 8 * 8 + 8 * 8 * 8 * 8);
        assert!(v["bytes"].as_u64().unwrap() > 10_000);
        // read in place from the file
        assert!(v["mapped"].as_u64().unwrap() * 10 > v["bytes"].as_u64().unwrap() * 9);
        assert_eq!(
            unsafe { xfill_wordlist_stats(null(), &mut stats) },
            XFILL_ERR_ARG
        );
        assert_eq!(
            unsafe { xfill_wordlist_stats(wl, null_mut()) },
            XFILL_ERR_ARG
        );
        unsafe { xfill_wordlist_free(wl) };
        unsafe { xfill_wordlist_free(null_mut()) };
        unsafe { xfill_string_free(null_mut()) };
    }

    #[test]
    fn build_and_open_errors() {
        let mut wl = null_mut();
        let mut err = null_mut();
        let path = temp("errors");
        let empty = b"# nothing\n";
        let build = |csv: *const u8, len: usize, path: *const c_char, err: *mut *mut c_char| unsafe {
            xfill_dict_build(csv, len, path, null_mut(), err)
        };
        assert_eq!(
            build(empty.as_ptr(), empty.len(), path.as_ptr(), &mut err),
            XFILL_ERR_DATA
        );
        assert!(take(err).contains("CSV"));
        // zero-length input with a null pointer is an empty CSV
        assert_eq!(build(null(), 0, path.as_ptr(), null_mut()), XFILL_ERR_DATA);
        assert_eq!(build(null(), 3, path.as_ptr(), &mut err), XFILL_ERR_ARG);
        assert!(!take(err).is_empty());
        assert_eq!(
            build(empty.as_ptr(), empty.len(), null(), null_mut()),
            XFILL_ERR_ARG
        );
        let bad_utf8 = [0xFFu8, 0];
        assert_eq!(
            build(
                empty.as_ptr(),
                empty.len(),
                bad_utf8.as_ptr().cast(),
                &mut err
            ),
            XFILL_ERR_ARG
        );
        assert!(take(err).contains("UTF-8"));
        let csv = csv();
        let no_dir = CString::new("/no/such/dir/dict.xfd").unwrap();
        assert_eq!(
            build(csv.as_ptr(), csv.len(), no_dir.as_ptr(), &mut err),
            XFILL_ERR_IO
        );
        assert!(take(err).contains("cannot write"));
        // open: missing, not a dictionary, bad arguments
        assert_eq!(
            unsafe { xfill_dict_open(no_dir.as_ptr(), &mut wl, &mut err) },
            XFILL_ERR_IO
        );
        assert!(take(err).contains("cannot open"));
        std::fs::write(
            path.to_str().unwrap(),
            b"not a dictionary file, just some text here....................",
        )
        .unwrap();
        assert_eq!(
            unsafe { xfill_dict_open(path.as_ptr(), &mut wl, &mut err) },
            XFILL_ERR_DATA
        );
        assert!(take(err).contains("not a dictionary"));
        assert!(wl.is_null());
        assert_eq!(
            unsafe { xfill_dict_open(path.as_ptr(), null_mut(), null_mut()) },
            XFILL_ERR_ARG
        );
        assert_eq!(
            unsafe { xfill_dict_open(null(), &mut wl, null_mut()) },
            XFILL_ERR_ARG
        );
        assert_eq!(
            unsafe { xfill_dict_open(bad_utf8.as_ptr().cast(), &mut wl, &mut err) },
            XFILL_ERR_ARG
        );
        take(err);
        std::fs::remove_file(path.to_str().unwrap()).ok();
    }

    #[test]
    fn generates_and_validates_a_rectangular_grid() {
        let wl = load();
        let (rc, out, err) = generate(
            wl,
            r#"{"rows": 5, "cols": 8, "blocks": 8, "candidates": 10, "node_budget": 3000,
                "theme_words": ["arte", "sera", "rete"]}"#,
            null(),
        );
        assert_eq!(rc, XFILL_OK, "{err:?}");
        let r = out.unwrap();
        assert_eq!(r["status"], "ok");
        assert_eq!((r["rows"].clone(), r["cols"].clone()), (5.into(), 8.into()));
        assert_eq!(r["grid"].as_array().unwrap().len(), 5);
        assert_eq!(r["theme_in_dictionary"], 3);
        // the result is a valid validation input
        let json = CString::new(r.to_string()).unwrap();
        let mut report = null_mut();
        assert_eq!(
            unsafe { xfill_validate(json.as_ptr(), &mut report) },
            XFILL_OK
        );
        let report: serde_json::Value = serde_json::from_str(&take(report)).unwrap();
        assert_eq!(report["valid"], true, "{report}");
        unsafe { xfill_wordlist_free(wl) };
    }

    #[test]
    fn generate_error_codes() {
        let wl = load();
        let (rc, out, err) = generate(wl, "{not json", null());
        assert_eq!(rc, XFILL_ERR_JSON);
        assert!(out.is_none() && err.is_some());
        let (rc, out, err) = generate(wl, r#"{"rows": 1}"#, null());
        assert_eq!(rc, XFILL_ERR_REQUEST);
        assert_eq!(out.unwrap()["status"], "invalid_request");
        assert!(err.unwrap().contains("rows"));
        let (rc, out, _) = generate(
            wl,
            r#"{"rows": 9, "cols": 9, "blocks": 0, "candidates": 3}"#,
            null(),
        );
        assert_eq!(rc, XFILL_OK);
        assert_eq!(out.unwrap()["status"], "no_grid");
        // null pointers
        let req = CString::new("{}").unwrap();
        let mut out = null_mut();
        let mut err = null_mut();
        let call = |wl, req, out| unsafe { xfill_generate(wl, req, null(), out, null_mut()) };
        assert_eq!(call(null(), req.as_ptr(), &mut out), XFILL_ERR_ARG);
        assert_eq!(call(wl, null(), &mut out), XFILL_ERR_ARG);
        assert_eq!(call(wl, req.as_ptr(), null_mut()), XFILL_ERR_ARG);
        // invalid UTF-8
        let bad = [0xffu8, 0xfe, 0];
        let rc = unsafe { xfill_generate(wl, bad.as_ptr().cast(), null(), &mut out, &mut err) };
        assert_eq!(rc, XFILL_ERR_ARG);
        assert!(take(err).contains("UTF-8"));
        unsafe { xfill_wordlist_free(wl) };
    }

    /// The caller's int cancels a running generation quickly; a flag set
    /// before the call cancels at once.
    #[test]
    fn cancellation_through_the_callers_int() {
        let wl = load();
        static FLAG: AtomicI32 = AtomicI32::new(0);
        let setter = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(150));
            FLAG.store(1, Ordering::SeqCst);
            Instant::now()
        });
        let (rc, out, _) = generate(
            wl,
            r#"{"rows": 9, "cols": 9, "node_budget": 0, "time_per_grid_s": 30,
                "candidates": 40, "pool": 40, "threads": 2}"#,
            FLAG.as_ptr(),
        );
        let done = Instant::now();
        let set = setter.join().unwrap();
        assert_eq!(rc, XFILL_CANCELLED);
        assert_eq!(out.unwrap()["status"], "cancelled");
        assert!(
            done.duration_since(set) < Duration::from_millis(500),
            "stopped {:?} after the flag",
            done.duration_since(set)
        );
        let (rc, _, _) = generate(wl, "{}", FLAG.as_ptr());
        assert_eq!(rc, XFILL_CANCELLED);
        unsafe { xfill_wordlist_free(wl) };
    }

    /// One dictionary serves concurrent generations.
    #[test]
    fn dictionary_is_shared_between_threads() {
        struct Handle(*mut XfillWordlist);
        unsafe impl Send for Handle {}
        unsafe impl Sync for Handle {}
        let wl = Handle(load());
        let results: Vec<String> = std::thread::scope(|s| {
            let hs: Vec<_> = (0..4)
                .map(|_| {
                    let wl = &wl;
                    s.spawn(move || {
                        let (rc, out, _) =
                            generate(wl.0, r#"{"rows": 5, "cols": 5, "blocks": 2, "seed": 3, "candidates": 6, "node_budget": 2000, "threads": 1}"#, null());
                        assert_eq!(rc, XFILL_OK);
                        out.unwrap()["grid"].to_string()
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        assert!(
            results.windows(2).all(|w| w[0] == w[1]),
            "same seed, same grid"
        );
        unsafe { xfill_wordlist_free(wl.0) };
    }

    #[test]
    fn validate_reports_and_errors() {
        let grid = r#"{"rows": 1, "cols": 3, "grid": ["SOL"],
            "entries": [{"number": 1, "direction": "across", "row": 0, "col": 0,
                         "length": 3, "answer": "SOL", "clue": "Astro"}]}"#;
        let json = CString::new(grid).unwrap();
        let mut report = null_mut();
        assert_eq!(
            unsafe { xfill_validate(json.as_ptr(), &mut report) },
            XFILL_OK
        );
        let r: serde_json::Value = serde_json::from_str(&take(report)).unwrap();
        assert_eq!(r["valid"], true);
        let bad = CString::new("[1, 2").unwrap();
        assert_eq!(
            unsafe { xfill_validate(bad.as_ptr(), &mut report) },
            XFILL_ERR_JSON
        );
        let r: serde_json::Value = serde_json::from_str(&take(report)).unwrap();
        assert_eq!(r["valid"], false);
        assert_eq!(r["errors"][0]["code"], "bad_json");
        assert_eq!(
            unsafe { xfill_validate(null(), &mut report) },
            XFILL_ERR_ARG
        );
        assert_eq!(
            unsafe { xfill_validate(json.as_ptr(), null_mut()) },
            XFILL_ERR_ARG
        );
    }

    /// A panic inside a call becomes an error code.
    #[test]
    fn panics_become_error_codes() {
        let mut err = null_mut();
        let rc = guard(&mut err, || panic!("boom"));
        assert_eq!(rc, XFILL_ERR_PANIC);
        assert!(take(err).contains("boom"));
        let rc = guard(null_mut(), || panic!("{}", String::from("owned")));
        assert_eq!(rc, XFILL_ERR_PANIC);
        let rc = guard(&mut err, || std::panic::panic_any(7u8));
        assert_eq!(rc, XFILL_ERR_PANIC);
        assert!(take(err).contains("panic"));
        assert_eq!(guard(null_mut(), || XFILL_OK), XFILL_OK);
        assert_eq!(guard_void(|| panic!("in free")), ());
    }

    #[test]
    fn messages_with_nul_bytes_are_kept() {
        let s = to_c_string("a\0b".to_string());
        assert_eq!(take(s), "a b");
    }
}
