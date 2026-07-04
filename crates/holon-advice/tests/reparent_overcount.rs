//! @pbt kind harness
//! @pbt covers matview-reparent-overcount
//!
//! Reproducer probes for the hypothesised Turso IVM **re-parent over-count**:
//! the `block` matview (a PURE LEFT JOIN of `block_raw` against per-junction
//! agg matviews, no GROUP BY on `block` itself) was reported to emit a
//! duplicate row for a block whose `parent_id` is UPDATEd, while `block_raw`
//! holds one row.
//!
//! RESULT (2026-07-23): **NEGATIVE at the holon-turso raw-SQL level.** All 10
//! variants below — plain re-parent, tagged, prod-faithful three-agg matview,
//! combined base-UPDATE+junction-INSERT deltas, subtree move, the holon
//! per-edge-write txn pattern, unmatched/null-padded left rows, and the
//! file-backed close+REOPEN path — stay GREEN. `variant_j` is a FAITHFULNESS
//! CONTROL replaying the exact chained-agg precedent reseed sequence
//! (`turso/tests/.../test_ivm_chained_agg_reopen_reseed_dup.rs`) whose
//! duplicate this same operator family once produced; it is green too, proving
//! (a) the reopen harness faithfully persists+reopens DBSP state and (b) the
//! pinned turso rev already carries the fixes. The compiled turso checkout
//! (Cargo rev fa2c9d…) contains `join_operator.rs` delta consolidation, the
//! antijoin TryAdvance fix, and merge_operator deterministic reopen-stable
//! rowids. Therefore the reported over-count does NOT reproduce against raw SQL
//! on this build; the trigger must live in the CDC/consolidator/sync path (see
//! the handoff doc at the worktree root). Kept as a regression guard: any of
//! these goes red if a future turso bump regresses the IVM duplicate fixes.

use std::collections::HashMap;

use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

/// Faithful minimal boot of the production `block` matview chain: a `block_raw`
/// base carrying `parent_id` (the join-key that changes on a re-parent), one
/// per-junction agg matview (`block_tags_agg`), and the `block` LEFT-JOIN
/// matview built exactly like `block_matview_select_with_computed`.
async fn boot() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl(
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT, updated_at INTEGER NOT \
             NULL DEFAULT 0)",
        )
        .await
        .expect("create block_raw");
    handle
        .execute_ddl(
            "CREATE TABLE block_tags (block_id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY \
             (block_id, tag))",
        )
        .await
        .expect("create block_tags");
    handle
}

async fn insert_block(handle: &DbHandle, id: &str, parent: &str, updated_at: i64) {
    handle
        .execute(
            "INSERT INTO block_raw (id, parent_id, updated_at) VALUES (?, ?, ?)",
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Text(parent.into()),
                turso::Value::Integer(updated_at),
            ],
        )
        .await
        .expect("insert block");
}

async fn tag(handle: &DbHandle, block_id: &str, tag: &str) {
    handle
        .execute(
            "INSERT INTO block_tags (block_id, tag) VALUES (?, ?)",
            vec![
                turso::Value::Text(block_id.into()),
                turso::Value::Text(tag.into()),
            ],
        )
        .await
        .expect("insert tag");
}

/// Build the per-junction agg matview + the `block` LEFT-JOIN matview exactly
/// like prod (`block_matview_select_with_computed`).
async fn build_block_matview(handle: &DbHandle) {
    holon_turso::matview_manager::reconcile_named_view(
        handle,
        "block_tags_agg",
        "SELECT block_id AS source_id, json_group_array(tag) AS vals FROM block_tags GROUP BY \
         block_id",
    )
    .await
    .expect("build block_tags_agg");
    holon_turso::matview_manager::reconcile_named_view(
        handle,
        "block",
        "SELECT b.id, b.parent_id, b.updated_at, COALESCE(block_tags_agg.vals, '[]') AS tags FROM \
         block_raw b LEFT OUTER JOIN block_tags_agg ON block_tags_agg.source_id = b.id WHERE b.id \
         != 'sentinel:no_parent'",
    )
    .await
    .expect("build block matview");
}

/// Row-count per id in the `block` matview.
async fn matview_count(handle: &DbHandle, id: &str) -> usize {
    handle
        .query(
            &format!("SELECT id FROM block WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("count block for {id}: {e:#}"))
        .len()
}

async fn raw_count(handle: &DbHandle, id: &str) -> usize {
    handle
        .query(
            &format!("SELECT id FROM block_raw WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("count block_raw for {id}: {e:#}"))
        .len()
}

/// Dump every over-counted id with its base parent_id and the matview rows.
async fn assert_no_overcount(handle: &DbHandle, ids: &[&str], label: &str) {
    for id in ids {
        let mv = matview_count(handle, id).await;
        let raw = raw_count(handle, id).await;
        if mv != raw {
            let mv_rows = handle
                .query(
                    &format!("SELECT id, parent_id, updated_at, tags FROM block WHERE id = '{id}'"),
                    HashMap::new(),
                )
                .await
                .unwrap();
            let raw_parent = handle
                .query(
                    &format!("SELECT parent_id FROM block_raw WHERE id = '{id}'"),
                    HashMap::new(),
                )
                .await
                .unwrap();
            panic!(
                "[{label}] OVER-COUNT for id={id}: block matview has {mv} rows, block_raw has \
                 {raw}. block_raw.parent_id={raw_parent:?}. block matview rows: {mv_rows:?}"
            );
        }
    }
}

// ---- Variant (a): plain re-parent of Q (no tag, per-statement autocommit)
// ----
#[tokio::test]
async fn variant_a_plain_reparent_no_tag() {
    let handle = boot().await;
    insert_block(&handle, "P", "sentinel:no_parent", 100).await;
    insert_block(&handle, "P2", "sentinel:no_parent", 100).await;
    insert_block(&handle, "Q", "P", 100).await;
    insert_block(&handle, "C", "Q", 100).await;
    build_block_matview(&handle).await;
    handle.transition_to_ready().await.unwrap();
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "a:initial").await;

    handle
        .execute(
            "UPDATE block_raw SET parent_id = 'P2', updated_at = 200 WHERE id = 'Q'",
            vec![],
        )
        .await
        .expect("reparent Q");
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "a:reparent").await;
}

// ---- Variant (b): re-parent Q that carries a tag (LEFT-JOIN-over-agg) ----
#[tokio::test]
async fn variant_b_reparent_with_tag() {
    let handle = boot().await;
    insert_block(&handle, "P", "sentinel:no_parent", 100).await;
    insert_block(&handle, "P2", "sentinel:no_parent", 100).await;
    insert_block(&handle, "Q", "P", 100).await;
    insert_block(&handle, "C", "Q", 100).await;
    tag(&handle, "Q", "proj").await;
    build_block_matview(&handle).await;
    handle.transition_to_ready().await.unwrap();
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "b:initial").await;

    handle
        .execute(
            "UPDATE block_raw SET parent_id = 'P2', updated_at = 200 WHERE id = 'Q'",
            vec![],
        )
        .await
        .expect("reparent Q");
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "b:reparent").await;
}

// ---- Variant (c): re-parent interleaved with a tag write in same txn ----
#[tokio::test]
async fn variant_c_reparent_with_interleaved_tag_write() {
    let handle = boot().await;
    insert_block(&handle, "P", "sentinel:no_parent", 100).await;
    insert_block(&handle, "P2", "sentinel:no_parent", 100).await;
    insert_block(&handle, "Q", "P", 100).await;
    insert_block(&handle, "C", "Q", 100).await;
    tag(&handle, "Q", "proj").await;
    build_block_matview(&handle).await;
    handle.transition_to_ready().await.unwrap();
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "c:initial").await;

    handle
        .transaction(vec![
            (
                "UPDATE block_raw SET parent_id = 'P2', updated_at = 200 WHERE id = 'Q'".into(),
                vec![],
            ),
            ("DELETE FROM block_tags WHERE block_id = 'Q'".into(), vec![]),
            (
                "INSERT INTO block_tags (block_id, tag) VALUES ('Q', 'urgent')".into(),
                vec![],
            ),
        ])
        .await
        .expect("reparent + retag in txn");
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "c:reparent+retag").await;
}

// ---- Variant (d): subtree re-parent (Q and child C) in a single txn ----
#[tokio::test]
async fn variant_d_subtree_reparent_in_txn() {
    let handle = boot().await;
    insert_block(&handle, "P", "sentinel:no_parent", 100).await;
    insert_block(&handle, "P2", "sentinel:no_parent", 100).await;
    insert_block(&handle, "Q", "P", 100).await;
    insert_block(&handle, "C", "Q", 100).await;
    tag(&handle, "Q", "proj").await;
    build_block_matview(&handle).await;
    handle.transition_to_ready().await.unwrap();
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "d:initial").await;

    handle
        .transaction(vec![
            (
                "UPDATE block_raw SET parent_id = 'P2', updated_at = 200 WHERE id = 'Q'".into(),
                vec![],
            ),
            (
                "UPDATE block_raw SET parent_id = 'P2', updated_at = 200 WHERE id = 'C'".into(),
                vec![],
            ),
        ])
        .await
        .expect("subtree reparent in txn");
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "d:subtree").await;
}

// ==== Full three-agg prod-faithful block matview (tags + requires + advice)
// ====

async fn boot3() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl(
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT, updated_at INTEGER NOT \
             NULL DEFAULT 0)",
        )
        .await
        .expect("create block_raw");
    handle
        .execute_ddl(
            "CREATE TABLE block_tags (block_id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY \
             (block_id, tag))",
        )
        .await
        .expect("create block_tags");
    handle
        .execute_ddl(
            "CREATE TABLE block_requires (block_id TEXT NOT NULL, required_id TEXT NOT NULL, \
             PRIMARY KEY (block_id, required_id))",
        )
        .await
        .expect("create block_requires");
    handle
        .execute_ddl(
            "CREATE TABLE advice_suppressed (anchor_id TEXT NOT NULL, lesson_id TEXT NOT NULL, \
             PRIMARY KEY (anchor_id, lesson_id))",
        )
        .await
        .expect("create advice_suppressed");
    handle
}

/// Build all THREE per-junction agg matviews + the `block` matview with three
/// LEFT OUTER JOINs, exactly like `block_matview_select_with_computed` in prod.
async fn build_block_matview3(handle: &DbHandle) {
    for (name, jt, src, tgt) in [
        ("block_tags_agg", "block_tags", "block_id", "tag"),
        (
            "block_requires_agg",
            "block_requires",
            "block_id",
            "required_id",
        ),
        (
            "advice_suppressed_agg",
            "advice_suppressed",
            "anchor_id",
            "lesson_id",
        ),
    ] {
        holon_turso::matview_manager::reconcile_named_view(
            handle,
            name,
            &format!(
                "SELECT {src} AS source_id, json_group_array({tgt}) AS vals FROM {jt} GROUP BY \
                 {src}"
            ),
        )
        .await
        .unwrap_or_else(|e| panic!("build {name}: {e:#}"));
    }
    holon_turso::matview_manager::reconcile_named_view(
        handle,
        "block",
        "SELECT b.id, b.parent_id, b.updated_at, COALESCE(block_tags_agg.vals, '[]') AS tags, \
         COALESCE(block_requires_agg.vals, '[]') AS requires, \
         COALESCE(advice_suppressed_agg.vals, '[]') AS advice_suppressed FROM block_raw b LEFT \
         OUTER JOIN block_tags_agg ON block_tags_agg.source_id = b.id LEFT OUTER JOIN \
         block_requires_agg ON block_requires_agg.source_id = b.id LEFT OUTER JOIN \
         advice_suppressed_agg ON advice_suppressed_agg.source_id = b.id WHERE b.id != \
         'sentinel:no_parent'",
    )
    .await
    .expect("build 3-agg block matview");
}

/// Variant (e): three-agg matview, ONE txn combining the re-parent UPDATE
/// (touches an OUTPUT column, parent_id) with an INSERT into the FIRST junction
/// (block_tags) — the exact combined-delta shape of the join precedent, now on
/// the pure LEFT-JOIN-over-agg `block` matview. Then a plain INSERT into a
/// LATER junction (block_requires) to probe the (suspected) ghost left-state.
#[tokio::test]
async fn variant_e_reparent_plus_first_junction_then_later_junction() {
    let handle = boot3().await;
    insert_block(&handle, "P", "sentinel:no_parent", 100).await;
    insert_block(&handle, "P2", "sentinel:no_parent", 100).await;
    insert_block(&handle, "Q", "P", 100).await;
    insert_block(&handle, "C", "Q", 100).await;
    build_block_matview3(&handle).await;
    handle.transition_to_ready().await.unwrap();
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "e:initial").await;

    // txn: base UPDATE of parent_id (output col) + INSERT into first junction.
    handle
        .transaction(vec![
            (
                "UPDATE block_raw SET parent_id = 'P2', updated_at = 200 WHERE id = 'Q'".into(),
                vec![],
            ),
            (
                "INSERT INTO block_tags (block_id, tag) VALUES ('Q', 'proj')".into(),
                vec![],
            ),
        ])
        .await
        .expect("reparent + first-junction insert in txn");
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "e:txn").await;

    // Plain insert into a LATER junction probes the suspected ghost left-state.
    handle
        .execute(
            "INSERT INTO block_requires (block_id, required_id) VALUES ('Q', 'C')",
            vec![],
        )
        .await
        .expect("later-junction insert");
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "e:later-junction").await;
}

/// Variant (f): holon's real per-edge-write txn pattern applied to a re-parent:
/// each edge write is ONE txn of {parent_id UPDATE + junction DELETE-all +
/// junction re-INSERT}. tags first, then requires — mirrors the join
/// precedent's holon_txn_pattern but with parent_id as the changing column.
#[tokio::test]
async fn variant_f_reparent_holon_per_edge_txn_pattern() {
    let handle = boot3().await;
    insert_block(&handle, "P", "sentinel:no_parent", 100).await;
    insert_block(&handle, "P2", "sentinel:no_parent", 100).await;
    insert_block(&handle, "Q", "P", 100).await;
    insert_block(&handle, "C", "Q", 100).await;
    tag(&handle, "Q", "proj").await;
    build_block_matview3(&handle).await;
    handle.transition_to_ready().await.unwrap();
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "f:initial").await;

    handle
        .transaction(vec![
            (
                "UPDATE block_raw SET parent_id = 'P2', updated_at = 200 WHERE id = 'Q'".into(),
                vec![],
            ),
            ("DELETE FROM block_tags WHERE block_id = 'Q'".into(), vec![]),
            (
                "INSERT INTO block_tags (block_id, tag) VALUES ('Q', 'proj')".into(),
                vec![],
            ),
        ])
        .await
        .expect("edge write 1 (tags) in txn");
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "f:edge1").await;

    handle
        .transaction(vec![
            (
                "UPDATE block_raw SET parent_id = 'P2', updated_at = 300 WHERE id = 'Q'".into(),
                vec![],
            ),
            (
                "DELETE FROM block_requires WHERE block_id = 'Q'".into(),
                vec![],
            ),
            (
                "INSERT INTO block_requires (block_id, required_id) VALUES ('Q', 'C')".into(),
                vec![],
            ),
        ])
        .await
        .expect("edge write 2 (requires) in txn");
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "f:edge2").await;
}

/// Variant (g): re-parent an UNMATCHED (null-padded) left row — Q has NO
/// junction rows in any of the three aggs, so all COALESCE to '[]'. The chained
/// precedent's minimal isolation showed retraction of an unmatched LEFT JOIN
/// left-row is the fragile path. Re-parent, then add a tag to force the agg to
/// probe Q's (suspected stale) left state.
#[tokio::test]
async fn variant_g_reparent_unmatched_then_tag() {
    let handle = boot3().await;
    insert_block(&handle, "P", "sentinel:no_parent", 100).await;
    insert_block(&handle, "P2", "sentinel:no_parent", 100).await;
    insert_block(&handle, "Q", "P", 100).await;
    insert_block(&handle, "C", "Q", 100).await;
    build_block_matview3(&handle).await;
    handle.transition_to_ready().await.unwrap();
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "g:initial").await;

    handle
        .execute(
            "UPDATE block_raw SET parent_id = 'P2', updated_at = 200 WHERE id = 'Q'",
            vec![],
        )
        .await
        .expect("reparent unmatched Q");
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "g:reparent").await;

    handle
        .execute(
            "INSERT INTO block_tags (block_id, tag) VALUES ('Q', 'proj')",
            vec![],
        )
        .await
        .expect("tag after reparent");
    assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "g:tag-after").await;
}

// ==== Reopen-path variants: the discriminating trigger from the chained ====
// ==== precedent (persisted DBSP state), now with a RE-PARENT after reopen.
// ====

use holon_turso::turso::TursoBackend as Backend;

fn fresh(path: &str) {
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{path}-wal"));
    let _ = std::fs::remove_file(format!("{path}-shm"));
}

/// Open a file-backed holon-turso handle (reopenable, unlike `new_in_memory`).
/// Returns the backend (owns the actor) + handle. Drop the backend to "close".
async fn open_file(path: &str) -> (Backend, DbHandle) {
    let db = Backend::open_database(path).expect("open db");
    let (cdc_tx, _rx) = tokio::sync::broadcast::channel(1024);
    Backend::new(db, cdc_tx).expect("backend")
}

async fn ddl_block_raw(handle: &DbHandle) {
    handle
        .execute_ddl(
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT, updated_at INTEGER NOT \
             NULL DEFAULT 0)",
        )
        .await
        .expect("create block_raw");
    handle
        .execute_ddl(
            "CREATE TABLE block_tags (block_id TEXT NOT NULL, tag TEXT NOT NULL, PRIMARY KEY \
             (block_id, tag))",
        )
        .await
        .expect("create block_tags");
}

/// Variant (h): file-backed. Session 1 seeds P/P2/Q(+tag)/C and the block
/// matview chain, verifies single rows, then CLOSES (drops backend → persists
/// DBSP btrees). Session 2 REOPENS and drives the RE-PARENT
/// `UPDATE block_raw SET parent_id='P2' WHERE id='Q'`. The chained precedent
/// shows retraction across a reopen is the fragile path; here the retracted
/// row is the OLD (parent=P) joined output.
#[tokio::test]
async fn variant_h_reopen_then_reparent() {
    let path = "/tmp/holon-reparent-overcount-h.db";
    fresh(path);
    {
        let (backend, handle) = open_file(path).await;
        ddl_block_raw(&handle).await;
        insert_block(&handle, "P", "sentinel:no_parent", 100).await;
        insert_block(&handle, "P2", "sentinel:no_parent", 100).await;
        insert_block(&handle, "Q", "P", 100).await;
        insert_block(&handle, "C", "Q", 100).await;
        tag(&handle, "Q", "proj").await;
        build_block_matview(&handle).await;
        handle.transition_to_ready().await.unwrap();
        assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "h:session1").await;
        drop(handle);
        drop(backend);
    }
    {
        let (_backend, handle) = open_file(path).await;
        assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "h:reopen-pre").await;
        handle
            .execute(
                "UPDATE block_raw SET parent_id = 'P2', updated_at = 200 WHERE id = 'Q'",
                vec![],
            )
            .await
            .expect("reparent after reopen");
        assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "h:reopen-reparent").await;
    }
    fresh(path);
}

/// Variant (i): file-backed reopen, then the holon per-edge txn re-parent
/// pattern (parent_id UPDATE + tag DELETE/re-INSERT in one txn) after reopen —
/// combines the reopen fragility with the combined-delta shape.
#[tokio::test]
async fn variant_i_reopen_then_reparent_edge_txn() {
    let path = "/tmp/holon-reparent-overcount-i.db";
    fresh(path);
    {
        let (backend, handle) = open_file(path).await;
        ddl_block_raw(&handle).await;
        insert_block(&handle, "P", "sentinel:no_parent", 100).await;
        insert_block(&handle, "P2", "sentinel:no_parent", 100).await;
        insert_block(&handle, "Q", "P", 100).await;
        insert_block(&handle, "C", "Q", 100).await;
        tag(&handle, "Q", "proj").await;
        build_block_matview(&handle).await;
        handle.transition_to_ready().await.unwrap();
        assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "i:session1").await;
        drop(handle);
        drop(backend);
    }
    {
        let (_backend, handle) = open_file(path).await;
        assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "i:reopen-pre").await;
        handle
            .transaction(vec![
                (
                    "UPDATE block_raw SET parent_id = 'P2', updated_at = 200 WHERE id = 'Q'".into(),
                    vec![],
                ),
                ("DELETE FROM block_tags WHERE block_id = 'Q'".into(), vec![]),
                (
                    "INSERT INTO block_tags (block_id, tag) VALUES ('Q', 'proj')".into(),
                    vec![],
                ),
            ])
            .await
            .expect("reparent edge txn after reopen");
        assert_no_overcount(&handle, &["P", "P2", "Q", "C"], "i:reopen-edge-txn").await;
    }
    fresh(path);
}

/// Variant (j) — FAITHFULNESS CONTROL. Replays the EXACT sequence of the turso
/// chained-agg precedent (`test_ivm_chained_agg_reopen_reseed_dup.rs`): session
/// 1 seed j(+Page tag), close; session 2 reopen + idempotent reseed in one txn
/// (sort_key UPDATE + tag DELETE + same-tag re-INSERT). That precedent's dup
/// was FIXED; this control confirms (a) our file-backed reopen harness persists
/// & reopens DBSP state faithfully, and (b) the fix is present in THIS turso
/// build. If this is GREEN, a green re-parent result is a true negative, not a
/// harness artifact.
#[tokio::test]
async fn variant_j_faithfulness_control_precedent_reseed() {
    let path = "/tmp/holon-reparent-overcount-j.db";
    fresh(path);
    {
        let (backend, handle) = open_file(path).await;
        handle
            .execute_ddl(
                "CREATE TABLE block_raw (id TEXT PRIMARY KEY, sort_key TEXT, content TEXT)",
            )
            .await
            .expect("create block_raw");
        handle
            .execute_ddl(
                "CREATE TABLE block_tags (block_id TEXT, tag TEXT, PRIMARY KEY (block_id, tag))",
            )
            .await
            .expect("create block_tags");
        holon_turso::matview_manager::reconcile_named_view(
            &handle,
            "block_tags_agg",
            "SELECT block_id AS source_id, json_group_array(tag) AS vals FROM block_tags GROUP BY \
             block_id",
        )
        .await
        .expect("agg");
        holon_turso::matview_manager::reconcile_named_view(
            &handle,
            "block",
            "SELECT b.id, b.sort_key, b.content, COALESCE(block_tags_agg.vals, '[]') AS tags FROM \
             block_raw b LEFT OUTER JOIN block_tags_agg ON block_tags_agg.source_id = b.id",
        )
        .await
        .expect("block");
        handle
            .execute(
                "INSERT INTO block_raw (id, sort_key, content) VALUES ('j', '80', 'Journals')",
                vec![],
            )
            .await
            .unwrap();
        handle
            .execute(
                "INSERT INTO block_tags (block_id, tag) VALUES ('j', 'Page')",
                vec![],
            )
            .await
            .unwrap();
        handle.transition_to_ready().await.unwrap();
        assert_eq!(matview_count(&handle, "j").await, 1, "j:session1 baseline");
        drop(handle);
        drop(backend);
    }
    {
        let (_backend, handle) = open_file(path).await;
        assert_eq!(matview_count(&handle, "j").await, 1, "j:reopen pre-reseed");
        handle
            .transaction(vec![
                (
                    "UPDATE block_raw SET sort_key = '81' WHERE id = 'j'".into(),
                    vec![],
                ),
                ("DELETE FROM block_tags WHERE block_id = 'j'".into(), vec![]),
                (
                    "INSERT INTO block_tags (block_id, tag) VALUES ('j', 'Page')".into(),
                    vec![],
                ),
            ])
            .await
            .expect("idempotent reseed txn");
        assert_eq!(
            matview_count(&handle, "j").await,
            1,
            "j:post-reseed — precedent dup must stay FIXED (faithfulness control)"
        );
    }
    fresh(path);
}
