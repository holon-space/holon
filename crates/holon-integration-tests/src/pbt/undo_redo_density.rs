//! Importance sampling for the undo→redo round trip.
//!
//! `inv-undo-redo-reference-heal` can only take a reading on a tick where a
//! `Redo` re-minted a block. Under the default keystone weights that shape is
//! effectively unreachable: `SplitBlock` alone carries weight 100 while
//! `UndoLastMutation` and `Redo` carry 2 each, and *any* intervening mutation
//! clears the redo stack. Measured on 2026-07-26: **zero** round trips across
//! ~77 generated sequences (~1500 ticks) — the invariant was selected the whole
//! time and measured nothing, which the engagement ledger cannot distinguish
//! from real coverage.
//!
//! So a sweep that wants a rate has to bias the sampler toward the rare shape.
//! This module is that knob, and nothing else:
//! `HOLON_PBT_UNDO_REDO_DENSITY=high` multiplies the generator weights of the
//! three transitions the round trip needs — `UndoLastMutation`, `Redo`, and
//! `PinBlock` (the only reference site reachable in the keystone that SURVIVES
//! an undo, because pins push no undo snapshot).
//!
//! **The default is untouched.** Unset (the keystone, every gate, every land
//! sweep) means multiplier 1 and byte-identical weights to before this module
//! existed; [`tests::default_is_exactly_one`] pins that.
//!
//! A rate measured under `high` is a CONDITIONAL rate — "given a round trip
//! occurs, how often is a reference left dangling" — and must be reported as
//! such. The unbiased sweeps measure the other half: how often the shape occurs
//! at all.

/// Env var selecting the sampling bias. Unset = the production keystone
/// distribution.
pub const DENSITY_ENV: &str = "HOLON_PBT_UNDO_REDO_DENSITY";

/// The only accepted value of [`DENSITY_ENV`].
pub const DENSITY_HIGH: &str = "high";

/// Weight multiplier applied under `high`. Chosen so the boosted transitions
/// (base weight 2) land at 80 — comparable to `SplitBlock`'s 100, so the round
/// trip is reachable without drowning out the mutations that make a tail to
/// re-mint in the first place.
const HIGH_MULTIPLIER: u32 = 40;

/// Parse [`DENSITY_ENV`] fail-loud, once. Unset or empty = 1 (no bias); the
/// documented value = [`HIGH_MULTIPLIER`]; anything else panics naming what was
/// passed, so a typo cannot silently produce an unbiased sweep the operator
/// believes was biased — the exact "looks fine, measured nothing" failure this
/// whole lane exists to eliminate.
fn multiplier() -> u32 {
    static CACHED: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *CACHED.get_or_init(|| match std::env::var(DENSITY_ENV) {
        Err(std::env::VarError::NotPresent) => 1,
        Err(std::env::VarError::NotUnicode(v)) => panic!(
            "{DENSITY_ENV} is not valid unicode ({v:?}); expected either unset or `{DENSITY_HIGH}`"
        ),
        Ok(v) if v.is_empty() => 1,
        Ok(v) if v == DENSITY_HIGH => HIGH_MULTIPLIER,
        Ok(v) => panic!(
            "{DENSITY_ENV}={v:?} is not a recognised value. The ONLY accepted value is \
             `{DENSITY_HIGH}`; unset (or empty) is the default keystone distribution."
        ),
    })
}

/// The generator weight a round-trip-participating transition should declare.
/// Identity unless [`DENSITY_ENV`] is set.
pub fn weight(base: u32) -> u32 {
    base * multiplier()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keystone's distribution must be EXACTLY unchanged when the knob is
    /// unset. Runs in the normal test process, where the var is never set.
    #[test]
    fn default_is_exactly_one() {
        assert_eq!(
            std::env::var(DENSITY_ENV).ok(),
            None,
            "this test asserts the UNSET default; something set {DENSITY_ENV}"
        );
        assert_eq!(weight(2), 2);
        assert_eq!(weight(100), 100);
    }
}
