//! Does the edge-aggregate matview lose a DELETE's retraction when its DBSP
//! state was persisted by a PRIOR boot?
//!
//! Martin's live vault came back from a write-back with every `:REQUIRES:`
//! target written twice. The state archaeology (bugfunnel entry
//! `2026-08-30-matview-edge-agg-doubles-every-requires-target`) put the
//! doubling in exactly one place: the persisted DBSP state of the `block`
//! matview, fed by `block_requires_agg`, at weight 1 — while the base
//! `block_requires` junction stayed clean and single (its PRIMARY KEY makes a
//! duplicate unrepresentable).
//!
//! Junction writes are a coarse wipe-and-rebuild: `edge_field_replace_sql`
//! (crates/holon/src/core/sql_operation_provider.rs:1032-1053) emits one
//! `DELETE FROM block_requires WHERE block_id = …` then one plain `INSERT` per
//! target. Over an unchanged target set that pair must net to zero in the
//! aggregate. A uniform multiplicity of exactly 2 is what an `INSERT` reaching
//! the persisted state while the `DELETE`'s retraction does not would produce.
//!
//! These tests state that as a contract over the REAL production SELECT
//! (`edge_agg_view_select`, pinned by
//! `edge_agg_view_select_groups_targets_by_source`): a wipe-and-rebuild of an
//! unchanged target set leaves the aggregate holding each target ONCE — same
//! boot, and across a reboot that leaves the view and its DBSP state on disk.

use std::collections::HashMap;

use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

/// The production aggregate SELECT for `requires`, verbatim as
/// `edge_agg_view_select` renders the shipped descriptor (`source_col =
/// block_id`, `target_col = required_id`) — the same string Martin's live
/// database carries in `sqlite_master`.
const AGG_SELECT: &str = "SELECT block_id AS source_id, json_group_array(required_id) AS vals FROM \
                          block_requires GROUP BY block_id";

/// The production wipe-and-rebuild, verbatim in shape from
/// `edge_field_replace_sql`: DELETE every row for the source, then one plain
/// INSERT per target.
fn replace_sql(block_id: &str, targets: &[&str]) -> Vec<(String, Vec<turso::Value>)> {
    let mut out = vec![(
        format!("DELETE FROM block_requires WHERE \"block_id\" = '{block_id}'"),
        Vec::new(),
    )];
    for t in targets {
        out.push((
            format!(
                "INSERT INTO block_requires (\"block_id\", \"required_id\") VALUES ('{block_id}', \
                 '{t}')"
            ),
            Vec::new(),
        ));
    }
    out
}

async fn create_schema(handle: &holon_turso::turso::DbHandle) {
    handle
        .execute_ddl("CREATE TABLE block_raw (id TEXT PRIMARY KEY, content TEXT)")
        .await
        .expect("create block_raw");
    handle
        .execute_ddl(
            "CREATE TABLE block_requires (block_id TEXT NOT NULL, required_id TEXT NOT NULL, \
             PRIMARY KEY (block_id, required_id))",
        )
        .await
        .expect("create block_requires");
    handle
        .execute_ddl(&format!(
            "CREATE MATERIALIZED VIEW block_requires_agg AS {AGG_SELECT}"
        ))
        .await
        .expect("create block_requires_agg");
}

/// The aggregate's `vals` for one source, as the matview serves it.
async fn agg_vals(handle: &holon_turso::turso::DbHandle, block_id: &str) -> String {
    let rows = handle
        .query(
            &format!("SELECT vals FROM block_requires_agg WHERE source_id = '{block_id}'"),
            HashMap::new(),
        )
        .await
        .expect("read block_requires_agg");
    rows.iter()
        .filter_map(|r| r.get("vals"))
        .map(|v| match v {
            holon_api::Value::String(s) => s.clone(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join("|")
}

/// The same read, recomputed from the base junction — what
/// `inv-matview-consistent-with-recompute` compares the matview against.
async fn recompute_vals(handle: &holon_turso::turso::DbHandle, block_id: &str) -> String {
    let rows = handle
        .query(
            &format!("SELECT vals FROM ({AGG_SELECT}) WHERE source_id = '{block_id}'"),
            HashMap::new(),
        )
        .await
        .expect("recompute the aggregate SELECT");
    rows.iter()
        .filter_map(|r| r.get("vals"))
        .map(|v| match v {
            holon_api::Value::String(s) => s.clone(),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join("|")
}

async fn seed(handle: &holon_turso::turso::DbHandle) {
    for id in ["block:src", "block:dep-a", "block:dep-b"] {
        handle
            .execute(
                &format!("INSERT INTO block_raw (id, content) VALUES ('{id}', 'x')"),
                Vec::new(),
            )
            .await
            .expect("seed block_raw");
    }
    handle
        .transaction(replace_sql("block:src", &["block:dep-a", "block:dep-b"]))
        .await
        .expect("seed the junction through the production wipe-and-rebuild");
}

/// Control: within ONE boot, replaying the wipe-and-rebuild over an unchanged
/// target set must leave the aggregate holding each target once. If this fails,
/// the reboot is not the variable and the defect is plain IVM maintenance.
#[tokio::test(flavor = "multi_thread")]
async fn wipe_and_rebuild_within_one_boot_keeps_each_target_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("oneboot.db");

    let db = TursoBackend::open_database(&path).expect("open");
    let (backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    create_schema(&handle).await;
    seed(&handle).await;

    assert_eq!(
        agg_vals(&handle, "block:src").await,
        r#"["block:dep-a","block:dep-b"]"#,
        "baseline: the aggregate holds each target once"
    );

    // The unchanged-set replay: exactly what a re-ingest of an unedited org
    // file emits for this block.
    handle
        .transaction(replace_sql("block:src", &["block:dep-a", "block:dep-b"]))
        .await
        .expect("replay the wipe-and-rebuild");

    let matview = agg_vals(&handle, "block:src").await;
    let recompute = recompute_vals(&handle, "block:src").await;
    assert_eq!(
        matview, recompute,
        "the aggregate matview disagrees with its own defining SELECT after an unchanged-set \
         wipe-and-rebuild (same boot)"
    );
    assert_eq!(
        matview, r#"["block:dep-a","block:dep-b"]"#,
        "a DELETE+INSERT of an unchanged target set must net to zero"
    );

    handle.shutdown().await.expect("shutdown");
    drop(backend);
}

/// The production shape: the view AND its DBSP state were written by a prior
/// boot, and this boot replays the wipe-and-rebuild over an unchanged target
/// set — the org re-ingest every startup performs. If the `DELETE`'s retraction
/// does not reach the persisted aggregate state, each target comes back twice,
/// which is the vault-wide doubling.
#[tokio::test(flavor = "multi_thread")]
async fn wipe_and_rebuild_after_reboot_keeps_each_target_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("reboot.db");

    // -- Boot 1: create the view and populate it, then shut down cleanly so the
    // view definition and its DBSP state persist to the file.
    {
        let db = TursoBackend::open_database(&path).expect("open for seeding");
        let (backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
        create_schema(&handle).await;
        seed(&handle).await;
        assert_eq!(
            agg_vals(&handle, "block:src").await,
            r#"["block:dep-a","block:dep-b"]"#,
            "boot 1 baseline"
        );
        handle.shutdown().await.expect("shutdown boot 1");
        drop(backend);
    }

    // -- Boot 2: the view is already on disk; nothing recreates it.
    let db = TursoBackend::open_database(&path).expect("reopen");
    let (backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");

    assert_eq!(
        agg_vals(&handle, "block:src").await,
        r#"["block:dep-a","block:dep-b"]"#,
        "the aggregate must survive the reboot intact before anything is written"
    );

    handle
        .transaction(replace_sql("block:src", &["block:dep-a", "block:dep-b"]))
        .await
        .expect("replay the wipe-and-rebuild on the second boot");

    let matview = agg_vals(&handle, "block:src").await;
    let recompute = recompute_vals(&handle, "block:src").await;
    assert_eq!(
        matview, recompute,
        "the aggregate matview disagrees with its own defining SELECT after a post-reboot \
         unchanged-set wipe-and-rebuild — this is the vault-doubling shape"
    );
    assert_eq!(
        matview, r#"["block:dep-a","block:dep-b"]"#,
        "each target must appear ONCE; a doubled value here is the production defect"
    );

    handle.shutdown().await.expect("shutdown");
    drop(backend);
}

/// The boot Holon actually performs: `ensure_schema` re-issues every
/// `CREATE TABLE IF NOT EXISTS` / `CREATE MATERIALIZED VIEW IF NOT EXISTS` on
/// EVERY start, over a database whose view and DBSP state are already on disk.
/// If that re-issue re-populates the aggregate from the base rows without
/// clearing the state it inherited, every target gains a second copy — exactly
/// once, uniformly, for every block in the vault, which is the observed shape.
#[tokio::test(flavor = "multi_thread")]
async fn reissuing_the_view_ddl_on_a_later_boot_does_not_double_the_aggregate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("reissue.db");

    {
        let db = TursoBackend::open_database(&path).expect("open for seeding");
        let (backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
        create_schema(&handle).await;
        seed(&handle).await;
        handle.shutdown().await.expect("shutdown boot 1");
        drop(backend);
    }

    let db = TursoBackend::open_database(&path).expect("reopen");
    let (backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");

    // `ensure_schema`'s idempotent re-issue, verbatim in shape.
    handle
        .execute_ddl(&format!(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS block_requires_agg AS {AGG_SELECT}"
        ))
        .await
        .expect("re-issue the view DDL on the later boot");

    // Then the org re-ingest replays the unchanged target set.
    handle
        .transaction(replace_sql("block:src", &["block:dep-a", "block:dep-b"]))
        .await
        .expect("replay the wipe-and-rebuild after the DDL re-issue");

    let matview = agg_vals(&handle, "block:src").await;
    let recompute = recompute_vals(&handle, "block:src").await;
    assert_eq!(
        matview, recompute,
        "the aggregate disagrees with its own defining SELECT after a boot that re-issued the \
         view DDL over persisted state"
    );
    assert_eq!(
        matview, r#"["block:dep-a","block:dep-b"]"#,
        "each target must appear ONCE"
    );

    handle.shutdown().await.expect("shutdown");
    drop(backend);
}
