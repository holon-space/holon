//! Increment 2 check of the Loro typed-entity store (`LoroTypedEntityStore.md`
//! section 4): a deterministic id (`RepairRecord`) that was purged must be
//! creatable again, clean, when the same violation recurs.
//!
//! Shape A's entity child has a container id derived from its key, so the
//! purge rule decides what a re-create sees. Each candidate rule is measured:
//! - `KeyRemoval`: the purge removes the root key only (increment 1's purge)
//! - `ClearThenRemove`: the purge clears the child's keys, the overflow bag's
//!   keys included, then removes the root key
//! - `OccurrenceId`: the purge removes the root key; the id derivation includes
//!   the occurrence, so a recurrence writes a different key
//!
//! Ruled (#168): `OccurrenceId`, with the occurrence computed from replicated
//! state. `requirement_*` holds under that rule; the per-rule measurements keep
//! the evidence that both key-only rules fail it.

mod typed_entity_shape;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::ops::ControlFlow;

use loro::ContainerID;
use loro::ID;
use loro::LoroDoc;
use loro::LoroMap;
use loro::LoroValue;
use typed_entity_shape::Clock;
use typed_entity_shape::FieldPath;
use typed_entity_shape::PROPERTIES;
use typed_entity_shape::Peer;
use typed_entity_shape::Shape;
use typed_entity_shape::TOMBSTONE;
use typed_entity_shape::purge_entity;
use typed_entity_shape::put_field;
use typed_entity_shape::read_entity;
use typed_entity_shape::root;
use typed_entity_shape::scan_ids;
use typed_entity_shape::sync_pair;
use typed_entity_shape::sync_to_closure;
use typed_entity_shape::write_field;

const TYPE: &str = "repair-record";
const SHAPE: Shape = Shape::MergeableChild;
const STAMP: &str = "2026-10-11T00:00:00Z";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PurgeRule {
    KeyRemoval,
    ClearThenRemove,
    OccurrenceId,
}

const RULES: [PurgeRule; 3] = [
    PurgeRule::KeyRemoval,
    PurgeRule::ClearThenRemove,
    PurgeRule::OccurrenceId,
];

impl PurgeRule {
    /// The store key of the `n`-th occurrence of the violation `r0`.
    fn id(self, occurrence: u32) -> String {
        match self {
            PurgeRule::KeyRemoval | PurgeRule::ClearThenRemove => "r0".to_string(),
            PurgeRule::OccurrenceId => format!("r0.{occurrence}"),
        }
    }

    fn purge(self, peer: &Peer, clock: &mut Clock, id: &str) {
        if self == PurgeRule::ClearThenRemove {
            let entity = root(&peer.doc, TYPE)
                .ensure_mergeable_map(id)
                .expect("the purged entity's child map opens");
            entity
                .ensure_mergeable_map(PROPERTIES)
                .expect("the overflow bag opens")
                .clear()
                .expect("the overflow bag clears");
            entity.clear().expect("the entity map clears");
            peer.doc.commit();
        }
        purge_entity(peer, clock, SHAPE, TYPE, id);
    }
}

fn write(peer: &Peer, clock: &mut Clock, id: &str, field: FieldPath, value: LoroValue) {
    write_field(peer, clock, SHAPE, TYPE, id, &field, value)
        .expect("a changed value is written, not skipped");
}

/// The first occurrence: created, tombstoned, acked by every peer.
fn first_occurrence(rule: PurgeRule, peers: &[Peer], clock: &mut Clock) {
    let id = rule.id(1);
    write(
        &peers[0],
        clock,
        &id,
        FieldPath::scalar("subject"),
        "block-7".into(),
    );
    write(
        &peers[0],
        clock,
        &id,
        FieldPath::scalar("severity"),
        "low".into(),
    );
    write(
        &peers[0],
        clock,
        &id,
        FieldPath::property("note"),
        "first".into(),
    );
    write(
        &peers[0],
        clock,
        &id,
        FieldPath::scalar(TOMBSTONE),
        STAMP.into(),
    );
    sync_to_closure(peers);
}

/// The recurrence as a create writes it: the declared fields it has, nothing
/// else.
fn recur(rule: PurgeRule, peer: &Peer, clock: &mut Clock) {
    let id = rule.id(2);
    // Skipped when the unpurged first occurrence already holds this value.
    let _ = write_field(
        peer,
        clock,
        SHAPE,
        TYPE,
        &id,
        &FieldPath::scalar("subject"),
        "block-7".into(),
    );
    write(
        peer,
        clock,
        &id,
        FieldPath::scalar("device"),
        "laptop".into(),
    );
}

fn recurrence_fields() -> BTreeMap<String, LoroValue> {
    BTreeMap::from([
        ("subject".to_string(), LoroValue::from("block-7")),
        ("device".to_string(), LoroValue::from("laptop")),
    ])
}

/// Purge seen everywhere, then the violation recurs on peer 2.
fn recreate_after_a_seen_purge(rule: PurgeRule) -> Option<BTreeMap<String, LoroValue>> {
    let peers: Vec<Peer> = (1..=2u64).map(Peer::new).collect();
    let mut clock = Clock::default();
    first_occurrence(rule, &peers, &mut clock);
    rule.purge(&peers[0], &mut clock, &rule.id(1));
    sync_to_closure(&peers);
    assert_eq!(
        read_entity(&peers[1].doc, SHAPE, TYPE, &rule.id(1)),
        None,
        "{rule:?}: the purge must be visible on peer 2 before the recurrence"
    );
    recur(rule, &peers[1], &mut clock);
    sync_to_closure(&peers);
    let a = read_entity(&peers[0].doc, SHAPE, TYPE, &rule.id(2));
    assert_eq!(a, read_entity(&peers[1].doc, SHAPE, TYPE, &rule.id(2)));
    a
}

/// Peer 2 acked the tombstone; the violation recurs on peer 2 while peer 1,
/// not yet aware of it, purges. `purge_last` orders the purge after the
/// recurrence by Lamport.
fn recreate_concurrent_with_the_purge(
    rule: PurgeRule,
    purge_last: bool,
) -> Option<BTreeMap<String, LoroValue>> {
    let peers: Vec<Peer> = (1..=2u64).map(Peer::new).collect();
    let mut clock = Clock::default();
    first_occurrence(rule, &peers, &mut clock);
    let restore = |clock: &mut Clock| {
        if rule.id(2) == rule.id(1) {
            write(
                &peers[1],
                clock,
                &rule.id(2),
                FieldPath::scalar(TOMBSTONE),
                LoroValue::Null,
            );
        }
    };
    if purge_last {
        recur(rule, &peers[1], &mut clock);
        restore(&mut clock);
        rule.purge(&peers[0], &mut clock, &rule.id(1));
    } else {
        rule.purge(&peers[0], &mut clock, &rule.id(1));
        recur(rule, &peers[1], &mut clock);
        restore(&mut clock);
    }
    sync_pair(&peers[0].doc, &peers[1].doc);
    let a = read_entity(&peers[0].doc, SHAPE, TYPE, &rule.id(2));
    assert_eq!(a, read_entity(&peers[1].doc, SHAPE, TYPE, &rule.id(2)));
    a
}

fn describe(state: &Option<BTreeMap<String, LoroValue>>) -> String {
    match state {
        None => "GONE".to_string(),
        Some(fields) => format!(
            "{{{}}}",
            fields
                .iter()
                .map(|(k, v)| format!("{k}={v:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[test]
fn recreate_after_a_seen_purge_is_measured_per_rule() {
    let mut rows = Vec::new();
    for rule in RULES {
        let state = recreate_after_a_seen_purge(rule);
        rows.push(format!(
            "{:<16} seen purge, recurrence -> {}",
            format!("{rule:?}"),
            describe(&state)
        ));
        match rule {
            PurgeRule::KeyRemoval => {
                let fields = state.expect("the recurrence exists");
                assert_eq!(
                    fields.get(TOMBSTONE),
                    Some(&LoroValue::from(STAMP)),
                    "the old tombstone comes back"
                );
                assert_eq!(
                    fields.get("severity"),
                    Some(&LoroValue::from("low")),
                    "the old field comes back"
                );
                assert_eq!(
                    fields.get(&format!("{PROPERTIES}/note")),
                    Some(&LoroValue::from("first")),
                    "the old overflow key comes back"
                );
            }
            PurgeRule::ClearThenRemove | PurgeRule::OccurrenceId => {
                assert_eq!(
                    state,
                    Some(recurrence_fields()),
                    "{rule:?}: the recurrence starts clean"
                )
            }
        }
    }
    println!(
        "\n== create on a purged deterministic id, shape A (debug profile; outcome is profile-free)"
    );
    for row in rows {
        println!("{row}");
    }
}

#[test]
fn recreate_concurrent_with_the_purge_is_measured_per_rule() {
    let mut rows = Vec::new();
    for rule in RULES {
        for purge_last in [true, false] {
            let state = recreate_concurrent_with_the_purge(rule, purge_last);
            rows.push(format!(
                "{:<16} purge {:<6} recurrence -> {}",
                format!("{rule:?}"),
                if purge_last { "after" } else { "before" },
                describe(&state)
            ));
            match rule {
                // The recurrence writes inside the child; the purge's root-key
                // removal is the only op on the root key, so it wins.
                PurgeRule::KeyRemoval | PurgeRule::ClearThenRemove => {
                    assert_eq!(
                        state, None,
                        "{rule:?}: the concurrent recurrence is lost in silence"
                    )
                }
                PurgeRule::OccurrenceId => {
                    assert_eq!(
                        state,
                        Some(recurrence_fields()),
                        "the recurrence has its own key"
                    )
                }
            }
        }
    }
    println!(
        "\n== recurrence concurrent with the purge, shape A (debug profile; outcome is profile-free)"
    );
    for row in rows {
        println!("{row}");
    }
}

// -- the ruled rule: the occurrence comes from replicated state --

/// The occurrence of a violation on `subject`: the causally latest op among
/// the changes that touched it, as `<peer>.<counter>`. Every input is in the
/// oplog, so two devices at the same version of the subject compute one id.
fn occurrence(doc: &LoroDoc, subject: &ContainerID) -> String {
    let heads: Vec<ID> = doc.oplog_frontiers().iter().collect();
    let mut latest: Option<(u32, u64, i32)> = None;
    doc.travel_change_ancestors(&heads, &mut |meta| {
        if doc
            .get_changed_containers_in(meta.id, meta.len)
            .contains(subject)
        {
            let last = (
                meta.lamport + meta.len as u32 - 1,
                meta.id.peer,
                meta.id.counter + meta.len as i32 - 1,
            );
            latest = Some(latest.map_or(last, |l| l.max(last)));
        }
        ControlFlow::Continue(())
    })
    .expect("the full history is travelable");
    let (_, peer, counter) = latest.expect("a violated subject has at least one change");
    format!("{peer}.{counter}")
}

fn subject(doc: &LoroDoc) -> LoroMap {
    doc.get_map("subjects")
        .ensure_mergeable_map("b7")
        .expect("the subject's child map opens")
}

fn violate_on(peer: &Peer, round: u32) {
    subject(&peer.doc)
        .insert("x", format!("round {round} peer {}", peer.peer_id))
        .expect("the subject takes a field");
    peer.doc.commit();
}

/// Peers 1 and 2 write the subject's `x` concurrently: the violation.
fn violate(peers: &[Peer], round: u32) {
    for peer in peers.iter().take(2) {
        violate_on(peer, round);
    }
}

/// Each detecting peer writes the record under the id it derives, plus one
/// field only it knows.
fn repair(peer: &Peer, detector_field: &str) -> String {
    let id = format!(
        "r0@{}",
        occurrence(&peer.doc, &loro::ContainerTrait::id(&subject(&peer.doc)))
    );
    put_field(
        &peer.doc,
        SHAPE,
        TYPE,
        &id,
        &FieldPath::scalar("subject"),
        "block-7".into(),
    );
    put_field(
        &peer.doc,
        SHAPE,
        TYPE,
        &id,
        &FieldPath::scalar(detector_field),
        LoroValue::from(true),
    );
    peer.doc.commit();
    id
}

#[test]
fn two_devices_that_repair_one_occurrence_write_one_record() {
    let peers: Vec<Peer> = (1..=3u64).map(Peer::new).collect();
    violate(&peers, 1);
    sync_to_closure(&peers);

    // Peer 2 has an unrelated change on top; it does not touch the subject.
    peers[1]
        .doc
        .get_map("unrelated")
        .insert("k", 1)
        .expect("a map takes a key");
    peers[1].doc.commit();

    let first_a = repair(&peers[0], "seen_by_1");
    let first_b = repair(&peers[1], "seen_by_2");
    assert_eq!(
        first_a, first_b,
        "both devices derive one id for one occurrence"
    );
    sync_to_closure(&peers);
    assert_eq!(
        scan_ids(&peers[2].doc, SHAPE, TYPE),
        BTreeSet::from([first_a.clone()]),
        "one record per occurrence"
    );
    assert_eq!(
        read_entity(&peers[2].doc, SHAPE, TYPE, &first_a),
        Some(BTreeMap::from([
            ("seen_by_1".to_string(), LoroValue::from(true)),
            ("seen_by_2".to_string(), LoroValue::from(true)),
            ("subject".to_string(), LoroValue::from("block-7")),
        ])),
        "the record holds the union of both repairs"
    );

    let mut clock = Clock::default();
    purge_entity(&peers[0], &mut clock, SHAPE, TYPE, &first_a);
    sync_to_closure(&peers);

    violate(&peers, 2);
    sync_to_closure(&peers);
    let second_a = repair(&peers[0], "seen_by_1");
    let second_b = repair(&peers[1], "seen_by_2");
    assert_eq!(
        second_a, second_b,
        "the recurrence also converges to one id"
    );
    assert_ne!(second_a, first_a, "the recurrence is a new occurrence");
    sync_to_closure(&peers);
    assert_eq!(
        scan_ids(&peers[2].doc, SHAPE, TYPE),
        BTreeSet::from([second_a.clone()]),
        "the purged occurrence stays gone"
    );
    assert_eq!(
        read_entity(&peers[2].doc, SHAPE, TYPE, &second_a)
            .expect("the recurrence exists")
            .len(),
        3,
        "the recurrence starts clean"
    );
}

/// Under the ruled rule a recurrence derives a new id and starts clean, whether
/// every peer saw the purge first or the purge is concurrent with it.
#[test]
fn requirement_a_purged_deterministic_id_is_created_again_clean() {
    for purge_seen in [true, false] {
        let peers: Vec<Peer> = (1..=2u64).map(Peer::new).collect();
        let mut clock = Clock::default();
        violate(&peers, 1);
        sync_to_closure(&peers);
        let first = repair(&peers[0], "seen_by_1");
        put_field(
            &peers[0].doc,
            SHAPE,
            TYPE,
            &first,
            &FieldPath::property("note"),
            "first".into(),
        );
        put_field(
            &peers[0].doc,
            SHAPE,
            TYPE,
            &first,
            &FieldPath::scalar(TOMBSTONE),
            STAMP.into(),
        );
        peers[0].doc.commit();
        sync_to_closure(&peers);

        purge_entity(&peers[0], &mut clock, SHAPE, TYPE, &first);
        if purge_seen {
            sync_to_closure(&peers);
        }
        violate_on(&peers[1], 2);
        let second = repair(&peers[1], "seen_by_2");
        sync_to_closure(&peers);

        assert_ne!(second, first, "purge_seen={purge_seen}: a new occurrence");
        for peer in &peers {
            assert_eq!(
                scan_ids(&peer.doc, SHAPE, TYPE),
                BTreeSet::from([second.clone()]),
                "purge_seen={purge_seen}: the purged occurrence stays gone, the recurrence stays"
            );
            assert_eq!(
                read_entity(&peer.doc, SHAPE, TYPE, &second),
                Some(BTreeMap::from([
                    ("seen_by_2".to_string(), LoroValue::from(true)),
                    ("subject".to_string(), LoroValue::from("block-7")),
                ])),
                "purge_seen={purge_seen}: the recurrence holds its own fields only"
            );
        }
    }
}
