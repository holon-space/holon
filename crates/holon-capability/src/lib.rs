//! @c4 component
//! @c4 layer Adapters
//! Pattern: Specification
//! @c4 uses holon-api "the Value carriers a profile constrains" "Rust"
//!
//! Capability profiles: what each durable FORMAT can carry without loss.
//!
//! A profile is DATA — a yaml file next to the adapter, parsed here into
//! closed enums — not a type parameter. Datatypes are runtime-declared, so
//! capability cannot be monomorphized (design Fork 2).
//!
//! The point of the crate is that a clause can be FALSIFIED: [`certify`]
//! drives the format's real round trip and reports a [`Violation`] for each
//! declared restriction that is not real, and a [`TighteningPrompt`] for each
//! restriction the format does not actually need.
//!
//! **Every CERTIFIED clause is falsifiable; uncertified clauses are marked as
//! such in the yaml.** Increment 2b.1 certifies three:
//! `property_keys.reserved_prefixes`, `property_values.types` and
//! `property_values.empty_string`. The rest of axes 3 and 4 are declared with a
//! `file:line` citation and carry `# NOT YET CERTIFIED (2b.2)` — a citation is
//! EVIDENCE, not a gate, and a reader must not mistake one for the other. A
//! profile is only as trustworthy as the clauses the certifier actually drives.
//!
//! ## Dependency direction, both ways
//!
//! This crate depends on `holon-api` and on NO format crate. The reverse is
//! equally forbidden: a format crate must not gain a non-test dependency on
//! this one, or it starts reading its own profile at runtime and the profile
//! stops being an independent statement ABOUT it. Each [`CertifiableFormat`]
//! impl therefore lives in the format crate's `tests/` directory. Both
//! directions are pinned by
//! `crates/holon-architecture-tests/tests/architecture_rules.rs`.

pub mod axes;
pub mod certify;
pub mod clause;
pub mod diff;
#[cfg(test)]
pub(crate) mod fixture;
pub mod profile;
pub mod registry;
pub mod supports;
pub mod violation;

pub use axes::AssetsAxis;
pub use axes::Attachments;
pub use axes::BinaryInline;
pub use axes::BlockConstruct;
pub use axes::CarrierDisagreement;
pub use axes::Collision;
pub use axes::ComputedAxis;
pub use axes::ComputedLive;
pub use axes::ComputedPersisted;
pub use axes::ConcurrentInsert;
pub use axes::ConflictSurface;
pub use axes::ConstraintId;
pub use axes::ContentAxis;
pub use axes::ContentRepresentation;
pub use axes::Cycles;
pub use axes::Extension;
pub use axes::HierarchyAxis;
pub use axes::HierarchyShape;
pub use axes::HostedKind;
pub use axes::IdCarrier;
pub use axes::IdOrigin;
pub use axes::IdSpace;
pub use axes::IdentityAxis;
pub use axes::InlineConstruct;
pub use axes::KeyCase;
pub use axes::KeyCharset;
pub use axes::MaxDepth;
pub use axes::MergeGranularity;
pub use axes::MultiValue;
pub use axes::MultiValueScope;
pub use axes::MultiValueSemantics;
pub use axes::MutationAxis;
pub use axes::OrderKeyDurability;
pub use axes::OrderingAxis;
pub use axes::PropertyKey;
pub use axes::PropertyKeysAxis;
pub use axes::PropertyOrder;
pub use axes::PropertyValuesAxis;
pub use axes::ReferenceValues;
pub use axes::RenameKind;
pub use axes::Reparent;
pub use axes::Representability;
pub use axes::ReservedPrefix;
pub use axes::SchemaRequirement;
pub use axes::Separator;
pub use axes::SiblingOrder;
pub use axes::ValueKind;
pub use axes::WriteLeg;
pub use axes::WriteUnit;
pub use certify::Carrier;
pub use certify::CertifiableFormat;
pub use certify::CertificationReport;
pub use certify::ConstructOutcome;
pub use certify::DisagreementOutcome;
pub use certify::MultiValueReadback;
pub use certify::Readback;
pub use certify::ReferenceReadback;
pub use certify::WriteAttempt;
pub use certify::certify;
pub use clause::ClauseId;
pub use clause::CoverageGap;
pub use clause::DeferralSite;
pub use clause::DeferredClause;
pub use clause::EnforcementLayer;
pub use clause::EnforcementMap;
pub use clause::GapReason;
pub use diff::CapabilityLoss;
pub use profile::CapabilityProfile;
pub use profile::CapabilityProfileId;
pub use profile::FidelityAxes;
pub use profile::ProfileRevision;
pub use registry::ProfileRegistry;
pub use supports::Feature;
pub use supports::HomeSupport;
pub use supports::Support;
pub use violation::Axis;
pub use violation::Clause;
pub use violation::Leg;
pub use violation::Outcome;
pub use violation::TighteningPrompt;
pub use violation::Violation;
