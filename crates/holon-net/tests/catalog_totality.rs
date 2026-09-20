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

/// Catalog analyzability, pinned. The six write ops declare both halves; an
/// op that declares one and not the other is unanalyzable with exactly the
/// missing half named — never silently empty.
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
    for name in [
        "set_field",
        "create",
        "delete",
        "move_block",
        "split_block",
        "join_block",
    ] {
        assert_eq!(
            analyzability(name),
            Analyzability::Analyzable,
            "{name} declares both halves"
        );
    }
    assert_eq!(
        analyzability("indent"),
        Analyzability::Unanalyzable {
            undeclared: vec![UndeclaredHalf::Arcs, UndeclaredHalf::MarkingDelta]
        },
        "indent declares neither half, and the census must keep saying so"
    );
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
                .arcs()
                .into_iter()
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
                        // An `excluded` emit changes an arc's FLOW, never its
                        // presence: a relocate keeps its read half, so the
                        // place set is the plain aspect lowering.
                        expected.insert(place.to_string());
                    }
                }
            }
            let delta_places: BTreeSet<String> = transition
                .arcs()
                .into_iter()
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
            .arcs()
            .into_iter()
            .any(|a| matches!(a.origin, ArcOrigin::DeclaredRead | ArcOrigin::DeclaredEmit)),
        "declared-empty arcs contribute no arc-declaration arcs"
    );
}

/// An `excluded` emit is the only sanctioned way to say "this op does not
/// write this place", and it carries a reason. The marking delta's aspect
/// vocabulary is coarser than places — `structural` lowers onto BOTH placement
/// columns — so without subtraction the coarse half would silently overrule
/// the specific one, and the compiled net would report a write the declaration
/// explicitly denies.
#[test]
fn an_excluded_place_is_not_resurrected_by_the_delta_half() {
    let ops = block_catalog();
    let net = holon_net::derive_net(&ops, &[]).unwrap();
    let set_field = net
        .transition(&TransitionKey::operation("block", "set_field").expect("dotless entity"))
        .expect("the catalog advertises set_field");
    let written: Vec<String> = set_field
        .written_places()
        .iter()
        .map(|p| p.to_string())
        .collect();
    assert!(
        !written.contains(&"block.sort_key".to_string()),
        "set_field declares block.sort_key EXCLUDED (the ordering authority mints order \
         keys), yet the compiled net lists it as a write: {written:?}"
    );
    assert!(
        written.contains(&"block.parent_id".to_string()),
        "the subtraction must remove only the excluded place — parent_id is the other \
         half of the same structural lowering and must survive: {written:?}"
    );
}

/// A descriptor whose ONLY emit is an exclusion, so every write it compiles to
/// comes from the delta half and nothing else can supply one.
fn excluding_descriptor(
    structural: holon_api::marking::StructuralFlow,
    existence: holon_api::marking::ExistenceFlow,
) -> OperationDescriptor {
    use holon_api::marking::KindDelta;
    use holon_pattern::arcs::ArcEmit;
    use holon_pattern::arcs::ArcPlace;

    OperationDescriptor {
        entity_name: "block".into(),
        entity_short_name: "block".to_string(),
        id_column: "id".to_string(),
        name: "probe_op".to_string(),
        display_name: "Probe".to_string(),
        description: "A synthetic descriptor exercising the exclusion rule".to_string(),
        required_params: vec![],
        affected_fields: vec![],
        param_mappings: vec![],
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::Internal,
        },
        boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
        target_scope: holon_api::TargetScope::Block,
        trigger: None,
        bound_params: Default::default(),
        marking_delta: MarkingDelta::Static {
            kinds: vec![KindDelta {
                kind: holon_api::ArcRelation::block(),
                structural,
                text: holon_api::marking::TextFlow::Untouched,
                existence,
            }],
        },
        guard: holon_api::pattern::OpGuard::None,
        arcs: TransitionArcs::Declared {
            reads: vec![],
            emits: vec![ArcEmit::Excluded {
                place: ArcPlace::parse("block.sort_key").expect("a declared place"),
                reason: "the ordering authority mints order keys".to_string(),
            }],
        },
    }
}

/// The exclusion rule, from both sides at once. Dropping too little leaves the
/// excluded write standing; dropping too much takes the rest of the aspect
/// with it, and only this descriptor can catch that — the real catalog's ops
/// all declare their writes explicitly as well, so an explicit emit would mask
/// the loss.
#[test]
fn an_exclusion_removes_that_write_and_only_that_write() {
    use holon_api::marking::ExistenceFlow;
    use holon_api::marking::StructuralFlow;

    let net = holon_net::derive_net(
        &[excluding_descriptor(
            StructuralFlow::Relocates,
            ExistenceFlow::Untouched,
        )],
        &[],
    )
    .expect("the probe descriptor compiles");
    let transition = &net.transitions[0];
    let written: Vec<String> = transition
        .written_places()
        .iter()
        .map(|p| p.to_string())
        .collect();

    assert!(
        !written.contains(&"block.sort_key".to_string()),
        "the excluded place must not be a write: {written:?}"
    );
    assert_eq!(
        written,
        vec!["block.parent_id".to_string()],
        "the other half of the same structural lowering has no explicit emit behind it,          so losing it would mean the subtraction took the whole aspect"
    );
}

/// `ArcEmit::Excluded` says the op does not WRITE the place. Every flow that
/// also reads must keep that half — a relocate depends on the token it moves,
/// a consume on the one it takes away.
#[test]
fn an_exclusion_leaves_the_read_half_of_every_reading_flow() {
    use holon_api::marking::ExistenceFlow;
    use holon_api::marking::StructuralFlow;

    for structural in [StructuralFlow::Relocates, StructuralFlow::Consumes] {
        let net = holon_net::derive_net(
            &[excluding_descriptor(structural, ExistenceFlow::Reads)],
            &[],
        )
        .expect("the probe descriptor compiles");
        let transition = &net.transitions[0];
        let read: Vec<String> = transition
            .read_places()
            .iter()
            .map(|p| p.to_string())
            .collect();

        assert!(
            read.contains(&"block.sort_key".to_string()),
            "excluding a write must not silently drop the read {structural:?} carries: \
             {read:?}"
        );
        assert!(
            read.contains(&"block.id".to_string()),
            "a delta-derived READ on an unexcluded place must survive untouched: {read:?}"
        );
    }
}
