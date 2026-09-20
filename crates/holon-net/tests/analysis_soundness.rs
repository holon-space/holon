//! Do correlated places and firing modes change what the conflict and cycle
//! analyses report?
//!
//! Two claims, each pinned here before Inc 1 relies on it.
//!
//! 1. A correlation is refinement-like, so it is invisible to the analyses.
//!    Proven on the field that already has that property: adding a `refinement`
//!    to every arc leaves both reports byte-identical. A `Correlation` sits
//!    beside it with the same opacity, so the place-level contention set cannot
//!    move.
//! 2. Every enabledness-relevant flow must reach `read_places`. Proven by the
//!    consequence of omitting one: a real contention edge disappears. That is
//!    the unsoundness an inhibitor flow would introduce if it were left out.

use holon_net::Analyzability;
use holon_net::ArcOrigin;
use holon_net::CompiledNet;
use holon_net::Flow;
use holon_net::NetArc;
use holon_net::NetEntity;
use holon_net::NetTransition;
use holon_net::Refinement;
use holon_net::TransitionMode;
use holon_net::TransitionSource;
use holon_net::conflicts;
use holon_net::cycles;
use holon_pattern::Value;
use holon_pattern::arcs::ArcPlace;
use holon_pattern::pattern::CmpOp;

fn arc(field: &str, flow: Flow) -> NetArc {
    NetArc {
        place: ArcPlace::new("block", field),
        flow,
        origin: ArcOrigin::DeclaredRead,
        refinement: None,
        binding: None,
    }
}

fn transition(op: &str, arcs: Vec<NetArc>) -> NetTransition {
    NetTransition {
        source: TransitionSource::Operation {
            entity: NetEntity::parse("block").expect("dotless"),
            op: op.to_string(),
        },
        analyzability: Analyzability::Analyzable,
        modes: vec![TransitionMode::new(arcs)],
        residue: vec![],
    }
}

/// Two transitions contending on `block.parent_id`, one of them also reading
/// `block.content` — enough shape for both analyses to have something to say.
fn contended_net() -> CompiledNet {
    CompiledNet {
        transitions: vec![
            transition(
                "move_block",
                vec![
                    arc("parent_id", Flow::Relocate),
                    arc("content", Flow::Read),
                    arc("sort_key", Flow::Produce),
                ],
            ),
            transition(
                "set_field",
                vec![
                    arc("parent_id", Flow::Produce),
                    arc("content", Flow::Produce),
                    arc("id", Flow::Read),
                ],
            ),
            transition(
                "indent",
                vec![arc("parent_id", Flow::Read), arc("sort_key", Flow::Read)],
            ),
        ],
    }
}

fn a_refinement() -> Refinement {
    Refinement::Cell {
        op: CmpOp::Eq,
        rhs: Value::String("source".to_string()),
    }
}

/// Claim 1. A predicate riding on an arc is opaque to both analyses, so the
/// correlation the hop adds cannot move the contention set.
#[test]
fn rc_a_refinement_on_every_arc_leaves_both_reports_identical() {
    let plain = contended_net();
    let mut refined = contended_net();
    for transition in &mut refined.transitions {
        for mode in &mut transition.modes {
            for arc in &mut mode.arcs {
                arc.refinement = Some(a_refinement());
            }
        }
    }

    let (plain_conflicts, refined_conflicts) = (conflicts(&plain), conflicts(&refined));
    let (plain_cycles, refined_cycles) = (cycles(&plain), cycles(&refined));

    assert!(
        !plain_conflicts.contentions.is_empty(),
        "the fixture must contend somewhere or this test proves nothing"
    );
    assert_eq!(
        serde_json::to_string(&plain_conflicts).expect("serialize"),
        serde_json::to_string(&refined_conflicts).expect("serialize"),
        "a refinement moved the conflict report"
    );
    assert_eq!(
        serde_json::to_string(&plain_cycles).expect("serialize"),
        serde_json::to_string(&refined_cycles).expect("serialize"),
        "a refinement moved the cycle report"
    );
    println!(
        "[analysis] contentions with and without refinements: {} / {}",
        plain_conflicts.contentions.len(),
        refined_conflicts.contentions.len()
    );
}

/// Claim 1, second half. Firing modes keep `arcs` as their union, and a union
/// is what the analyses read — so re-grouping arcs into modes is a no-op here
/// as long as no arc is dropped. Pinned by re-ordering the arcs, the only
/// observable a mode split could change at this level.
#[test]
fn rc_arc_order_does_not_move_the_reports() {
    let plain = contended_net();
    let mut reordered = contended_net();
    for transition in &mut reordered.transitions {
        for mode in &mut transition.modes {
            mode.arcs.reverse();
        }
    }
    assert_eq!(
        serde_json::to_string(&conflicts(&plain)).expect("serialize"),
        serde_json::to_string(&conflicts(&reordered)).expect("serialize"),
        "the conflict report depends on arc order, so a mode split could move it"
    );
    assert_eq!(
        serde_json::to_string(&cycles(&plain)).expect("serialize"),
        serde_json::to_string(&cycles(&reordered)).expect("serialize"),
        "the cycle report depends on arc order, so a mode split could move it"
    );
    println!("[analysis] both reports are independent of arc order");
}

/// Claim 2. An enabledness-relevant arc that does not reach `read_places`
/// costs a real contention edge. This is what omitting `Flow::Inhibit` from
/// `NetTransition::read_places` would do, stated as a measurement rather than
/// as a review note.
#[test]
fn rc_an_enabledness_arc_outside_read_places_loses_a_contention() {
    let with_read = contended_net();
    let reader = with_read
        .transitions
        .iter()
        .find(|t| t.key().as_str().ends_with("indent"))
        .expect("the reading transition");
    assert!(
        reader
            .read_places()
            .contains(&ArcPlace::new("block", "parent_id")),
        "the fixture's reader must read the contended place"
    );

    let baseline = conflicts(&with_read);
    let parent_id = ArcPlace::new("block", "parent_id");
    let readers_before = baseline
        .contentions
        .iter()
        .find(|c| c.place == parent_id)
        .map(|c| c.readers.len())
        .expect("the contended place is reported");

    // Drop the reading transition's arcs entirely — the effect of a flow the
    // read set does not recognise.
    let mut without_read = contended_net();
    for transition in &mut without_read.transitions {
        if transition.key().as_str().ends_with("indent") {
            for mode in &mut transition.modes {
                mode.arcs.clear();
            }
        }
    }
    let readers_after = conflicts(&without_read)
        .contentions
        .iter()
        .find(|c| c.place == parent_id)
        .map(|c| c.readers.len())
        .unwrap_or(0);

    assert!(
        readers_after < readers_before,
        "omitting an enabledness arc must lose a reader, else this test has no teeth: \
         before={readers_before} after={readers_after}"
    );
    println!(
        "[analysis] readers of block.parent_id with the arc: {readers_before}, without it: \
         {readers_after} — omission is detectable"
    );
}
