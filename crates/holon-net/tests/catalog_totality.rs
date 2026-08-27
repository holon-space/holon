//! Compilation totality and agreement over the real block operation catalog.
//!
//! The declarations are read off the macro-generated descriptors, not copies:
//! any drift between what an op declares and what its compiled transition
//! carries reds here.

use std::collections::BTreeSet;

use holon_api::OperationDescriptor;
use holon_api::marking::MarkingDelta;
use holon_net::Analyzability;
use holon_net::ArcOrigin;
use holon_net::Aspect;
use holon_net::TransitionKey;
use holon_net::net::UndeclaredHalf;
use holon_net::net::aspect_places;
use holon_pattern::arcs::TransitionArcs;

fn block_catalog() -> Vec<OperationDescriptor> {
    let mut ops =
        holon_core::__operations_crud_operations::crud_operations("block", "block", "block", "id");
    ops.extend(holon_core::__operations_block_operations::block_operations(
        "block", "block", "block", "id",
    ));
    ops
}

#[test]
fn every_catalog_descriptor_compiles() {
    let ops = block_catalog();
    assert!(!ops.is_empty());
    let net = holon_net::derive_net(&ops, &[]).expect("the block catalog compiles");
    assert_eq!(net.transitions.len(), ops.len());
}

/// Catalog analyzability, pinned: `set_field` declares both halves; the
/// other delta-declaring ops carry no `#[reads]`/`#[emits]`, so they are
/// unanalyzable with exactly the arcs half missing — never silently empty.
#[test]
fn catalog_analyzability_matches_the_declared_halves() {
    let ops = block_catalog();
    let net = holon_net::derive_net(&ops, &[]).unwrap();
    let analyzability = |name: &str| {
        net.transition(&TransitionKey::operation("block", name).expect("dotless entity"))
            .unwrap_or_else(|| panic!("catalog advertises {name}"))
            .analyzability
            .clone()
    };
    assert_eq!(analyzability("set_field"), Analyzability::Analyzable);
    for name in [
        "create",
        "delete",
        "move_block",
        "split_block",
        "join_block",
    ] {
        assert_eq!(
            analyzability(name),
            Analyzability::Unanalyzable {
                undeclared: vec![UndeclaredHalf::Arcs]
            },
            "{name} declares a marking delta but no arcs"
        );
    }
}

/// Agreement with the declarations: arc-declaration arcs carry exactly the
/// declared places, delta arcs exactly the aspect lowering of the declared
/// kinds.
#[test]
fn compiled_arcs_agree_with_the_declarations() {
    let ops = block_catalog();
    let net = holon_net::derive_net(&ops, &[]).unwrap();
    for (op, transition) in ops.iter().zip(&net.transitions) {
        let arc_places = |origin: ArcOrigin| -> BTreeSet<String> {
            transition
                .arcs
                .iter()
                .filter(|a| a.origin == origin)
                .map(|a| a.place.to_string())
                .collect()
        };

        if let TransitionArcs::Declared { reads, .. } = &op.arcs {
            let declared_reads: BTreeSet<String> = reads.iter().map(|p| p.to_string()).collect();
            assert_eq!(
                arc_places(ArcOrigin::DeclaredRead),
                declared_reads,
                "{} reads",
                op.name
            );
            let declared_writes: BTreeSet<String> = op
                .arcs
                .written_places()
                .iter()
                .map(|p| p.to_string())
                .collect();
            assert_eq!(
                arc_places(ArcOrigin::DeclaredEmit),
                declared_writes,
                "{} emits",
                op.name
            );
        }

        if let Some(kinds) = op.marking_delta.kinds() {
            let mut expected = BTreeSet::new();
            for kind in kinds {
                use holon_api::marking::ExistenceFlow;
                use holon_api::marking::StructuralFlow;
                use holon_api::marking::TextFlow;
                let declared = [
                    (
                        Aspect::Structural,
                        kind.structural != StructuralFlow::Untouched,
                    ),
                    (Aspect::Text, kind.text != TextFlow::Untouched),
                    (
                        Aspect::Existence,
                        kind.existence != ExistenceFlow::Untouched,
                    ),
                ];
                for (aspect, touched) in declared {
                    if !touched {
                        continue;
                    }
                    for place in aspect_places(&kind.kind, aspect).unwrap() {
                        expected.insert(place.to_string());
                    }
                }
            }
            let delta_places: BTreeSet<String> = transition
                .arcs
                .iter()
                .filter(|a| matches!(a.origin, ArcOrigin::Delta { .. }))
                .map(|a| a.place.to_string())
                .collect();
            assert_eq!(delta_places, expected, "{} delta lowering", op.name);
        }
    }
}

/// `Undeclared` must surface as unanalyzable — and stay distinguishable from
/// a declared-empty half, which compiles to an analyzable transition with no
/// arcs from that half.
#[test]
fn undeclared_halves_are_unanalyzable_and_distinct_from_declared_empty() {
    let ops = block_catalog();
    let set_field = ops.iter().find(|op| op.name == "set_field").unwrap();

    // Distinct op names: three variants of one op would claim one key.
    let mut arcless = set_field.clone();
    arcless.name = "arcless".to_string();
    arcless.arcs = TransitionArcs::Undeclared;
    let mut deltaless = set_field.clone();
    deltaless.name = "deltaless".to_string();
    deltaless.marking_delta = MarkingDelta::Undeclared;
    let mut empty_arcs = set_field.clone();
    empty_arcs.name = "empty_arcs".to_string();
    empty_arcs.arcs = TransitionArcs::Declared {
        reads: vec![],
        emits: vec![],
    };

    let net = holon_net::derive_net(&[arcless, deltaless, empty_arcs], &[]).unwrap();
    let transition = |name: &str| {
        net.transition(&TransitionKey::operation("block", name).expect("dotless entity"))
            .unwrap_or_else(|| panic!("the net carries {name}"))
    };
    assert_eq!(
        transition("arcless").analyzability,
        Analyzability::Unanalyzable {
            undeclared: vec![UndeclaredHalf::Arcs]
        }
    );
    assert_eq!(
        transition("deltaless").analyzability,
        Analyzability::Unanalyzable {
            undeclared: vec![UndeclaredHalf::MarkingDelta]
        }
    );
    assert_eq!(
        transition("empty_arcs").analyzability,
        Analyzability::Analyzable
    );
    assert!(
        !transition("empty_arcs")
            .arcs
            .iter()
            .any(|a| matches!(a.origin, ArcOrigin::DeclaredRead | ArcOrigin::DeclaredEmit)),
        "declared-empty arcs contribute no arc-declaration arcs"
    );
}
