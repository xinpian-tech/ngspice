//! `ngparse` CLI — standalone driver for developing and validating the parser
//! before it is linked into ngspice.
//!
//! Subcommands:
//!   ngparse lines   <deck>   dump logical lines (continuations joined, comments stripped)
//!   ngparse flatten <deck>   expand all .inc/.lib sections into a flat card list
//!
//! `flatten` output is what gets diffed against ngspice's expanded-deck dump.

use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::num::NonZeroUsize;
use std::time::Instant;

fn usage(prog: &str) -> ExitCode {
    eprintln!("usage:");
    eprintln!("  {prog} lines   <deck>   dump logical lines");
    eprintln!("  {prog} flatten <deck>   expand .inc/.lib into a flat card list");
    eprintln!("  {prog} resolve <deck>   flatten + evaluate params -> numeric deck");
    eprintln!("");
    eprintln!("flags:");
    eprintln!("  --strict             fail if any parameter could not be resolved");
    eprintln!("  --topo-reduce        remove dangling passives (two-terminal R/C on a");
    eprintln!("                       node referenced nowhere else) from the deck");
    eprintln!("  --cores <n>          worker cores (default 1; >1 not yet implemented)");
    eprintln!("env:");
    eprintln!("  NGPARSE_DEBUG_DROP=1 list every dropped parameter");
    ExitCode::from(2)
}

/// The title to emit as line 1 of an expanded deck.
///
/// Passed through verbatim so a round-trip through ngspice reproduces the original
/// deck's title exactly. An empty title (unreadable/empty entry file) still has to
/// occupy line 1 — something will be consumed as the title regardless, so it had
/// better be a placeholder and not the first real card.
fn title_line(title: &str) -> String {
    if title.trim().is_empty() {
        "*".to_string()
    } else {
        title.to_string()
    }
}

/// Parse `--cores <n>` (or `--cores=<n>`) from anywhere in argv.
///
/// Defaults to 1. A request for more is ACCEPTED but not yet honored, and says so
/// loudly — never let a user believe they got 4 cores when they got 1. See
/// `Config::effective_cores` for why the parser is sequential: it is 0.21s of a
/// 2.0s load, the rest being ngspice's own model ingest.
fn parse_cores(argv: &[String]) -> Result<ngparse::Config, String> {
    let mut val: Option<&str> = None;
    for (i, a) in argv.iter().enumerate() {
        if let Some(v) = a.strip_prefix("--cores=") {
            val = Some(v);
        } else if a == "--cores" {
            val = Some(argv.get(i + 1).map(String::as_str).unwrap_or(""));
        }
    }
    let Some(v) = val else {
        return Ok(ngparse::Config::single());
    };
    let n: usize = v
        .parse()
        .map_err(|_| format!("--cores: expected a positive integer, got {v:?}"))?;
    let n = NonZeroUsize::new(n).ok_or_else(|| "--cores: must be >= 1".to_string())?;
    Ok(ngparse::Config::with_cores(n))
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    // flags may appear anywhere after the subcommand
    let strict = argv.iter().any(|a| a == "--strict");
    // `--pspice` mirrors ngspice's `ngbehavior=ps`: apply the PSpice conversions
    // (if->ternary_fcn, VSWITCH->sw, VALUE={TABLE(..)}->native TABLE, pwr/pwrs/
    // stp/int) on emit. The glue sets this from ngbehavior; the flag lets the CLI
    // reproduce a ps-mode run.
    let compat = if argv.iter().any(|a| a == "--pspice") {
        ngparse::config::Compat::Pspice
    } else {
        ngparse::config::Compat::Default
    };
    // `--topo-reduce` removes dangling passives from the expanded deck (see
    // Config::topo_reduce). Off by default.
    let topo = argv.iter().any(|a| a == "--topo-reduce");
    let cfg = match parse_cores(&argv) {
        Ok(c) => c.with_compat(compat).with_topo_reduce(topo),
        Err(e) => {
            eprintln!("ngparse: {e}");
            return ExitCode::from(2);
        }
    };
    // `--cores <n>` puts a bare number in argv; drop it so it is not read as a path.
    let args: Vec<String> = {
        let mut out = Vec::new();
        let mut skip = false;
        for a in &argv {
            if skip {
                skip = false;
                continue;
            }
            if a == "--cores" {
                skip = true;
                continue;
            }
            if !a.starts_with("--") {
                out.push(a.clone());
            }
        }
        out
    };
    if args.len() < 3 {
        return usage(&argv[0]);
    }
    let (cmd, path) = (args[1].as_str(), args[2].as_str());

    match cmd {
        "lines" => {
            let src = match std::fs::read_to_string(path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("ngparse: cannot read {path}: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let lines = ngparse::logical_lines(&src, Arc::from(path));
            for l in &lines {
                println!("{}", l.text);
            }
            eprintln!("ngparse: {} logical lines", lines.len());
            ExitCode::SUCCESS
        }
        "flatten" => {
            let t0 = Instant::now();
            let mut ex = ngparse::Expander::new();
            match ex.expand_file(Path::new(path)) {
                Ok(lines) => {
                    for l in &lines {
                        println!("{}", l.text);
                    }
                    let dt = t0.elapsed();
                    eprintln!(
                        "ngparse: flattened to {} cards in {:.3}s",
                        lines.len(),
                        dt.as_secs_f64()
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("ngparse: flatten failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        "getp" => {
            // getp <deck> <param>  — flatten, collect, resolve one param (debug)
            let name = args.get(3).map(String::as_str).unwrap_or("");
            let mut ex = ngparse::Expander::new();
            let flat = match ex.expand_file(Path::new(path)) {
                Ok(l) => l,
                Err(e) => { eprintln!("flatten failed: {e}"); return ExitCode::FAILURE; }
            };
            let mut table = ngparse::params::ParamTable::new();
            table.collect(&flat);
            eprintln!("collected {} params", table.param_count());
            match table.eval_str(name) {
                Ok(v) => println!("{name} = {v}"),
                Err(e) => println!("{name} : ERROR {e}"),
            }
            ExitCode::SUCCESS
        }
        "expand" => {
            // full pipeline: flatten -> subckt-expand + param-resolve -> flat numeric deck
            let t0 = Instant::now();
            let mut ex = ngparse::Expander::with_config(cfg);
            let flat = match ex.expand_file(Path::new(path)) {
                Ok(l) => l,
                Err(e) => { eprintln!("ngparse: flatten failed: {e}"); return ExitCode::FAILURE; }
            };
            let t_flat = t0.elapsed();
            let se = ngparse::subckt::SubcktExpander::with_config(&flat, cfg);
            let r = se.expand();
            let dt = t0.elapsed();
            // Line 1 must be the title: whoever reads this deck back — ngspice via
            // `source`, or the C glue via if_inpdeck — consumes it and starts the
            // netlist at line 2. Without it the first real card is silently eaten.
            println!("{}", title_line(ex.title()));
            for c in &r.cards {
                println!("{c}");
            }
            eprintln!(
                "ngparse: expand done in {:.3}s (flatten {:.3}s) -> {} cards",
                dt.as_secs_f64(),
                t_flat.as_secs_f64(),
                r.cards.len()
            );
            // A dropped parameter is never silent: it means a device/model quietly
            // falls back to a DEFAULT, which yields a wrong-but-converging answer
            // (this exact failure mode gave foundry_a a dead transistor via u0=0, and
            // bxpressn-1 a malformed B source via a dropped `v=`).
            if !r.drops.is_empty() {
                eprintln!(
                    "ngparse: WARNING: {} parameter(s) could not be resolved and were DROPPED;",
                    r.drops.len()
                );
                eprintln!("  the affected device/model silently falls back to its DEFAULT value.");
                let show = 10.min(r.drops.len());
                for d in r.drops.iter().take(show) {
                    eprintln!("    {d}");
                }
                if r.drops.len() > show {
                    eprintln!(
                        "    ... and {} more (NGPARSE_DEBUG_DROP=1 to see every one)",
                        r.drops.len() - show
                    );
                }
                if strict {
                    eprintln!("ngparse: --strict: refusing to emit a deck with dropped parameters");
                    return ExitCode::FAILURE;
                }
                eprintln!("  (use --strict to make this an error)");
            }
            ExitCode::SUCCESS
        }
        "resolve" => {
            let t0 = Instant::now();
            let mut ex = ngparse::Expander::new();
            let flat = match ex.expand_file(Path::new(path)) {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("ngparse: flatten failed: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let t_flat = t0.elapsed();

            let mut table = ngparse::params::ParamTable::new();
            table.collect(&flat);
            let t_collect = t0.elapsed();

            let mut stats = ngparse::params::SubstStats::default();
            let mut nparam = 0usize;
            println!("{}", title_line(ex.title())); // line 1 is the title — see `expand`
            for l in &flat {
                let t = l.text.trim_start();
                if t.len() >= 6 && t[..6].eq_ignore_ascii_case(".param") {
                    nparam += 1;
                    continue; // .param lines are dropped from the resolved netlist
                }
                let out = ngparse::params::substitute_line(&table, &l.text, "0", &mut stats);
                println!("{out}");
            }
            let dt = t0.elapsed();
            eprintln!(
                "ngparse: resolve done in {:.3}s (flatten {:.3}s, collect {:.3}s)",
                dt.as_secs_f64(),
                t_flat.as_secs_f64(),
                (t_collect - t_flat).as_secs_f64()
            );
            eprintln!(
                "ngparse: {nparam} .param cards, {} inline exprs ({} failed to evaluate)",
                stats.exprs_total, stats.exprs_failed
            );
            ExitCode::SUCCESS
        }
        _ => usage(&args[0]),
    }
}
