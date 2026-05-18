//! Phase 7 invariant bodies — each invariant migrates from an inline
//! closure in `sut.rs::check_invariants_async` to a free-function-style
//! `Invariant<R, S>` impl with explicit capability-trait bounds.
//!
//! Slice opt-in is structural: an invariant whose `where` clause the
//! slice's `S` doesn't satisfy simply doesn't compile into that slice's
//! invariant tuple. The wide PBT's `check_invariants_async` retains the
//! inline assertions until every invariant is migrated, then deletes
//! them in Phase 10.

pub mod loro_no_errors;
