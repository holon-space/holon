//! File-per-transition pattern for `proptest-state-machine`.
//!
//! Each transition lives in its own file under `src/transitions/`,
//! holds its own struct + `impl E2ETransition`, and contributes a
//! generator strategy via the static `enabled()` method.
//!
//! Adding a transition = create a file + add one line to the
//! `declare_transitions!` invocation in `transitions.rs`. No match
//! arms, no central dispatch table.

pub mod reference;
pub mod sut;
pub mod transition;
#[macro_use]
pub mod macros;
pub mod transitions;
