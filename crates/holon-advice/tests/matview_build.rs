//! Build the synthesized advice matview against the real advice schema
//! (`block_raw` + `block_tags` + `advice_suppressed`) in an in-memory Turso DB
//! and assert it (a) CREATEs without hanging and (b) the anchor-denormalized
//! tag-overlap rows are correct. Recency + per-anchor top-K are applied at READ
//! time (Rust-side). Suppression is an anti-join: IVM CANNOT maintain it fused
//! with the scoring `GROUP BY` aggregate (`probe_ivm_shape_findings` FINDING 2),
//! but IT CAN in a plain non-aggregating OUTER view over the scored matview
//! (`probe_outer_antijoin_is_incrementally_maintained`) — which is why the live
//! weaver watches that outer read (see `holon_frontend::advice_weaver`). Mirrors
//! the harness in `holon-turso/tests/sidecar_views.rs`.

use std::collections::HashMap;

use holon_advice::{BUNDLED_LESSONS_FOR_TASKS_YAML, parse_advice_rule, reconcile_advice_rule};
use holon_turso::turso::{DbHandle, TursoBackend};

async fn setup() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl(
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY, block_type TEXT NOT NULL DEFAULT 'text', \
             properties TEXT, updated_at INTEGER NOT NULL DEFAULT 0)",
        )
        .await
        .expect("create block_raw");
    handle
        .execute_ddl(
            "CREATE TABLE block_tags (block_id TEXT NOT NULL, tag TEXT NOT NULL, \
             PRIMARY KEY (block_id, tag))",
        )
        .await
        .expect("create block_tags");
    handle
        .execute_ddl(
            "CREATE TABLE advice_suppressed (anchor_id TEXT NOT NULL, lesson_id TEXT NOT NULL, \
             PRIMARY KEY (anchor_id, lesson_id))",
        )
        .await
        .expect("create advice_suppressed");
    handle
        .execute_ddl(
            "CREATE TABLE block_requires (block_id TEXT NOT NULL, required_id TEXT NOT NULL, \
             PRIMARY KEY (block_id, required_id))",
        )
        .await
        .expect("create block_requires");
    handle
}

async fn block(handle: &DbHandle, id: &str, updated_at: i64) {
    handle
        .execute(
            "INSERT INTO block_raw (id, updated_at) VALUES (?, ?)",
            vec![
                turso::Value::Text(id.into()),
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

async fn suppress(handle: &DbHandle, anchor: &str, lesson: &str) {
    handle
        .execute(
            "INSERT INTO advice_suppressed (anchor_id, lesson_id) VALUES (?, ?)",
            vec![
                turso::Value::Text(anchor.into()),
                turso::Value::Text(lesson.into()),
            ],
        )
        .await
        .expect("insert suppression");
}

/// The read-time weave: filter the matview to one anchor, drop suppressed lessons
/// via the anti-join (IVM can't do it inside the matview), order by shared-tag
/// count then recency, cap at K. This is what the renderer runs (ADR 0022).
///
/// The trailing `, v.lesson_id` is a UI-STABILITY tiebreak only: it makes the
/// display order deterministic when shared-tag count AND recency tie. It does NOT
/// strengthen the PBT oracle — `updated_at` remains unobservable to the reference,
/// so the reference still cannot pin recency order; lesson_id merely stops the
/// renderer flickering between equal-ranked rows.
fn read_query(anchor: &str, k: u8) -> String {
    format!(
        "SELECT v.lesson_id \
         FROM advice_rule_lessons_for_tasks v \
         LEFT JOIN advice_suppressed s ON s.anchor_id = v.anchor_id AND s.lesson_id = v.lesson_id \
         JOIN block_raw c ON c.id = v.lesson_id \
         WHERE v.anchor_id = '{anchor}' AND s.lesson_id IS NULL \
         ORDER BY v.shared_tag_count DESC, c.updated_at DESC, v.lesson_id \
         LIMIT {k}"
    )
}

async fn lessons_under(handle: &DbHandle, anchor: &str, k: u8) -> Vec<String> {
    handle
        .query(&read_query(anchor, k), HashMap::new())
        .await
        .expect("read the advice weave")
        .iter()
        .map(|r| r.get("lesson_id").unwrap().as_string().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn bundled_rule_matview_builds_and_scores_correctly() {
    let handle = setup().await;

    // task1 shares {proj, urgent} with lessonA (2), {proj} with lessonB (1),
    // nothing with lessonC.
    block(&handle, "task1", 100).await;
    block(&handle, "lessonA", 300).await;
    block(&handle, "lessonB", 200).await;
    block(&handle, "lessonC", 400).await;
    for (b, t) in [
        ("task1", "task"),
        ("task1", "proj"),
        ("task1", "urgent"),
        ("lessonA", "lesson"),
        ("lessonA", "proj"),
        ("lessonA", "urgent"),
        ("lessonB", "lesson"),
        ("lessonB", "proj"),
        ("lessonC", "lesson"),
        ("lessonC", "other"),
    ] {
        tag(&handle, b, t).await;
    }
    handle
        .transition_to_ready()
        .await
        .expect("transition to ready");

    let rule = parse_advice_rule(BUNDLED_LESSONS_FOR_TASKS_YAML).expect("bundled rule parses");
    let outcome = reconcile_advice_rule(&handle, &rule)
        .await
        .expect("advice matview must build without hang");
    assert_eq!(
        outcome,
        holon_advice::ReconcileOutcome::Created,
        "first reconcile creates the matview"
    );

    // Before suppression: lessonA (2 shared) ranks above lessonB (1); lessonC
    // shares no tag so it never enters the matview.
    assert_eq!(
        lessons_under(&handle, "task1", 5).await,
        vec!["lessonA".to_string(), "lessonB".to_string()],
        "tag-overlap ordering: A(2) before B(1), C absent"
    );

    // Dismissing lessonA under task1 removes it via the read-time anti-join.
    suppress(&handle, "task1", "lessonA").await;
    assert_eq!(
        lessons_under(&handle, "task1", 5).await,
        vec!["lessonB".to_string()],
        "suppression removes lessonA at read time"
    );

    // K caps the woven rows.
    assert_eq!(
        lessons_under(&handle, "task1", 1).await.len(),
        1,
        "K bounds the per-anchor result"
    );

    // Re-reconcile must not error or hang. Whether it reports Unchanged vs
    // Created depends on `reconcile_named_view`'s SQL normalizer matching Turso's
    // canonicalized stored matview form (Turso rewrites complex join/aggregate
    // SELECTs) — a `reconcile_named_view` concern, not advice synthesis.
    let re = handle
        .query(
            "SELECT sql FROM sqlite_master WHERE name='advice_rule_lessons_for_tasks'",
            HashMap::new(),
        )
        .await
        .unwrap();
    eprintln!(
        "STORED_MATVIEW_SQL: {}",
        re.first()
            .and_then(|r| r.get("sql"))
            .and_then(|v| v.as_string())
            .unwrap_or("<none>")
    );
    reconcile_advice_rule(&handle, &rule)
        .await
        .expect("re-reconcile must not error/hang");

    // Inactive rule tears the matview down.
    let mut inactive = rule.clone();
    inactive.active = false;
    assert_eq!(
        reconcile_advice_rule(&handle, &inactive)
            .await
            .expect("teardown"),
        holon_advice::ReconcileOutcome::TornDown,
    );
    let present = handle
        .query(
            "SELECT name FROM sqlite_master WHERE type='view' AND name='advice_rule_lessons_for_tasks'",
            HashMap::new(),
        )
        .await
        .expect("check view gone");
    assert_eq!(present.len(), 0, "inactive rule must drop its matview");

    handle.shutdown().await.expect("shutdown");
}

/// Probe record: the IVM behaviours that shaped the synthesized DDL. These are
/// kept as executable documentation of *why* suppression + recency are read-time
/// (Spike-2 stage-5). Each asserts what current Turso IVM actually does.
#[tokio::test]
async fn probe_ivm_shape_findings() {
    let handle = setup().await;
    block(&handle, "t1", 100).await;
    block(&handle, "lA", 300).await;
    block(&handle, "lB", 200).await;
    for (b, t) in [
        ("t1", "proj"),
        ("t1", "urgent"),
        ("lA", "proj"),
        ("lA", "urgent"),
        ("lB", "proj"),
    ] {
        tag(&handle, b, t).await;
    }
    suppress(&handle, "t1", "lA").await;
    handle.transition_to_ready().await.unwrap();

    // FINDING 1 (GOOD): block_tags self-join + `<>` + aggregate is IVM-correct.
    let created = holon_turso::matview_manager::reconcile_named_view(
        &handle,
        "probe_overlap",
        "SELECT atg.block_id AS anchor_id, ctg.block_id AS lesson_id, COUNT(*) AS n \
         FROM block_tags atg JOIN block_tags ctg ON ctg.tag = atg.tag AND ctg.block_id <> atg.block_id \
         GROUP BY atg.block_id, ctg.block_id",
    )
    .await
    .expect("overlap matview builds");
    assert!(created);
    let rows = handle
        .query(
            "SELECT lesson_id, n FROM probe_overlap WHERE anchor_id='t1' ORDER BY n DESC, lesson_id",
            HashMap::new(),
        )
        .await
        .unwrap();
    let v: Vec<(String, i64)> = rows
        .iter()
        .map(|r| {
            (
                r.get("lesson_id").unwrap().as_string().unwrap().to_string(),
                r.get("n").unwrap().as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(v, vec![("lA".into(), 2), ("lB".into(), 1)]);

    // FINDING 2 (BAD): an in-matview suppression LEFT-JOIN anti-join is IGNORED by
    // IVM — lA is suppressed yet still present. This is why suppression is read-time.
    holon_turso::matview_manager::reconcile_named_view(
        &handle,
        "probe_suppress",
        "SELECT atg.block_id AS anchor_id, ctg.block_id AS lesson_id, COUNT(*) AS n \
         FROM block_tags atg JOIN block_tags ctg ON ctg.tag = atg.tag AND ctg.block_id <> atg.block_id \
         LEFT JOIN advice_suppressed s ON s.anchor_id = atg.block_id AND s.lesson_id = ctg.block_id \
         WHERE s.lesson_id IS NULL \
         GROUP BY atg.block_id, ctg.block_id",
    )
    .await
    .expect("suppress-in-matview builds");
    let still_present = handle
        .query(
            "SELECT lesson_id FROM probe_suppress WHERE anchor_id='t1' AND lesson_id='lA'",
            HashMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        still_present.len(),
        1,
        "IVM ignores the in-matview suppression anti-join → suppression MUST be read-time"
    );

    handle.shutdown().await.unwrap();
}

/// PROBE (Option-B crux): is the suppression anti-join maintained INCREMENTALLY
/// in a *non-aggregating* outer matview? FINDING 2 above shows the anti-join is
/// ignored when fused with a `GROUP BY` aggregate — which is why the current
/// design keeps suppression read-time. But the advice canonical read applies
/// suppression in a PLAIN outer SELECT over the (already-scored) rule matview
/// `v`; there is no aggregate at that level (top-K is Rust-side). If IVM
/// maintains the anti-join here, the entire canonical read can become a single
/// watched matview delivering signal + data, retiring the one-shot read and the
/// proxy-trigger watches (and the id-less-junction-row enrichment panic with it).
///
/// This is the "test the common route first" probe for the advice-panic fix.
#[tokio::test]
async fn probe_outer_antijoin_is_incrementally_maintained() {
    let handle = setup().await;
    block(&handle, "task1", 100).await;
    block(&handle, "lessonA", 300).await;
    block(&handle, "lessonB", 200).await;
    for (b, t) in [
        ("task1", "task"),
        ("task1", "proj"),
        ("task1", "urgent"),
        ("lessonA", "lesson"),
        ("lessonA", "proj"),
        ("lessonA", "urgent"),
        ("lessonB", "lesson"),
        ("lessonB", "proj"),
    ] {
        tag(&handle, b, t).await;
    }
    handle.transition_to_ready().await.unwrap();

    let rule = parse_advice_rule(BUNDLED_LESSONS_FOR_TASKS_YAML).expect("bundled rule parses");
    reconcile_advice_rule(&handle, &rule)
        .await
        .expect("rule matview builds");

    // The advice canonical read shape MINUS ORDER BY / per-anchor top-K (Rust
    // sorts + caps the delivered set). `lesson_id AS id` makes each row an
    // entity-shaped lesson row — enrichable, no id-less junction rows.
    let sql = "SELECT v.anchor_id AS anchor_id, v.lesson_id AS id, \
                      v.shared_tag_count AS shared_tag_count \
               FROM advice_rule_lessons_for_tasks v \
               LEFT JOIN advice_suppressed s \
                 ON s.anchor_id = v.anchor_id AND s.lesson_id = v.lesson_id \
               JOIN block_raw c ON c.id = v.lesson_id \
               WHERE s.lesson_id IS NULL";
    let created =
        holon_turso::matview_manager::reconcile_named_view(&handle, "probe_advice_read", sql)
            .await
            .expect("outer read matview builds without hang");
    assert!(created, "outer read matview created");

    async fn present(handle: &DbHandle, anchor: &str) -> Vec<String> {
        let mut v: Vec<String> = handle
            .query(
                &format!("SELECT id FROM probe_advice_read WHERE anchor_id = '{anchor}'"),
                HashMap::new(),
            )
            .await
            .expect("read probe matview")
            .iter()
            .map(|r| r.get("id").unwrap().as_string().unwrap().to_string())
            .collect();
        v.sort();
        v
    }

    // Initial: both overlapping lessons present under task1.
    assert_eq!(
        present(&handle, "task1").await,
        vec!["lessonA".to_string(), "lessonB".to_string()],
        "initial outer-matview contents"
    );

    // THE CRUX: suppress AFTER ready. Does the anti-join drop lessonA
    // incrementally, or (like FINDING 2 with GROUP BY) silently ignore it?
    suppress(&handle, "task1", "lessonA").await;
    assert_eq!(
        present(&handle, "task1").await,
        vec!["lessonB".to_string()],
        "incremental anti-join MUST drop the newly-suppressed lesson from the watched matview"
    );

    handle.shutdown().await.unwrap();
}

/// Fetch `col` for `id` from `view` as a parsed JSON string array, plus the
/// number of rows the view holds for that id (dup detection).
async fn json_col(handle: &DbHandle, view: &str, id: &str, col: &str) -> (Vec<String>, usize) {
    let rows = handle
        .query(
            &format!("SELECT {col} FROM {view} WHERE id = '{id}'"),
            HashMap::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("query {view}.{col} for {id}: {e:#}"));
    let n = rows.len();
    let raw = rows
        .first()
        .unwrap_or_else(|| panic!("no {view} row for {id}"))
        .get(col)
        .unwrap()
        .as_string()
        .unwrap()
        .to_string();
    let mut parsed: Vec<String> =
        serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{view}.{col} not JSON ({raw}): {e}"));
    parsed.sort();
    (parsed, n)
}

/// Probe record for the multi-junction fan-out fix (the `block` matview bug:
/// ONE GROUP BY over the LEFT-JOIN cross-product of THREE junctions repeats
/// each junction's values |other junctions| times). Pins, against the patched
/// Turso IVM:
///   FINDING 3 (BUG): the pre-fix single-view triple-LEFT-JOIN shape fans out —
///     plain-SQL cross-product semantics, both initially and incrementally.
///   FINDING 4: json_group_array(DISTINCT ...) in that shape is accepted and
///     maintained correctly. Viable, but NOT chosen: it still materializes the
///     cross-product and silently collapses any future edge field whose values
///     may legitimately repeat.
///   FINDING 5 (CHOSEN): chained per-junction agg matviews + final LEFT-JOIN
///     view are correct through inserts, deletes, and the production
///     DELETE-all+re-INSERT-in-txn write pattern; COALESCE gives '[]' for
///     blocks with no junction rows; exactly one row per block throughout.
#[tokio::test]
async fn probe_multi_junction_fanout_fix_shapes() {
    let handle = setup().await;
    block(&handle, "b1", 100).await;
    block(&handle, "b2", 200).await;
    for t in ["t1", "t2", "t3"] {
        tag(&handle, "b1", t).await;
    }
    handle
        .execute(
            "INSERT INTO block_requires (block_id, required_id) VALUES ('b1', 'b2')",
            vec![],
        )
        .await
        .expect("insert requires");
    suppress(&handle, "b1", "x1").await;
    handle.transition_to_ready().await.unwrap();

    let single_view_shape = |distinct: &str| {
        format!(
            "SELECT b.id, \
             COALESCE(json_group_array({distinct}bt.tag) FILTER (WHERE bt.tag IS NOT NULL), '[]') AS tags, \
             COALESCE(json_group_array({distinct}br.required_id) FILTER (WHERE br.required_id IS NOT NULL), '[]') AS requires, \
             COALESCE(json_group_array({distinct}asup.lesson_id) FILTER (WHERE asup.lesson_id IS NOT NULL), '[]') AS advice_suppressed \
             FROM block_raw b \
             LEFT OUTER JOIN block_tags bt ON bt.block_id = b.id \
             LEFT OUTER JOIN block_requires br ON br.block_id = b.id \
             LEFT OUTER JOIN advice_suppressed asup ON asup.anchor_id = b.id \
             GROUP BY b.id"
        )
    };

    // FINDING 3 (BUG PIN): the pre-fix single-view shape fans out — with 3 tags
    // the single requires/suppressed value is repeated 3 times.
    holon_turso::matview_manager::reconcile_named_view(
        &handle,
        "probe_fanout_current",
        &single_view_shape(""),
    )
    .await
    .expect("pre-fix shape builds");
    let (tags, _) = json_col(&handle, "probe_fanout_current", "b1", "tags").await;
    let (req, _) = json_col(&handle, "probe_fanout_current", "b1", "requires").await;
    let (sup, _) = json_col(&handle, "probe_fanout_current", "b1", "advice_suppressed").await;
    assert_eq!(
        tags,
        vec!["t1", "t2", "t3"],
        "tags survive (1 requires × 1 sup)"
    );
    assert_eq!(
        req,
        vec!["b2", "b2", "b2"],
        "FAN-OUT: 1 requires value repeated |tags| times"
    );
    assert_eq!(
        sup,
        vec!["x1", "x1", "x1"],
        "FAN-OUT: 1 suppressed value repeated |tags| times"
    );

    // FINDING 4: DISTINCT variant — accepted, not ignored, initially correct.
    holon_turso::matview_manager::reconcile_named_view(
        &handle,
        "probe_fanout_distinct",
        &single_view_shape("DISTINCT "),
    )
    .await
    .expect("DISTINCT shape is accepted by IVM");
    let (tags, _) = json_col(&handle, "probe_fanout_distinct", "b1", "tags").await;
    let (req, _) = json_col(&handle, "probe_fanout_distinct", "b1", "requires").await;
    let (sup, _) = json_col(&handle, "probe_fanout_distinct", "b1", "advice_suppressed").await;
    assert_eq!(tags, vec!["t1", "t2", "t3"]);
    assert_eq!(req, vec!["b2"], "DISTINCT collapses the fan-out");
    assert_eq!(sup, vec!["x1"], "DISTINCT collapses the fan-out");

    // FINDING 5: chained shape — one agg matview per junction, final view LEFT
    // JOINs the aggs (single row each) so no cross-product exists.
    for (name, jt, src, tgt) in [
        ("probe_tags_agg", "block_tags", "block_id", "tag"),
        (
            "probe_requires_agg",
            "block_requires",
            "block_id",
            "required_id",
        ),
        (
            "probe_sup_agg",
            "advice_suppressed",
            "anchor_id",
            "lesson_id",
        ),
    ] {
        holon_turso::matview_manager::reconcile_named_view(
            &handle,
            name,
            &format!("SELECT {src} AS source_id, json_group_array({tgt}) AS vals FROM {jt} GROUP BY {src}"),
        )
        .await
        .unwrap_or_else(|e| panic!("agg matview {name} builds: {e:#}"));
    }
    holon_turso::matview_manager::reconcile_named_view(
        &handle,
        "probe_block_chained",
        "SELECT b.id, \
         COALESCE(ta.vals, '[]') AS tags, \
         COALESCE(ra.vals, '[]') AS requires, \
         COALESCE(sa.vals, '[]') AS advice_suppressed \
         FROM block_raw b \
         LEFT OUTER JOIN probe_tags_agg ta ON ta.source_id = b.id \
         LEFT OUTER JOIN probe_requires_agg ra ON ra.source_id = b.id \
         LEFT OUTER JOIN probe_sup_agg sa ON sa.source_id = b.id",
    )
    .await
    .expect("chained final view builds");

    let assert_chained = |label: &'static str,
                          id: &'static str,
                          want_tags: &'static [&'static str],
                          want_req: &'static [&'static str],
                          want_sup: &'static [&'static str]| {
        let handle = handle.clone();
        async move {
            let (tags, n1) = json_col(&handle, "probe_block_chained", id, "tags").await;
            let (req, n2) = json_col(&handle, "probe_block_chained", id, "requires").await;
            let (sup, n3) = json_col(&handle, "probe_block_chained", id, "advice_suppressed").await;
            assert_eq!(
                n1.max(n2).max(n3),
                1,
                "[{label}] {id}: exactly one matview row"
            );
            assert_eq!(tags, want_tags, "[{label}] {id} tags");
            assert_eq!(req, want_req, "[{label}] {id} requires");
            assert_eq!(sup, want_sup, "[{label}] {id} advice_suppressed");
        }
    };
    assert_chained("initial", "b1", &["t1", "t2", "t3"], &["b2"], &["x1"]).await;
    assert_chained("initial", "b2", &[], &[], &[]).await;

    // Incremental: insert a 4th tag.
    tag(&handle, "b1", "t4").await;
    assert_chained("+t4", "b1", &["t1", "t2", "t3", "t4"], &["b2"], &["x1"]).await;

    // Incremental: a second requires edge.
    handle
        .execute(
            "INSERT INTO block_requires (block_id, required_id) VALUES ('b1', 'r2')",
            vec![],
        )
        .await
        .unwrap();
    assert_chained(
        "+r2",
        "b1",
        &["t1", "t2", "t3", "t4"],
        &["b2", "r2"],
        &["x1"],
    )
    .await;

    // Incremental: delete a tag.
    handle
        .execute(
            "DELETE FROM block_tags WHERE block_id = 'b1' AND tag = 't1'",
            vec![],
        )
        .await
        .unwrap();
    assert_chained("-t1", "b1", &["t2", "t3", "t4"], &["b2", "r2"], &["x1"]).await;

    // The production edge-field writer pattern: DELETE-all + re-INSERT in one txn.
    handle
        .transaction(vec![
            (
                "DELETE FROM block_tags WHERE block_id = 'b1'".into(),
                vec![],
            ),
            (
                "INSERT INTO block_tags (block_id, tag) VALUES ('b1', 'u1')".into(),
                vec![],
            ),
            (
                "INSERT INTO block_tags (block_id, tag) VALUES ('b1', 'u2')".into(),
                vec![],
            ),
        ])
        .await
        .expect("txn replace tags");
    assert_chained("txn-replace", "b1", &["u1", "u2"], &["b2", "r2"], &["x1"]).await;
    assert_chained("txn-replace", "b2", &[], &[], &[]).await;

    // The single-view shapes after the same mutations: the pre-fix shape keeps
    // fanning out incrementally (2 tags × 2 requires cross-product), DISTINCT
    // stays correct.
    let (tags, _) = json_col(&handle, "probe_fanout_current", "b1", "tags").await;
    let (req, _) = json_col(&handle, "probe_fanout_current", "b1", "requires").await;
    let (sup, _) = json_col(&handle, "probe_fanout_current", "b1", "advice_suppressed").await;
    assert_eq!(
        tags,
        vec!["u1", "u1", "u2", "u2"],
        "incremental fan-out: tags × |requires|"
    );
    assert_eq!(
        req,
        vec!["b2", "b2", "r2", "r2"],
        "incremental fan-out: requires × |tags|"
    );
    assert_eq!(
        sup,
        vec!["x1", "x1", "x1", "x1"],
        "incremental fan-out: sup × |tags×requires|"
    );
    let (tags, _) = json_col(&handle, "probe_fanout_distinct", "b1", "tags").await;
    let (req, _) = json_col(&handle, "probe_fanout_distinct", "b1", "requires").await;
    let (sup, _) = json_col(&handle, "probe_fanout_distinct", "b1", "advice_suppressed").await;
    assert_eq!(tags, vec!["u1", "u2"]);
    assert_eq!(req, vec!["b2", "r2"]);
    assert_eq!(sup, vec!["x1"]);

    // FINDING 6 (UPGRADE PATH): reconciling a matview to a CHANGED definition
    // while another matview chains on it. This is exactly what existing vaults
    // hit when the `block` DDL changes under `block_requirement_edges` /
    // `block_with_path`.
    holon_turso::matview_manager::reconcile_named_view(
        &handle,
        "probe_dependent",
        "SELECT id, tags FROM probe_block_chained",
    )
    .await
    .expect("dependent matview builds on probe_block_chained");
    let changed = holon_turso::matview_manager::reconcile_named_view(
        &handle,
        "probe_block_chained",
        "SELECT b.id, \
         COALESCE(ta.vals, '[]') AS tags, \
         COALESCE(ra.vals, '[]') AS requires, \
         COALESCE(sa.vals, '[]') AS advice_suppressed, \
         b.updated_at \
         FROM block_raw b \
         LEFT OUTER JOIN probe_tags_agg ta ON ta.source_id = b.id \
         LEFT OUTER JOIN probe_requires_agg ra ON ra.source_id = b.id \
         LEFT OUTER JOIN probe_sup_agg sa ON sa.source_id = b.id",
    )
    .await;
    assert!(
        changed.expect("recreate under a dependent must not error"),
        "changed definition reports recreated"
    );
    // Turso does NOT cascade a matview recreate into its dependents — left in
    // place they end up with DUPLICATE rows (old contents + the recreated
    // base's rows as fresh inserts). `reconcile_named_view` therefore
    // cascade-drops dependents; their owning modules recreate them later in
    // boot order. Pin the cascade:
    let gone = handle
        .query(
            "SELECT name FROM sqlite_master WHERE type='view' AND name='probe_dependent'",
            HashMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        gone.len(),
        0,
        "cascade must drop the dependent when its base matview is recreated"
    );
    // The dependent's owning module recreates it; it must be correct and live.
    holon_turso::matview_manager::reconcile_named_view(
        &handle,
        "probe_dependent",
        "SELECT id, tags FROM probe_block_chained",
    )
    .await
    .expect("dependent recreates on its base's new definition");
    tag(&handle, "b1", "post-upgrade").await;
    let (tags, n) = json_col(&handle, "probe_dependent", "b1", "tags").await;
    assert_eq!(n, 1, "exactly one dependent row after upgrade");
    assert_eq!(
        tags,
        vec!["post-upgrade", "u1", "u2"],
        "recreated dependent is incrementally maintained"
    );

    handle.shutdown().await.unwrap();
}
