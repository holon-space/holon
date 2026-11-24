//! @c4 component
//! @c4 layer Engine
//! Pattern: Policy
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-core "core datasource traits" "Rust"
//! @c4 uses holon-loro "CRDT substrate for the owner log" "Rust"
//!
//! `holon-sharing` — the ADR 0028 sharing machinery: the owner-scoped,
//! totally-ordered crossing log, owner-signed policy with lease membership,
//! the receiver-side admission of synced envelopes, and the owner-private
//! alias ledger.
//!
//! The log arbitrates *policy*, which is not Loro's concern, so this is its own
//! crate on top of the Loro substrate (`doc_lamport_height` for the owner-log
//! clock, delta export/import for merge).
//!
//! ## Modules
//! - [`log::CrossingLog`] — one dedicated `LoroDoc` per owner scope; entries
//!   are stamped with the frozen tuple `(lamport_height(log_doc) at append,
//!   stable_peer_id)`; crossings and policy edits share the one log.
//! - [`arbitration`] — concurrent divergent crossings ordered
//!   deterministically; the loser is rejected loudly and returned as a
//!   *keepable divergent copy*.
//! - [`journal`] — idempotent, resumable migration; the journal is the unit of
//!   atomicity; leak direction: widen = create-shared-first, narrow =
//!   delete-shared-first.
//! - [`undo`] — inverse-crossing undo through the same log; a rejected loser
//!   never enters the undo stack.
//! - [`policy`] — owner-signed `{selector: stable block id, principals,
//!   capabilities, delegation, lease}` committed through [`policy::PolicySet`],
//!   which rejects nested or overlapping selectors at commit and mirrors each
//!   commit as a `PolicyEdit` into the log. [`policy::evaluate_grant`] turns a
//!   non-owner grant without delegation rights into a pending request.
//! - [`lease`] — short-lived membership certs measured on the injected
//!   [`holon_api::Clock`]; revocation = non-renewal; owner→delegate→peer chain
//!   verification ([`lease::verify_membership`]).
//! - [`acceptor`] and [`sync`] — the receiver's admit/refuse decision for one
//!   envelope, and the push/pull round over a [`holon_loro`] sync transport.
//! - [`enforcement`], [`boundary`] — the [`holon_core`] boundary-enforcer
//!   implementation over the policy set, and the allow/reject-loud decision for
//!   one [`holon_api::BoundaryBehavior`] (`Unclassified` fails closed).
//! - [`projection`], [`registry_binding`] — SQL projection semantics across the
//!   migration window and the orphan-row tripwire, bound to the container
//!   registry.
//! - [`alias_ledger::AliasLedger`] — owner-private, never replicated `old-id →
//!   new-id` chains and the owner-signed [`alias_ledger::SuccessionPointer`];
//!   recipients get fresh public ids with no correlation handle.
//!
//! ## Signatures
//! Every log entry carries a [`types::OwnerSig`] from a
//! [`types::SigningAuthority`]. [`types::OwnerKeyAuthority`] signs with the
//! durable Ed25519 owner key and verifies via [`types::OwnerSig::verify`] and
//! [`log::CrossingLog::verify_all`]. [`types::UnverifiedAuthority`] is a blake3
//! test double whose output verification rejects.

pub mod acceptor;
pub mod alias_ledger;
pub mod arbitration;
pub mod boundary;
pub mod enforcement;
pub mod journal;
pub mod lease;
pub mod log;
pub mod policy;
pub mod projection;
pub mod registry_binding;
pub mod sync;
pub mod types;

pub use acceptor::AcceptorContext;
pub use acceptor::AdmitDecision;
pub use acceptor::admit;
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
pub use enforcement::PolicyOverlayEnforcer;
pub use enforcement::ROOT_CONTAINER;
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
pub use registry_binding::RegistryContainment;
pub use registry_binding::assert_registry_no_orphan_rows;
pub use sync::OutboundAuth;
pub use sync::SyncReport;
pub use sync::SyncSession;
pub use sync::sync_once;
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
