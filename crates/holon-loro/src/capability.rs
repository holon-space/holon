//! Capability descriptor + consolidator selection.
//!
//! The types now live in `holon_api::capability` (the shared low layer) so that
//! every block-handling component — `holon-orgmode`, `holon-core`'s
//! `BlockOrdering`, and `holon` — can speak the same capability vocabulary
//! (D rework, `docs/Architecture/Replication.md` §2). Re-exported here for the
//! existing `crate::capability::…` call sites.
pub use holon_api::capability::CapabilityProfile;
pub use holon_api::capability::Consolidator;
pub use holon_api::capability::SessionCapabilities;
