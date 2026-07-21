//! `holon-sharing` — ADR 0028 sharing machinery (Increment 4: the H2
//! owner-scoped, totally-ordered crossing log + its migration/undo/enforcement
//! seams).
//!
//! Home ratified by OQ2: a NEW crate (not an extension of `holon-loro`) — the
//! log arbitrates *policy*, which is not Loro's concern. It reuses the Loro
//! substrate (`doc_lamport_height` for the dedicated owner-log clock, delta
//! export/import for merge) and the `share_peer_id` device-key derivation, but
//! owns the sharing domain types and arbitration rule.
//!
//! ## What lands here (Inc 4)
//! - [`log::CrossingLog`] — one dedicated `LoroDoc` per owner scope, one clock,
//!   entries stamped with the FROZEN tuple `(lamport_height(log_doc) at append,
//!   stable_peer_id)`; both crossings and policy edits in the one log.
//! - [`arbitration`] — concurrent divergent crossings ordered
//!   deterministically; the loser is rejected loudly and returned as a
//!   *keepable divergent copy*.
//! - [`journal`] — idempotent, resumable migration; the journal is the unit of
//!   atomicity; leak-direction contract (widen = create-shared-first, narrow =
//!   delete-shared-first).
//! - [`undo`] — inverse-crossing undo through the same log; a rejected loser
//!   NEVER enters the undo stack.
//! - [`projection`] — SQL projection semantics across the migration window +
//!   the orphan-row tripwire (wired at Inc 3).
//! - [`boundary`] — the first runtime consumer of
//!   [`holon_api::BoundaryBehavior`]: the allow/reject-loud decision function,
//!   incl. the `Unclassified` fail-closed contract and the descriptor-less-op
//!   classification design.
//!
//! ## Signature shape (OQ4) — DISCLOSED not-yet-verifiable
//! Every entry carries an [`types::OwnerSig`] produced by a
//! [`types::SigningAuthority`]. At Inc 4 the only impl is
//! [`types::UnverifiedAuthority`] (a blake3 stand-in): entries are
//! signed-*shaped* but the signature does not yet verify owner identity. Real
//! owner-identity key custody lands with Inc 5 (OQ4) — swapping the authority
//! impl requires no log-layout change.

pub mod arbitration;
pub mod boundary;
pub mod journal;
pub mod log;
pub mod projection;
pub mod types;

pub use arbitration::Arbitration;
pub use arbitration::DivergentCopy;
pub use arbitration::MalformedCrossing;
pub use arbitration::RejectedCrossing;
pub use arbitration::arbitrate;
pub use boundary::BoundaryDecision;
pub use boundary::check_boundary;
pub use journal::JournalStep;
pub use journal::MigrationJournal;
pub use journal::MigrationOp;
pub use journal::StepStatus;
pub use log::CrossingLog;
pub use log::WitnessError;
pub use projection::OrphanRowError;
pub use projection::assert_no_orphan_rows;
pub use types::BlockContent;
pub use types::BlockId;
pub use types::ContainerId;
pub use types::Crossing;
pub use types::CrossingId;
pub use types::CrossingKey;
pub use types::LogEntry;
pub use types::LogEntryBody;
pub use types::OwnerSig;
pub use types::PolicyChange;
pub use types::PolicyEdit;
pub use types::PolicyEditId;
pub use types::SigningAuthority;
pub use types::StablePeerId;
pub use types::UnverifiedAuthority;

mod undo;
pub use undo::UndoError;
