//! `derive_net` — the pure derivation from sources to [`CompiledNet`].

use std::collections::BTreeSet;

use holon_api::OperationDescriptor;
use holon_api::arcs::ArcEmit;
use holon_api::arcs::ArcPlace;
use holon_api::marking::ExistenceFlow;
use holon_api::marking::MarkingDelta;
use holon_api::marking::StructuralFlow;
use holon_api::marking::TextFlow;
use holon_api::pattern::OpGuard;
use holon_pattern::arcs::TransitionArcs;
use holon_pattern::schema::block;
use holon_pattern::schema::clock;
use holon_rules::HolonRule;
use holon_rules::TemplateSegment;

use crate::bridge::TransitionSource;
use crate::guards::ClassifiedGuard;
use crate::guards::classify_guard;
use crate::net::Analyzability;
use crate::net::ArcOrigin;
use crate::net::Aspect;
use crate::net::CompiledNet;
use crate::net::Flow;
use crate::net::NetArc;
use crate::net::NetCompileError;
use crate::net::NetTransition;
use crate::net::UndeclaredHalf;
use crate::net::aspect_places;

/// The rule watcher's verdict on one discovered `holon_rule` block.
///
/// The watcher is the SOLE authority for whether a rule fires, so this is what
/// a rule transition's `active` flag reads. Nothing re-derives that answer from
/// the rule's own shape — a mirror drifts the moment the watcher grows a
/// refusal the mirror does not know about.
///
/// Every variant becomes a transition. Declared automation that does not run is
/// still declared: modelling it as an absence would hide it from every analysis
/// (ADR 0032 fail-closed posture).
#[derive(Debug, Clone, PartialEq)]
pub enum RuleAcceptance {
    /// Parsed, owned by the watcher, and firing.
    Running(HolonRule),
    /// Parsed, but this watcher does not fire it — a guard-only rule the advice
    /// reconciler owns, or a guard subject with no reactive binding. Fully
    /// analyzable: the declaration is readable, it just does not run.
    Parked { rule: HolonRule, reason: String },
    /// Never parsed into a rule — a malformed body, or a block whose firing
    /// another watcher owns so this one never read it. There is no declaration
    /// to compile, which is `Unanalyzable` ("cannot say"), never absence.
    Opaque { reason: String },
}

impl RuleAcceptance {
    /// Whether the watcher runs this rule.
    pub fn is_running(&self) -> bool {
        matches!(self, RuleAcceptance::Running(_))
    }

    /// The parsed rule, when the watcher managed to read one.
    pub fn rule(&self) -> Option<&HolonRule> {
        match self {
            RuleAcceptance::Running(rule) | RuleAcceptance::Parked { rule, .. } => Some(rule),
            RuleAcceptance::Opaque { .. } => None,
        }
    }
}

/// A rule block as the discovery query yields it: the block's id plus the
/// watcher's verdict on it.
#[derive(Debug, Clone, PartialEq)]
pub struct RuleSource {
    pub block_id: String,
    pub acceptance: RuleAcceptance,
}

/// Compile the declared automation surface into one net. Pure: same inputs,
/// same net.
pub fn derive_net(
    descriptors: &[OperationDescriptor],
    rules: &[RuleSource],
) -> Result<CompiledNet, NetCompileError> {
    let mut transitions = Vec::with_capacity(descriptors.len() + rules.len());
    let mut claimed = BTreeSet::new();
    for descriptor in descriptors {
        transitions.push(compile_operation(descriptor)?);
    }
    for rule in rules {
        transitions.push(compile_rule(rule));
    }
    for transition in &transitions {
        let key = transition.key();
        if !claimed.insert(key.clone()) {
            return Err(NetCompileError::DuplicateTransition { key });
        }
    }
    Ok(CompiledNet { transitions })
}

/// Compile one descriptor. An `Undeclared` arcs or marking-delta half makes
/// the transition unanalyzable while keeping whatever the other half
/// declares.
pub fn compile_operation(
    descriptor: &OperationDescriptor,
) -> Result<NetTransition, NetCompileError> {
    let mut transition = NetTransition {
        source: TransitionSource::Operation {
            entity: descriptor.entity_name.as_str().to_string(),
            op: descriptor.name.clone(),
        },
        analyzability: Analyzability::Analyzable,
        arcs: Vec::new(),
        residue: Vec::new(),
    };
    let mut undeclared = Vec::new();

    match &descriptor.arcs {
        TransitionArcs::Undeclared => undeclared.push(UndeclaredHalf::Arcs),
        TransitionArcs::Declared { reads, emits } => {
            for place in reads {
                transition.push_arc(plain_arc(
                    place.clone(),
                    Flow::Read,
                    ArcOrigin::DeclaredRead,
                ));
            }
            for emit in emits {
                match emit {
                    ArcEmit::Writes(place) => transition.push_arc(plain_arc(
                        place.clone(),
                        Flow::Produce,
                        ArcOrigin::DeclaredEmit,
                    )),
                    // A declared non-write: no arc, by declaration.
                    ArcEmit::Excluded { .. } => {}
                }
            }
        }
    }

    match &descriptor.marking_delta {
        MarkingDelta::Undeclared => undeclared.push(UndeclaredHalf::MarkingDelta),
        MarkingDelta::Static { kinds } | MarkingDelta::Envelope { kinds, .. } => {
            for kind in kinds {
                let flows = [
                    (Aspect::Structural, structural_flow(kind.structural)),
                    (Aspect::Text, text_flow(kind.text)),
                    (Aspect::Existence, existence_flow(kind.existence)),
                ];
                for (aspect, flow) in flows {
                    let Some(flow) = flow else { continue };
                    for place in aspect_places(&kind.kind, aspect)? {
                        transition.push_arc(plain_arc(place, flow, ArcOrigin::Delta { aspect }));
                    }
                }
            }
        }
    }

    if let OpGuard::Declared { guard, .. } = &descriptor.guard {
        let ClassifiedGuard { arcs, residue } = classify_guard(guard);
        for arc in arcs {
            transition.push_arc(arc);
        }
        transition.residue = residue;
    }

    if !undeclared.is_empty() {
        transition.analyzability = Analyzability::Unanalyzable { undeclared };
    }
    Ok(transition)
}

/// Compile one rule block. A parsed rule is analyzable — guard and emit are
/// fully declared by parse — and `active` is the watcher's own verdict. A block
/// the watcher never parsed becomes an `Unanalyzable` transition with no arcs:
/// declared automation whose declaration cannot be read.
pub fn compile_rule(source: &RuleSource) -> NetTransition {
    let Some(rule) = source.acceptance.rule() else {
        return NetTransition {
            source: TransitionSource::Rule {
                block_id: source.block_id.clone(),
                // No parsed rule, so no authored name — the block id is the
                // only handle that exists.
                name: source.block_id.clone(),
                active: false,
            },
            analyzability: Analyzability::Unanalyzable {
                undeclared: vec![UndeclaredHalf::Arcs, UndeclaredHalf::MarkingDelta],
            },
            arcs: Vec::new(),
            residue: Vec::new(),
        };
    };
    let ClassifiedGuard { arcs, residue } = classify_guard(&rule.guard);
    let mut transition = NetTransition {
        source: TransitionSource::Rule {
            block_id: source.block_id.clone(),
            name: rule.name.as_str().to_string(),
            active: source.acceptance.is_running(),
        },
        analyzability: Analyzability::Analyzable,
        arcs: Vec::new(),
        residue,
    };
    for arc in arcs {
        transition.push_arc(arc);
    }

    if let Some(emit) = &rule.emit {
        // A ratcheted create: a new row (existence), placed under the emit
        // root (placement), named per the template (content).
        for field in [block::ID, block::PARENT_ID, block::CONTENT] {
            transition.push_arc(plain_arc(
                ArcPlace::new(block::RELATION, field),
                Flow::Produce,
                ArcOrigin::RuleEmit,
            ));
        }
        if emit.place.is_page() {
            transition.push_arc(plain_arc(
                ArcPlace::new(block::RELATION, block::TAGS),
                Flow::Produce,
                ArcOrigin::RuleEmit,
            ));
        }
        if emit
            .name
            .segments
            .iter()
            .any(|s| matches!(s, TemplateSegment::Builtin(_)))
        {
            transition.push_arc(plain_arc(
                ArcPlace::new(clock::RELATION, clock::TODAY),
                Flow::Read,
                ArcOrigin::RuleEmit,
            ));
        }
    }
    transition
}

fn plain_arc(place: ArcPlace, flow: Flow, origin: ArcOrigin) -> NetArc {
    NetArc {
        place,
        flow,
        origin,
        refinement: None,
        binding: None,
    }
}

fn structural_flow(flow: StructuralFlow) -> Option<Flow> {
    match flow {
        StructuralFlow::Untouched => None,
        StructuralFlow::Reads => Some(Flow::Read),
        StructuralFlow::Produces => Some(Flow::Produce),
        StructuralFlow::Consumes => Some(Flow::Consume),
        StructuralFlow::Relocates => Some(Flow::Relocate),
    }
}

fn text_flow(flow: TextFlow) -> Option<Flow> {
    match flow {
        TextFlow::Untouched => None,
        TextFlow::Reads => Some(Flow::Read),
        TextFlow::Produces => Some(Flow::Produce),
    }
}

fn existence_flow(flow: ExistenceFlow) -> Option<Flow> {
    match flow {
        ExistenceFlow::Untouched => None,
        ExistenceFlow::Reads => Some(Flow::Read),
        ExistenceFlow::Produces => Some(Flow::Produce),
    }
}
