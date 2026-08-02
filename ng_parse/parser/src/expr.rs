//! SPICE/HSPICE numeric expression engine: number parsing, a Pratt expression
//! parser, and an evaluator. This is the computational core of the numparam
//! replacement — `.param` values and inline `{...}` expressions are parsed here
//! into an AST and evaluated against an [`Env`] (which supplies parameter values
//! and user-defined functions).
//!
//! Values are `f64` (SPICE numbers are doubles). String-valued constructs
//! (`str(...)`, table string keys) are not modeled yet.
//!
//! Grammar (loosest → tightest binding):
//!   ternary   ?:            (right assoc)
//!   logical   || &&
//!   equality  == != < > <= >=
//!   additive  + -
//!   multiplicative * / %
//!   power     ** ^          (right assoc)
//!   unary     - + !
//!   atom      number | ident | ident(args) | ( expr )
//!
//! Comparisons/booleans yield 1.0 (true) / 0.0 (false), matching ngspice.

use std::fmt;

/// Expression AST.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Num(f64),
    Var(String),
    Unary(UnOp, Box<Expr>),
    Binary(BinOp, Box<Expr>, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    Call(String, Vec<Expr>),
    /// A string literal. SPICE expressions are numeric, but HSPICE's
    /// `table_param(str("file"), ...)` takes a filename — the one place a string
    /// reaches the evaluator. It never participates in arithmetic: `eval` rejects
    /// it, and only `table_param` reads it (via `str_arg`).
    Str(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Pos,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Pow,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvalError {
    UnknownVar(String),
    UnknownFunc(String),
    Arity { func: String, got: usize },
    Parse(String),
}

impl fmt::Display for EvalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EvalError::UnknownVar(n) => write!(f, "unknown parameter `{n}`"),
            EvalError::UnknownFunc(n) => write!(f, "unknown function `{n}`"),
            EvalError::Arity { func, got } => {
                write!(f, "function `{func}` called with {got} args")
            }
            EvalError::Parse(m) => write!(f, "expression parse error: {m}"),
        }
    }
}
impl std::error::Error for EvalError {}

/// Environment supplying parameter values and user functions to the evaluator.
///
/// Methods take `&self`; implementations use interior mutability (RefCell) for
/// memoization so hierarchical scopes with parent chains compose without `&mut`
/// aliasing headaches during recursive evaluation.
pub trait Env {
    /// Resolve a bare identifier to a value (parameter lookup, possibly memoized).
    fn var(&self, name: &str) -> Result<f64, EvalError>;
    /// Call a user-defined function. Return `Ok(None)` to signal "not a user
    /// function" so the evaluator can fall back to built-ins.
    fn call_user(&self, name: &str, args: &[f64]) -> Result<Option<f64>, EvalError>;
}

// ---------------------------------------------------------------------------
// Number parsing (SPICE engineering suffixes)
// ---------------------------------------------------------------------------

/// Parse a SPICE number at the start of `s`, returning (value, bytes_consumed).
///
/// Handles a leading float (with optional `e`/`E` exponent) followed by an
/// optional engineering suffix. `meg`/`MEG` (1e6) is checked before `m` (1e-3).
/// Any trailing alphabetic characters after the suffix are part of the token but
/// ignored for the value (e.g. `1kohm` -> 1000, `10uF` -> 1e-5), matching SPICE.
pub fn parse_number(s: &str) -> Option<(f64, usize)> {
    let b = s.as_bytes();
    let mut i = 0;

    // optional sign
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let digits_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
    }
    // need at least one digit in the mantissa
    if s[digits_start..i].bytes().filter(|c| c.is_ascii_digit()).count() == 0 {
        return None;
    }
    // exponent
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_digits = {
            let start = j;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            j > start
        };
        if exp_digits {
            i = j;
        }
    }

    let mant_str = &s[..i];
    let mantissa: f64 = mant_str.parse().ok()?;

    // Engineering suffix. For the power-of-10 suffixes, fold the exponent into the
    // mantissa's decimal string and re-parse (`14n` -> `"14e-9"`) so the result is
    // the correctly-rounded f64 that a direct decimal (`0.014e-6`) would produce.
    // Doing `mantissa * 1e-9` instead loses a ULP and breaks the exact geometry
    // equality tests PDK models rely on (e.g. `l == 0.014e-6`).
    let rest = &s[i..];
    let lower = rest.to_ascii_lowercase();
    // The micro sign as a suffix: ngspice accepts `µ`/`μ` for `u` (1e-6), and PDK
    // and hand-written decks use it (`2µ` = 2uA). Both are 2-byte UTF-8 chars, so
    // the ASCII byte match below would miss them and stop the number at the digit,
    // turning `2µ` into a bare 2 -- 1e6x wrong. Handle them first, by char.
    let (pow10, suf_len): (Option<i32>, usize) = if rest.starts_with('\u{00B5}') {
        (Some(-6), '\u{00B5}'.len_utf8())
    } else if rest.starts_with('\u{03BC}') {
        (Some(-6), '\u{03BC}'.len_utf8())
    } else if lower.starts_with("meg") {
        (Some(6), 3)
    } else if lower.starts_with("mil") {
        (None, 3) // 25.4e-6, not a power of ten
    } else {
        match lower.bytes().next() {
            Some(b't') => (Some(12), 1),
            Some(b'g') => (Some(9), 1),
            Some(b'k') => (Some(3), 1),
            Some(b'm') => (Some(-3), 1),
            Some(b'u') => (Some(-6), 1),
            Some(b'n') => (Some(-9), 1),
            Some(b'p') => (Some(-12), 1),
            Some(b'f') => (Some(-15), 1),
            Some(b'a') => (Some(-18), 1),
            _ => (Some(0), 0),
        }
    };
    let value = match (suf_len, pow10) {
        (0, _) => mantissa,
        (_, None) => mantissa * 25.4e-6, // mil
        (_, Some(0)) => mantissa,
        (_, Some(p)) => {
            // if the mantissa already carries an exponent, fall back to a multiply
            if mant_str.contains(['e', 'E']) {
                mantissa * 10f64.powi(p)
            } else {
                format!("{mant_str}e{p}")
                    .parse()
                    .unwrap_or_else(|_| mantissa * 10f64.powi(p))
            }
        }
    };
    i += suf_len;
    // consume trailing alphanumerics of the unit (ignored), e.g. "ohm" in "1kohm"
    let rb = s.as_bytes();
    while i < rb.len() && (rb[i].is_ascii_alphanumeric() || rb[i] == b'_') {
        i += 1;
    }

    Some((value, i))
}

// ---------------------------------------------------------------------------
// Lexer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Str(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
    Question,
    Colon,
}

fn lex(input: &str) -> Result<Vec<Tok>, EvalError> {
    let mut toks = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut depth = 0usize; // paren nesting depth
    // Depth of an open `v(`/`i(` argument list. Inside it, arguments are NODE
    // NAMES, not expressions, so `+`/`-` are part of the name.
    let mut vi_paren: Option<usize> = None;
    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        // Inside a v()/i() argument list the args are node/source names, not
        // arithmetic: `v(vin+,vin-)` names two differential nodes and the trailing
        // `+`/`-` belong to the name (opamp macromodels are full of these). Lex
        // each argument as one raw token so such names parse instead of blowing up
        // the whole expression (which would then be kept verbatim, unsubstituted
        // and unrenamed -- e.g. an undefined `gain` reaching ngspice).
        if vi_paren == Some(depth) {
            match c {
                b')' => {
                    vi_paren = None;
                    depth = depth.saturating_sub(1);
                    toks.push(Tok::RParen);
                    i += 1;
                }
                b',' => {
                    toks.push(Tok::Comma);
                    i += 1;
                }
                _ => {
                    let start = i;
                    while i < bytes.len() {
                        let d = bytes[i];
                        if d.is_ascii_whitespace() || d == b',' || d == b')' || d == b'(' {
                            break;
                        }
                        i += 1;
                    }
                    toks.push(Tok::Ident(input[start..i].to_string()));
                }
            }
            continue;
        }
        // `'...'` delimits an EXPRESSION in SPICE, not a string, so the quotes are
        // transparent to the lexer: `agauss('1-mc_sw',1,3)` is agauss(1-mc_sw,1,3).
        // Nested quoting like this is common in foundry statistical blocks (foundry_c's
        // fet_dist.inc), and rejecting it made the whole `.param` unparsable, so
        // `aclv_nest_ags` never resolved and every dependent parameter was dropped
        // across all 26 foundry_c decks. String LITERALS use `"` (see Tok::Str) — no
        // deck in the corpus writes `str('...')`.
        if c == b'\'' {
            i += 1;
            continue;
        }
        // `{...}` is the OTHER SPICE expression delimiter, so a `{` nested inside an
        // already-braced value is transparent too: `{TABLE(v(a,b),2,{64e-6-iee},...)}`
        // carries an inner braced sub-expression, and rejecting it made the whole
        // behavioral source unparsable (kept verbatim with nested braces, which
        // ngspice then reports as a "mal formed E source"). Treated exactly like the
        // `'...'` case above -- the outer span is stripped by subst_exprs before we
        // get here, so any brace we see now is a nested sub-expression.
        if c == b'{' || c == b'}' {
            i += 1;
            continue;
        }
        // number (digit, or leading '.' followed by digit). A leading sign is
        // handled as a unary operator by the parser, not the number lexer, so
        // that `a-1` lexes as `a`,`-`,`1`.
        if c.is_ascii_digit() || (c == b'.' && i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit())
        {
            if let Some((v, used)) = parse_number(&input[i..]) {
                toks.push(Tok::Num(v));
                i += used;
                continue;
            }
        }
        // identifier
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' )
            {
                i += 1;
            }
            toks.push(Tok::Ident(input[start..i].to_string()));
            continue;
        }
        // multi-char operators
        let two = if i + 1 < bytes.len() { &input[i..i + 2] } else { "" };
        match two {
            "**" => { toks.push(Tok::Op("**")); i += 2; continue; }
            "==" => { toks.push(Tok::Op("==")); i += 2; continue; }
            "!=" => { toks.push(Tok::Op("!=")); i += 2; continue; }
            "<=" => { toks.push(Tok::Op("<=")); i += 2; continue; }
            ">=" => { toks.push(Tok::Op(">=")); i += 2; continue; }
            "&&" => { toks.push(Tok::Op("&&")); i += 2; continue; }
            "||" => { toks.push(Tok::Op("||")); i += 2; continue; }
            _ => {}
        }
        match c {
            b'(' => {
                depth += 1;
                // A `(` immediately preceded by a `v`/`i` identifier opens a
                // node-argument list (voltage/current probe), lexed name-wise above.
                if let Some(Tok::Ident(name)) = toks.last() {
                    if name.eq_ignore_ascii_case("v") || name.eq_ignore_ascii_case("i") {
                        vi_paren = Some(depth);
                    }
                }
                toks.push(Tok::LParen);
            }
            b')' => {
                depth = depth.saturating_sub(1);
                toks.push(Tok::RParen);
            }
            b',' => toks.push(Tok::Comma),
            b'?' => toks.push(Tok::Question),
            b':' => toks.push(Tok::Colon),
            b'+' => toks.push(Tok::Op("+")),
            b'-' => toks.push(Tok::Op("-")),
            b'*' => toks.push(Tok::Op("*")),
            b'/' => toks.push(Tok::Op("/")),
            b'%' => toks.push(Tok::Op("%")),
            b'^' => toks.push(Tok::Op("^")),
            b'<' => toks.push(Tok::Op("<")),
            b'>' => toks.push(Tok::Op(">")),
            b'!' => toks.push(Tok::Op("!")),
            // SPICE behavioral expressions use single `|`/`&` as logical or/and
            // (PSpice `IF((V(a)>0 | V(b)>0),1,0)`); treat them as the boolean
            // operators, same as `||`/`&&` (matched above when doubled).
            b'|' => toks.push(Tok::Op("||")),
            b'&' => toks.push(Tok::Op("&&")),
            b'"' => {
                // string literal: `str("./x.table")`. No escapes — SPICE paths
                // never contain quotes, and neither HSPICE nor ngspice's reader
                // define an escape here.
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j] != b'"' {
                    j += 1;
                }
                if j >= bytes.len() {
                    return Err(EvalError::Parse("unterminated string literal".into()));
                }
                toks.push(Tok::Str(input[start..j].to_string()));
                i = j; // the trailing `i += 1` below steps past the closing quote
            }
            _ => return Err(EvalError::Parse(format!("unexpected char {:?}", c as char))),
        }
        i += 1;
    }
    Ok(toks)
}

// ---------------------------------------------------------------------------
// Parser (Pratt)
// ---------------------------------------------------------------------------

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }
    fn eat(&mut self, t: &Tok) -> Result<(), EvalError> {
        if self.peek() == Some(t) {
            self.pos += 1;
            Ok(())
        } else {
            Err(EvalError::Parse(format!("expected {t:?}, found {:?}", self.peek())))
        }
    }

    /// left binding power for binary operators; higher binds tighter.
    fn bin_bp(op: &str) -> Option<(u8, BinOp, bool)> {
        // (bp, op, right_assoc)
        Some(match op {
            "||" => (1, BinOp::Or, false),
            "&&" => (2, BinOp::And, false),
            "==" => (3, BinOp::Eq, false),
            "!=" => (3, BinOp::Ne, false),
            "<" => (4, BinOp::Lt, false),
            ">" => (4, BinOp::Gt, false),
            "<=" => (4, BinOp::Le, false),
            ">=" => (4, BinOp::Ge, false),
            "+" => (5, BinOp::Add, false),
            "-" => (5, BinOp::Sub, false),
            "*" => (6, BinOp::Mul, false),
            "/" => (6, BinOp::Div, false),
            "%" => (6, BinOp::Rem, false),
            "**" | "^" => (8, BinOp::Pow, true),
            _ => return None,
        })
    }

    fn parse_expr(&mut self, min_bp: u8) -> Result<Expr, EvalError> {
        // Prefix operators bind looser than `**`/`^` (bp 8) but tighter than the
        // multiplicative operators (bp 6), so `-2**2` == `-(2**2)` and
        // `-2*3` == `(-2)*3`, matching HSPICE.
        const PREFIX_BP: u8 = 7;
        let mut lhs = match self.peek() {
            Some(Tok::Op("-")) => { self.pos += 1; Expr::Unary(UnOp::Neg, Box::new(self.parse_expr(PREFIX_BP)?)) }
            Some(Tok::Op("+")) => { self.pos += 1; Expr::Unary(UnOp::Pos, Box::new(self.parse_expr(PREFIX_BP)?)) }
            Some(Tok::Op("!")) => { self.pos += 1; Expr::Unary(UnOp::Not, Box::new(self.parse_expr(PREFIX_BP)?)) }
            _ => self.parse_atom()?,
        };
        loop {
            let op = match self.peek() {
                Some(Tok::Op(o)) => *o,
                _ => break,
            };
            let Some((bp, binop, right)) = Self::bin_bp(op) else { break };
            if bp < min_bp {
                break;
            }
            self.pos += 1;
            let next_min = if right { bp } else { bp + 1 };
            let rhs = self.parse_expr(next_min)?;
            lhs = Expr::Binary(binop, Box::new(lhs), Box::new(rhs));
        }
        // ternary has the lowest precedence; bind it once lhs is complete.
        if min_bp == 0 {
            if let Some(Tok::Question) = self.peek() {
                self.pos += 1;
                let then_e = self.parse_expr(0)?;
                self.eat(&Tok::Colon)?;
                let else_e = self.parse_expr(0)?;
                lhs = Expr::Ternary(Box::new(lhs), Box::new(then_e), Box::new(else_e));
            }
        }
        Ok(lhs)
    }

    fn parse_atom(&mut self) -> Result<Expr, EvalError> {
        match self.next() {
            Some(Tok::Num(v)) => Ok(Expr::Num(v)),
            Some(Tok::LParen) => {
                let e = self.parse_expr(0)?;
                self.eat(&Tok::RParen)?;
                Ok(e)
            }
            Some(Tok::Ident(name)) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.pos += 1;
                    let mut args = Vec::new();
                    if self.peek() != Some(&Tok::RParen) {
                        loop {
                            args.push(self.parse_expr(0)?);
                            match self.peek() {
                                Some(Tok::Comma) => { self.pos += 1; }
                                _ => break,
                            }
                        }
                    }
                    self.eat(&Tok::RParen)?;
                    Ok(Expr::Call(name, args))
                } else {
                    Ok(Expr::Var(name))
                }
            }
            Some(Tok::Str(v)) => Ok(Expr::Str(v)),
            other => Err(EvalError::Parse(format!("unexpected token {other:?}"))),
        }
    }
}

/// Parse an expression string into an AST.
pub fn parse(input: &str) -> Result<Expr, EvalError> {
    let toks = lex(input)?;
    let mut p = Parser { toks, pos: 0 };
    let e = p.parse_expr(0)?;
    if p.pos != p.toks.len() {
        return Err(EvalError::Parse(format!(
            "trailing tokens from {:?}",
            &p.toks[p.pos..]
        )));
    }
    Ok(e)
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

fn as_bool(v: f64) -> bool {
    v != 0.0
}
fn from_bool(b: bool) -> f64 {
    if b { 1.0 } else { 0.0 }
}

/// Evaluate a built-in function. Returns `None` if `name` is not a built-in.
/// Extract a string argument: either a bare literal or `str("...")`, which is how
/// HSPICE decks spell a filename (`table_param(str("./x.table"), ...)`).
/// HSPICE's `str()` is a to-string coercion; on a literal it is the identity, so
/// unwrapping it here is exact — and it is the only context a string can appear in.
pub fn str_arg(e: &Expr) -> Option<&str> {
    match e {
        Expr::Str(s) => Some(s),
        Expr::Call(f, a) if f.eq_ignore_ascii_case("str") && a.len() == 1 => str_arg(&a[0]),
        _ => None,
    }
}

/// Evaluate a `table_param(file, N_int, ints.., N_real, reals.., col)` call from
/// its argument ASTs. `evala` evaluates a numeric argument.
///
/// Returns `None` if this is not a well-formed `table_param` call, so the caller
/// can fall through to the ordinary builtin path.
pub fn eval_table_param<F>(
    name: &str,
    args: &[Expr],
    mut evala: F,
) -> Option<Result<f64, EvalError>>
where
    F: FnMut(&Expr) -> Result<f64, EvalError>,
{
    if !name.eq_ignore_ascii_case("table_param") {
        return None;
    }
    let bad = |m: &str| Some(Err(EvalError::Parse(format!("table_param: {m}"))));
    let Some(path) = args.first().and_then(str_arg) else {
        return bad("first argument must be a file name");
    };

    // file, N_int, <N_int values>, N_real, <N_real values>, output_col
    let mut i = 1;
    // `count(i)` reads a key-count argument: a non-negative integer.
    macro_rules! count {
        () => {{
            let Some(a) = args.get(i) else {
                return bad("missing key count");
            };
            let v = match evala(a) {
                Ok(v) => v,
                Err(e) => return Some(Err(e)),
            };
            i += 1;
            if v < 0.0 || v.fract() != 0.0 {
                return bad("key count must be a non-negative integer");
            }
            v as usize
        }};
    }
    let n_int = count!();
    let mut ints = Vec::with_capacity(n_int);
    for _ in 0..n_int {
        match args.get(i).map(&mut evala) {
            Some(Ok(v)) => ints.push(v),
            Some(Err(e)) => return Some(Err(e)),
            None => return bad("too few integer keys"),
        }
        i += 1;
    }
    let n_real = count!();
    let mut reals = Vec::with_capacity(n_real);
    for _ in 0..n_real {
        match args.get(i).map(&mut evala) {
            Some(Ok(v)) => reals.push(v),
            Some(Err(e)) => return Some(Err(e)),
            None => return bad("too few real keys"),
        }
        i += 1;
    }
    let col = match args.get(i).map(&mut evala) {
        Some(Ok(v)) => v as i64,
        Some(Err(e)) => return Some(Err(e)),
        None => return bad("missing output column"),
    };
    if i + 1 != args.len() {
        return bad("too many arguments");
    }

    match crate::table::lookup(path, &ints, &reals, col) {
        Some(v) => Some(Ok(v)),
        None => Some(Err(EvalError::Parse(format!(
            "table_param: lookup failed in {path:?}"
        )))),
    }
}

pub fn eval_builtin(name: &str, a: &[f64]) -> Option<Result<f64, EvalError>> {
    let n = name.to_ascii_lowercase();
    let one = |f: fn(f64) -> f64| -> Option<Result<f64, EvalError>> {
        if a.len() == 1 { Some(Ok(f(a[0]))) }
        else { Some(Err(EvalError::Arity { func: n.clone(), got: a.len() })) }
    };
    match n.as_str() {
        "sqrt" => one(f64::sqrt),
        "exp" => one(f64::exp),
        "ln" | "log" => one(f64::ln),
        "log10" => one(f64::log10),
        "abs" => one(f64::abs),
        "sin" => one(f64::sin),
        "cos" => one(f64::cos),
        "tan" => one(f64::tan),
        "asin" => one(f64::asin),
        "acos" => one(f64::acos),
        "atan" => one(f64::atan),
        "sinh" => one(f64::sinh),
        "cosh" => one(f64::cosh),
        "tanh" => one(f64::tanh),
        "asinh" => one(f64::asinh),
        "acosh" => one(f64::acosh),
        "atanh" => one(f64::atanh),
        // Step/ramp and compare-to-zero helpers, matching ngspice
        // `spicelib/parser/ptfuncs.c` exactly (PTustep/PTustep2/PTuramp/PTeq0...).
        "u" => one(|x| if x < 0.0 { 0.0 } else if x > 0.0 { 1.0 } else { 0.5 }),
        "u2" => one(|x| if x <= 0.0 { 0.0 } else if x <= 1.0 { x } else { 1.0 }),
        "uramp" => one(|x| if x < 0.0 { 0.0 } else { x }),
        "eq0" => one(|x| if x == 0.0 { 1.0 } else { 0.0 }),
        "ne0" => one(|x| if x != 0.0 { 1.0 } else { 0.0 }),
        "gt0" => one(|x| if x > 0.0 { 1.0 } else { 0.0 }),
        "lt0" => one(|x| if x < 0.0 { 1.0 } else { 0.0 }),
        "ge0" => one(|x| if x >= 0.0 { 1.0 } else { 0.0 }),
        "le0" => one(|x| if x <= 0.0 { 1.0 } else { 0.0 }),
        "int" => one(f64::trunc),
        // ngspice PTnint uses nearbyint(): round half-integers to the nearest EVEN
        // integer (banker's rounding), NOT away-from-zero like f64::round().
        // e.g. nint(2.5)=2, nint(0.5)=0, nint(-0.5)=0, nint(-2.5)=-2.
        "nint" => one(f64::round_ties_even),
        "ceil" => one(f64::ceil),
        "floor" => one(f64::floor),
        "sgn" | "sign" => one(|x| if x > 0.0 { 1.0 } else if x < 0.0 { -1.0 } else { 0.0 }),
        "pow" | "pwr" => {
            if a.len() == 2 { Some(Ok(a[0].powf(a[1]))) }
            else { Some(Err(EvalError::Arity { func: n, got: a.len() })) }
        }
        "min" => {
            if a.len() == 2 { Some(Ok(a[0].min(a[1]))) }
            else { Some(Err(EvalError::Arity { func: n, got: a.len() })) }
        }
        "max" => {
            if a.len() == 2 { Some(Ok(a[0].max(a[1]))) }
            else { Some(Err(EvalError::Arity { func: n, got: a.len() })) }
        }
        // Statistical draws: in a non-Monte-Carlo run these resolve to the
        // nominal (first argument). agauss(nom,var,sigma), aunif(nom,var), etc.
        //
        // `limit(nominal, abs_variation)` belongs to the same family — ngspice
        // lists it alongside the others (`inp.c:979` iterates
        // {agauss, gauss, aunif, unif, limit}) and implements it as
        //     nominal + (drand() > 0 ? abs_variation : -abs_variation)
        // with `drand()` uniform on [-1,+1), i.e. a true coin flip about
        // `nominal`. So its mean is the first argument, like the rest. Unseeded
        // and therefore non-reproducible run-to-run, which is why we return the
        // nominal rather than chase a bit-match (the accepted agauss policy).
        //
        // foundry_c's PDK needs this: `limit(-0.5, 0.5)` appears inside every
        // `prdsw_<dev>` chain, and without it the whole expression failed to fold
        // and the parameter was DROPPED (silently defaulting) on all 26 decks.
        "agauss" | "gauss" | "aunif" | "unif" | "limit" => {
            if a.is_empty() { Some(Err(EvalError::Arity { func: n, got: 0 })) }
            // 3-arg `limit(x,lo,hi)` is PSpice's clamp (HSPICE's MC `limit` takes
            // 2 args) — the arity alone disambiguates the dialects.
            else if n == "limit" && a.len() == 3 {
                let (lo, hi) = if a[1] < a[2] { (a[1], a[2]) } else { (a[2], a[1]) };
                Some(Ok(a[0].max(lo).min(hi)))
            }
            else { Some(Ok(a[0])) }
        }
        // `if(cond, a, b)` functional form of the ternary.
        "if" => {
            if a.len() == 3 { Some(Ok(if as_bool(a[0]) { a[1] } else { a[2] })) }
            else { Some(Err(EvalError::Arity { func: n, got: a.len() })) }
        }
        _ => None,
    }
}

/// Evaluate an expression against an environment.
pub fn eval(e: &Expr, env: &dyn Env) -> Result<f64, EvalError> {
    match e {
        Expr::Num(v) => Ok(*v),
        Expr::Var(name) => env.var(name),
        Expr::Unary(op, x) => {
            let v = eval(x, env)?;
            Ok(match op {
                UnOp::Neg => -v,
                UnOp::Pos => v,
                UnOp::Not => from_bool(!as_bool(v)),
            })
        }
        Expr::Binary(op, l, r) => {
            let a = eval(l, env)?;
            let b = eval(r, env)?;
            Ok(match op {
                BinOp::Add => a + b,
                BinOp::Sub => a - b,
                BinOp::Mul => a * b,
                BinOp::Div => a / b,
                BinOp::Rem => a % b,
                BinOp::Pow => a.powf(b),
                BinOp::Eq => from_bool(a == b),
                BinOp::Ne => from_bool(a != b),
                BinOp::Lt => from_bool(a < b),
                BinOp::Gt => from_bool(a > b),
                BinOp::Le => from_bool(a <= b),
                BinOp::Ge => from_bool(a >= b),
                BinOp::And => from_bool(as_bool(a) && as_bool(b)),
                BinOp::Or => from_bool(as_bool(a) || as_bool(b)),
            })
        }
        Expr::Ternary(c, t, f) => {
            if as_bool(eval(c, env)?) { eval(t, env) } else { eval(f, env) }
        }
        Expr::Str(_) => Err(EvalError::Parse(
            "string literal used where a number is required".into(),
        )),
        Expr::Call(name, args) => {
            // table_param needs its args unevaluated: the first is a string.
            if let Some(r) = eval_table_param(name, args, |a| eval(a, env)) {
                return r;
            }
            let mut vals = Vec::with_capacity(args.len());
            for a in args {
                vals.push(eval(a, env)?);
            }
            // user functions take precedence, then built-ins
            if let Some(v) = env.call_user(name, &vals)? {
                return Ok(v);
            }
            match eval_builtin(name, &vals) {
                Some(r) => r,
                None => Err(EvalError::UnknownFunc(name.clone())),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HSPICE spells a table filename `str("...")`; the parser must accept the
    /// string literal, and `str_arg` must see through the `str()` wrapper.
    #[test]
    fn parses_string_literals_and_str_wrapper() {
        let e = parse(r#"str("./RF_COMPONENTS/egnfet_SHE.table")"#).unwrap();
        assert_eq!(str_arg(&e), Some("./RF_COMPONENTS/egnfet_SHE.table"));
        let e = parse(r#""bare.table""#).unwrap();
        assert_eq!(str_arg(&e), Some("bare.table"));
        // a string is not a number
        assert!(eval(&parse(r#""x""#).unwrap(), &MapEnv(HashMap::new())).is_err());
        assert!(parse(r#"str("unterminated"#).is_err());
    }

    /// The full foundry_b call shape must parse: file, N_int, ints, N_real, reals, col.
    #[test]
    fn parses_table_param_call() {
        let e = parse(
            r#"table_param(str("./x.table"),2, xnf_clamp, nfin_clamp, 1, l_clamp, 1)"#,
        )
        .unwrap();
        match &e {
            Expr::Call(n, a) => {
                assert!(n.eq_ignore_ascii_case("table_param"));
                assert_eq!(a.len(), 7);
                assert_eq!(str_arg(&a[0]), Some("./x.table"));
            }
            other => panic!("expected a call, got {other:?}"),
        }
    }

    /// A failed lookup must be an ERROR, never a silent default — a wrong rth0
    /// would quietly change self-heating and converge to the wrong answer.
    #[test]
    fn failed_lookup_is_an_error_not_a_default() {
        let e = parse(r#"table_param(str("/nonexistent/x.table"), 1, 1, 0, 1)"#).unwrap();
        assert!(eval(&e, &MapEnv(HashMap::new())).is_err());
    }
    use std::collections::HashMap;

    struct MapEnv(HashMap<String, f64>);
    impl Env for MapEnv {
        fn var(&self, name: &str) -> Result<f64, EvalError> {
            self.0
                .get(&name.to_ascii_lowercase())
                .copied()
                .ok_or_else(|| EvalError::UnknownVar(name.to_string()))
        }
        fn call_user(&self, _n: &str, _a: &[f64]) -> Result<Option<f64>, EvalError> {
            Ok(None)
        }
    }

    fn ev(s: &str, vars: &[(&str, f64)]) -> f64 {
        let map = vars.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let env = MapEnv(map);
        eval(&parse(s).unwrap(), &env).unwrap()
    }

    #[test]
    fn numbers_with_suffixes() {
        assert_eq!(parse_number("1k").unwrap().0, 1000.0);
        // micro sign: `µ` (U+00B5) and Greek `μ` (U+03BC) both mean u = 1e-6.
        assert_eq!(parse_number("2\u{00B5}").unwrap().0, 2e-6);
        assert_eq!(parse_number("2\u{03BC}").unwrap().0, 2e-6);
        assert_eq!(parse_number("2.5e-08").unwrap().0, 2.5e-8);
        assert_eq!(parse_number("550n").unwrap().0, 550e-9);
        assert_eq!(parse_number("16u").unwrap().0, 16e-6);
        assert_eq!(parse_number("1meg").unwrap().0, 1e6);
        assert_eq!(parse_number("1mil").unwrap().0, 25.4e-6);
        assert_eq!(parse_number("2p").unwrap().0, 2e-12);
        assert_eq!(parse_number("1kohm").unwrap().0, 1000.0); // trailing unit ignored
    }

    #[test]
    fn precedence_and_power() {
        assert_eq!(ev("2+3*4", &[]), 14.0);
        assert_eq!(ev("(2+3)*4", &[]), 20.0);
        assert_eq!(ev("2**3**2", &[]), 512.0); // right assoc
        assert_eq!(ev("-2**2", &[]), -4.0); // unary binds looser than ** -> -(2**2)
        assert_eq!(ev("-2*3", &[]), -6.0);
    }

    #[test]
    fn comparisons_and_ternary() {
        assert_eq!(ev("3==3", &[]), 1.0);
        assert_eq!(ev("m==m1", &[("m", 5.0), ("m1", 5.0)]), 1.0);
        assert_eq!(ev("a>b ? 10 : 20", &[("a", 1.0), ("b", 2.0)]), 20.0);
        assert_eq!(ev("5*(x==m2)", &[("x", 2.0), ("m2", 2.0)]), 5.0);
    }

    #[test]
    fn builtins_and_params() {
        assert_eq!(ev("max(3,7)", &[]), 7.0);
        assert_eq!(ev("sqrt(16)", &[]), 4.0);
        assert_eq!(ev("abs(0-5)", &[]), 5.0);
        assert_eq!(ev("agauss(1.5, 0.2, 3)", &[]), 1.5); // nominal
        // `limit(nominal, abs_variation)` is the same family: ngspice returns
        // nominal +/- abs_variation on an unseeded coin flip, so the nominal is
        // the first argument. foundry_c's prdsw_* chains use limit(-0.5, 0.5).
        assert_eq!(ev("limit(-0.5, 0.5)", &[]), -0.5);
        assert_eq!(ev("limit(2.0, 0.25)", &[]), 2.0);
        assert_eq!(ev("0*(corner_sigma/3)", &[("corner_sigma", 3.0)]), 0.0);
        assert_eq!(ev("if(1>0, 2, 3)", &[]), 2.0);
    }

    /// Inside v()/i(), a `+`/`-` suffix is part of the node name (differential
    /// pins like VIN+/VIN-), not an operator. Such names must parse -- otherwise
    /// the whole behavioral expression is kept verbatim, unsubstituted (an opamp
    /// macromodel's `gain` param then reaches ngspice undefined).
    #[test]
    fn node_names_with_sign() {
        // node names with +/- parse as single Var arguments to v()/i()
        match parse("v(vin+,vin-)").unwrap() {
            Expr::Call(f, args) => {
                assert_eq!(f, "v");
                assert!(matches!(&args[0], Expr::Var(n) if n == "vin+"));
                assert!(matches!(&args[1], Expr::Var(n) if n == "vin-"));
            }
            other => panic!("expected v() call, got {other:?}"),
        }
        // the outer expression still parses (was previously a hard parse error)
        assert!(parse("limit(g*v(vin+,vin-),v(vp-,vin-),v(vp+,vin-))").is_ok());
        // arithmetic OUTSIDE a v()/i() call is unaffected: `a-1` is still `a` `-` `1`
        assert_eq!(ev("a-1", &[("a", 5.0)]), 4.0);
        assert_eq!(ev("2*(x-y)", &[("x", 3.0), ("y", 1.0)]), 4.0);
    }
}
