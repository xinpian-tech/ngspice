//! ngparse — a fast SPICE/HSPICE deck preprocessor and parser for ngspice.
//!
//! Goal: replace ngspice's O(n^2) text-expansion frontend (`.lib` extraction +
//! `numparam` substitution + `.subckt` expansion) with a single-pass, optionally
//! parallel parser, and hand ngspice a fully-resolved card stream. See the project
//! README and `docs/` for architecture.
//!
//! This crate is developed standalone-first: it can emit the flattened, resolved
//! deck as text so its output can be validated against ngspice's own expanded-deck
//! dump before any C glue is wired up.

pub mod config;
pub mod expr;
pub mod ffi;
pub mod params;
pub mod preprocess;
pub mod reader;
pub mod subckt;
pub mod table;

pub use config::{Compat, Config};
pub use preprocess::{ExpandError, Expander};
pub use reader::{logical_lines, LogicalLine};
