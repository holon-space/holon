//! Entity identity skeleton.
//!
//! Schema-and-operations seam so every future integration plugs into one
//! canonical-identity surface instead of growing ad-hoc identity columns.
//!
//! - **Schema** (already landed): `canonical_entity`, `entity_alias`,
//!   `proposal_queue`. See [`IdentitySchemaModule`](crate::storage::IdentitySchemaModule).
//! - **Operations** (this module): user-facing `merge_entities`,
//!   `propose_merge`, `accept_proposal`, `reject_proposal`; internal undo
//!   primitives `restore_canonical_after_merge`, `delete_proposal`,
//!   `revert_proposal_status`. All routed through `OperationDispatcher` and
//!   logged by `OperationLogObserver` for undo/redo replay.

mod provider;

pub use provider::{ENTITY_NAME, IdentityProvider, SHORT_NAME};
