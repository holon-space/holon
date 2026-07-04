//! Two-replica at-most-once convergence property (ADR 0024 P4).
//!
//! Prod hypothesis: when two replicas independently fire the SAME create rule
//! for the SAME firing key, WP2's deterministic effect id makes both mint the
//! SAME block id, so the CRDT merge collapses them by naming — exactly one
//! journal block per day. This is the P4 property no execution log could give.

use std::collections::HashSet;
use std::sync::Arc;

use holon_api::Value;
use holon_api::effect_id::FiringKey;
use holon_api::effect_id::OutputSlot;
use holon_api::effect_id::RuleId;
use holon_api::effect_id::deterministic_block_id;
use holon_api::entity::StorageEntity;
use loro::ExportMode;
use loro::LoroDoc;
use proptest::prelude::*;

use crate::peer_ops::peer_alive_blocks;
use crate::peer_ops::peer_create_block;

const PARENT_SID: &str = "journals-root";
const RULE_ID: &str = "journals::action::0";

fn day_row(day: &str) -> StorageEntity {
    let mut row = StorageEntity::new();
    row.insert(Arc::from("name"), Value::String(day.to_string()));
    row
}

/// A fresh replica seeded with the shared journals-parent history.
fn replica_with_parent(peer_id: u64, base_snapshot: &[u8]) -> LoroDoc {
    let doc = LoroDoc::new();
    doc.set_peer_id(peer_id).unwrap();
    doc.import(base_snapshot).unwrap();
    doc
}

/// The DISTINCT journal-child ids (the naming-convergence measure, matching
/// `multi_peer::get_alive_stable_ids`'s HashSet idiom): two CRDT containers
/// that share a stable id are one logical block, collapsed by the SQL
/// `block_raw` PK on projection.
fn distinct_journal_children(doc: &LoroDoc) -> HashSet<String> {
    peer_alive_blocks(doc)
        .into_iter()
        .filter(|b| b.parent_stable_id.as_deref() == Some(PARENT_SID))
        .map(|b| b.stable_id)
        .collect()
}

/// A snapshot of a doc holding only the shared journals parent — the common
/// history both replicas start from.
fn shared_base() -> Vec<u8> {
    let seed = LoroDoc::new();
    seed.set_peer_id(1).unwrap();
    peer_create_block(&seed, None, "Journals", PARENT_SID);
    seed.export(ExportMode::Snapshot).unwrap()
}

/// Merge `from` into `into` (one-directional delta import), mirroring
/// `LoroSut::apply_merge_from_peer`.
fn merge_into(into: &LoroDoc, from: &LoroDoc) {
    let vv = into.oplog_vv();
    let delta = from.export(ExportMode::updates(&vv)).unwrap();
    into.import(&delta).unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig {
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Deterministic id ⇒ concurrent same-key firing converges to ONE journal.
    #[test]
    fn two_replicas_same_day_converge_to_one_journal(dom in 1u32..=28) {
        let day = format!("2026-07-{dom:02}");
        let rule = RuleId::new(RULE_ID);
        let key = FiringKey::from_row(&day_row(&day));
        let det_id = deterministic_block_id(&rule, &key, &OutputSlot::first())
            .id()
            .to_string();

        let base = shared_base();
        let replica_a = replica_with_parent(10, &base);
        let replica_b = replica_with_parent(20, &base);

        // Both replicas fire the journal rule for the same day independently —
        // same rule id + same firing key ⇒ same deterministic block id.
        peer_create_block(&replica_a, Some(PARENT_SID), &day, &det_id);
        peer_create_block(&replica_b, Some(PARENT_SID), &day, &det_id);

        merge_into(&replica_a, &replica_b);

        let children = distinct_journal_children(&replica_a);
        prop_assert_eq!(
            children.len(),
            1,
            "same-key concurrent firing must converge to exactly one journal"
        );
        prop_assert!(children.contains(&det_id));
    }
}

/// Discriminating control: had the two replicas minted DIFFERENT ids (the
/// pre-WP2 random-v4 behaviour), the merge keeps BOTH — proving the property
/// above is not vacuous and the harness detects duplication.
#[test]
fn distinct_ids_do_not_converge_control() {
    let base = shared_base();
    let replica_a = replica_with_parent(10, &base);
    let replica_b = replica_with_parent(20, &base);

    peer_create_block(&replica_a, Some(PARENT_SID), "2026-07-10", "id-from-a");
    peer_create_block(&replica_b, Some(PARENT_SID), "2026-07-10", "id-from-b");

    merge_into(&replica_a, &replica_b);

    let children = distinct_journal_children(&replica_a);
    assert_eq!(
        children.len(),
        2,
        "distinct ids must NOT converge (control)"
    );
}
