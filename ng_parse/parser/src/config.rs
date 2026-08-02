//! Run configuration — the core count and the compatibility dialect.

use std::num::NonZeroUsize;

/// Which input dialect ngparse should emulate when it emits the flat deck.
///
/// ngparse replaces ngspice's expansion front end, so the PSpice conversions that
/// ngspice's own `pspice_compat` (inpcompat.c) applies — `if`→`ternary_fcn`,
/// `VSWITCH`→`sw`, `VALUE={TABLE(..)}`→native `TABLE`, `pwr`/`pwrs`/`stp`/`int`
/// → native — never run on our output: that pass is triggered per `.include`d
/// file, and we have already inlined every include. In [`Compat::Pspice`] we do
/// those conversions ourselves so a `ngbehavior=ps` run matches the reference.
///
/// The mode is not discoverable from the deck (it lives in `ngbehavior`, set in
/// spinit), so the C glue reads it via `cp_getvar("ngbehavior", ..)` and passes
/// it across the FFI. [`Compat::Default`] is ngparse's long-standing behavior and
/// covers the HSPICE/standard decks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Compat {
    /// Standard / HSPICE-style — ngparse's original behavior. No PSpice rewrites.
    #[default]
    Default,
    /// PSpice (`ngbehavior=ps`): apply the `pspice_compat` conversions on emit.
    Pspice,
}

impl Compat {
    /// Map ngspice's `ngbehavior` string to a mode. Only an exact/leading `ps`
    /// selects PSpice; everything else (hs, spe, unset) is [`Compat::Default`],
    /// matching how ngspice itself keys `newcompat.ps`.
    pub fn from_ngbehavior(s: &str) -> Compat {
        if s.trim().to_ascii_lowercase().starts_with("ps") {
            Compat::Pspice
        } else {
            Compat::Default
        }
    }

    /// From the FFI integer: 1 = PSpice, anything else = Default.
    pub fn from_ffi(v: i32) -> Compat {
        if v == 1 {
            Compat::Pspice
        } else {
            Compat::Default
        }
    }
}

/// How ngparse should run. Carried by [`crate::Expander`] and
/// [`crate::subckt::SubcktExpander`] so the parallel seams have somewhere to read
/// their budget from without an API change later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    /// Worker cores for subckt expansion. Defaults to 1; more fans the
    /// independent top-level cards across threads (see
    /// `SubcktExpander::expand_parallel`), byte-identical to single-core.
    ///
    /// Single-core is the default because on today's decks expansion is small:
    /// the whole foundry_b expand is ~0.21s of a ~2.0s load, the rest being
    /// ngspice's own `INPpas1/2/3` and BSIM-CMG/OSDI model setup, which run after
    /// us and which parser cores cannot speed up. Multi-core is built and tested
    /// for the decks not seen yet — a library an order of magnitude larger could
    /// put real time into expansion, where more cores pay off.
    pub cores: NonZeroUsize,

    /// Input dialect to emulate on emit — see [`Compat`]. Defaults to
    /// [`Compat::Default`]; the C glue raises it to [`Compat::Pspice`] when
    /// `ngbehavior=ps`.
    pub compat: Compat,

    /// Remove dangling passives (two-terminal R/C whose far node is referenced
    /// nowhere else) from the expanded deck. This is the "reduce EARLY, remove
    /// the device entirely" scheme ngspice's own during-setup attempt (commit
    /// aac195, since reverted) could not deliver: done here, `.probe`/`.save`
    /// and AC see only surviving devices and the matrix shrinks. OFF by
    /// default — the emitted deck stays byte-identical unless requested via
    /// `--topo-reduce` (CLI) or `NGPARSE_TOPO_REDUCE=1` (integrated build).
    pub topo_reduce: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            cores: NonZeroUsize::new(1).unwrap(),
            compat: Compat::Default,
            topo_reduce: false,
        }
    }
}

impl Config {
    /// Sequential — the default.
    pub fn single() -> Self {
        Config::default()
    }

    /// Request `cores` worker threads for subckt expansion. 1 (the default) runs
    /// inline; more fans the independent top-level cards across that many threads.
    pub fn with_cores(cores: NonZeroUsize) -> Self {
        Config {
            cores,
            ..Config::default()
        }
    }

    /// Set the compatibility dialect, keeping other settings.
    pub fn with_compat(mut self, compat: Compat) -> Self {
        self.compat = compat;
        self
    }

    /// Enable/disable dangling-passive removal, keeping other settings.
    pub fn with_topo_reduce(mut self, on: bool) -> Self {
        self.topo_reduce = on;
        self
    }

    /// True in PSpice dialect ([`Compat::Pspice`]).
    pub fn is_pspice(&self) -> bool {
        self.compat == Compat::Pspice
    }

    /// The number of worker cores this run will actually use for expansion.
    ///
    /// Subckt expansion ([`crate::subckt`]) fans the independent top-level cards
    /// across this many threads (see `SubcktExpander::expand_parallel`); the
    /// default is 1. The `.lib`/`.inc` walk ([`crate::preprocess`]) stays
    /// sequential — a file must be read and indexed before we learn what it pulls
    /// in, so the reference chain is discovered as it is walked.
    pub fn effective_cores(&self) -> usize {
        self.cores.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_single_core() {
        let c = Config::default();
        assert_eq!(c.cores.get(), 1);
        assert_eq!(c.effective_cores(), 1);
    }

    /// A multi-core request is honored: effective_cores reflects it.
    #[test]
    fn extra_cores_are_honored() {
        let c = Config::with_cores(NonZeroUsize::new(4).unwrap());
        assert_eq!(c.cores.get(), 4);
        assert_eq!(c.effective_cores(), 4);
    }
}
