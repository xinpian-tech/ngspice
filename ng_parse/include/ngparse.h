/*
 * ngparse — fast SPICE/HSPICE deck expansion for ngspice.
 *
 * Replaces ngspice's text-expansion frontend (.lib/.inc extraction, numparam
 * substitution, .subckt expansion) and hands back the flat, fully-resolved card
 * list that if_inpdeck/INPpas1 expects.  Everything downstream of expansion —
 * eval_agauss(), ENHtranslate_poly(), inp_dodeck() — remains ngspice's job.
 *
 * Link against libngparse.a (plus -lpthread -ldl -lm).
 *
 *   NgpDeck *d = ngparse_expand_file(path, 1, 0);
 *   if (!d) { fprintf(stderr, "%s\n", ngparse_last_error()); return 1; }
 *   for (size_t i = 0; i < ngparse_deck_len(d); i++)
 *       puts(ngparse_deck_card(d, i));      // card 0 is the TITLE
 *   ngparse_deck_free(d);
 *
 * Ownership: every pointer returned belongs to the NgpDeck it came from and is
 * valid until ngparse_deck_free().  Do not free or modify them.  Copy anything
 * that must outlive the deck (ngspice's struct card owns its own line, so the
 * glue copies).
 */
#ifndef NGPARSE_H
#define NGPARSE_H

#include <stddef.h>

#ifdef __cplusplus
extern "C" {
#endif

/* An expanded deck. Opaque. */
typedef struct NgpDeck NgpDeck;

/*
 * Expand the deck at `path` (the top-level netlist) into resolved cards.
 *
 * cores: worker-core budget.  PASS 1.  Values > 1 are accepted for forward
 * compatibility but are not honored yet — expansion is single-threaded, and is
 * only ~10% of deck-load time (the rest is ngspice's own model ingest).  0 is
 * treated as 1.
 *
 * compat: input dialect.  1 = PSpice (ngspice's ngbehavior=ps); anything else =
 * default/HSPICE.  ngparse has taken over expansion, so ngspice's pspice_compat
 * pass no longer runs on the deck; in PSpice mode ngparse applies those
 * conversions (if->ternary_fcn, VSWITCH->sw, VALUE={TABLE()}->pwl, pwr/pwrs/
 * stp/int) itself.  The glue reads ngbehavior and passes it here.
 *
 * Returns NULL on failure; call ngparse_last_error() for the reason.
 * Release with ngparse_deck_free().
 */
NgpDeck *ngparse_expand_file(const char *path, int cores, int compat);

/* Number of cards, including the title at index 0. 0 if d is NULL. */
size_t ngparse_deck_len(const NgpDeck *d);

/*
 * Card i, or NULL if out of range.  INDEX 0 IS THE TITLE: SPICE consumes the
 * first card of a deck as the title and starts the netlist at index 1, so this
 * list must be handed over whole — dropping index 0 silently eats a real card.
 */
const char *ngparse_deck_card(const NgpDeck *d, size_t i);

/*
 * Parameters that could not be resolved.
 *
 * A drop is never harmless: the affected device or model silently falls back to
 * its DEFAULT, which yields a wrong-but-converging answer rather than an error.
 * Surface these; refuse the deck if you want the CLI's --strict behavior.
 */
size_t ngparse_deck_drop_count(const NgpDeck *d);
const char *ngparse_deck_drop(const NgpDeck *d, size_t i);

/* Release a deck.  NULL-safe.  Invalidates every pointer taken from it. */
void ngparse_deck_free(NgpDeck *d);

/* Last error on this thread, or NULL.  Owned by ngparse; do not free. */
const char *ngparse_last_error(void);

/* ngparse version string.  Static storage. */
const char *ngparse_version(void);

#ifdef __cplusplus
}
#endif

#endif /* NGPARSE_H */
