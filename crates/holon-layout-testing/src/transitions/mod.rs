//! Shared per-variant `TransitionFactory` + `TransitionImpl` impls for
//! the three UI variants in `holon-pbt-core`. Each file owns one
//! variant.
//!
//! These impls are reusable across every PBT and every frontend
//! because they're written against the [`crate::sut`] capability traits
//! and the [`crate::sut::LayoutRefState`] reference-state trait — the
//! orphan rule is satisfied via the local [`crate::sut::LayoutSut`] and
//! [`crate::sut::LayoutRef`] wrappers (see `sut.rs` docs).
//!
//! ## How a PBT consumes these
//!
//! 1. Implement [`crate::sut::Clickable`] + [`crate::sut::LiveBlockSink`]
//!    on the test session.
//! 2. Implement [`crate::sut::LayoutRefState`] on the reference-state
//!    type (or a generation-context struct for the layout PBT).
//! 3. At the dispatch site: wrap with `LayoutSut::new(&mut session)` /
//!    `LayoutRef::new(&ref_state)` and call
//!    `transition.apply_to_sut(...)` / `weighted_generator(...)`.
//! 4. Done — no per-variant code in the PBT crate.

pub mod deliver_block_content;
pub mod switch_view_mode;
pub mod toggle_collapse;
pub mod toggle_drawer;
