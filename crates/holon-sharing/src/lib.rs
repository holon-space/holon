//! `holon-sharing` — ADR 0028 sharing machinery (Increments 4-6: the H2
//! owner-scoped, totally-ordered crossing log + its migration/undo/enforcement
//! seams; the D4/H8 owner-signed policy objects + lease membership; and the H4
//! owner-private alias ledger / re-encode baseline).
//!
//! ## What lands here (Inc 5 — [`policy`], [`lease`])
//! - [`policy::Policy`] — owner-signed `{selector: stable block id (H6, never a
//!   name), principals, capabilities, delegation, lease}`; committed through
//!   [`policy::PolicySet`], which enforces the **A7 disjointness invariant**
//!   (nested/overlapping selectors rejected loud at commit) and mirrors each
//!   commit as a `PolicyEdit` into the Inc-4 log.
//! - [`lease`] — lease-based membership (H8): short-lived certs measured on the
//!   injected [`holon_api::Clock`] seam (NO `SystemTime::now`), **revocation =
//!   non-renewal**, and owner→delegate→peer chain verification
//!   ([`lease::verify_membership`]) — else membership is unclaimable.
//! - [`policy::evaluate_grant`] — a non-owner grant without delegation rights
//!   becomes a **pending request** (D4).
//!
//! ## What lands here (Inc 6 — [`alias_ledger`])
//! - [`alias_ledger::AliasLedger`] — owner-private,
//!   [`alias_ledger::NonReplicated`] (OQ3) `old-id → new-id` chains; re-encode
//!   + owner-signed [`alias_ledger::SuccessionPointer`] baseline. Owner
//!   backlinks rewrite through the SQL `block_links` junction; recipients get
//!   fresh public ids with no correlation handle and their dangling refs render
//!   loud (fail-loud).
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
//! ## Signature shape (OQ4) — REAL as of the W4 ceremony work
//! Every entry carries an [`types::OwnerSig`] produced by a
//! [`types::SigningAuthority`]. The production impl is
//! [`types::OwnerKeyAuthority`], backed by the durable Ed25519
//! [`holon_loro::owner_identity::OwnerIdentityKey`]; its signatures verify
//! under the owner's public key via [`types::OwnerSig::verify`] and
//! [`log::CrossingLog::verify_all`]. [`types::UnverifiedAuthority`] (a blake3
//! stand-in) remains ONLY as a test double and its output is REJECTED by
//! verification. Swapping authorities required no log-layout change (the field
//! shape was committed at Inc 4).

pub mod alias_ledger;
pub mod arbitration;
pub mod boundary;
pub mod journal;
pub mod lease;
pub mod log;
pub mod policy;
pub mod projection;
pub mod types;

pub use alias_ledger::AliasLedger;
pub use alias_ledger::NonReplicated;
pub use alias_ledger::SuccessionPointer;
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
pub use lease::DelegationError;
pub use lease::Issuer;
pub use lease::Lease;
pub use lease::MembershipCert;
pub use lease::MembershipChain;
pub use lease::MembershipError;
pub use lease::issue_delegated_cert;
pub use lease::verify_membership;
pub use log::CrossingLog;
pub use log::WitnessError;
pub use policy::Capabilities;
pub use policy::Capability;
pub use policy::GrantOutcome;
pub use policy::Granter;
pub use policy::MapContainment;
pub use policy::PendingRequest;
pub use policy::Policy;
pub use policy::PolicyCommitError;
pub use policy::PolicySet;
pub use policy::Principal;
pub use policy::SignedPolicy;
pub use policy::SubtreeContainment;
pub use policy::UnverifiedVerifier;
pub use policy::VerifyingAuthority;
pub use policy::evaluate_grant;
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
pub use types::OwnerKeyAuthority;
pub use types::OwnerSig;
pub use types::PolicyChange;
pub use types::PolicyEdit;
pub use types::PolicyEditId;
pub use types::SigningAuthority;
pub use types::StablePeerId;
pub use types::UnverifiedAuthority;

mod undo;
pub use undo::UndoError;
