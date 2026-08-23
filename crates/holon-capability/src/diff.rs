//! What moving content from one home to another COSTS.
//!
//! `source.diff(&target)` answers one question per clause: does the target
//! declare less than the source, and if so, what is lost. It is DIRECTIONAL —
//! `org.diff(native)` and `native.diff(org)` are different questions — and it
//! is TOTAL over `ALL_CLAUSES`, because a clause the comparison forgets is a
//! loss nobody is told about, which is the promotion story's whole hazard.
//!
//! Totality is enforced by the compiler: [`loss_for`] matches every
//! [`ClauseId`] with no wildcard arm, so adding a clause to the vocabulary
//! without deciding how it diffs does not build.

use crate::axes::Attachments;
use crate::axes::BinaryInline;
use crate::axes::Collision;
use crate::axes::ComputedLive;
use crate::axes::ComputedPersisted;
use crate::axes::ConflictSurface;
use crate::axes::ContentRepresentation;
use crate::axes::Cycles;
use crate::axes::ExpressionClosure;
use crate::axes::HierarchyShape;
use crate::axes::KeyCase;
use crate::axes::KeyCharset;
use crate::axes::MaxDepth;
use crate::axes::MergeGranularity;
use crate::axes::MultiValue;
use crate::axes::PropertyOrder;
use crate::axes::Reparent;
use crate::axes::Representability;
use crate::axes::SchemaRequirement;
use crate::axes::SiblingOrder;
use crate::axes::TagMinting;
use crate::axes::TagWrite;
use crate::axes::UnknownTagReference;
use crate::axes::WriteLeg;
use crate::axes::WriteUnit;
use crate::clause::ALL_CLAUSES;
use crate::clause::ClauseId;
use crate::profile::CapabilityProfile;
use crate::profile::CapabilityProfileId;

/// One thing the target cannot carry that the source can.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityLoss {
    pub from: CapabilityProfileId,
    pub to: CapabilityProfileId,
    pub clause: ClauseId,
    /// What the source declares, in the clause's own vocabulary.
    pub source: String,
    /// What the target declares instead.
    pub target: String,
    /// What a user would notice.
    pub effect: String,
}

impl std::fmt::Display for CapabilityLoss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} → {}: {} ({} → {}) — {}",
            self.from, self.to, self.clause, self.source, self.target, self.effect
        )
    }
}

impl CapabilityProfile {
    /// What moving content from THIS home to `target` loses.
    ///
    /// Empty means the target carries everything this profile declares. It
    /// does NOT mean the two are equal: the target may carry more, which costs
    /// the mover nothing.
    pub fn diff(&self, target: &CapabilityProfile) -> Vec<CapabilityLoss> {
        ALL_CLAUSES
            .iter()
            .filter_map(|clause| loss_for(*clause, self, target))
            .collect()
    }
}

/// A set-valued clause: everything in `source` the `target` does not name.
///
/// Takes iterators rather than one collection type: the vocabulary stores some
/// member sets as `BTreeSet` and some as `Vec`, and that is a storage detail
/// this comparison must not care about.
fn missing<'a, T>(
    source: impl IntoIterator<Item = &'a T>,
    target: impl IntoIterator<Item = &'a T>,
) -> Vec<String>
where
    T: PartialEq + std::fmt::Debug + 'a,
{
    let target: Vec<&T> = target.into_iter().collect();
    source
        .into_iter()
        .filter(|m| !target.contains(m))
        .map(|m| format!("{m:?}"))
        .collect()
}

/// The per-clause comparison. NO wildcard arm — see the module doc.
fn loss_for(
    clause: ClauseId,
    from: &CapabilityProfile,
    to: &CapabilityProfile,
) -> Option<CapabilityLoss> {
    let loss = |source: String, target: String, effect: &str| {
        Some(CapabilityLoss {
            from: from.id().clone(),
            to: to.id().clone(),
            clause,
            source,
            target,
            effect: effect.to_string(),
        })
    };
    let lost_members = |lost: Vec<String>, effect: &str| {
        if lost.is_empty() {
            None
        } else {
            Some(CapabilityLoss {
                from: from.id().clone(),
                to: to.id().clone(),
                clause,
                source: lost.join(", "),
                target: "not declared".to_string(),
                effect: effect.to_string(),
            })
        }
    };

    match clause {
        ClauseId::HostedKinds => lost_members(
            missing(from.hosted_kinds(), to.hosted_kinds()),
            "an entity of this kind has no home in the target",
        ),

        ClauseId::ContentRepresentation => {
            use ContentRepresentation::*;
            // Ordered by how much structure survives. Moving DOWN this order
            // loses structure; moving up costs nothing.
            let rank = |r: ContentRepresentation| match r {
                None => 0,
                OpaqueText => 1,
                MarkedText => 2,
                StructuredTree => 3,
            };
            let (a, b) = (from.content().representation, to.content().representation);
            (rank(b) < rank(a)).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "content structure is flattened on arrival",
            )
        }
        ClauseId::ContentInlineConstructs => lost_members(
            missing(
                &from.content().inline_constructs,
                &to.content().inline_constructs,
            ),
            "the inline construct is not carried by the target",
        ),
        ClauseId::ContentBlockConstructs => lost_members(
            missing(
                &from.content().block_constructs,
                &to.content().block_constructs,
            ),
            "the block construct is not carried by the target",
        ),

        ClauseId::PropertyKeysCharset => {
            use KeyCharset::*;
            let rank = |c: KeyCharset| match c {
                Identifier => 0,
                KeywordNamespaced => 1,
                NoWhitespace => 2,
                Any => 3,
            };
            let (a, b) = (from.property_keys().charset, to.property_keys().charset);
            (rank(b) < rank(a)).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "keys the source accepts are illegal in the target",
            )
        }
        ClauseId::PropertyKeysCase => {
            let (a, b) = (from.property_keys().case, to.property_keys().case);
            (a == KeyCase::Sensitive && b != KeyCase::Sensitive).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "keys differing only in case collide in the target",
            )
        }
        ClauseId::PropertyKeysReservedPrefixes => lost_members(
            missing(
                &to.property_keys().reserved_prefixes,
                &from.property_keys().reserved_prefixes,
            ),
            "the target RESERVES this prefix, so a key carrying it is not the author's any more",
        ),
        // Losing a REFUSAL is a loss, same argument as `tags_resolution`
        // below: the target takes the key the source turned away, and the
        // author's value is overwritten instead of being rejected.
        ClauseId::PropertyKeysEngineOwnedKeys => lost_members(
            missing(
                &from.property_keys().engine_owned_keys,
                &to.property_keys().engine_owned_keys,
            ),
            "the target ACCEPTS this key instead of refusing it, so an authored value is \
             silently replaced",
        ),
        ClauseId::PropertyKeysCollision => {
            let (a, b) = (from.property_keys().collision, to.property_keys().collision);
            (a == Collision::MultiValued && b != Collision::MultiValued).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "the target keeps ONE value where the source kept every one",
            )
        }
        ClauseId::PropertyKeysSchemaRequired => {
            let (a, b) = (
                from.property_keys().schema_required,
                to.property_keys().schema_required,
            );
            (a == SchemaRequirement::Open && b == SchemaRequirement::Declared).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "an undeclared key is refused by the target",
            )
        }

        ClauseId::PropertyValuesTypes => lost_members(
            missing(&from.property_values().types, &to.property_values().types),
            "a value of this kind arrives re-typed or not at all",
        ),
        ClauseId::PropertyValuesEmptyString => {
            let (a, b) = (
                from.property_values().empty_string,
                to.property_values().empty_string,
            );
            (a == Representability::Representable && b != Representability::Representable)
                .then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "an empty value does not survive the move",
            )
        }
        ClauseId::PropertyValuesNull => {
            let (a, b) = (from.property_values().null, to.property_values().null);
            (a == Representability::Representable && b != Representability::Representable)
                .then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "an explicit null becomes indistinguishable from an absent key",
            )
        }
        ClauseId::PropertyValuesMultiValue => {
            let (a, b) = (
                &from.property_values().multi_value,
                &to.property_values().multi_value,
            );
            let carries = |m: &MultiValue| !matches!(m, MultiValue::None);
            (carries(a) && !carries(b)).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "a multi-valued property collapses to a single value",
            )
        }
        ClauseId::PropertyValuesReferenceValues => {
            let (a, b) = (
                from.property_values().reference_values,
                to.property_values().reference_values,
            );
            use crate::axes::ReferenceValues::None as NoRefs;
            (a != NoRefs && b == NoRefs).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "a reference arrives as ordinary text — the link is gone",
            )
        }

        ClauseId::OrderingSiblingOrder => {
            let (a, b) = (from.ordering().sibling_order, to.ordering().sibling_order);
            (a != SiblingOrder::Unordered && b == SiblingOrder::Unordered).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "sibling order is not modelled by the target",
            )
        }
        ClauseId::OrderingOrderKeyDurable => None,
        ClauseId::OrderingConcurrentInsert => None,
        ClauseId::OrderingPropertyOrder => {
            let (a, b) = (from.ordering().property_order, to.ordering().property_order);
            (a == PropertyOrder::Preserved && b != PropertyOrder::Preserved).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "the authored order of properties is not kept",
            )
        }

        ClauseId::HierarchyShape => {
            use HierarchyShape::*;
            let rank = |s: HierarchyShape| match s {
                Flat => 0,
                Tree => 1,
                Forest => 2,
                Dag => 3,
            };
            let (a, b) = (from.hierarchy().shape, to.hierarchy().shape);
            (rank(b) < rank(a)).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "the target cannot express this shape, so structure is rewritten on arrival",
            )
        }
        ClauseId::HierarchyMaxDepth => {
            let (a, b) = (from.hierarchy().max_depth, to.hierarchy().max_depth);
            let deeper = match (a, b) {
                (MaxDepth::Unbounded, MaxDepth::Limit(_)) => true,
                (MaxDepth::Limit(x), MaxDepth::Limit(y)) => y < x,
                _ => false,
            };
            deeper.then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "deep subtrees do not fit the target",
            )
        }
        ClauseId::HierarchyReparent => {
            use Reparent::*;
            let rank = |r: Reparent| match r {
                None => 0,
                Constrained => 1,
                Free => 2,
            };
            let (a, b) = (from.hierarchy().reparent, to.hierarchy().reparent);
            (rank(b) < rank(a)).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "moves the source allows are refused in the target",
            )
        }
        ClauseId::HierarchyConstraints => lost_members(
            missing(&to.hierarchy().constraints, &from.hierarchy().constraints),
            "the target ENFORCES this rule, so content legal in the source is refused",
        ),
        ClauseId::HierarchyCycles => {
            let (a, b) = (from.hierarchy().cycles, to.hierarchy().cycles);
            (a == Cycles::Representable && b == Cycles::Rejected).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "a cyclic structure is refused by the target",
            )
        }

        ClauseId::IdentityIdSpace => None,
        ClauseId::IdentityIdOrigin => None,
        ClauseId::IdentityIdConstraints => lost_members(
            missing(
                &to.identity().id_constraints,
                &from.identity().id_constraints,
            ),
            "the target CONSTRAINS ids this way, so an id legal in the source is refused",
        ),
        ClauseId::IdentityRenameStability => lost_members(
            missing(
                &from.identity().rename_stability,
                &to.identity().rename_stability,
            ),
            "identity does not survive this rename in the target",
        ),
        ClauseId::IdentityCarriers => lost_members(
            missing(&from.identity().carriers, &to.identity().carriers),
            "the target has no place to put identity carried this way",
        ),
        ClauseId::IdentityCarrierDisagreement => None,

        ClauseId::ComputedLive => {
            use ComputedLive::*;
            let rank = |c: ComputedLive| match c {
                None => 0,
                ScriptOnly => 1,
                Full => 2,
            };
            let (a, b) = (from.computed().computed_live, to.computed().computed_live);
            (rank(b) < rank(a)).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "a live computation cannot be served by the target",
            )
        }
        ClauseId::ComputedPersisted => {
            let (a, b) = (
                &from.computed().computed_persisted,
                &to.computed().computed_persisted,
            );
            let rank = |c: &ComputedPersisted| match c {
                ComputedPersisted::None => 0,
                ComputedPersisted::StringOnly => 1,
                ComputedPersisted::TypedSubset { .. } => 2,
                ComputedPersisted::FullAlgebra => 3,
            };
            (rank(b) < rank(a)).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "a persisted computation loses fidelity in the target",
            )
        }
        ClauseId::ComputedExpressionClosure => {
            use ExpressionClosure::*;
            let rank = |c: ExpressionClosure| match c {
                None => 0,
                ComputationAlgebra => 1,
                ComputationPlusScript => 2,
            };
            let (a, b) = (
                from.computed().expression_closure,
                to.computed().expression_closure,
            );
            (rank(b) < rank(a)).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "expressions the source can state have no form in the target",
            )
        }

        ClauseId::MutationWriteLeg => {
            let (a, b) = (from.mutation().write_leg, to.mutation().write_leg);
            (a != WriteLeg::Absent && b == WriteLeg::Absent).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "the target is READ-ONLY: nothing moved there can be edited in place",
            )
        }
        ClauseId::MutationUnitOfWrite => {
            use WriteUnit::*;
            let rank = |u: WriteUnit| match u {
                File => 0,
                Container => 1,
                Entity => 2,
                Field => 3,
            };
            let (a, b) = (from.mutation().unit_of_write, to.mutation().unit_of_write);
            (rank(b) < rank(a)).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "one edit rewrites more than it did in the source",
            )
        }
        ClauseId::MutationMergeGranularity => {
            use MergeGranularity::*;
            let rank = |g: MergeGranularity| match g {
                File => 0,
                Entity => 1,
                Field => 2,
                Character => 3,
            };
            let (a, b) = (
                from.mutation().merge_granularity,
                to.mutation().merge_granularity,
            );
            (rank(b) < rank(a)).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "concurrent edits that merged cleanly in the source now conflict",
            )
        }
        ClauseId::MutationConflictSurface => {
            use ConflictSurface::*;
            let rank = |s: ConflictSurface| match s {
                None => 0,
                Log => 1,
                PropertyBanner => 2,
                Ui => 3,
            };
            let (a, b) = (
                from.mutation().conflict_surface,
                to.mutation().conflict_surface,
            );
            (rank(b) < rank(a)).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "a conflict is surfaced less visibly, or not at all",
            )
        }

        ClauseId::AssetsAttachments => {
            let (a, b) = (from.assets().attachments, to.assets().attachments);
            (a != Attachments::None && b == Attachments::None).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "attachments have no home in the target",
            )
        }
        ClauseId::AssetsBinaryInline => {
            let (a, b) = (from.assets().binary_inline, to.assets().binary_inline);
            (a != BinaryInline::None && b == BinaryInline::None).then(|| ())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "inline binary content cannot be represented in the target",
            )
        }
        ClauseId::AssetsExtensions => lost_members(
            missing(&from.assets().extensions, &to.assets().extensions),
            "an attachment of this type is not recognised by the target",
        ),

        // A tag the source can attach and the target cannot is content the
        // move drops silently: the entity arrives, its classification does
        // not.
        ClauseId::TagsAttachExisting => {
            let (a, b) = (from.tags().attach_existing, to.tags().attach_existing);
            (a == TagWrite::Carried && b != TagWrite::Carried).then_some(())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "a tag cannot be attached in the target, so classification does not move",
            )
        }
        ClauseId::TagsDetachExisting => {
            let (a, b) = (from.tags().detach_existing, to.tags().detach_existing);
            (a == TagWrite::Carried && b != TagWrite::Carried).then_some(())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "a tag cannot be removed in the target, so a classification becomes permanent",
            )
        }
        // Losing a REFUSAL is a loss even though it removes a restriction:
        // the target accepts what the source turned away, and writes a
        // reference to nothing instead of saying no.
        ClauseId::TagsResolutionRefusesUnknown => {
            let (a, b) = (from.tags().unknown_reference, to.tags().unknown_reference);
            (a == UnknownTagReference::Refused && b == UnknownTagReference::Dangling)
                .then_some(())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "an unresolvable tag is written as a dangling reference instead of refused",
            )
        }
        ClauseId::TagsMintNew => {
            let (a, b) = (from.tags().mint_new, to.tags().mint_new);
            (a == TagMinting::Supported && b == TagMinting::None).then_some(())?;
            loss(
                format!("{a:?}"),
                format!("{b:?}"),
                "a tag that does not exist yet cannot be created in the target",
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::MINIMAL;
    use crate::fixture::minimal_with;

    fn profile(yaml: &str) -> CapabilityProfile {
        CapabilityProfile::from_yaml(yaml).expect("fixture parses")
    }

    /// The identity law. A profile loses NOTHING moving to itself, and a diff
    /// that reported something here would be comparing noise.
    #[test]
    fn a_profile_loses_nothing_moving_to_itself() {
        let p = profile(MINIMAL);
        assert_eq!(p.diff(&p), Vec::new(), "self-diff must be empty");
    }

    /// The direction matters, and the diff must say WHICH clause pays.
    #[test]
    fn a_narrower_target_costs_exactly_the_clause_it_narrows() {
        let wide = profile(&minimal_with("types: [string]", "types: [string, integer]"));
        let narrow = profile(MINIMAL);

        let losses = wide.diff(&narrow);
        assert_eq!(losses.len(), 1, "exactly one clause narrows: {losses:?}");
        assert_eq!(losses[0].clause, ClauseId::PropertyValuesTypes);
        assert!(
            losses[0].source.contains("Integer"),
            "the loss must name the kind that does not fit: {:?}",
            losses[0]
        );
        assert!(
            narrow.diff(&wide).is_empty(),
            "moving to a WIDER home costs nothing"
        );
    }

    /// A read-only target is the loss a promotion story must not hide.
    #[test]
    fn a_read_only_target_reports_the_write_leg() {
        let writable = profile(MINIMAL);
        let read_only = profile(&minimal_with("write_leg: file", "write_leg: absent"));
        let losses = writable.diff(&read_only);
        assert!(
            losses
                .iter()
                .any(|l| l.clause == ClauseId::MutationWriteLeg && l.effect.contains("READ-ONLY")),
            "moving to a read-only home must be reported: {losses:?}"
        );
    }

    /// A target that RESERVES a prefix takes keys away from the author, even
    /// though it declares MORE than the source. Direction alone is not enough;
    /// the comparison has to know which way each clause points.
    #[test]
    fn a_target_that_reserves_more_is_itself_a_loss() {
        let open = profile(MINIMAL);
        let reserving = profile(&minimal_with(
            "reserved_prefixes: []",
            "reserved_prefixes: [\"_\"]",
        ));
        let losses = open.diff(&reserving);
        assert!(
            losses
                .iter()
                .any(|l| l.clause == ClauseId::PropertyKeysReservedPrefixes),
            "a newly reserved prefix is a loss for the mover: {losses:?}"
        );
    }
}
