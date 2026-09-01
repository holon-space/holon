//! KITCHEN D.0 SPIKE — **THROWAWAY, NOT PRODUCTION WIRING.**
//!
//! The riskiest unverified assumption behind Kitchen Inc D
//! (`docs/Plans/Kitchen.md` §3.4) is one sentence: *"lower `Agg` to a scalar
//! correlated subquery … parameter-free and inlinable ⇒ it plants into the
//! matview SELECT with no change to the plant site."* That is an assertion
//! about the FORK'S IVM planner, and the same planner already rejects `CASE`
//! (`json_extract_matview_spike.rs`).
//!
//! **VERDICT (this file, green, is the executable record):**
//!   * correlated scalar subquery in a matview SELECT list → **REJECTED** at
//!     DDL. The fork says so by name and prescribes the rewrite: LEFT OUTER
//!     JOIN + GROUP BY. §3.4's "no change to the plant site" is REFUTED.
//!   * GROUP BY side-matview + LEFT OUTER JOIN → accepted and IVM-maintained
//!     across child insert / update / delete / re-parent, and over a parent
//!     with no children at all. This is already the shape `block` uses for its
//!     edge fields, so `Agg` lowers onto a landed production pattern.
//!   * `COUNT(*) FILTER (WHERE …)` vs `SUM(iif(…, 1, 0))` — the two candidate
//!     lowerings for `unmatched_count`; the probes below record which plants.
//!   * The JOIN lowering also DELETES a correctness hazard the subquery
//!     lowering carried: an inner column that only the parent owns is a hard
//!     DDL error there, where the subquery form would have silently bound it to
//!     the outer row.
//!
//! Nothing here is imported by production. The proposed `Computation::Agg`
//! variant is prototyped as a LOCAL type ([`AggProto`]) so the experiment
//! measures both seats end to end without adding a variant to `holon-api`.
//!
//! **RETAINED AS FORK-CAPABILITY GUARDS** (the rest of this file is superseded
//! once Inc D's real differential exists): the two `probe_a_*` rejections and
//! `probe_c_filtered_count_via_filter_clause`. A RED there does not mean the
//! test broke — it means a fork bump changed what plants, and Kitchen.md §3.4's
//! lowering must be revisited deliberately. `probe_b`'s childless-parent `0.0`
//! assertion is likewise a permanent guard on the `COALESCE`.

use std::collections::HashMap;

use holon_api::Value;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

// ---------------------------------------------------------------------------
// Fixture: a parent/child pair shaped like `recipe` / `ingredient_use`.
// ---------------------------------------------------------------------------

async fn setup() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend); // keep the actor alive for the test
    handle
        .execute_ddl("CREATE TABLE rcp (id TEXT PRIMARY KEY, title TEXT, servings INTEGER)")
        .await
        .expect("create parent table");
    handle
        .execute_ddl(
            "CREATE TABLE iuse (id TEXT PRIMARY KEY, recipe_id TEXT, grams REAL, product_id TEXT)",
        )
        .await
        .expect("create child table");
    handle
}

async fn put_recipe(handle: &DbHandle, id: &str, servings: i64) {
    handle
        .execute(
            "INSERT INTO rcp (id, title, servings) VALUES (?, ?, ?) ON CONFLICT(id) DO UPDATE SET \
             servings = excluded.servings",
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Text(format!("recipe {id}")),
                turso::Value::Integer(servings),
            ],
        )
        .await
        .expect("upsert recipe");
}

/// `product = None` models D2's unmatched ingredient.
async fn put_use(handle: &DbHandle, id: &str, recipe_id: &str, grams: f64, product: Option<&str>) {
    handle
        .execute(
            "INSERT INTO iuse (id, recipe_id, grams, product_id) VALUES (?, ?, ?, ?) ON \
             CONFLICT(id) DO UPDATE SET recipe_id = excluded.recipe_id, grams = excluded.grams, \
             product_id = excluded.product_id",
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Text(recipe_id.into()),
                turso::Value::Real(grams),
                match product {
                    Some(p) => turso::Value::Text(p.into()),
                    None => turso::Value::Null,
                },
            ],
        )
        .await
        .expect("upsert ingredient_use");
}

async fn delete_use(handle: &DbHandle, id: &str) {
    handle
        .execute(
            "DELETE FROM iuse WHERE id = ?",
            vec![turso::Value::Text(id.into())],
        )
        .await
        .expect("delete ingredient_use");
}

fn as_f64(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Float(f)) => *f,
        Some(Value::Integer(i)) => *i as f64,
        Some(Value::Null) => panic!("aggregate column is NULL; COALESCE should have covered it"),
        other => panic!("unexpected aggregate value: {other:?}"),
    }
}

/// `(recipe_id, aggregate)` from a single-aggregate view, sorted.
async fn read_agg(handle: &DbHandle, view: &str, col: &str) -> Vec<(String, f64)> {
    let rows = handle
        .query(
            &format!("SELECT id, {col} FROM {view} ORDER BY id"),
            HashMap::new(),
        )
        .await
        .unwrap_or_else(|e| panic!("query {view}: {e}"));
    let mut out: Vec<(String, f64)> = rows
        .iter()
        .map(|r| {
            let id = match r.get("id") {
                Some(Value::String(s)) => s.clone(),
                other => panic!("id: unexpected {other:?}"),
            };
            (id, as_f64(r.get(col)))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ---------------------------------------------------------------------------
// A. The correlated scalar subquery — the shape §3.4 assumes. REJECTED.
// ---------------------------------------------------------------------------

/// Pins the rejection AND the fork's prescribed rewrite. If a future fork bump
/// makes correlated subqueries plantable, this test flips red and the `Agg` SQL
/// seat may be revisited — the same guard style `probe_searched_case_rejected`
/// uses for `CASE`.
#[tokio::test]
async fn probe_a_correlated_sum_subquery_is_rejected_at_ddl() {
    let handle = setup().await;
    let select = "SELECT r.id, r.title, (SELECT COALESCE(SUM(c.grams), 0.0) FROM iuse c WHERE \
                  c.recipe_id = r.id) AS grams_total FROM rcp r";
    let err = reconcile_named_view(&handle, "v_sum_corr", select)
        .await
        .expect_err("the fork's IVM compiler must reject a correlated scalar subquery");
    let msg = err.to_string();
    assert!(
        msg.contains(
            "Correlated scalar subqueries in materialized view SELECT lists are not yet \
                      supported"
        ),
        "expected the correlated-subquery rejection, got: {msg}"
    );
    assert!(
        msg.contains("LEFT OUTER JOIN with GROUP BY"),
        "the fork prescribes the replacement lowering; that prescription is what Inc D builds on: \
         {msg}"
    );
}

/// The same rejection for a correlated `COUNT`, so the verdict is about the
/// SHAPE and not about `SUM` in particular.
#[tokio::test]
async fn probe_a_correlated_count_subquery_is_rejected_at_ddl() {
    let handle = setup().await;
    let select =
        "SELECT r.id, (SELECT COUNT(*) FROM iuse c WHERE c.recipe_id = r.id) AS n FROM rcp r";
    let err = reconcile_named_view(&handle, "v_cnt_corr", select)
        .await
        .expect_err("correlated COUNT must be rejected for the same reason as SUM");
    assert!(
        err.to_string().contains("Correlated scalar subqueries"),
        "got: {err}"
    );
}

// ---------------------------------------------------------------------------
// B. The prescribed lowering: GROUP BY side-matview + LEFT OUTER JOIN.
// ---------------------------------------------------------------------------

/// One side view per RELATION carrying every aggregate over it, joined once —
/// which is what keeps N aggregates over one relation to a single extra join.
const IUSE_AGG: &str = "SELECT recipe_id, SUM(grams) AS grams_sum, COUNT(*) AS n_uses FROM iuse \
                        GROUP BY recipe_id";

const JOINED: &str = "SELECT r.id, COALESCE(a.grams_sum, 0.0) AS grams_total, \
                      COALESCE(a.n_uses, 0) AS n_uses FROM rcp r LEFT OUTER JOIN v_iuse_agg a ON \
                      a.recipe_id = r.id";

async fn setup_joined(handle: &DbHandle) {
    reconcile_named_view(handle, "v_iuse_agg", IUSE_AGG)
        .await
        .unwrap_or_else(|e| panic!("GROUP BY side view must plant: {e}"));
    reconcile_named_view(handle, "v_joined", JOINED)
        .await
        .unwrap_or_else(|e| panic!("join over the side view must plant: {e}"));
}

/// The full maintenance matrix. The matview's own FROM is the PARENT, but every
/// value here depends on CHILD rows — an IVM that did not treat the child as a
/// source would serve a stale total forever, which is worse than a rejection.
#[tokio::test]
async fn probe_b_join_lowering_is_maintained_across_every_child_mutation() {
    let handle = setup().await;
    setup_joined(&handle).await;

    put_recipe(&handle, "r1", 2).await;
    put_recipe(&handle, "r2", 4).await;
    assert_eq!(
        read_agg(&handle, "v_joined", "grams_total").await,
        vec![("r1".into(), 0.0), ("r2".into(), 0.0)],
        "a parent with NO children must read 0.0, not vanish from the view — the COALESCE is \
         load-bearing, not cosmetic"
    );

    put_use(&handle, "u1", "r1", 100.0, Some("p1")).await;
    put_use(&handle, "u2", "r1", 50.0, Some("p2")).await;
    put_use(&handle, "u3", "r2", 7.5, None).await;
    assert_eq!(
        read_agg(&handle, "v_joined", "grams_total").await,
        vec![("r1".into(), 150.0), ("r2".into(), 7.5)],
        "child INSERT"
    );

    put_use(&handle, "u2", "r1", 25.0, Some("p2")).await;
    assert_eq!(
        read_agg(&handle, "v_joined", "grams_total").await,
        vec![("r1".into(), 125.0), ("r2".into(), 7.5)],
        "child UPDATE"
    );

    delete_use(&handle, "u1").await;
    assert_eq!(
        read_agg(&handle, "v_joined", "grams_total").await,
        vec![("r1".into(), 25.0), ("r2".into(), 7.5)],
        "child DELETE must retract through BOTH matview levels"
    );

    put_use(&handle, "u3", "r1", 7.5, None).await;
    assert_eq!(
        read_agg(&handle, "v_joined", "grams_total").await,
        vec![("r1".into(), 32.5), ("r2".into(), 0.0)],
        "an FK change must MOVE the contribution between parents, never duplicate it"
    );

    delete_use(&handle, "u2").await;
    delete_use(&handle, "u3").await;
    assert_eq!(
        read_agg(&handle, "v_joined", "grams_total").await,
        vec![("r1".into(), 0.0), ("r2".into(), 0.0)],
        "emptying a relation must return the parent to the no-children reading, not strand the \
         last value"
    );
}

// ---------------------------------------------------------------------------
// C. `unmatched_count` — the filtered aggregate, two candidate lowerings.
// ---------------------------------------------------------------------------

/// `SUM(iif(pred, 1, 0))`. `iif` is already proven plantable
/// (`json_extract_matview_spike.rs`), which makes this the lowering that needs
/// no new fork capability.
#[tokio::test]
async fn probe_c_filtered_count_via_sum_of_iif() {
    let handle = setup().await;
    reconcile_named_view(
        &handle,
        "v_unmatched_iif",
        "SELECT recipe_id, SUM(iif(product_id IS NULL, 1, 0)) AS unmatched FROM iuse GROUP BY \
         recipe_id",
    )
    .await
    .unwrap_or_else(|e| panic!("SUM(iif(...)) filtered count must plant: {e}"));

    put_use(&handle, "u1", "r1", 10.0, Some("p1")).await;
    put_use(&handle, "u2", "r1", 20.0, None).await;
    let rows = handle
        .query(
            "SELECT recipe_id AS id, unmatched FROM v_unmatched_iif",
            HashMap::new(),
        )
        .await
        .expect("query");
    assert_eq!(as_f64(rows[0].get("unmatched")), 1.0);

    // Binding the unmatched ingredient must clear D2's incompleteness signal.
    put_use(&handle, "u2", "r1", 20.0, Some("p9")).await;
    let rows = handle
        .query(
            "SELECT recipe_id AS id, unmatched FROM v_unmatched_iif",
            HashMap::new(),
        )
        .await
        .expect("query");
    assert_eq!(as_f64(rows[0].get("unmatched")), 0.0);
}

/// `COUNT(*) FILTER (WHERE …)` is what the fork's own error message suggests.
/// Recorded either way: if it plants, `Agg`'s filter lowers directly; if not,
/// the `iif` form above is the one to generate. Not an assertion of support —
/// a measurement of it.
#[tokio::test]
async fn probe_c_filtered_count_via_filter_clause() {
    let handle = setup().await;
    let err = reconcile_named_view(
        &handle,
        "v_unmatched_filter",
        "SELECT recipe_id, COUNT(*) FILTER (WHERE product_id IS NULL) AS unmatched FROM iuse GROUP \
         BY recipe_id",
    )
    .await
    .expect_err("the FILTER clause the fork's own error message suggests is itself unsupported");
    assert!(
        err.to_string().contains("FILTER not supported with Count"),
        "expected the FILTER limitation; a different error (or success) means the fork's aggregate \
         support moved and §3.4's filtered-count lowering must be revisited: {err}"
    );
}

// ---------------------------------------------------------------------------
// D. The hazard the JOIN lowering removes.
// ---------------------------------------------------------------------------

/// `Computation::compile_sql` emits BARE column names, so under the SUBQUERY
/// lowering an inner expression naming a column the child does not own would
/// have bound to the OUTER row instead — no error, a plausible and wrong
/// number. Under the JOIN lowering the aggregate's SELECT sees the child table
/// alone, so the same mistake is a hard DDL error. That is a design argument in
/// the fallback's favour, and this probe is what makes it a fact rather than a
/// claim.
#[tokio::test]
async fn probe_d_inner_column_the_child_lacks_is_a_hard_error_not_a_silent_outer_binding() {
    let handle = setup().await;
    // `servings` exists on the PARENT only.
    let err = reconcile_named_view(
        &handle,
        "v_bad_inner",
        "SELECT recipe_id, SUM(servings) AS bad FROM iuse GROUP BY recipe_id",
    )
    .await
    .expect_err("an inner column the child does not own must FAIL, never resolve outward");
    println!("VERDICT D: rejected as expected: {err}");
}

// ---------------------------------------------------------------------------
// E. The dual-seat oracle in miniature.
// ---------------------------------------------------------------------------

/// THROWAWAY prototype of the proposed `Computation::Agg`. Local to this spike
/// so the experiment needs no variant in `holon-api`.
///
/// Two properties of the proposal are load-bearing and both are modelled: the
/// inner expression is scoped to the CHILD row (never the parent's), and the
/// relation is named, resolving to `(child_table, fk_column)`.
enum AggProto {
    /// `sum(<rel>, <inner>)` — `inner` reads child columns only.
    Sum { inner_col: &'static str },
    /// `count(<rel>, <pred>)`, with the pred restricted to `col == ()` — the
    /// `unmatched_count` shape.
    CountWhereNull { pred_col: &'static str },
}

/// A child row as the eval seat sees it: its own `Context`.
type ChildRow = HashMap<String, Value>;

impl AggProto {
    /// The eval seat. Note what the signature says on its own: aggregation
    /// cannot be served by today's `eval(&Context)` — it needs the child rows
    /// too. That is P13's scope change in one line.
    fn eval(&self, children: &[ChildRow]) -> f64 {
        match self {
            AggProto::Sum { inner_col } => children
                .iter()
                .map(|c| match c.get(*inner_col) {
                    Some(Value::Float(f)) => *f,
                    Some(Value::Integer(i)) => *i as f64,
                    other => panic!("non-numeric inner value {other:?}"),
                })
                .sum(),
            AggProto::CountWhereNull { pred_col } => children
                .iter()
                .filter(|c| matches!(c.get(*pred_col), Some(Value::Null) | None))
                .count() as f64,
        }
    }

    /// The SQL seat, in the side-view half of the JOIN lowering. The child
    /// alias is applied HERE, at compile time, rather than trusting name
    /// resolution (probe D).
    fn side_view_column(&self, alias: &str) -> String {
        match self {
            AggProto::Sum { inner_col } => format!("SUM(c.{inner_col}) AS {alias}"),
            AggProto::CountWhereNull { pred_col } => {
                format!("SUM(iif(c.{pred_col} IS NULL, 1, 0)) AS {alias}")
            }
        }
    }

    /// The outer half: what the parent's matview SELECT reads.
    fn outer_column(&self, alias: &str) -> String {
        match self {
            AggProto::Sum { .. } => format!("COALESCE(agg.{alias}, 0.0) AS {alias}"),
            AggProto::CountWhereNull { .. } => format!("COALESCE(agg.{alias}, 0) AS {alias}"),
        }
    }
}

/// Both seats must agree after EVERY mutation, not only at the end.
/// Divergence-on-a-later-step is precisely the class of bug the dual-compile
/// design exists to prevent.
#[tokio::test]
async fn probe_e_eval_and_sql_seats_agree_across_a_mutation_sequence() {
    let handle = setup().await;
    let sum = AggProto::Sum { inner_col: "grams" };
    let unmatched = AggProto::CountWhereNull {
        pred_col: "product_id",
    };

    let side = format!(
        "SELECT c.recipe_id, {}, {} FROM iuse c GROUP BY c.recipe_id",
        sum.side_view_column("grams_total"),
        unmatched.side_view_column("unmatched")
    );
    reconcile_named_view(&handle, "v_dual_agg", &side)
        .await
        .unwrap_or_else(|e| panic!("side view: {e}"));
    let outer = format!(
        "SELECT r.id, {}, {} FROM rcp r LEFT OUTER JOIN v_dual_agg agg ON agg.recipe_id = r.id",
        sum.outer_column("grams_total"),
        unmatched.outer_column("unmatched")
    );
    reconcile_named_view(&handle, "v_dual", &outer)
        .await
        .unwrap_or_else(|e| panic!("outer view: {e}"));

    put_recipe(&handle, "r1", 2).await;
    let mut model: Vec<ChildRow> = Vec::new();

    let steps: [(&str, f64, Option<&str>); 5] = [
        ("u1", 100.0, Some("p1")),
        ("u2", 12.5, None),
        ("u3", 0.0, None),
        ("u1", 250.0, Some("p1")), // update in place
        ("u2", 12.5, Some("p2")),  // unmatched -> matched
    ];
    for (id, grams, product) in steps {
        put_use(&handle, id, "r1", grams, product).await;
        model.retain(|c| c.get("id") != Some(&Value::String(id.to_string())));
        model.push(HashMap::from([
            ("id".to_string(), Value::String(id.to_string())),
            ("grams".to_string(), Value::Float(grams)),
            (
                "product_id".to_string(),
                match product {
                    Some(p) => Value::String(p.to_string()),
                    None => Value::Null,
                },
            ),
        ]));
        assert_eq!(
            read_agg(&handle, "v_dual", "grams_total").await,
            vec![("r1".into(), sum.eval(&model))],
            "seats disagree on sum after upserting {id}"
        );
        assert_eq!(
            read_agg(&handle, "v_dual", "unmatched").await,
            vec![("r1".into(), unmatched.eval(&model))],
            "seats disagree on unmatched after upserting {id}"
        );
    }

    for id in ["u2", "u1", "u3"] {
        delete_use(&handle, id).await;
        model.retain(|c| c.get("id") != Some(&Value::String(id.to_string())));
        assert_eq!(
            read_agg(&handle, "v_dual", "grams_total").await,
            vec![("r1".into(), sum.eval(&model))],
            "seats disagree on sum after deleting {id}"
        );
        assert_eq!(
            read_agg(&handle, "v_dual", "unmatched").await,
            vec![("r1".into(), unmatched.eval(&model))],
            "seats disagree on unmatched after deleting {id}"
        );
    }
}

// ---------------------------------------------------------------------------
// F. Composition: aggregating over a child's own COMPUTED column.
// ---------------------------------------------------------------------------

/// §3.3 does not sum a stored column — `recipe.kcal_total` sums
/// `ingredient_use.grams`, which is ITSELF a computed (row-scoped) field. So
/// the side view cannot be built over the child's RAW table; it must sit on the
/// child's own matview, making the chain
/// `child_raw -> child matview -> relation agg view -> parent matview`: three
/// levels of matview-on-matview.
///
/// Whether that chain maintains is the difference between "Inc D reuses the
/// landed plant pattern" and "Inc D needs the child's computed fields
/// materialised some other way first", so it is measured here rather than
/// assumed.
#[tokio::test]
async fn probe_f_aggregate_over_a_childs_computed_column_through_a_three_level_chain() {
    let handle = setup().await;
    // Level 1: the child's own matview, carrying a row-scoped computed column
    // in the shape `Computation::Case` lowers to (`iif`, never `CASE`).
    // `grams` here is `quantity` converted by a per-row factor.
    reconcile_named_view(
        &handle,
        "v_iuse_mv",
        "SELECT id, recipe_id, product_id, iif(product_id IS NULL, 0.0, grams * 2.0) AS grams_conv \
         FROM iuse",
    )
    .await
    .unwrap_or_else(|e| panic!("child matview with a computed column: {e}"));

    // Level 2: the relation aggregate, over the child's MATVIEW.
    reconcile_named_view(
        &handle,
        "v_iuse_conv_agg",
        "SELECT recipe_id, SUM(grams_conv) AS conv_total FROM v_iuse_mv GROUP BY recipe_id",
    )
    .await
    .unwrap_or_else(|e| panic!("relation agg over the child MATVIEW: {e}"));

    // Level 3: the parent, joining it.
    reconcile_named_view(
        &handle,
        "v_rcp_conv",
        "SELECT r.id, COALESCE(a.conv_total, 0.0) AS conv_total FROM rcp r LEFT OUTER JOIN \
         v_iuse_conv_agg a ON a.recipe_id = r.id",
    )
    .await
    .unwrap_or_else(|e| panic!("parent matview joining the relation agg: {e}"));

    put_recipe(&handle, "r1", 1).await;
    put_use(&handle, "u1", "r1", 100.0, Some("p1")).await;
    put_use(&handle, "u2", "r1", 10.0, None).await; // unmatched => contributes 0.0
    assert_eq!(
        read_agg(&handle, "v_rcp_conv", "conv_total").await,
        vec![("r1".into(), 200.0)],
        "a write to the RAW child table must propagate through all three matview levels"
    );

    // Binding the unmatched ingredient changes the child's computed column,
    // which must re-flow to the parent's aggregate.
    put_use(&handle, "u2", "r1", 10.0, Some("p9")).await;
    assert_eq!(
        read_agg(&handle, "v_rcp_conv", "conv_total").await,
        vec![("r1".into(), 220.0)],
        "a change in the CHILD's computed column must re-flow to the parent's aggregate"
    );

    delete_use(&handle, "u1").await;
    assert_eq!(
        read_agg(&handle, "v_rcp_conv", "conv_total").await,
        vec![("r1".into(), 20.0)],
        "retraction through three levels"
    );
}
