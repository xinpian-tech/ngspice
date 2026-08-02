//! Section/include expansion — the replacement for ngspice `inpcom.c`'s
//! `.lib`/`.inc` handling, which is the O(n^2) hotspot on large PDK decks.
//!
//! Semantics mirrored from `src/frontend/inpcom.c` (`read_a_lib`,
//! `find_section_definition`, `expand_section_ref`, `expand_section_references`),
//! targeting **hs (HSPICE) compatibility** — the mode the foundry_b 14nm deck uses:
//!
//!   * `.inc <file>` / `.include <file>` — textually include the whole file
//!     (path resolved relative to the *referencing* file's directory).
//!   * `.lib <file> <section>` — reference: splice the body of the `.lib <section>`
//!     definition found in <file>, from just after the definition line up to (but
//!     excluding) the first matching `.endl`. Nested `.lib <file> <section>`
//!     references inside the body are expanded recursively. `.endl` nesting is NOT
//!     counted — the first `.endl` ends the section (matches ngspice).
//!   * `.lib <section>` (one token) — a section *definition* boundary. Indexed;
//!     never emitted on its own.
//!
//! Each source file is read and indexed exactly once, then cached (keyed by
//! canonical path) — the same reuse ngspice's global `libraries[]` gives, but with
//! an O(1) section-name index instead of a linear scan per reference.
//!
//! Deliberately NOT handled yet (absent from the foundry_b tree — see project notes):
//! `.alter` block skipping, `.title`, `.hdl`, `.biaschk`, `.del`, `$ENV`/`$var`
//! path expansion, old-style one-token `.lib <file>` includes (lt/ps modes).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::Config;
use crate::reader::{logical_lines, LogicalLine};

/// Errors surfaced while expanding a deck.
#[derive(Debug)]
pub enum ExpandError {
    Read { path: PathBuf, err: std::io::Error },
    Resolve { token: String, base: PathBuf },
    Section { file: PathBuf, name: String },
    MissingEndl { file: PathBuf, name: String },
    Recursion { path: PathBuf },
}

impl std::fmt::Display for ExpandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExpandError::Read { path, err } => {
                write!(f, "cannot read {}: {err}", path.display())
            }
            ExpandError::Resolve { token, base } => write!(
                f,
                "cannot resolve include/library path {token:?} relative to {}",
                base.display()
            ),
            ExpandError::Section { file, name } => write!(
                f,
                "library file {}: section .lib {name} not found",
                file.display()
            ),
            ExpandError::MissingEndl { file, name } => write!(
                f,
                "library file {}: section .lib {name} has no matching .endl",
                file.display()
            ),
            ExpandError::Recursion { path } => {
                write!(f, "include/library recursion detected at {}", path.display())
            }
        }
    }
}
impl std::error::Error for ExpandError {}

/// A source file loaded once: its logical lines plus a name->index map of the
/// `.lib <section>` definitions it contains.
struct LoadedFile {
    lines: Vec<LogicalLine>,
    /// lowercased section name -> index in `lines` of its `.lib <name>` line.
    sections: HashMap<String, usize>,
}

/// Quote-aware token splitter: whitespace-separated, with `'`/`"` grouping and
/// stripped. SPICE only uses quotes to hold paths together, never for escaping.
fn split_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for ch in s.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else {
                    cur.push(ch);
                }
            }
            None => {
                if ch == '\'' || ch == '"' {
                    quote = Some(ch);
                    has = true;
                } else if ch.is_whitespace() {
                    if has {
                        out.push(std::mem::take(&mut cur));
                        has = false;
                    }
                } else {
                    cur.push(ch);
                    has = true;
                }
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

/// Lowercased first token of a line, for dot-command dispatch.
fn keyword(line: &str) -> String {
    line.split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

fn is_kw(line: &str, kw: &str) -> bool {
    let lstart = line.trim_start();
    lstart.len() >= kw.len()
        && lstart.as_bytes()[..kw.len()].eq_ignore_ascii_case(kw.as_bytes())
        && matches!(
            lstart.as_bytes().get(kw.len()),
            None | Some(b' ') | Some(b'\t')
        )
}

/// Rewrite HSPICE spellings into the canonical SPICE ones, mirroring ngspice's
/// `inpcom.c::inp_fix_macro_param_func_paren_io` (line 2952):
///
/// ```text
/// .macro name ...      ->  .subckt name ...
/// .eom [name]          ->  .ends [name]
/// .subckt name (a b c) ->  .subckt name a b c
/// x1 (a b c) sub       ->  x1 a b c sub
/// ```
///
/// Done once, centrally, so no later stage has to know these aliases exist.
///
/// Without it a `.macro` body is emitted verbatim and its `x` instances are never
/// expanded, and parenthesized ports parse as the tokens `(a` and `c)` — which
/// silently leaves the subckt's devices wired to its INTERNAL node names instead
/// of the caller's. foundry_c's PDK needs both: `.macro`/`.eom` x5, `.subckt name (…)`
/// x20 and `x… (…)` x31 in one deck.
fn normalize_hspice(l: &mut LogicalLine) {
    let t = l.text.trim_start();
    // `.macro`/`.eom` -> `.subckt`/`.ends`, keeping the rest of the line verbatim.
    for (from, to) in [(".macro", ".subckt"), (".eom", ".ends")] {
        if is_kw(t, from) {
            l.text = format!("{to}{}", &t[from.len()..]);
            break;
        }
    }

    // Strip the parentheses around a port/connection list. ngspice blanks the
    // first `(` and its matching `)` on `.subckt` and `x` cards only.
    let t = l.text.trim_start();
    let is_sub = is_kw(t, ".subckt");
    let is_x = t.as_bytes().first().is_some_and(|c| c.eq_ignore_ascii_case(&b'x'));
    if !(is_sub || is_x) {
        return;
    }
    // Skip the leading keyword, plus the subckt's name.
    let mut rest = t.split_at(t.find(char::is_whitespace).unwrap_or(t.len())).1;
    if is_sub {
        let s = rest.trim_start();
        rest = s.split_at(s.find(char::is_whitespace).unwrap_or(s.len())).1;
    }
    let head_len = l.text.len() - rest.len();
    let Some(open) = rest.find('(') else { return };
    // Only a port list — never touch a `(` that belongs to an expression or a
    // value, which always follows a `=` or other text on these cards.
    if !rest[..open].trim().is_empty() {
        return;
    }
    let Some(close) = rest.find(')') else { return };
    if close < open {
        return;
    }
    let mut body = rest.to_string();
    body.replace_range(open..open + 1, " ");
    body.replace_range(close..close + 1, " ");
    l.text = format!("{}{}", &l.text[..head_len], body);
}

/// Resolve relative `table_param(str("./x.table"), ...)` paths against the
/// directory of the file the line came from, rewriting them in place to absolute.
///
/// HSPICE resolves such a path against the *referencing* file's directory, and
/// ngspice's `table_param_lookup` takes a `dir_hint` for exactly this ("the
/// directory of the file that originated this call — typically the .lib file
/// containing the table_param() invocation"). foundry_b's `.param rth0_n` lives in
/// `<pdk>/fets_hp.lib` and asks for `./RF_COMPONENTS/egnfet_SHE.table`, which
/// exists only relative to the PDK dir — never relative to the simulation cwd.
///
/// Doing it here, during expansion, is what keeps the later stages simple: only
/// `LogicalLine` knows its source file, and once the path is absolute the lookup
/// is a pure function that `ParamTable` and `SubcktExpander` can both evaluate
/// without carrying provenance.
///
/// A path that does not resolve to an existing file is left untouched, so the
/// error names what the deck actually wrote.
fn rewrite_table_paths(l: &mut LogicalLine) {
    if !l.text.to_ascii_lowercase().contains("table_param") {
        return;
    }
    let Some(dir) = Path::new(l.file.as_ref()).parent().map(Path::to_path_buf) else {
        return;
    };
    let mut out = String::with_capacity(l.text.len());
    let mut rest = l.text.as_str();
    while let Some(open) = rest.find('"') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('"') else { break };
        let (path, tail) = (&after[..close], &after[close + 1..]);
        out.push_str(&rest[..=open]);
        let abs = dir.join(path);
        match (Path::new(path).is_absolute(), abs.canonicalize()) {
            (false, Ok(p)) => out.push_str(&p.to_string_lossy()),
            _ => out.push_str(path),
        }
        out.push('"');
        rest = tail;
    }
    if !out.is_empty() {
        out.push_str(rest);
        l.text = out;
    }
}

/// The deck expander. Owns the file cache for one expansion run.
pub struct Expander {
    cache: HashMap<PathBuf, Arc<LoadedFile>>,
    /// Paths currently on the expansion stack — cheap cycle guard.
    active: Vec<PathBuf>,
    /// The top deck's title line (see [`Expander::title`]).
    title: String,
    /// Run configuration. The `.lib` walk below is the first parallel seam — see
    /// `Config::effective_cores` for why it is sequential today.
    cfg: Config,
}

impl Expander {
    pub fn new() -> Self {
        Expander::with_config(Config::default())
    }

    /// An expander running under `cfg`.
    pub fn with_config(cfg: Config) -> Self {
        Expander {
            cache: HashMap::new(),
            active: Vec::new(),
            title: String::new(),
            cfg,
        }
    }

    /// This expander's run configuration.
    pub fn config(&self) -> Config {
        self.cfg
    }

    /// The top deck's title: its first physical line, whatever it contains.
    ///
    /// SPICE unconditionally consumes line 1 of the top deck as the title — it is
    /// never a card, even when it looks exactly like one. Verified against the
    /// reference: a deck whose first line is `v1 n1 0 1` reports `v(n1) = 0`
    /// (no such source exists); prepend a comment line and it reports `v(n1) = 1`.
    ///
    /// The same applies to a netlist pulled in by `source` from a `.control` block
    /// — the PDK harness path — but NOT to `.include`/`.lib` files, whose first
    /// line stays an ordinary card.
    ///
    /// Emitting this back as line 1 is what makes `expand` output re-readable:
    /// ngspice will eat the title again, leaving the cards intact. It equally
    /// satisfies the C glue, where `if_inpdeck` walks straight into `INPpas1` from
    /// card #1 with no title skip of its own (`inp.c` does the skipping, at 1146).
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Expand a top-level deck file into a flat list of logical lines with all
    /// `.inc`/`.lib` references resolved. `.param`/`{}`/`.subckt` are left intact
    /// for later stages.
    pub fn expand_file(&mut self, entry: &Path) -> Result<Vec<LogicalLine>, ExpandError> {
        let mut out = Vec::new();
        let file = self.load(entry)?;
        let dir = entry.parent().unwrap_or(Path::new(".")).to_path_buf();

        // Split off the title before walking: line 1 of the TOP deck is never a
        // card (see `title()`). A comment-style title (`* foo`) has already been
        // dropped by the reader; a bare-text one (`check scoping of ...`, which is
        // what tests/regression/subckt-processing/model-scope-5.cir uses) is still
        // in `lines` and would otherwise be emitted as a bogus card — there, a
        // capacitor, since it happens to start with `c`.
        self.title = self.first_physical_line(entry);
        let body: Vec<LogicalLine> = file
            .lines
            .iter()
            .filter(|l| l.line_no != 1)
            .cloned()
            .collect();
        self.walk(&body, &dir, &mut out)?;
        for l in &mut out {
            normalize_hspice(l);
            rewrite_table_paths(l);
        }
        Ok(out)
    }

    /// The entry deck's first physical line, verbatim (minus the trailing newline).
    /// Read from source rather than taken from the logical lines because the reader
    /// has already stripped comments, and a title is usually a comment.
    fn first_physical_line(&self, entry: &Path) -> String {
        let Ok(bytes) = std::fs::read(entry) else {
            return String::new();
        };
        String::from_utf8_lossy(&bytes)
            .lines()
            .next()
            .unwrap_or("")
            .trim_end()
            .to_string()
    }

    /// Resolve a path token relative to `base_dir`, canonicalizing it.
    fn resolve(&self, token: &str, base_dir: &Path) -> Result<PathBuf, ExpandError> {
        let raw = Path::new(token);
        let joined = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            base_dir.join(raw)
        };
        joined.canonicalize().map_err(|_| ExpandError::Resolve {
            token: token.to_string(),
            base: base_dir.to_path_buf(),
        })
    }

    /// Load + index a file (cached by canonical path).
    fn load(&mut self, path: &Path) -> Result<Arc<LoadedFile>, ExpandError> {
        let key = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf());
        if let Some(f) = self.cache.get(&key) {
            return Ok(Arc::clone(f));
        }
        // Read as bytes and decode lossily: PDK model files are frequently
        // Latin-1 / not strictly UTF-8 (special chars in comments, etc.), and
        // `read_to_string` would reject them. SPICE syntax is ASCII, so any
        // replacement of stray non-UTF-8 bytes only affects comment text.
        let bytes = std::fs::read(&key).map_err(|err| ExpandError::Read {
            path: key.clone(),
            err,
        })?;
        let src = String::from_utf8_lossy(&bytes);
        let lines = logical_lines(&src, Arc::from(key.to_string_lossy().as_ref()));

        // Index `.lib <name>` definitions (exactly one token after `.lib`).
        let mut sections = HashMap::new();
        for (i, l) in lines.iter().enumerate() {
            if is_kw(&l.text, ".lib") {
                let toks = split_tokens(&l.text);
                if toks.len() == 2 {
                    sections
                        .entry(toks[1].to_ascii_lowercase())
                        .or_insert(i); // first definition wins
                }
            }
        }
        let loaded = Arc::new(LoadedFile { lines, sections });
        self.cache.insert(key, Arc::clone(&loaded));
        Ok(loaded)
    }

    /// Walk a body of lines at "deck level" (top file or an `.inc`'d file),
    /// resolving includes and section references into `out`.
    fn walk(
        &mut self,
        lines: &[LogicalLine],
        base_dir: &Path,
        out: &mut Vec<LogicalLine>,
    ) -> Result<(), ExpandError> {
        for l in lines {
            let kw = keyword(&l.text);
            match kw.as_str() {
                ".inc" | ".include" => {
                    let toks = split_tokens(&l.text);
                    if let Some(file) = toks.get(1) {
                        let resolved = self.resolve(file, base_dir)?;
                        self.include(&resolved, out)?;
                    }
                }
                ".lib" => {
                    let toks = split_tokens(&l.text);
                    if toks.len() >= 3 {
                        // reference: .lib <file> <section>
                        let resolved = self.resolve(&toks[1], base_dir)?;
                        self.expand_section(&resolved, &toks[2], out)?;
                    }
                    // one-token `.lib <name>` at deck level (hs): definition
                    // boundary — drop it (nothing to emit).
                }
                ".endl" => { /* stray at deck level — drop */ }
                _ => out.push(l.clone()),
            }
        }
        Ok(())
    }

    /// Include a whole file at deck level (`.inc`).
    fn include(&mut self, path: &Path, out: &mut Vec<LogicalLine>) -> Result<(), ExpandError> {
        if self.active.iter().any(|p| p == path) {
            return Err(ExpandError::Recursion {
                path: path.to_path_buf(),
            });
        }
        let file = self.load(path)?;
        let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        self.active.push(path.to_path_buf());
        let r = self.walk(&file.lines, &dir, out);
        self.active.pop();
        r
    }

    /// Expand a `.lib <file> <section>` reference: splice the section body from
    /// just after `.lib <section>` up to the first `.endl`, recursing on nested
    /// references.
    fn expand_section(
        &mut self,
        file: &Path,
        section: &str,
        out: &mut Vec<LogicalLine>,
    ) -> Result<(), ExpandError> {
        let loaded = self.load(file)?;
        let start = *loaded
            .sections
            .get(&section.to_ascii_lowercase())
            .ok_or_else(|| ExpandError::Section {
                file: file.to_path_buf(),
                name: section.to_string(),
            })?;
        let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();

        let mut i = start + 1;
        let mut saw_endl = false;
        while i < loaded.lines.len() {
            let l = &loaded.lines[i];
            let kw = keyword(&l.text);
            match kw.as_str() {
                ".endl" => {
                    saw_endl = true;
                    break;
                }
                ".inc" | ".include" => {
                    let toks = split_tokens(&l.text);
                    if let Some(f) = toks.get(1) {
                        let resolved = self.resolve(f, &dir)?;
                        self.include(&resolved, out)?;
                    }
                }
                ".lib" => {
                    let toks = split_tokens(&l.text);
                    if toks.len() >= 3 {
                        let resolved = self.resolve(&toks[1], &dir)?;
                        self.expand_section(&resolved, &toks[2], out)?;
                    } else {
                        // nested one-token `.lib <name>` definition inside a body:
                        // ngspice leaves it literal. Preserve to match.
                        out.push(l.clone());
                    }
                }
                _ => out.push(l.clone()),
            }
            i += 1;
        }
        if !saw_endl {
            return Err(ExpandError::MissingEndl {
                file: file.to_path_buf(),
                name: section.to_string(),
            });
        }
        Ok(())
    }
}

impl Default for Expander {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        p
    }

    /// A scratch directory of its own per test — tests share a process, so a
    /// single fixed name would let them clobber each other's files.
    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ngparse_pp_{}_{tag}", std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn tokens_strip_quotes() {
        assert_eq!(split_tokens(".lib './a b.lib' TT"), vec![".lib", "./a b.lib", "TT"]);
    }

    /// SPICE consumes line 1 of the TOP deck as the title, whatever it holds.
    /// Verified against the reference: a deck whose line 1 is `v1 n1 0 1` reports
    /// `v(n1)=0` (the source does not exist); add a title line and it reports 1.
    #[test]
    fn title_line_is_never_a_card() {
        let dir = tmpdir("title_card");
        // a bare-text title that would otherwise parse as a device (leading `c`)
        let top = write(
            &dir,
            "top.net",
            "check scoping of nested .model definitions\nR1 1 0 1k\n.end\n",
        );
        let mut ex = Expander::new();
        let out = ex.expand_file(&top).unwrap();
        let texts: Vec<&str> = out.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(ex.title(), "check scoping of nested .model definitions");
        assert_eq!(texts, vec!["R1 1 0 1k", ".end"]);
    }

    /// A comment title is stripped by the reader, but must still be recoverable so
    /// `expand` can re-emit it as line 1.
    #[test]
    fn comment_title_is_captured() {
        let dir = tmpdir("title_comment");
        let top = write(&dir, "top.net", "* my title\nR1 1 0 1k\n.end\n");
        let mut ex = Expander::new();
        let out = ex.expand_file(&top).unwrap();
        assert_eq!(ex.title(), "* my title");
        assert_eq!(out[0].text, "R1 1 0 1k");
    }

    /// Only the TOP deck has a title. An `.include`d file's first line stays an
    /// ordinary card — confirmed against the reference, where a resistor on line 1
    /// of an included file is present and measurable.
    #[test]
    fn included_file_keeps_its_first_line() {
        let dir = tmpdir("title_inc");
        write(&dir, "sub.inc", "R_from_inc 1 0 2k\nC_sub 1 0 1p\n");
        let top = write(&dir, "top.net", "* title\n.inc './sub.inc'\n.end\n");
        let mut ex = Expander::new();
        let out = ex.expand_file(&top).unwrap();
        let texts: Vec<&str> = out.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, vec!["R_from_inc 1 0 2k", "C_sub 1 0 1p", ".end"]);
    }

    #[test]
    fn expands_sections_includes_and_nesting() {
        let dir = std::env::temp_dir().join(format!("ngparse_pp_{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        // models.lib: section TT includes a sub-file and references a nested section.
        write(
            &dir,
            "models.lib",
            "\
.lib TT
.inc './sub.inc'
.lib './corner.lib' TT_core
R_end 1 0 1
.endl TT
",
        );
        write(&dir, "sub.inc", "* sub\nC_sub 1 0 1p\n");
        write(
            &dir,
            "corner.lib",
            "\
.lib TT_core
.param vth=0.3
.endl TT_core
.lib OTHER
.param unused=1
.endl OTHER
",
        );
        // Top deck references models.lib section TT. Line 1 is the title — SPICE
        // consumes it unconditionally, so a real deck never starts with a card.
        let top = write(
            &dir,
            "top.net",
            "* top title\nV1 1 0 1\n.lib './models.lib' TT\n.end\n",
        );

        let mut ex = Expander::new();
        let out = ex.expand_file(&top).unwrap();
        let texts: Vec<&str> = out.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(ex.title(), "* top title");

        assert_eq!(
            texts,
            vec![
                "V1 1 0 1",
                "C_sub 1 0 1p",   // from .inc
                ".param vth=0.3", // from nested .lib corner.lib TT_core
                "R_end 1 0 1",    // rest of TT body
                ".end",
            ]
        );
        // OTHER section must not leak in.
        assert!(!texts.iter().any(|t| t.contains("unused")));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
