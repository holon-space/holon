//! `inv-watch-rows-match-ref` (STRICT).
//!
//! @pbt oracle correspondence
//! @pbt covers cdc-watch-divergence — per-watch CDC-delivered ui_model rows
//!   (id set, selected fields, parent_id) disagree with ref expected rows
//! @pbt slips-if-removed a Turso IVM CDC bug delivers wrong/missing/spurious
//!   rows or a wrong parent_id (e.g. block:root-layout) to a live watch; the
//!   watched query panel shows stale or wrong data
//!
//! Per-watch equality between each registered watch's CDC-delivered rows
//! (the SUT's `ui_model`, keyed by query_id) and the reference model's
//! expected rows for that watch (`query_results(watch_spec)`). Checks the
//! id set, then the per-row content fields the query selected, then
//! `parent_id` (normalized for document-root sentinels). Any divergence is a
//! `Fail`.
//!
//! # No CDC-lag downgrade — the settle covers it
//!
//! `ui_model` is fed by Turso IVM CDC, which historically could lag a write
//! already landed in the write-side `block_raw` table. This body used to carry
//! two `block_raw`-consulting staleness classifiers (id-set and per-field) that
//! downgraded a lagging watch/field to `Skipped`. Both were proven dead under
//! the deterministic convergence settle (`WideE2E::settle_after_apply` →
//! `converge_projections`, covering CDC): a probe converting both `Lag` arms to
//! hard failures kept the keystone green across three full runs. They were
//! removed (2026-07-04) — the settle quiesces `ui_model` before the check, so a
//! divergence is now a real bug, not a delivery race. `parent_id` was already
//! asserted hard (the path that catches the `block:root-layout` CDC parent_id
//! bug); that is unchanged.

use std::collections::HashSet;

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefWatch;
use holon_pbt_core::capabilities::SutWatch;
use holon_pbt_core::capabilities::WatchRow;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvWatchRowsMatchRef;

impl InvWatchRowsMatchRef {
    pub const ID: InvariantId = InvariantId("inv-watch-rows-match-ref");
}

/// Normalize a content/text field before comparing: org round-trips strip
/// trailing whitespace per line, so both sides are trimmed identically (matches
/// `normalize_block`).
fn normalize_content(s: &str) -> String {
    s.lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Normalize a `parent_id` field: sentinel / no-parent URIs collapse to
/// the synthetic document-root marker (a `normalize_parent` step). Both sides
/// are already SUT-ID-space resolved, so no `resolve` is applied (the runner
/// did it via `with_resolved_doc_uris`).
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

        for query_id in ref_.active_watch_ids() {
            if !sut_watch_ids.contains(&query_id) {
                continue;
            }

            let ui_rows = sut.watch_rows(&query_id).await;
            let expected_rows = ref_.expected_watch_rows(&query_id);

            let ui_ids: HashSet<EntityUri> = ui_rows.iter().filter_map(id_of).collect();
            let expected_ids: HashSet<EntityUri> = expected_rows.iter().filter_map(id_of).collect();

            // ── id-set check (strict; the settle quiesced ui_model) ────────
            if ui_ids != expected_ids {
                let missing: Vec<&EntityUri> = expected_ids.difference(&ui_ids).collect();
                let spurious: Vec<&EntityUri> = ui_ids.difference(&expected_ids).collect();
                return InvariantResult::Fail(format!(
                    "CDC UI model for watch '{query_id}' has wrong block IDs.\nExpected {} \
                     blocks: {expected_ids:?}\nGot {} blocks (ui_model): {ui_ids:?}\nMissing in \
                     ui_model: {missing:?}\nSpurious in ui_model: {spurious:?}",
                    expected_ids.len(),
                    ui_ids.len(),
                ));
            }

            // ── per-row field + parent_id checks (strict) ──────────────
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

                    if actual_val != expected_val {
                        return InvariantResult::Fail(format!(
                            "CDC field '{field}' mismatch for block '{expected_id}' in watch \
                             '{query_id}'\nactual_ui_model={actual_val:?}\nexpected={expected_val:\
                             ?}"
                        ));
                    }
                }

                if check_parent {
                    let actual_parent =
                        normalize_parent(ui_row.get("parent_id").and_then(|v| v.as_ref()));
                    let expected_parent =
                        normalize_parent(expected_row.get("parent_id").and_then(|v| v.as_ref()));
                    if actual_parent != expected_parent {
                        // No CDC-lag downgrade — parent_id is asserted hard. This
                        // is the path that catches the pre-existing
                        // `block:root-layout` parent_id bug.
                        return InvariantResult::Fail(format!(
                            "CDC parent_id mismatch for {expected_id} in watch \
                             '{query_id}'\nactual_ui_model={actual_parent:?}\\
                             nexpected={expected_parent:?}"
                        ));
                    }
                }
            }
        }

        InvariantResult::Ok
    }
}
