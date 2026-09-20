//! Classifier totality over the generated guard grammar.
//!
//! The model side walks the pattern tree independently of the classifier's
//! traversal (disjunctive normal form, hop peeling, refinement short-cuts):
//! it collects every leaf atom and maps each to its places in one flat pass.
//! The properties pin that no named place is ever dropped whatever the
//! nesting, that every mode is a usable firing mode, and that nothing the arc
//! language can express is left in residue.

use std::collections::BTreeSet;

use holon_net::ArcOrigin;
use holon_net::MAX_HOP_CHAIN;
use holon_net::MAX_MODES;
use holon_net::guards::classify_guard;
use holon_pattern::Value;
use holon_pattern::pattern::BuiltinRef;
use holon_pattern::pattern::CmpOp;
use holon_pattern::pattern::FieldRef;
use holon_pattern::pattern::Guard;
use holon_pattern::pattern::Operand;
use holon_pattern::pattern::PathPattern;
use holon_pattern::pattern::PathSegment;
use holon_pattern::pattern::Pattern;
use holon_pattern::pattern::Subject;
use proptest::prelude::*;

fn arb_field() -> impl Strategy<Value = FieldRef> {
    prop_oneof![
        Just(FieldRef::Name),
        "[a-z]{1,6}".prop_map(FieldRef::Property),
        ("[a-z]{1,6}", "[a-z]{1,6}")
            .prop_map(|(relation, name)| FieldRef::Column { relation, name }),
    ]
}

fn arb_operand() -> impl Strategy<Value = Operand> {
    prop_oneof![
        "[a-z]{0,6}".prop_map(|s| Operand::Lit(Value::String(s))),
        any::<i64>().prop_map(|n| Operand::Lit(Value::Integer(n))),
        Just(Operand::Builtin(BuiltinRef::Today)),
    ]
}

fn arb_path() -> impl Strategy<Value = PathPattern> {
    prop::collection::vec(
        prop_oneof![
            "[A-Za-z]{1,8}".prop_map(PathSegment::Lit),
            Just(PathSegment::Builtin(BuiltinRef::Today)),
        ],
        1..3,
    )
    .prop_map(|segments| PathPattern { segments })
}

fn arb_pattern() -> impl Strategy<Value = Pattern> {
    let leaf = prop_oneof![
        (arb_field(), arb_operand()).prop_map(|(field, rhs)| Pattern::Field {
            field,
            op: CmpOp::Eq,
            rhs,
        }),
        "[a-z]{1,8}".prop_map(Pattern::HasTag),
        arb_path().prop_map(Pattern::BlockExists),
    ];
    leaf.prop_recursive(3, 24, 4, |inner| {
        prop_oneof![
            inner.clone().prop_map(|p| Pattern::Parent(Box::new(p))),
            inner.clone().prop_map(|p| Pattern::Sibling(Box::new(p))),
            inner.clone().prop_map(|p| Pattern::Not(Box::new(p))),
            prop::collection::vec(inner.clone(), 1..4).prop_map(Pattern::And),
            prop::collection::vec(inner, 1..4).prop_map(Pattern::Or),
        ]
    })
}

fn arb_subject() -> impl Strategy<Value = Subject> {
    prop_oneof![
        Just(Subject::Clock),
        Just(Subject::Block),
        Just(Subject::Relation("integration".to_string())),
    ]
}

/// Model place walk: every leaf's places, one flat recursion, the mapping
/// table restated inline.
fn model_places(pattern: &Pattern, out: &mut BTreeSet<String>) {
    match pattern {
        Pattern::Field { field, rhs, .. } => {
            out.insert(match field {
                FieldRef::Name => "block.content".to_string(),
                FieldRef::Property(_) => "block.properties".to_string(),
                FieldRef::Column { relation, name } => format!("{relation}.{name}"),
            });
            if matches!(rhs, Operand::Builtin(BuiltinRef::Today)) {
                out.insert("clock.today".to_string());
            }
        }
        Pattern::HasTag(_) => {
            out.insert("block.tags".to_string());
        }
        Pattern::BlockExists(path) => {
            out.insert("block.id".to_string());
            out.insert("block.content".to_string());
            if path.segments.len() > 1 {
                out.insert("block.parent_id".to_string());
            }
            if path
                .segments
                .iter()
                .any(|s| matches!(s, PathSegment::Builtin(BuiltinRef::Today)))
            {
                out.insert("clock.today".to_string());
            }
        }
        Pattern::Parent(inner) | Pattern::Child(inner) | Pattern::Sibling(inner) => {
            // A hop reads both places it traverses: the cell it follows and
            // the identity it lands on.
            out.insert("block.parent_id".to_string());
            out.insert("block.id".to_string());
            model_places(inner, out);
        }
        Pattern::And(ps) | Pattern::Or(ps) => {
            for p in ps {
                model_places(p, out);
            }
        }
        Pattern::Not(inner) => model_places(inner, out),
    }
}

fn model_subject_place(subject: &Subject) -> String {
    match subject {
        Subject::Clock => "clock.today".to_string(),
        Subject::Block => "block.id".to_string(),
        Subject::Relation(r) => format!("{r}.id"),
    }
}

proptest! {
    // Auto-persisted seed files are banned: a permanent regression belongs in
    // `hand-authored-regressions/keystone.jsonl`, not in a file proptest
    // writes behind the author's back.
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]
    /// Every mode is a firing mode a caller can evaluate: it exists, it binds
    /// the subject, and the disjunctive normal form stays bounded.
    #[test]
    fn every_mode_is_well_formed_and_bounded(
        body in arb_pattern(),
        subject in arb_subject(),
    ) {
        let guard = Guard { subject: subject.clone(), body };
        let Ok(classified) = classify_guard(&guard, "op:block.probe") else {
            // Over a bound the guard does not compile at all, which the
            // bound tests cover; these laws speak about compiled guards.
            return Ok(());
        };
        prop_assert!(!classified.modes.is_empty(), "a guard compiles to at least one mode");
        prop_assert!(
            classified.modes.len() <= MAX_MODES,
            "the normal form must stay bounded, got {} modes",
            classified.modes.len()
        );
        let subject_place = model_subject_place(&subject);
        for mode in &classified.modes {
            prop_assert!(
                mode.arcs.iter().any(|a| a.origin == ArcOrigin::Subject
                    && a.place.to_string() == subject_place),
                "every mode binds the guard's subject"
            );
        }
    }

    /// Every arc of a correlated group names that group's binding, so one
    /// token answers the whole hop. Splitting a hop's conjunction across
    /// tokens is the unsoundness this representation exists to prevent.
    #[test]
    fn a_hop_group_binds_all_of_its_arcs(
        body in arb_pattern(),
        subject in arb_subject(),
    ) {
        let guard = Guard { subject, body };
        let Ok(classified) = classify_guard(&guard, "op:block.probe") else {
            // Over a bound the guard does not compile at all, which the
            // bound tests cover; these laws speak about compiled guards.
            return Ok(());
        };
        for mode in &classified.modes {
            for group in &mode.hops {
                prop_assert!(!group.arcs.is_empty(), "an empty hop group tests nothing");
                prop_assert!(
                    group.correlation.hops.len() <= MAX_HOP_CHAIN,
                    "a hop chain must stay inside the bound"
                );
                for arc in &group.arcs {
                    prop_assert_eq!(arc.binding.as_ref(), Some(&group.binding));
                    prop_assert!(arc.refinement.is_some(), "a hop arc carries its predicate");
                }
            }
        }
    }

    /// No named place is dropped: the union of the classifier's arc places
    /// equals the model walk's places plus the subject binding place.
    #[test]
    fn no_guard_read_is_ever_dropped(
        body in arb_pattern(),
        subject in arb_subject(),
    ) {
        let guard = Guard { subject: subject.clone(), body: body.clone() };
        let Ok(classified) = classify_guard(&guard, "op:block.probe") else {
            // Over a bound the guard does not compile at all, which the
            // bound tests cover; these laws speak about compiled guards.
            return Ok(());
        };
        let sut: BTreeSet<String> = classified
            .modes
            .iter()
            .flat_map(|m| m.all_arcs())
            .map(|a| a.place.to_string())
            .collect();
        let mut model = BTreeSet::new();
        model_places(&body, &mut model);
        model.insert(model_subject_place(&subject));
        prop_assert_eq!(sut, model);
    }

    /// Refinement arcs carry their conjunct; residue predicates are exactly
    /// the inexpressible conjuncts.
    #[test]
    fn refinements_carry_predicates_and_residue_is_inexpressible(
        body in arb_pattern(),
        subject in arb_subject(),
    ) {
        let guard = Guard { subject, body };
        let Ok(classified) = classify_guard(&guard, "op:block.probe") else {
            // Over a bound the guard does not compile at all, which the
            // bound tests cover; these laws speak about compiled guards.
            return Ok(());
        };
        for arc in classified
            .modes
            .iter()
            .flat_map(|m| m.all_arcs())
            .filter(|a| a.origin == ArcOrigin::GuardRefinement)
        {
            prop_assert!(arc.refinement.is_some());
        }
        for residue in &classified.residue {
            let expressible = matches!(
                &residue.predicate,
                Pattern::Field { rhs: Operand::Lit(_), .. } | Pattern::HasTag(_)
            );
            prop_assert!(!expressible, "expressible conjunct left in residue: {:?}", residue);
        }
    }
}
