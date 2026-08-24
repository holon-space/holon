//! Guard classification, on two independent axes.
//!
//! Axis one, predicate expressibility: a conjunct whose shape the arc
//! language carries — a subject column, name, property, or tag test against a
//! literal — becomes a refined read arc. Everything else stays an opaque
//! [`GuardResidue`], never lossily approximated into an arc (the
//! guard-language/arc-language split of ADR 0031/0032).
//!
//! Axis two, place footprint: a guard reads every place it tests, whatever
//! its predicate. The grammar is closed (`not`/`and`/`or` over field
//! comparisons, `has_tag`, `block_exists`, `parent`), so the footprint is
//! extractable for every guard, and residue conjuncts contribute
//! [`ArcOrigin::GuardFootprint`] read arcs. Dropping those reads would
//! under-approximate the read set and lose real conflict and cycle edges.

use std::collections::BTreeSet;

use holon_pattern::arcs::ArcPlace;
use holon_pattern::pattern::BuiltinRef;
use holon_pattern::pattern::FieldRef;
use holon_pattern::pattern::Guard;
use holon_pattern::pattern::Operand;
use holon_pattern::pattern::PathSegment;
use holon_pattern::pattern::Pattern;
use holon_pattern::pattern::Subject;
use holon_pattern::schema;
use holon_pattern::schema::block;
use holon_pattern::schema::clock;

use crate::net::ArcOrigin;
use crate::net::Flow;
use crate::net::GuardResidue;
use crate::net::NetArc;

/// A classified guard: its read arcs (subject binding, expressible conjuncts
/// as refinements, residue footprints) and its opaque residue.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedGuard {
    pub arcs: Vec<NetArc>,
    pub residue: Vec<GuardResidue>,
}

/// Classify `guard` into arcs and residue. Total: every conjunct lands in
/// exactly one of the two, and every place the guard names appears on an arc.
pub fn classify_guard(guard: &Guard) -> ClassifiedGuard {
    let mut arcs = vec![read_arc(
        subject_place(&guard.subject),
        ArcOrigin::Subject,
        None,
    )];
    let mut residue = Vec::new();
    for conjunct in flatten_and(&guard.body) {
        match refinement_place(conjunct) {
            Some(place) => arcs.push(read_arc(
                place,
                ArcOrigin::GuardRefinement,
                Some(conjunct.clone()),
            )),
            None => {
                for place in pattern_footprint(conjunct) {
                    arcs.push(read_arc(place, ArcOrigin::GuardFootprint, None));
                }
                residue.push(GuardResidue {
                    predicate: conjunct.clone(),
                });
            }
        }
    }
    dedupe(&mut arcs);
    ClassifiedGuard { arcs, residue }
}

/// Every place `pattern` syntactically tests. One mapping, stated once:
///
/// - a field comparison → the field's place (`name` → `block.content`,
///   `property` → `block.properties`, a relation column → itself)
/// - a `{today}` operand or path segment → `clock.today`
/// - `has_tag` → `block.tags`
/// - `block_exists` → `block.id`, `block.content`, plus `block.parent_id` when
///   the path has ancestors
/// - `parent(inner)` → `block.parent_id` plus `inner`'s footprint
pub fn pattern_footprint(pattern: &Pattern) -> BTreeSet<ArcPlace> {
    let mut out = BTreeSet::new();
    walk_footprint(pattern, &mut out);
    out
}

fn walk_footprint(pattern: &Pattern, out: &mut BTreeSet<ArcPlace>) {
    match pattern {
        Pattern::Field { field, rhs, .. } => {
            out.insert(field_place(field));
            if let Operand::Builtin(b) = rhs {
                out.insert(builtin_place(b));
            }
        }
        Pattern::HasTag(_) => {
            out.insert(ArcPlace::new(block::RELATION, block::TAGS));
        }
        Pattern::BlockExists(path) => {
            out.insert(ArcPlace::new(block::RELATION, block::ID));
            out.insert(ArcPlace::new(block::RELATION, block::CONTENT));
            if path.segments.len() > 1 {
                out.insert(ArcPlace::new(block::RELATION, block::PARENT_ID));
            }
            for segment in &path.segments {
                if let PathSegment::Builtin(b) = segment {
                    out.insert(builtin_place(b));
                }
            }
        }
        Pattern::Parent(inner) => {
            out.insert(ArcPlace::new(block::RELATION, block::PARENT_ID));
            walk_footprint(inner, out);
        }
        Pattern::And(ps) | Pattern::Or(ps) => {
            for p in ps {
                walk_footprint(p, out);
            }
        }
        Pattern::Not(inner) => walk_footprint(inner, out),
    }
}

/// The place an expressible conjunct refines, or `None` for residue. The
/// expressible shapes are a subject attribute against a literal (a relation
/// column, the name, a property) and subject tag membership; a clock operand,
/// a hop, an existence test, negation, and disjunction are not.
fn refinement_place(conjunct: &Pattern) -> Option<ArcPlace> {
    match conjunct {
        Pattern::Field {
            field,
            rhs: Operand::Lit(_),
            ..
        } => Some(field_place(field)),
        Pattern::HasTag(_) => Some(ArcPlace::new(block::RELATION, block::TAGS)),
        _ => None,
    }
}

fn field_place(field: &FieldRef) -> ArcPlace {
    match field {
        FieldRef::Name => ArcPlace::new(block::RELATION, block::CONTENT),
        FieldRef::Property(_) => ArcPlace::new(block::RELATION, block::PROPERTIES),
        FieldRef::Column { relation, name } => ArcPlace::new(relation.clone(), name.clone()),
    }
}

fn builtin_place(builtin: &BuiltinRef) -> ArcPlace {
    match builtin {
        BuiltinRef::Today => ArcPlace::new(clock::RELATION, clock::TODAY),
    }
}

/// The binding row a guard's subject iterates.
fn subject_place(subject: &Subject) -> ArcPlace {
    match subject {
        Subject::Clock => ArcPlace::new(clock::RELATION, clock::TODAY),
        Subject::Block => ArcPlace::new(block::RELATION, block::ID),
        Subject::Relation(relation) => {
            let id_column = schema::builtin_entity(relation)
                .and_then(|e| e.binding)
                .map(|b| b.id_column)
                .unwrap_or("id");
            ArcPlace::new(relation.clone(), id_column)
        }
    }
}

/// Top-level conjuncts: nested `And`s flattened, anything else one conjunct.
fn flatten_and(pattern: &Pattern) -> Vec<&Pattern> {
    match pattern {
        Pattern::And(ps) => ps.iter().flat_map(flatten_and).collect(),
        other => vec![other],
    }
}

fn read_arc(place: ArcPlace, origin: ArcOrigin, refinement: Option<Pattern>) -> NetArc {
    NetArc {
        place,
        flow: Flow::Read,
        origin,
        refinement,
        binding: None,
    }
}

fn dedupe(arcs: &mut Vec<NetArc>) {
    let mut seen = Vec::new();
    arcs.retain(|arc| {
        if seen.contains(arc) {
            false
        } else {
            seen.push(arc.clone());
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use holon_pattern::pattern::Guard;

    use super::*;

    fn places(arcs: &[NetArc], origin: ArcOrigin) -> Vec<String> {
        arcs.iter()
            .filter(|a| a.origin == origin)
            .map(|a| a.place.to_string())
            .collect()
    }

    /// The production `integration.begin_oauth` guard: three subject-column
    /// equalities, all expressible.
    #[test]
    fn a_conjoined_column_guard_is_all_refinements() {
        let guard = Guard::parse(
            "integration.config_status == \"unconfigured\" and integration.configurable == 1 \
             and integration.configure_progress == \"\"",
        )
        .unwrap();
        let classified = classify_guard(&guard);
        assert!(classified.residue.is_empty(), "{:?}", classified.residue);
        assert_eq!(
            places(&classified.arcs, ArcOrigin::GuardRefinement),
            [
                "integration.config_status",
                "integration.configurable",
                "integration.configure_progress"
            ]
        );
        assert!(
            classified
                .arcs
                .iter()
                .filter(|a| a.origin == ArcOrigin::GuardRefinement)
                .all(|a| a.refinement.is_some())
        );
    }

    #[test]
    fn a_tag_guard_is_one_refinement_on_the_tags_place() {
        let guard = Guard::parse("has_tag(\"flaggable\")").unwrap();
        let classified = classify_guard(&guard);
        assert!(classified.residue.is_empty());
        assert_eq!(
            places(&classified.arcs, ArcOrigin::GuardRefinement),
            ["block.tags"]
        );
    }

    /// The parent hop is the shape the arc language deliberately lacks: the
    /// hop conjunct stays residue, its footprint keeps the reads.
    #[test]
    fn a_parent_hop_conjunct_is_residue_with_its_footprint() {
        let guard = Guard::parse("has_tag(\"Page\") and parent(not has_tag(\"Page\"))").unwrap();
        let classified = classify_guard(&guard);
        assert_eq!(
            places(&classified.arcs, ArcOrigin::GuardRefinement),
            ["block.tags"]
        );
        assert_eq!(classified.residue.len(), 1);
        let footprint = places(&classified.arcs, ArcOrigin::GuardFootprint);
        assert!(
            footprint.contains(&"block.parent_id".to_string()),
            "{footprint:?}"
        );
        assert!(
            footprint.contains(&"block.tags".to_string()),
            "{footprint:?}"
        );
    }

    /// Negated existence with a clock builtin: all residue, and the footprint
    /// names the existence, name, ancestry, and clock places the test reads.
    #[test]
    fn a_negated_existence_guard_is_residue_with_a_full_footprint() {
        let guard = Guard::parse("not block_exists(\"Journals/{today}\")").unwrap();
        let classified = classify_guard(&guard);
        assert!(places(&classified.arcs, ArcOrigin::GuardRefinement).is_empty());
        assert_eq!(classified.residue.len(), 1);
        let footprint = places(&classified.arcs, ArcOrigin::GuardFootprint);
        for expected in [
            "block.id",
            "block.content",
            "block.parent_id",
            "clock.today",
        ] {
            assert!(
                footprint.contains(&expected.to_string()),
                "{expected} missing: {footprint:?}"
            );
        }
    }
}
