/*
 * ngparse glue — use the ngparse Rust parser for deck expansion.
 *
 * ngparse replaces the slow part of the frontend: .lib/.inc section extraction,
 * numparam substitution and .subckt expansion.  On the foundry_b 14LPU PDK deck
 * that takes model load from 15m27s to ~2s.
 *
 * The glue is deliberately shallow.  ngparse expands the top deck into a flat,
 * resolved netlist; inp_readall() is then run over THAT, so every compatibility
 * pass it performs (inp_compat, inp_bsource_compat, inp_dot_if,
 * inp_temper_compat, inp_meas_control, inp_add_series_resistor, renumbering)
 * still happens, and inp_subcktexpand() still drives numparam over whatever
 * expressions ngparse deliberately left symbolic (anything depending on
 * `temper`, v(), i(), time, ...).  .lib/.inc expansion simply finds nothing left
 * to do.  Nothing downstream of expansion changes.
 *
 * Opt-in, and OFF by default:
 *
 *     ngspice deck.cir                            parses as it always has
 *     ngspice --ngparse deck.cir                  uses ngparse (single core)
 *     ngparse_cores=4 ngspice --ngparse deck.cir  asks for 4 cores
 *
 * ngparse_cores asks for more than one core; there is no reason to set
 * it to 1, which is the default.  It is a forward-compatibility hook: expansion
 * is single-threaded today and says so if asked for more.
 */
#ifndef NGPARSE_GLUE_H
#define NGPARSE_GLUE_H

#include <stdio.h>          /* FILE (ngparse_glue_realpath) */

#include "ngspice/bool.h"

/* Turn ngparse on/off.  Called by main.c for `--ngparse`; off by default. */
void ngparse_glue_request(bool on);

/* TRUE if ngparse was requested AND can handle this source.  Command files
 * (*ng_script) are .control scripts, not netlists, and an internal/array deck
 * has no file to read, so both take the normal path. */
bool ngparse_glue_enabled(bool comfile, bool intfile, const char *filename);

/*
 * Expand `filename` with ngparse into a temporary netlist.
 *
 * Returns a malloc'd path the caller must unlink() and tfree(), or NULL on
 * failure (reported to stderr).  The caller opens it and passes it to
 * inp_readall() in place of the original file.
 *
 * A temporary file rather than an in-memory hand-off because it reproduces
 * exactly the `source <expanded deck>` path that ngparse is validated against;
 * the write/read costs a few ms against a ~2s load.
 */
char *ngparse_glue_expand(const char *filename);

/*
 * Resolve the real path of the file ngspice already opened (`fp`, located via
 * sourcepath/inputdir) so ngparse reads THAT rather than re-opening the possibly
 * relative `filename` against the process cwd. Returns a malloc'd path the caller
 * must tfree(); falls back to a copy of `filename` when the fp path is
 * unavailable. Pass the result to ngparse_glue_enabled()/ngparse_glue_expand().
 */
char *ngparse_glue_realpath(FILE *fp, const char *filename);

#endif /* NGPARSE_GLUE_H */
