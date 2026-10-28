//! Increment 1 of the Loro typed-entity store (`LoroTypedEntityStore.md`
//! section 10): does the candidate on-disk shape converge per field across
//! three peers, and what does a delete do against a concurrent write?
//!
//! The shape is the one part of the plan a later fix cannot change without a
//! wipe, so it is settled here before any wiring exists.
//!
//! - `three_peers_converge_per_field` is the PBT over shapes A and B
//! - `plain_container_child_loses_a_field_on_concurrent_create` is its teeth:
//!   the same harness refutes shape A built with `insert_container`
//! - `red_proof_plain_container_child_fails_the_convergence_oracle` is that
//!   refutation as a failing run, for the red log (`--ignored`)
//! - the `delete_*` / `purge_*` tests are the measurements that decide the
//!   delete rule (plan question 2)

use std::collections::BTreeMap;

use loro::LoroValue;
use proptest::prelude::*;
use typed_entity_shape::CANDIDATES;
use typed_entity_shape::Clock;
use typed_entity_shape::FieldPath;
use typed_entity_shape::Oracle;
use typed_entity_shape::Peer;
use typed_entity_shape::Shape;
use typed_entity_shape::TOMBSTONE;
use typed_entity_shape::diverges;
use typed_entity_shape::purge_entity;
use typed_entity_shape::read_all;
use typed_entity_shape::read_entity;
use typed_entity_shape::sync_pair;
use typed_entity_shape::sync_to_closure;
use typed_entity_shape::write_field;

use crate::typed_entity_shape;

const TYPE: &str = "repair-record";
const IDS: [&str; 3] = ["r0", "r1", "r2"];
const PEERS: usize = 3;

fn field_of(i: u8) -> FieldPath {
    match i % 5 {
        0 => FieldPath::scalar("subject"),
        1 => FieldPath::scalar("severity"),
        2 => FieldPath::scalar(TOMBSTONE),
        3 => FieldPath::property("color"),
        _ => FieldPath::property("origin"),
    }
}

fn value_of(i: u8) -> LoroValue {
    match i % 4 {
        0 => LoroValue::from("alpha"),
        1 => LoroValue::from("beta"),
        2 => LoroValue::from(7i64),
        _ => LoroValue::Null,
    }
}

#[derive(Clone, Debug)]
enum Step {
    Write {
        peer: usize,
        entity: usize,
        field: u8,
        value: u8,
    },
    Sync {
        a: usize,
        b: usize,
    },
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        4 => (0..PEERS, 0..IDS.len(), 0u8..5, 0u8..4).prop_map(
            |(peer, entity, field, value)| Step::Write { peer, entity, field, value }
        ),
        1 => (0..PEERS, 0..PEERS).prop_map(|(a, b)| Step::Sync { a, b }),
    ]
}

/// Apply `script` to three fresh peers, sync to closure, and compare every
/// peer against the per-field last-writer-wins oracle.
fn run(shape: Shape, script: &[Step]) -> Result<(), String> {
    let peers: Vec<Peer> = (1..=PEERS as u64).map(Peer::new).collect();
    let mut clock = Clock::default();
    let mut oracle = Oracle::default();

    for step in script {
        match step {
            Step::Write {
                peer,
                entity,
                field,
                value,
            } => {
                let field = field_of(*field);
                let value = value_of(*value);
                let id = IDS[*entity];
                if let Some(lamport) = write_field(
                    &peers[*peer],
                    &mut clock,
                    shape,
                    TYPE,
                    id,
                    &field,
                    value.clone(),
                ) {
                    oracle.record(id, &field, value, lamport);
                }
            }
            Step::Sync { a, b } if a != b => sync_pair(&peers[*a].doc, &peers[*b].doc),
            Step::Sync { .. } => {}
        }
    }

    sync_to_closure(&peers);
    let expected = oracle.expected();
    for peer in &peers {
        let actual = read_all(&peer.doc, shape, TYPE);
        let label = format!("{} peer {}", shape.label(), peer.peer_id);
        if let Some(report) = diverges(&label, &actual, &expected) {
            return Err(report);
        }
    }
    Ok(())
}

proptest! {
    #[test]
    fn three_peers_converge_per_field(script in prop::collection::vec(step(), 1..24)) {
        for shape in CANDIDATES {
            let outcome = run(shape, &script);
            prop_assert!(outcome.is_ok(), "{}", outcome.unwrap_err());
        }
    }
}

/// Two peers create one id concurrently, each with its own field, then sync.
fn concurrent_create() -> Vec<Step> {
    vec![
        Step::Write {
            peer: 0,
            entity: 0,
            field: 0,
            value: 0,
        },
        Step::Write {
            peer: 1,
            entity: 0,
            field: 1,
            value: 1,
        },
        Step::Sync { a: 0, b: 1 },
    ]
}

/// The teeth of the PBT: with a plain `insert_container` child the parent key's
/// LWW keeps one container and the other peer's field is gone. Shape A exists
/// only because of this.
#[test]
fn plain_container_child_loses_a_field_on_concurrent_create() {
    let report = run(Shape::PlainContainerChild, &concurrent_create())
        .expect_err("a plain container child must lose a field on concurrent create");
    assert!(
        report.contains("lost field"),
        "the loss must be a missing field, not some other divergence; got: {report}"
    );

    for shape in CANDIDATES {
        run(shape, &concurrent_create())
            .unwrap_or_else(|e| panic!("{} must keep both fields: {e}", shape.label()));
    }
}

/// The red log of increment 1: the same oracle, run against the shape the plan
/// rejects, as a failing test.
///
/// `cargo test -p holon-loro --test main
/// typed_entity_convergence_pbt::red_proof -- --ignored`
#[test]
#[ignore = "red proof for the increment-1 log: asserts the shape the plan rejects"]
fn red_proof_plain_container_child_fails_the_convergence_oracle() {
    run(Shape::PlainContainerChild, &concurrent_create()).unwrap();
}

// -- delete against a concurrent write (plan question 2) --

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeleteRule {
    /// (2a) remove the entity's keys.
    KeyRemoval,
    /// (2b) set the soft-delete stamp, purge later.
    Tombstone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Order {
    /// The delete carries the later Lamport.
    DeleteLast,
    /// The concurrent field write carries the later Lamport.
    WriteLast,
}

const STAMP: &str = "2026-10-11T00:00:00Z";

fn seed_two_peers(shape: Shape) -> (Vec<Peer>, Clock) {
    let peers: Vec<Peer> = (1..=2u64).map(Peer::new).collect();
    let mut clock = Clock::default();
    for (field, value) in [("subject", "block-7"), ("severity", "low")] {
        write_field(
            &peers[0],
            &mut clock,
            shape,
            TYPE,
            "r0",
            &FieldPath::scalar(field),
            LoroValue::from(value),
        )
        .expect("a fresh field is written, not skipped");
    }
    sync_pair(&peers[0].doc, &peers[1].doc);
    (peers, clock)
}

/// Peer 1 deletes `r0` while peer 2 concurrently writes `severity`; the two ops
/// are ordered by Lamport but never synced before the merge.
fn delete_against_concurrent_write(
    shape: Shape,
    rule: DeleteRule,
    order: Order,
) -> Option<BTreeMap<String, LoroValue>> {
    let (peers, mut clock) = seed_two_peers(shape);

    let delete = |clock: &mut Clock| match rule {
        DeleteRule::KeyRemoval => purge_entity(&peers[0], clock, shape, TYPE, "r0"),
        DeleteRule::Tombstone => {
            write_field(
                &peers[0],
                clock,
                shape,
                TYPE,
                "r0",
                &FieldPath::scalar(TOMBSTONE),
                LoroValue::from(STAMP),
            )
            .expect("a fresh stamp is written, not skipped");
        }
    };
    let write = |clock: &mut Clock| {
        write_field(
            &peers[1],
            clock,
            shape,
            TYPE,
            "r0",
            &FieldPath::scalar("severity"),
            LoroValue::from("high"),
        )
        .expect("a changed value is written, not skipped");
    };

    match order {
        Order::DeleteLast => {
            write(&mut clock);
            delete(&mut clock);
        }
        Order::WriteLast => {
            delete(&mut clock);
            write(&mut clock);
        }
    }

    sync_to_closure(&peers);
    let a = read_entity(&peers[0].doc, shape, TYPE, "r0");
    let b = read_entity(&peers[1].doc, shape, TYPE, "r0");
    assert_eq!(a, b, "{}: the peers disagree after sync", shape.label());
    a
}

fn describe(state: &Option<BTreeMap<String, LoroValue>>) -> String {
    match state {
        None => "GONE".to_string(),
        Some(fields) => {
            let mut parts: Vec<String> = fields
                .iter()
                .map(|(k, v)| format!("{k}={}", render(v)))
                .collect();
            parts.sort();
            format!("{{{}}}", parts.join(", "))
        }
    }
}

fn render(v: &LoroValue) -> String {
    match v {
        LoroValue::String(s) => s.to_string(),
        other => format!("{other:?}"),
    }
}

/// The measurement that decides question 2: a tombstone keeps BOTH the delete
/// and the concurrent write; a key removal silently loses one of them, which
/// one depending on the shape and the Lamport order.
#[test]
fn delete_against_a_concurrent_write_is_measured_for_both_shapes() {
    let mut rows: Vec<String> = Vec::new();
    for shape in CANDIDATES {
        for rule in [DeleteRule::KeyRemoval, DeleteRule::Tombstone] {
            for order in [Order::DeleteLast, Order::WriteLast] {
                let state = delete_against_concurrent_write(shape, rule, order);
                rows.push(format!(
                    "{:<24} {:<12} {:<11} -> {}",
                    shape.label(),
                    format!("{rule:?}"),
                    format!("{order:?}"),
                    describe(&state)
                ));

                match (shape, rule) {
                    // The tombstone and the field live on different keys, so
                    // neither can overwrite the other: both survive.
                    (_, DeleteRule::Tombstone) => {
                        let fields = state.expect("a tombstoned entity still exists");
                        assert_eq!(
                            fields.get(TOMBSTONE),
                            Some(&LoroValue::from(STAMP)),
                            "the tombstone must survive the concurrent write"
                        );
                        assert_eq!(
                            fields.get("severity"),
                            Some(&LoroValue::from("high")),
                            "the concurrent write must survive the tombstone"
                        );
                    }
                    // The concurrent write lands INSIDE the child container and
                    // never touches the root key, so the key removal wins
                    // regardless of order and the write is lost in silence.
                    (
                        Shape::MergeableChild | Shape::PlainContainerChild,
                        DeleteRule::KeyRemoval,
                    ) => {
                        assert_eq!(
                            state, None,
                            "a key removal discards the whole entity, concurrent write included"
                        );
                    }
                    // Both ops touch `r0/severity`, so the later Lamport wins:
                    // either the write is lost or the entity half-survives as
                    // the one field the write re-created.
                    (Shape::FlatKeys, DeleteRule::KeyRemoval) => match order {
                        Order::DeleteLast => assert_eq!(
                            state, None,
                            "a later key removal discards the concurrent write"
                        ),
                        Order::WriteLast => assert_eq!(
                            state,
                            Some(BTreeMap::from([(
                                "severity".to_string(),
                                LoroValue::from("high")
                            )])),
                            "a later write resurrects the entity as that one field"
                        ),
                    },
                }
            }
        }
    }
    println!("\n== delete against a concurrent write (debug profile; outcome is profile-free)");
    for row in rows {
        println!("{row}");
    }
}

// -- purge against a late concurrent write (plan question 2b) --

/// Peer 3 is offline across the tombstone AND the purge, writes the entity, and
/// only then comes back.
#[test]
fn a_purge_discards_a_late_write_from_an_offline_peer() {
    let mut rows: Vec<String> = Vec::new();
    for shape in CANDIDATES {
        let peers: Vec<Peer> = (1..=3u64).map(Peer::new).collect();
        let mut clock = Clock::default();
        for (field, value) in [("subject", "block-7"), ("severity", "low")] {
            write_field(
                &peers[0],
                &mut clock,
                shape,
                TYPE,
                "r0",
                &FieldPath::scalar(field),
                LoroValue::from(value),
            )
            .expect("a fresh field is written, not skipped");
        }
        sync_to_closure(&peers);

        // Peers 1 and 2 tombstone and then purge; peer 3 never sees either.
        write_field(
            &peers[0],
            &mut clock,
            shape,
            TYPE,
            "r0",
            &FieldPath::scalar(TOMBSTONE),
            LoroValue::from(STAMP),
        )
        .expect("a fresh stamp is written, not skipped");
        sync_pair(&peers[0].doc, &peers[1].doc);
        purge_entity(&peers[0], &mut clock, shape, TYPE, "r0");
        sync_pair(&peers[0].doc, &peers[1].doc);

        write_field(
            &peers[2],
            &mut clock,
            shape,
            TYPE,
            "r0",
            &FieldPath::scalar("severity"),
            LoroValue::from("high"),
        )
        .expect("a changed value is written, not skipped");

        sync_to_closure(&peers);
        let state = read_entity(&peers[0].doc, shape, TYPE, "r0");
        rows.push(format!(
            "{:<24} late set_field after purge -> {}",
            shape.label(),
            describe(&state)
        ));

        match shape {
            // The late write sits inside the purged child container: the root
            // key stays deleted, so the write is unreachable and silent.
            Shape::MergeableChild | Shape::PlainContainerChild => assert_eq!(
                state, None,
                "the purge discards the offline peer's write without a trace"
            ),
            // The late write re-creates its own key with the later Lamport, so
            // the entity half-returns.
            Shape::FlatKeys => assert_eq!(
                state,
                Some(BTreeMap::from([(
                    "severity".to_string(),
                    LoroValue::from("high")
                )])),
                "the late write resurrects the entity as that one field"
            ),
        }
    }
    println!("\n== purge against a late write from an offline peer (debug profile)");
    for row in rows {
        println!("{row}");
    }
}

/// A peer that HAS seen the purge creates the same id again. Shape A's child
/// container id is deterministic from the key, so re-inserting the pointer
/// brings every purged field back; shape B starts clean.
#[test]
fn a_create_after_a_seen_purge_resurrects_shape_a_but_not_shape_b() {
    let mut rows: Vec<String> = Vec::new();
    for shape in CANDIDATES {
        let peers: Vec<Peer> = (1..=2u64).map(Peer::new).collect();
        let mut clock = Clock::default();
        for (field, value) in [("subject", "block-7"), ("severity", "low")] {
            write_field(
                &peers[0],
                &mut clock,
                shape,
                TYPE,
                "r0",
                &FieldPath::scalar(field),
                LoroValue::from(value),
            )
            .expect("a fresh field is written, not skipped");
        }
        purge_entity(&peers[0], &mut clock, shape, TYPE, "r0");
        sync_to_closure(&peers);
        assert_eq!(
            read_entity(&peers[1].doc, shape, TYPE, "r0"),
            None,
            "{}: the purge must be visible on peer 2 before it re-creates",
            shape.label()
        );

        write_field(
            &peers[1],
            &mut clock,
            shape,
            TYPE,
            "r0",
            &FieldPath::scalar("device"),
            LoroValue::from("laptop"),
        )
        .expect("a fresh field is written, not skipped");
        sync_to_closure(&peers);

        let state = read_entity(&peers[0].doc, shape, TYPE, "r0");
        rows.push(format!(
            "{:<24} create after a SEEN purge -> {}",
            shape.label(),
            describe(&state)
        ));

        let fields = state.expect("the re-created entity exists");
        assert_eq!(
            fields.get("device"),
            Some(&LoroValue::from("laptop")),
            "the new field is there in both shapes"
        );
        match shape {
            Shape::MergeableChild | Shape::PlainContainerChild => assert_eq!(
                fields.get("subject"),
                Some(&LoroValue::from("block-7")),
                "shape A's deterministic child id brings the purged fields back"
            ),
            Shape::FlatKeys => assert_eq!(
                fields.get("subject"),
                None,
                "shape B's purged keys stay gone"
            ),
        }
    }
    println!("\n== create on a purged id (debug profile)");
    for row in rows {
        println!("{row}");
    }
}
