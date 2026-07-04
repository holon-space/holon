//! Rule → matview synthesis (ADR 0022 "Engine owns DDL synthesis").
//!
//! Each active rule compiles to EXACTLY ONE anchor-denormalized materialized
//! view `advice_rule_{slug}` carrying an `anchor_id` output column, read at
//! render time with `WHERE anchor_id = ?`. This is the Spike-2 shape: one
//! matview per *rule*, denormalized over all anchors — never one matview per
//! anchor (the N-matview cliff). The suppression anti-join (`LEFT JOIN
//! advice_suppressed … IS NULL`) is implied by every rule and never
//! configurable (ADR 0021).

use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use thiserror::Error;

use crate::lowering::LoweringError;
use crate::rule::AdviceRule;
use crate::rule::ScoringTemplate;
use crate::rule::TagOverlapRecencySpec;

/// The synthesized matview for a rule: a stable name + the `SELECT` body that
/// `reconcile_named_view` wraps in `CREATE MATERIALIZED VIEW {name} AS
/// {select}`.
#[derive(Debug, Clone, PartialEq)]
pub struct SynthesizedMatview {
    pub view_name: String,
    pub select_sql: String,
}

/// A typed synthesis error, carried so a broken rule surfaces on its own block.
#[derive(Debug, Error, PartialEq)]
pub enum AdviceSynthesisError {
    #[error("advice-rule anchor could not lower to SQL: {0}")]
    Lowering(#[from] LoweringError),
}

/// Synthesize the anchor-denormalized matview `SELECT` for a rule.
///
/// Shape (Spike-2-proven IVM-safe; see the crate's probe tests):
/// - overlap = `block_tags atg` ⋈ `block_tags ctg` on equal tags, `ctg.block_id
///   <> atg.block_id` excludes self → one row per (anchor, candidate) shared
///   tag.
/// - `COUNT(*)` = shared-tag count (block_tags PK `(block_id, tag)` makes each
///   (atg, ctg) pairing unique per shared tag).
/// - anchor/candidate predicates lower to `block_tags` joins (`has_tag`) or a
///   1:1 `block_raw` join used ONLY in `WHERE` (`entity`/`prop_eq`).
///
/// DELIBERATELY NOT in this matview (Spike-2 findings — the naive in-matview
/// forms are silently miscompiled by current Turso IVM, see probe tests,
/// resolving ADR 0021's open stage-5 fork toward read-time application):
/// - **Suppression** (`LEFT JOIN advice_suppressed … IS NULL`): an anti-join
///   FUSED WITH this matview's `GROUP BY` aggregate is IGNORED by IVM, so it is
///   applied over the scored matview instead. (In a PLAIN non-aggregating outer
///   view the anti-join IS incrementally maintained — the live weaver watches
///   exactly such a read; see `holon_frontend::advice_weaver::advice_watch_sql`
///   and `matview_build::probe_outer_antijoin_is_incrementally_maintained`.)
/// - **Recency** (`updated_at`): a `block_raw` column in `GROUP BY`/`SELECT`
///   corrupts the IVM aggregate. The `updated_at` tiebreak is applied at READ
///   time (join `block` for `updated_at`).
/// - **top-K** (`k`) and **ordering**: the matview is un-capped and unordered
///   (denormalized over all anchors); the read-time weave applies per-anchor
///   `ORDER BY shared_tag_count DESC, updated_at DESC LIMIT k`. Keeping the
///   matview `ORDER BY`-free also keeps `reconcile_named_view` idempotent
///   across boots.
pub fn synthesize_matview(rule: &AdviceRule) -> Result<SynthesizedMatview, AdviceSynthesisError> {
    let mut join_seq = 0usize;
    let anchor = rule.anchor.lower("atg.block_id", "a", &mut join_seq)?;
    let ScoringTemplate::TagOverlapRecency(TagOverlapRecencySpec { source }) = &rule.candidates;
    let candidate = source.lower("ctg.block_id", "c", &mut join_seq)?;

    let mut joins = vec![
        "JOIN block_tags ctg ON ctg.tag = atg.tag AND ctg.block_id <> atg.block_id".to_string(),
    ];
    if anchor.needs_block_raw {
        joins.push("JOIN block_raw a ON a.id = atg.block_id".to_string());
    }
    if candidate.needs_block_raw {
        joins.push("JOIN block_raw c ON c.id = ctg.block_id".to_string());
    }
    joins.extend(anchor.joins);
    joins.extend(candidate.joins);

    let mut predicates = Vec::new();
    predicates.extend(anchor.predicates);
    predicates.extend(candidate.predicates);
    let where_clause = if predicates.is_empty() {
        String::new()
    } else {
        format!("\nWHERE {}", predicates.join(" AND "))
    };

    let select_sql = format!(
        "SELECT\n    atg.block_id AS anchor_id,\n    ctg.block_id AS lesson_id,\n    COUNT(*) AS \
         shared_tag_count\nFROM block_tags atg\n{joins}{where_clause}\nGROUP BY atg.block_id, \
         ctg.block_id",
        joins = joins.join("\n"),
    );

    Ok(SynthesizedMatview {
        view_name: rule.name.matview_name(),
        select_sql,
    })
}

/// Outcome of reconciling a rule's matview against the live DB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileOutcome {
    /// Active rule: matview was created or its definition changed and was
    /// recreated.
    Created,
    /// Active rule: matview already matched — no DDL churn.
    Unchanged,
    /// Inactive rule: any existing matview was torn down (or there was none).
    TornDown,
}

/// Reconcile a rule's matview through `matview_manager`'s
/// named/diffed/torn-down path — never the content-addressed
/// `watch_view_{hash}` path (which would mint one matview per anchor: the
/// N-matview cliff, ADR 0022).
///
/// - `active` rule → synthesize + `reconcile_named_view` (unchanged view
///   skipped; changed view recreated).
/// - inactive rule → DROP the matview if present (ADR 0022 sub-decision 1:
///   flipping `ACTIVE` off tears the view down).
pub async fn reconcile_advice_rule(
    db_handle: &DbHandle,
    rule: &AdviceRule,
) -> anyhow::Result<ReconcileOutcome> {
    let view_name = rule.name.matview_name();
    if !rule.active {
        db_handle
            .execute_ddl(&format!("DROP VIEW IF EXISTS {view_name}"))
            .await?;
        return Ok(ReconcileOutcome::TornDown);
    }
    let synth = synthesize_matview(rule)?;
    let created = reconcile_named_view(db_handle, &synth.view_name, &synth.select_sql).await?;
    Ok(if created {
        ReconcileOutcome::Created
    } else {
        ReconcileOutcome::Unchanged
    })
}
