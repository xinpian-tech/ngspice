//! C ABI for the ngspice glue.
//!
//! The contract is deliberately narrow: hand ngspice the same thing its own
//! frontend would have produced by the time it reaches `if_inpdeck` — a flat,
//! fully-resolved list of card texts, title first — and let it get on with
//! `INPpas1/2/3`. Everything downstream of expansion (`eval_agauss`,
//! `ENHtranslate_poly`, `inp_dodeck`) stays ngspice's job.
//!
//! Ownership: every pointer handed out belongs to the [`NgpDeck`] it came from and
//! stays valid until [`ngparse_deck_free`]. Nothing is copied out; the C side must
//! not free or mutate any of it.
//!
//! ```c
//! NgpDeck *d = ngparse_expand_file("tb_driver.net", 1, 0);
//! if (!d) { fprintf(stderr, "%s\n", ngparse_last_error()); return 1; }
//! for (size_t i = 0; i < ngparse_deck_len(d); i++)
//!     puts(ngparse_deck_card(d, i));            /* card 0 is the title */
//! ngparse_deck_free(d);
//! ```
//!
//! Panics never cross the boundary (that would be UB): every entry point traps
//! them and reports through [`ngparse_last_error`].

use std::cell::RefCell;
use std::ffi::{c_char, c_int, CStr, CString};
use std::num::NonZeroUsize;
use std::path::Path;

use crate::config::Config;
use crate::subckt::SubcktExpander;
use crate::Expander;

/// An expanded deck: the card texts, title first, plus the drops the expansion
/// could not resolve. Opaque to C.
pub struct NgpDeck {
    /// NUL-terminated card texts, kept alive for the deck's lifetime.
    cards: Vec<CString>,
    /// Parameters that could not be resolved (see `Expanded::drops`).
    drops: Vec<CString>,
}

thread_local! {
    static LAST_ERROR: RefCell<Option<CString>> = const { RefCell::new(None) };
}

fn set_error(msg: impl Into<Vec<u8>>) {
    let c = CString::new(msg).unwrap_or_else(|_| CString::new("error").unwrap());
    LAST_ERROR.with(|e| *e.borrow_mut() = Some(c));
}

/// Run `f`, converting a panic into a `NULL`/error return. A panic unwinding into
/// C is undefined behavior, so this must wrap every entry point.
fn guard<T>(default: T, f: impl FnOnce() -> T + std::panic::UnwindSafe) -> T {
    match std::panic::catch_unwind(f) {
        Ok(v) => v,
        Err(_) => {
            set_error("ngparse: internal panic (this is a bug; please report the deck)");
            default
        }
    }
}

/// The last error on this thread, or `NULL` if none. Owned by ngparse; valid until
/// the next failing call on this thread.
///
/// # Safety
/// The returned pointer must not be freed or retained across further ngparse calls.
#[no_mangle]
pub extern "C" fn ngparse_last_error() -> *const c_char {
    LAST_ERROR.with(|e| match &*e.borrow() {
        Some(c) => c.as_ptr(),
        None => std::ptr::null(),
    })
}

/// Expand `path` into a flat, resolved card list.
///
/// `cores` is the worker-core budget; **pass 1**. Values above 1 are accepted for
/// forward compatibility but are not yet honored — the parser is sequential (see
/// `Config::effective_cores`). 0 is treated as 1.
///
/// `compat` selects the input dialect: `1` = PSpice (`ngbehavior=ps`), anything
/// else = default/HSPICE. The glue reads ngspice's `ngbehavior` via `cp_getvar`
/// and passes it here so PSpice decks get the same conversions the reference
/// applies (see [`crate::config::Compat`]).
///
/// Returns `NULL` on failure, with the reason in [`ngparse_last_error`]. The result
/// must be released with [`ngparse_deck_free`].
///
/// # Safety
/// `path` must be a valid NUL-terminated C string.
#[no_mangle]
pub unsafe extern "C" fn ngparse_expand_file(
    path: *const c_char,
    cores: c_int,
    compat: c_int,
) -> *mut NgpDeck {
    guard(std::ptr::null_mut(), || {
        if path.is_null() {
            set_error("ngparse_expand_file: path is NULL");
            return std::ptr::null_mut();
        }
        let path = match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(p) => p,
            Err(_) => {
                set_error("ngparse_expand_file: path is not valid UTF-8");
                return std::ptr::null_mut();
            }
        };
        let cfg = Config::with_cores(
            NonZeroUsize::new(cores.max(1) as usize).unwrap_or(NonZeroUsize::MIN),
        )
        .with_compat(crate::config::Compat::from_ffi(compat))
        // Dangling-passive removal for the integrated build, without an ABI
        // change: NGPARSE_TOPO_REDUCE=1 in the environment turns it on.
        .with_topo_reduce(
            std::env::var("NGPARSE_TOPO_REDUCE").map(|v| v != "0" && !v.is_empty()).unwrap_or(false),
        );

        let mut ex = Expander::with_config(cfg);
        let flat = match ex.expand_file(Path::new(path)) {
            Ok(f) => f,
            Err(e) => {
                set_error(format!("ngparse: {path}: {e}"));
                return std::ptr::null_mut();
            }
        };
        let expanded = SubcktExpander::with_config(&flat, cfg).expand();

        // Card 0 is the TITLE. SPICE consumes the first card of a deck as the
        // title and starts the netlist at card 1 — `if_inpdeck` walks straight
        // into INPpas1 from card 0 with no title skip of its own, so the title has
        // to be here or the first real card is silently eaten.
        let mut cards = Vec::with_capacity(expanded.cards.len() + 1);
        let title = ex.title();
        let title = if title.trim().is_empty() { "*" } else { title };
        let mut push = |s: &str| {
            // A NUL cannot appear in a SPICE deck; if one somehow does, cut there
            // rather than fail the whole load.
            cards.push(CString::new(s).unwrap_or_else(|e| {
                let n = e.nul_position();
                CString::new(&e.into_vec()[..n]).unwrap()
            }));
        };
        push(title);
        for c in &expanded.cards {
            push(c);
        }
        let drops = expanded
            .drops
            .iter()
            .filter_map(|d| CString::new(d.as_str()).ok())
            .collect();

        Box::into_raw(Box::new(NgpDeck { cards, drops }))
    })
}

/// Number of cards, including the title at index 0.
///
/// # Safety
/// `d` must be a live deck from [`ngparse_expand_file`], or NULL.
#[no_mangle]
pub unsafe extern "C" fn ngparse_deck_len(d: *const NgpDeck) -> usize {
    if d.is_null() {
        return 0;
    }
    unsafe { &*d }.cards.len()
}

/// Card `i` as a NUL-terminated string, or `NULL` if out of range. Index 0 is the
/// title. Borrowed from the deck — do not free.
///
/// # Safety
/// `d` must be a live deck from [`ngparse_expand_file`], or NULL.
#[no_mangle]
pub unsafe extern "C" fn ngparse_deck_card(d: *const NgpDeck, i: usize) -> *const c_char {
    if d.is_null() {
        return std::ptr::null();
    }
    match unsafe { &*d }.cards.get(i) {
        Some(c) => c.as_ptr(),
        None => std::ptr::null(),
    }
}

/// Number of parameters that could not be resolved.
///
/// A drop is never harmless: it means a device or model silently fell back to a
/// DEFAULT, which yields a wrong-but-converging answer. The caller should surface
/// these (and may refuse the deck, as the CLI's `--strict` does).
///
/// # Safety
/// `d` must be a live deck from [`ngparse_expand_file`], or NULL.
#[no_mangle]
pub unsafe extern "C" fn ngparse_deck_drop_count(d: *const NgpDeck) -> usize {
    if d.is_null() {
        return 0;
    }
    unsafe { &*d }.drops.len()
}

/// Drop `i` as a human-readable message, or `NULL` if out of range. Borrowed.
///
/// # Safety
/// `d` must be a live deck from [`ngparse_expand_file`], or NULL.
#[no_mangle]
pub unsafe extern "C" fn ngparse_deck_drop(d: *const NgpDeck, i: usize) -> *const c_char {
    if d.is_null() {
        return std::ptr::null();
    }
    match unsafe { &*d }.drops.get(i) {
        Some(c) => c.as_ptr(),
        None => std::ptr::null(),
    }
}

/// Release a deck. Safe to call with NULL. All pointers previously handed out for
/// this deck become dangling.
///
/// # Safety
/// `d` must be a deck from [`ngparse_expand_file`] that has not already been
/// freed, or NULL.
#[no_mangle]
pub unsafe extern "C" fn ngparse_deck_free(d: *mut NgpDeck) {
    if d.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(d) });
}

/// ngparse's version string, for banners and bug reports. Static; never freed.
#[no_mangle]
pub extern "C" fn ngparse_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr() as *const c_char
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn deck(body: &str, tag: &str) -> String {
        let p = std::env::temp_dir().join(format!("ngparse_ffi_{}_{tag}.cir", std::process::id()));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p.to_string_lossy().into_owned()
    }

    fn card(d: *const NgpDeck, i: usize) -> String {
        let p = unsafe { ngparse_deck_card(d, i) };
        assert!(!p.is_null(), "card {i} missing");
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }

    #[test]
    fn expands_a_deck_title_first() {
        let p = deck("* my title\n.param r=2k\nr1 n1 0 {r}\nv1 n1 0 1\n.end\n", "ok");
        let cp = CString::new(p).unwrap();
        let d = unsafe { ngparse_expand_file(cp.as_ptr(), 1, 0) };
        assert!(!d.is_null(), "expand failed");
        // index 0 is the title; SPICE eats it and starts the netlist at 1
        assert_eq!(card(d, 0), "* my title");
        let all: Vec<String> = (0..unsafe { ngparse_deck_len(d) }).map(|i| card(d, i)).collect();
        // top-level `.param` cards are kept deliberately, so `.control` blocks can
        // still reference them; the device value is resolved regardless
        assert!(all.iter().any(|c| c.starts_with(".param")), "{all:?}");
        assert!(
            all.iter().any(|c| c.starts_with("r1 n1 0 2")),
            "resolved r1 not found: {all:?}"
        );
        assert_eq!(unsafe { ngparse_deck_drop_count(d) }, 0);
        assert!(unsafe { ngparse_deck_card(d, 999) }.is_null());
        unsafe { ngparse_deck_free(d) };
    }

    /// A deck whose first line is a real card: SPICE still consumes it as the
    /// title, so ngparse must emit one and not lose the card.
    #[test]
    fn bare_title_is_not_a_card() {
        let p = deck("check scoping\nr1 n1 0 1k\nv1 n1 0 1\n.end\n", "bare");
        let cp = CString::new(p).unwrap();
        let d = unsafe { ngparse_expand_file(cp.as_ptr(), 1, 0) };
        assert!(!d.is_null());
        assert_eq!(card(d, 0), "check scoping");
        assert!(card(d, 1).starts_with("r1"));
        unsafe { ngparse_deck_free(d) };
    }

    #[test]
    fn reports_drops() {
        // An EXPRESSION referencing an undefined param drops (a bare identifier is
        // instead kept verbatim -- see is_bare_word). `1+nosuch` can't resolve.
        let p = deck("* t\n.model dm d (is='1.0+nosuch')\nd1 n1 0 dm\nv1 n1 0 1\n.end\n", "drop");
        let cp = CString::new(p).unwrap();
        let d = unsafe { ngparse_expand_file(cp.as_ptr(), 1, 0) };
        assert!(!d.is_null());
        assert!(unsafe { ngparse_deck_drop_count(d) } > 0, "drop not reported");
        assert!(!unsafe { ngparse_deck_drop(d, 0) }.is_null());
        assert!(unsafe { ngparse_deck_drop(d, 999) }.is_null());
        unsafe { ngparse_deck_free(d) };
    }

    #[test]
    fn errors_are_reported_not_crashed() {
        let cp = CString::new("/nonexistent/deck.net").unwrap();
        let d = unsafe { ngparse_expand_file(cp.as_ptr(), 1, 0) };
        assert!(d.is_null(), "expected failure");
        let e = ngparse_last_error();
        assert!(!e.is_null(), "no error message");
        let msg = unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned();
        assert!(msg.contains("nonexistent"), "unhelpful message: {msg}");

        // NULL path must not crash
        assert!(unsafe { ngparse_expand_file(std::ptr::null(), 1, 0) }.is_null());
    }

    #[test]
    fn null_and_free_are_safe() {
        unsafe {
            assert_eq!(ngparse_deck_len(std::ptr::null()), 0);
            assert!(ngparse_deck_card(std::ptr::null(), 0).is_null());
            assert_eq!(ngparse_deck_drop_count(std::ptr::null()), 0);
            ngparse_deck_free(std::ptr::null_mut()); // no-op
        }
    }

    /// cores > 1 is accepted (forward-compatible) and must not change the result.
    #[test]
    fn cores_argument_is_accepted() {
        let p = deck("* t\nr1 n1 0 1k\nv1 n1 0 1\n.end\n", "cores");
        let cp = CString::new(p).unwrap();
        for c in [0, 1, 4] {
            let d = unsafe { ngparse_expand_file(cp.as_ptr(), c, 0) };
            assert!(!d.is_null(), "cores={c} failed");
            assert_eq!(card(d, 1), "r1 n1 0 1.000000000000000e3");
            unsafe { ngparse_deck_free(d) };
        }
    }
}
