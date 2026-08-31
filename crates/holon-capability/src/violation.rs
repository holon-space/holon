//! What the certifier reports when a profile's declaration is not true.

use holon_api::Value;

use crate::axes::ValueKind;
use crate::profile::CapabilityProfileId;
use crate::profile::ProfileRevision;

/// Which fidelity axis the failing declaration belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Content,
    Tags,
    PropertyKeys,
    PropertyValues,
    Ordering,
    Identity,
    Hierarchy,
    Mutation,
    Assets,
}

impl std::fmt::Display for Axis {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Content => "content",
            Self::PropertyKeys => "property_keys",
            Self::PropertyValues => "property_values",
            Self::Ordering => "ordering",
            Self::Identity => "identity",
            Self::Hierarchy => "hierarchy",
            Self::Tags => "tags",
            Self::Mutation => "mutation",
            Self::Assets => "assets",
        })
    }
}

/// The specific clause that turned out not to hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Clause {
    /// The key was not covered by `reserved_prefixes`/`reserved_keys`, so the
    /// profile claims it survives as an ordinary property.
    KeyNotReserved,
    /// `property_keys.engine_owned_keys` lists this key, so authoring it must
    /// be refused by a message that names it — on EVERY author-reachable write
    /// route into the leg, which is why the route is part of the finding.
    EngineOwnedKey { route: &'static str },
    /// `property_values.types` lists this kind — and the claim must hold on
    /// EVERY author-reachable write route into the leg, not just the one the
    /// first probe happened to drive, which is why the route is part of the
    /// finding.
    TypeDeclared {
        kind: ValueKind,
        route: &'static str,
    },
    /// `property_values.empty_string` says this.
    EmptyString,
    /// `ordering.property_order` says this.
    PropertyOrder,
    /// `identity.carrier_disagreement` says this.
    CarrierDisagreement,
    /// `content.block_constructs` lists this.
    BlockConstruct(crate::axes::BlockConstruct),
    /// `content.inline_constructs` lists this.
    InlineConstruct(crate::axes::InlineConstruct),
    /// `hierarchy.max_depth` says this.
    MaxDepth,
    /// `hierarchy.constraints` names this rule.
    HierarchyConstraint(crate::axes::ConstraintId),
    /// `hierarchy.cycles` says this.
    Cycles,
    /// `mutation.write_leg` says this.
    WriteLeg,
    /// `assets.extensions` lists this.
    AssetExtension,
    /// `property_keys.case` says this.
    KeyCase,
    /// `property_keys.schema_required` says this.
    SchemaRequired,
    /// `property_values.null` says this.
    Null,
    /// `property_values.multi_value` says this.
    MultiValue,
    /// `property_values.reference_values` says this.
    ReferenceValues,
    /// `property_keys.collision` says this.
    Collision,
    /// `identity.id_origin` says this.
    IdOrigin,
    /// `identity.carriers` lists this.
    Carriers,
    /// `ordering.sibling_order` says this.
    SiblingOrder,
    /// `hosted_kinds` lists this.
    HostedKinds,
    /// `tags.attach_existing` says this.
    TagAttach,
    /// `tags.detach_existing` says this.
    TagDetach,
    /// `tags.unknown_reference` says this.
    TagUnknownReference,
    /// `content.representation` says this.
    Representation,
    /// `ordering.order_key_durable` says this.
    OrderKeyDurable,
    /// `hierarchy.shape` says this.
    Shape,
    /// `mutation.unit_of_write` says this.
    UnitOfWrite,
    /// `assets.attachments` / `assets.binary_inline` say this.
    Attachments,
    /// `identity.id_constraints` names this rule.
    IdConstraint(crate::axes::ConstraintId),
    /// `identity.id_space` says this.
    IdSpace,
}

impl std::fmt::Display for Clause {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::KeyNotReserved => write!(f, "the key is not declared reserved"),
            Self::EngineOwnedKey { route } => write!(
                f,
                "property_keys.engine_owned_keys lists the key (write route: {route})"
            ),
            Self::TypeDeclared { kind, route } => write!(
                f,
                "property_values.types lists {kind:?} (write route: {route})"
            ),
            Self::EmptyString => write!(f, "property_values.empty_string"),
            Self::PropertyOrder => write!(f, "ordering.property_order"),
            Self::CarrierDisagreement => write!(f, "identity.carrier_disagreement"),
            Self::BlockConstruct(c) => write!(f, "content.block_constructs lists {c:?}"),
            Self::InlineConstruct(c) => write!(f, "content.inline_constructs lists {c:?}"),
            Self::MaxDepth => write!(f, "hierarchy.max_depth"),
            Self::HierarchyConstraint(c) => write!(f, "hierarchy.constraints names {c:?}"),
            Self::Cycles => write!(f, "hierarchy.cycles"),
            Self::WriteLeg => write!(f, "mutation.write_leg"),
            Self::AssetExtension => write!(f, "assets.extensions"),
            Self::KeyCase => write!(f, "property_keys.case"),
            Self::SchemaRequired => write!(f, "property_keys.schema_required"),
            Self::Null => write!(f, "property_values.null"),
            Self::MultiValue => write!(f, "property_values.multi_value"),
            Self::ReferenceValues => write!(f, "property_values.reference_values"),
            Self::Collision => write!(f, "property_keys.collision"),
            Self::IdOrigin => write!(f, "identity.id_origin"),
            Self::Carriers => write!(f, "identity.carriers"),
            Self::SiblingOrder => write!(f, "ordering.sibling_order"),
            Self::HostedKinds => write!(f, "hosted_kinds"),
            Self::Representation => write!(f, "content.representation"),
            Self::OrderKeyDurable => write!(f, "ordering.order_key_durable"),
            Self::Shape => write!(f, "hierarchy.shape"),
            Self::UnitOfWrite => write!(f, "mutation.unit_of_write"),
            Self::Attachments => write!(f, "assets.attachments"),
            Self::IdConstraint(c) => write!(f, "identity.id_constraints names {c:?}"),
            Self::IdSpace => write!(f, "identity.id_space"),
            Self::TagAttach => write!(f, "tags.attach_existing"),
            Self::TagDetach => write!(f, "tags.detach_existing"),
            Self::TagUnknownReference => write!(f, "tags.unknown_reference"),
        }
    }
}

/// WHICH code path lost the value.
///
/// A format with more than one property carrier fails DIFFERENTLY per carrier
/// — org drops a non-string on its flat leg and stringifies it on its
/// `org_properties` JSON leg — so a violation that does not name the leg sends
/// the reader to the wrong function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Leg(pub &'static str);

impl std::fmt::Display for Leg {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// How the round trip broke the declaration.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The value did not come back at all.
    Dropped,
    /// The value came back as something else — the accept-then-quietly-change
    /// outcome that is always red.
    Changed { got: Value },
    /// The boundary REFUSED the value. Only a violation when the profile
    /// claimed the value is carried; a refusal of something undeclared is the
    /// law's other legal branch, not a defect.
    Refused { reason: String },
    /// The profile declared the boundary REFUSES this, and it did not — it
    /// took the value and lost it, which is the silent loss the declaration
    /// denies.
    NotRefused,
}

impl std::fmt::Display for Outcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Dropped => write!(f, "DROPPED"),
            Self::Changed { got } => write!(f, "CHANGED to {got:?}"),
            Self::Refused { reason } => write!(f, "REFUSED ({reason})"),
            Self::NotRefused => write!(f, "NOT REFUSED (declared an error, but taken and lost)"),
        }
    }
}

/// One declared-but-broken finding: the profile said this survives, and it did
/// not.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub profile: CapabilityProfileId,
    pub rev: ProfileRevision,
    pub axis: Axis,
    pub clause: Clause,
    pub leg: Leg,
    pub key: String,
    pub sent: Value,
    pub outcome: Outcome,
}

impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}@{}] axis={} leg={} key={:?} sent={:?} -> {} (clause: {})",
            self.profile,
            self.rev,
            self.axis,
            self.leg,
            self.key,
            self.sent,
            self.outcome,
            self.clause,
        )
    }
}

/// One works-but-undeclared finding: the format is BETTER than the profile
/// admits.
///
/// Never a failure (CV-C) — it is a tightening prompt, and it reaches a human
/// through the ledger, never by failing a gate.
#[derive(Debug, Clone, PartialEq)]
pub struct TighteningPrompt {
    pub profile: CapabilityProfileId,
    pub axis: Axis,
    pub leg: Leg,
    pub key: String,
    pub sent: Value,
    pub note: String,
}

impl std::fmt::Display for TighteningPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] axis={} leg={} key={:?} sent={:?}: {}",
            self.profile, self.axis, self.leg, self.key, self.sent, self.note,
        )
    }
}
