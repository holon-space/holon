//! `inv-watch-rows-match-ref` (STRICT, CDC-lag tolerant).
//!
//! Per-watch equality between each registered watch's CDC-delivered rows
//! (the SUT's `ui_model`, keyed by query_id) and the reference model's
//! expected rows for that watch (`query_results(watch_spec)`). Checks the
//! id set, then the per-row content fields the query selected, then
//! `parent_id` (normalized for document-root sentinels).
//!
//! # CDC-lag handling → `Skipped`
//!
//! Each `ui_model` watch is fed by Turso IVM CDC, which can lag a write
//! that has already landed in the write-side `block_raw` table. Two
//! classifiers apply two CDC-lag downgrades:
//!
//! - **id-set lag**: when a watch's `ui_model` id set disagrees with the
//!   reference, re-run the watch's `to_block_raw_sql()` truth query. If
//!   `block_raw` matches the reference, the matview merely lagged → that
//!   watch is skipped. If `block_raw` ALSO disagrees, the write/parse
//!   pipeline has a real bug → `Fail`.
//! - **field-level lag**: when a per-row field disagrees, read the field
//!   directly from `block_raw`. If `block_raw` agrees with the reference,
//!   the matview lagged → that field is skipped. If `block_raw` also
//!   disagrees → `Fail`.
//!
//! `parent_id` has **no** CDC-lag downgrade — it is asserted hard.
//! A `parent_id` divergence is therefore always a
//! `Fail`. This is the path that currently catches the real pre-existing
//! `block:root-layout` CDC parent_id bug, and it must keep doing so.
//!
//! Across all registered watches the body returns `Fail` on the first
//! real divergence (id-set, field, or parent_id), else `Skipped` if any
//! watch was downgraded by a CDC-lag classifier, else `Ok`.
//!
//! # Why STRICT, not WARN
//!
//! Every real divergence must **fail** the run; the only non-panic paths are
//! the two CDC-lag downgrades, modelled here as `Skipped` (orthogonal to
//! `RunMode`). So the faithful run mode is `Strict` — the CDC-lag downgrade is
//! orthogonal to the run mode, not a reason to weaken it to `Warn`.

use std::collections::HashSet;

use holon_pbt_core::capabilities::{EntityUri, RefWatch, SutWatch, WatchRow};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

use crate::pbt::staleness::{Staleness, classify_staleness};

pub struct InvWatchRowsMatchRef;

impl InvWatchRowsMatchRef {
    pub const ID: InvariantId = InvariantId("inv-watch-rows-match-ref");
}

/// Normalize a content/text field before comparing: org round-trips strip trailing whitespace per line, so both
/// sides are trimmed identically (matches `normalize_block`).
fn normalize_content(s: &str) -> String {
    s.lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Normalize a `parent_id` field: sentinel / no-parent URIs collapse to
/// the synthetic document-root marker (a `normalize_parent` step). Both sides are already SUT-ID-space resolved, so no
/// `resolve` is applied (the runner did it via `with_resolved_doc_uris`).
fn normalize_parent(v: Option<&String>) -> Option<String> {
    v.map(|s| {
        let parsed = EntityUri::parse(s);
        if parsed.as_ref().is_ok_and(|u| {
            u.is_no_parent() || u.is_sentinel() || *u == holon_api::default_doc_block_uri()
        }) {
            // Prod's `__default__` layout root ≡ the ref model's
            // `__document_root__` (block-sync P3 sentinel equivalence).
            "__document_root__".to_string()
        } else {
            s.trim().to_string()
        }
    })
}

fn id_of(row: &WatchRow) -> Option<EntityUri> {
    row.get("id")
        .and_then(|v| v.as_ref())
        .map(|s| EntityUri::parse(s).expect("invalid entity URI in watch row"))
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvWatchRowsMatchRef
where
    R: RefWatch,
    S: SutWatch,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // Watches present on BOTH sides — `ui_model` query_ids that are also
        // in `active_watches`, i.e. the intersection.
        let sut_watch_ids: HashSet<String> = sut.watch_query_ids().await.into_iter().collect();
        let mut any_lag_skipped = false;

        for query_id in ref_.active_watch_ids() {
            if !sut_watch_ids.contains(&query_id) {
                continue;
            }

            let ui_rows = sut.watch_rows(&query_id).await;
            let expected_rows = ref_.expected_watch_rows(&query_id);

            let ui_ids: HashSet<EntityUri> = ui_rows.iter().filter_map(id_of).collect();
            let expected_ids: HashSet<EntityUri> = expected_rows.iter().filter_map(id_of).collect();

            // ── id-set CDC-lag classifier ──────────────────────────────
            // ui_model is the downstream projection; `block_raw` (via the watch's
            // `to_block_raw_sql()` truth query) the authoritative upstream.
            match classify_staleness(&ui_ids, &expected_ids, || async {
                let truth_sql = ref_.watch_block_raw_sql(&query_id);
                sut.block_raw_query_ids(&truth_sql)
                    .await
                    .into_iter()
                    .collect::<HashSet<EntityUri>>()
            })
            .await
            {
                Staleness::Converged => {}
                Staleness::Lag => {
                    // ui_model lagged for this watch — skip its per-row checks.
                    // Re-checking against stale rows would just mask the next signal.
                    any_lag_skipped = true;
                    continue;
                }
                // block_raw ALSO disagrees → real write/parse pipeline bug.
                Staleness::Divergent {
                    upstream: truth_ids,
                } => {
                    let missing: Vec<&EntityUri> = expected_ids.difference(&ui_ids).collect();
                    let spurious: Vec<&EntityUri> = ui_ids.difference(&expected_ids).collect();
                    return InvariantResult::Fail(format!(
                        "CDC UI model for watch '{query_id}' has wrong block IDs (block_raw also \
                         disagrees — real bug, not a CDC delivery race).\n\
                         Expected {} blocks: {expected_ids:?}\n\
                         Got {} blocks (ui_model): {ui_ids:?}\n\
                         Got {} blocks (block_raw truth): {truth_ids:?}\n\
                         Missing in ui_model: {missing:?}\n\
                         Spurious in ui_model: {spurious:?}",
                        expected_ids.len(),
                        ui_ids.len(),
                        truth_ids.len(),
                    ));
                }
            }

            // ── per-row field + parent_id checks ───────────────────────
            let query_cols = ref_.watch_query_columns(&query_id);
            let fields_to_check: Vec<&str> =
                ["content", "content_type", "source_language", "source_name"]
                    .into_iter()
                    .filter(|f| query_cols.iter().any(|c| c == f))
                    .collect();
            let check_parent = query_cols.iter().any(|c| c == "parent_id");

            for expected_row in &expected_rows {
                let Some(expected_id) = id_of(expected_row) else {
                    continue;
                };
                let Some(ui_row) = ui_rows
                    .iter()
                    .find(|r| id_of(r).as_ref() == Some(&expected_id))
                else {
                    continue;
                };

                for field in &fields_to_check {
                    let expected_val = expected_row
                        .get(*field)
                        .and_then(|v| v.as_ref())
                        .map(|s| normalize_content(s));
                    let actual_val = ui_row
                        .get(*field)
                        .and_then(|v| v.as_ref())
                        .map(|s| normalize_content(s));

                    // field-level CDC-lag classifier: ui_model field is downstream,
                    // the same field read straight from `block_raw` is upstream.
                    match classify_staleness(&actual_val, &expected_val, || async {
                        sut.block_raw_field(&expected_id, field)
                            .await
                            .map(|s| normalize_content(&s))
                    })
                    .await
                    {
                        Staleness::Converged => {}
                        Staleness::Lag => any_lag_skipped = true,
                        Staleness::Divergent { upstream: sql_val } => {
                            return InvariantResult::Fail(format!(
                                "CDC field '{field}' mismatch for block '{expected_id}' in watch \
                                 '{query_id}'\n\
                                 actual_ui_model={actual_val:?}\n\
                                 actual_sql={sql_val:?}\n\
                                 expected={expected_val:?}"
                            ));
                        }
                    }
                }

                if check_parent {
                    let actual_parent =
                        normalize_parent(ui_row.get("parent_id").and_then(|v| v.as_ref()));
                    let expected_parent =
                        normalize_parent(expected_row.get("parent_id").and_then(|v| v.as_ref()));
                    if actual_parent != expected_parent {
                        // No CDC-lag downgrade here — parent_id is asserted
                        // hard. This is the path that catches the
                        // pre-existing `block:root-layout` parent_id bug.
                        return InvariantResult::Fail(format!(
                            "CDC parent_id mismatch for {expected_id} in watch '{query_id}'\n\
                             actual_ui_model={actual_parent:?}\n\
                             expected={expected_parent:?}"
                        ));
                    }
                }
            }
        }

        if any_lag_skipped {
            InvariantResult::Skipped(
                "[inv-watch-rows-match-ref] one or more watches lagged (Turso IVM CDC delivery \
                 race): ui_model stale, block_raw matches reference"
                    .to_string(),
            )
        } else {
            InvariantResult::Ok
        }
    }
}
