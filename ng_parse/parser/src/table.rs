//! HSPICE `table_param()` — table-file parameter lookup.
//!
//! Foundry PDKs use it for self-heating thermal resistance and RF parasitics;
//! foundry_b's TT corner alone calls it ~1300 times (1,289 in the flattened
//! `tb_driver` deck).
//!
//! ```text
//! table_param(file, N_int, int_1..int_N, N_real, real_1..real_M, output_col)
//! ```
//!
//! Integer keys match exactly; real keys are multilinearly interpolated;
//! `output_col` is a 1-based index into the *value* columns (those after the
//! keys). The file is `#`-header-first:
//!
//! ```text
//! #nf nfin l rth0        <- header names the columns: 3 keys, 1 value
//! 1 1 1e-07 0.0105       <- data rows
//! ```
//!
//! This mirrors ngspice's own implementation
//! (`src/frontend/numparam/table_param.c`, added in commit 7131b97e0), which is
//! compiled into the reference binary — so its results are directly diffable.
//! The lookup is a pure function of `(path, int keys, real keys, column)`;
//! relative paths are resolved to absolute earlier, during deck expansion, where
//! the defining file is known (see `preprocess::rewrite_table_paths`).

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// A parsed table file: `n_cols`-wide rows of `f64`.
struct Table {
    n_cols: usize,
    rows: Vec<f64>,
}

impl Table {
    fn n_rows(&self) -> usize {
        self.rows.len() / self.n_cols
    }
    fn at(&self, row: usize, col: usize) -> f64 {
        self.rows[row * self.n_cols + col]
    }
}

/// Process-wide cache: foundry decks reference the same table file hundreds of
/// times, and ngspice caches for the process lifetime for the same reason.
fn cache() -> &'static Mutex<HashMap<String, Option<&'static Table>>> {
    static C: OnceLock<Mutex<HashMap<String, Option<&'static Table>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Load and parse a table file. The first non-blank line must be the `#` header;
/// its token count (minus the `#`) fixes the column count. `*`/`#` lines and
/// blanks are skipped; a row with the wrong arity is skipped.
fn load(path: &str) -> Option<Table> {
    let text = std::fs::read(path).ok()?;
    let text = String::from_utf8_lossy(&text);
    let mut lines = text.lines();

    let n_cols = loop {
        let l = lines.next()?;
        let t = l.trim_start();
        if t.is_empty() {
            continue;
        }
        // ngspice insists on a header and errors out otherwise.
        let hdr = t.strip_prefix('#')?;
        break hdr.split_whitespace().count();
    };
    if n_cols == 0 {
        return None;
    }

    let mut rows = Vec::new();
    for l in lines {
        let t = l.trim_start();
        if t.is_empty() || t.starts_with('*') || t.starts_with('#') {
            continue;
        }
        let vals: Vec<f64> = t
            .split_whitespace()
            .filter_map(|tok| tok.parse::<f64>().ok())
            .collect();
        if vals.len() == n_cols {
            rows.extend(vals);
        }
    }
    if rows.is_empty() {
        return None;
    }
    Some(Table { n_cols, rows })
}

fn get(path: &str) -> Option<&'static Table> {
    let mut c = cache().lock().ok()?;
    if let Some(hit) = c.get(path) {
        return *hit;
    }
    // Leaked deliberately: the cache lives for the process, exactly as ngspice's
    // does, and this hands out plain `&'static` refs without a lock on every read.
    let loaded = load(path).map(|t| &*Box::leak(Box::new(t)));
    c.insert(path.to_string(), loaded);
    loaded
}

/// Do this row's integer keys match? ngspice compares with a 0.5 tolerance —
/// these are integers stored as doubles.
fn int_keys_match(t: &Table, row: usize, ints: &[f64]) -> bool {
    ints.iter()
        .enumerate()
        .all(|(i, v)| (t.at(row, i) - v).abs() <= 0.5)
}

/// Evaluate a `table_param()` lookup. Returns `None` on any failure (missing
/// file, no matching rows, column out of range) — the caller then leaves the
/// expression symbolic and reports a drop rather than inventing a value.
pub fn lookup(path: &str, ints: &[f64], reals: &[f64], output_col: i64) -> Option<f64> {
    let t = get(path)?;
    let n_keys = ints.len() + reals.len();

    // output_col is 1-based within the VALUE columns, which start after the keys.
    if output_col < 1 {
        return None;
    }
    let data_col = n_keys + (output_col as usize - 1);
    if data_col >= t.n_cols {
        return None;
    }

    // Per real-key dimension, find the values bracketing the target among rows
    // whose integer keys match, and the weight between them. Out-of-range targets
    // clamp to the nearest endpoint — no extrapolation.
    let mut v_lo = vec![0.0; reals.len()];
    let mut v_hi = vec![0.0; reals.len()];
    let mut w = vec![0.0; reals.len()];
    let mut found_any = false;

    for (d, &target) in reals.iter().enumerate() {
        let (mut best_lo, mut best_hi) = (f64::NEG_INFINITY, f64::INFINITY);
        let (mut have_lo, mut have_hi) = (false, false);
        for r in 0..t.n_rows() {
            if !int_keys_match(t, r, ints) {
                continue;
            }
            found_any = true;
            let v = t.at(r, ints.len() + d);
            if v <= target && v > best_lo {
                best_lo = v;
                have_lo = true;
            }
            if v >= target && v < best_hi {
                best_hi = v;
                have_hi = true;
            }
        }
        if !have_lo && !have_hi {
            return None; // no rows matched the integer keys
        }
        if !have_lo {
            best_lo = best_hi;
        }
        if !have_hi {
            best_hi = best_lo;
        }
        v_lo[d] = best_lo;
        v_hi[d] = best_hi;
        w[d] = if best_hi == best_lo {
            0.0
        } else {
            ((target - best_lo) / (best_hi - best_lo)).clamp(0.0, 1.0)
        };
    }

    // With no real keys the integer keys alone select the row, so confirm a match.
    if reals.is_empty() {
        let row = (0..t.n_rows()).find(|&r| int_keys_match(t, r, ints))?;
        return Some(t.at(row, data_col));
    }
    if !found_any {
        return None;
    }

    // Sum the 2^n_real corners of the bracketing hyper-rectangle.
    let mut sum = 0.0;
    let mut wsum = 0.0;
    for c in 0..(1usize << reals.len()) {
        let mut weight = 1.0;
        let mut rk = vec![0.0; reals.len()];
        for d in 0..reals.len() {
            if c & (1 << d) != 0 {
                rk[d] = v_hi[d];
                weight *= w[d];
            } else {
                rk[d] = v_lo[d];
                weight *= 1.0 - w[d];
            }
        }
        if weight == 0.0 {
            continue;
        }
        for r in 0..t.n_rows() {
            if !int_keys_match(t, r, ints) {
                continue;
            }
            let hit = rk.iter().enumerate().all(|(d, &want)| {
                (t.at(r, ints.len() + d) - want).abs() <= 1e-12 * (want.abs() + 1e-30)
            });
            if hit {
                sum += weight * t.at(r, data_col);
                wsum += weight;
                break;
            }
        }
    }
    if wsum == 0.0 {
        return None;
    }
    Some(sum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn tbl(body: &str, tag: &str) -> String {
        let p = std::env::temp_dir().join(format!("ngparse_tbl_{}_{tag}", std::process::id()));
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p.to_string_lossy().into_owned()
    }

    /// Shape of foundry_b's egnfet_SHE.table: 2 int keys (nf, nfin), 1 real key (l),
    /// 1 value column (rth0).
    const SHE: &str = "\
#nf nfin l rth0
1 1 1e-07 0.0105
1 1 1.5e-07 0.0108
1 1 2e-07 0.0083
1 2 1e-07 0.0146
1 2 2e-07 0.0200
2 1 1e-07 0.0500
";

    #[test]
    fn exact_hit_on_a_grid_point() {
        let p = tbl(SHE, "exact");
        assert_eq!(lookup(&p, &[1.0, 1.0], &[1e-7], 1), Some(0.0105));
        assert_eq!(lookup(&p, &[1.0, 1.0], &[2e-7], 1), Some(0.0083));
        // integer keys select the row set
        assert_eq!(lookup(&p, &[2.0, 1.0], &[1e-7], 1), Some(0.05));
        assert_eq!(lookup(&p, &[1.0, 2.0], &[1e-7], 1), Some(0.0146));
    }

    #[test]
    fn interpolates_real_key() {
        let p = tbl(SHE, "interp");
        // midway between 1e-7 (0.0105) and 1.5e-7 (0.0108)
        let v = lookup(&p, &[1.0, 1.0], &[1.25e-7], 1).unwrap();
        assert!((v - 0.01065).abs() < 1e-12, "got {v}");
        // 1e-7 (0.0146) .. 2e-7 (0.0200), quarter of the way
        let v = lookup(&p, &[1.0, 2.0], &[1.25e-7], 1).unwrap();
        assert!((v - 0.01595).abs() < 1e-12, "got {v}");
    }

    /// Out-of-range clamps to the nearest endpoint — ngspice never extrapolates.
    #[test]
    fn clamps_instead_of_extrapolating() {
        let p = tbl(SHE, "clamp");
        assert_eq!(lookup(&p, &[1.0, 1.0], &[1e-9], 1), Some(0.0105)); // below min
        assert_eq!(lookup(&p, &[1.0, 1.0], &[1.0], 1), Some(0.0083)); // above max
    }

    #[test]
    fn rejects_bad_lookups() {
        let p = tbl(SHE, "bad");
        assert_eq!(lookup(&p, &[9.0, 9.0], &[1e-7], 1), None); // no such int keys
        assert_eq!(lookup(&p, &[1.0, 1.0], &[1e-7], 2), None); // only 1 value column
        assert_eq!(lookup(&p, &[1.0, 1.0], &[1e-7], 0), None); // 1-based
        assert_eq!(lookup("/nonexistent/x.table", &[1.0], &[1.0], 1), None);
    }

    #[test]
    fn header_is_required() {
        let p = tbl("1 1 1e-07 0.0105\n", "nohdr");
        assert_eq!(lookup(&p, &[1.0, 1.0], &[1e-7], 1), None);
    }

    /// Multiple value columns: output_col indexes them 1-based, after the keys.
    #[test]
    fn selects_output_column() {
        let p = tbl("#k a b c\n1 10 20 30\n2 40 50 60\n", "cols");
        assert_eq!(lookup(&p, &[1.0], &[], 1), Some(10.0));
        assert_eq!(lookup(&p, &[1.0], &[], 2), Some(20.0));
        assert_eq!(lookup(&p, &[1.0], &[], 3), Some(30.0));
        assert_eq!(lookup(&p, &[2.0], &[], 3), Some(60.0));
    }
}
