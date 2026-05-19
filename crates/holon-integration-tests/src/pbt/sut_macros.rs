//! Shared declarative macros used by the `sut*` sibling modules.
//!
//! Extracted from `sut.rs` (Phase D4) so the macro is reachable from
//! both `sut.rs` and the `sut_check_invariants.rs` extracted impl block.

/// Run one or more capability-bound `Invariant<R, S>` checks against
/// `$ref_state` and `$sut`. Panics on `Fail`, ignores `Ok`/`Skipped`.
/// Use when the default panic message (`{msg}`) is enough; sites that
/// enrich the panic with extra diagnostics keep their manual match.
macro_rules! assert_invariants {
    ($ref_state:expr, $sut:expr $(, $inv:expr )+ $(,)?) => {{
        $(
            match ::holon_pbt_core::invariant::Invariant::check(
                &$inv, $ref_state, $sut,
            ).await {
                ::holon_pbt_core::invariant::InvariantResult::Ok => {}
                ::holon_pbt_core::invariant::InvariantResult::Fail(msg) => panic!("{msg}"),
                ::holon_pbt_core::invariant::InvariantResult::Skipped(_) => {}
            }
        )+
    }};
}

pub(super) use assert_invariants;
