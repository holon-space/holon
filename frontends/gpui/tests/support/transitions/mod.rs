//! File-per-variant `TransitionImpl` impls for the GPUI layout PBT.
//!
//! Each module under here owns the `impl TransitionImpl<(), GpuiInteractionSession<'_>>`
//! for exactly one variant from `holon-pbt-core`. The variant body
//! orchestrates the GPUI primitives exposed by `GpuiInteractionSession`
//! (clicking by element id, delivering deferred block content, etc.)
//! — the SUT type stays a thin bag of primitives, the per-variant
//! semantics ("switch view mode = click VMS button at canonical id")
//! live in the variant's own file.
//!
//! The orphan rule is satisfied because each impl mentions
//! `GpuiInteractionSession`, which is local to this crate.

pub mod deliver_block_content;
pub mod switch_view_mode;
pub mod toggle_collapse;
pub mod toggle_drawer;
