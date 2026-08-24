//! The advice WEAVE — the frontend half of ADR 0022 (runtime-definable advice).
//!
//! ## Session-level sidecar (single source of truth)
//!
//! The weave is a **session-level sidecar** on the [`ReactiveEngine`]: a
//! `HashMap<anchor, Vec<Arc<DataRow>>>` of already-synthesized advice rows,
//! keyed by anchor. It is read **synchronously and purely** by
//! [`crate::reactive::BuilderServices::advice_children`] during interpretation
//! — mirroring how the creation slot reads `virtual_child_config` synchronously
//! — so the woven rows are visible on the pure/snapshot/MCP render path, not
//! only on the streaming (GPUI) driver path.
//!
//! Population is a single [`recompute_sidecar`] pass that runs the canonical
//! advice read ONCE for every anchor the active rule produces and replaces the
//! whole sidecar. It is driven two ways, both writing the SAME sidecar:
//! - deterministically from the composed test/keystone settle
//!   (`ReactiveEngine::refresh_advice_sidecar`, fail-loud convergence), and
//! - reactively in a live session by [`spawn_session_weaver`], which watches
//!   the rule set + suppression table and recomputes on each change.
//!
//! ## Read = one-shot (Rust top-K); trigger = watch the canonical read matview
//!
//! The canonical read ([`advice_read_all_sql`]) — an anti-join + `ORDER BY`
//! over the synthesized `advice_rule_{slug}` matview, with per-anchor top-K
//! applied in Rust — runs as a **one-shot**
//! [`holon_api::QueryEngine::execute_query`] for the actual data (ordering +
//! top-K are read-time / Rust-side, not maintained by IVM).
//!
//! To know WHEN to recompute, [`spawn_session_weaver`] watches (via
//! `watch_query`) two things: the rule set (`GET_ADVICE_RULES_SQL`) and the
//! active rule's canonical read as an entity-shaped matview
//! ([`advice_watch_sql`], `lesson_id AS id`). The latter transitively depends
//! on both the rule matview AND `advice_suppressed`; Turso IVM DOES
//! incrementally maintain the suppression anti-join in that non-aggregating
//! outer view (proven by holon-advice
//! `probe_outer_antijoin_is_incrementally_maintained`), so ONE watch fires on
//! scoring changes, new lessons, and dismiss/undismiss alike. The watch is a
//! reactive TRIGGER — its content is not consumed for data (the one-shot read
//! is the source of truth); rows carry `id` only so the enrichment boundary
//! (`row_id`) never sees an id-less junction row — the bug this replaced.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use holon_api::EntityUri;
use holon_api::Occurrence;
use holon_api::OccurrenceId;
use holon_api::QueryLanguage;
use holon_api::RowKey;
use holon_api::Value;
use holon_api::render_requirements::RenderRequirements;
use holon_api::widget_spec::DataRow;

/// The session-level advice sidecar: anchor → synthesized advice rows (rank
/// order). Read synchronously by `advice_children`, replaced wholesale by
/// [`recompute_sidecar`].
pub type AdviceSidecar = Arc<Mutex<HashMap<EntityUri, Vec<Arc<DataRow>>>>>;

/// Max-scalar sentinel prefix (same one the creation slot uses): sorts a woven
/// advice row AFTER every real sibling under `sort_value`'s string compare. The
/// 4-digit zero-padded rank suffix then orders advice rows AMONG THEMSELVES by
/// score (rank 0 = best), so MutableTree's `(sort_key, id)` ordering renders
/// SCORE order — not lesson-id order. A single flat sentinel key would collapse
/// that ordering; the keystone invariant (non-increasing score) is designed to
/// go red on exactly that mistake.
const ADVICE_SORT_SENTINEL: char = '\u{10FFFF}';

/// Rank-encoded suffix `sort_key` for the advice row at 0-based `rank`.
pub fn advice_sort_key(rank: usize) -> String {
    format!("{ADVICE_SORT_SENTINEL}{rank:04}")
}

/// The canonical advice read (ADR 0022) over EVERY anchor the rule produces,
/// with the lesson content joined in. Mirrors the pinned per-anchor
/// `read_query` in `crates/holon-advice/tests/matview_build.rs` — read-time
/// suppression anti-join (Turso IVM can't do it inside the matview),
/// score+recency ordering, and the `lesson_id` UI-stability tiebreak — but
/// grouped by anchor (the per-anchor top-K truncation is applied in Rust after
/// this ordered read).
///
/// **One-shot ONLY** — see the module doc. `view` is
/// `rule.name.matview_name()`. Ordered by `anchor_id` first so
/// [`recompute_sidecar`] can group contiguously.
fn advice_read_all_sql(view: &str) -> String {
    format!(
        "SELECT v.anchor_id, v.lesson_id, c.content FROM {view} v LEFT JOIN advice_suppressed s \
         ON s.anchor_id = v.anchor_id AND s.lesson_id = v.lesson_id JOIN block_raw c ON c.id = \
         v.lesson_id WHERE s.lesson_id IS NULL ORDER BY v.anchor_id, v.shared_tag_count DESC, \
         c.updated_at DESC, v.lesson_id"
    )
}

/// The active rule's canonical read as a WATCHABLE, entity-shaped matview: the
/// same join + suppression anti-join as [`advice_read_all_sql`], but projecting
/// `lesson_id AS id` (so every row enriches cleanly — no id-less junction rows
/// at the `row_id` boundary) and WITHOUT `ORDER BY` / top-K (both are read-time
/// / Rust-side; a watched multiset is unordered anyway). Watched purely as a
/// reactive trigger: its transitive deps (the rule matview +
/// `advice_suppressed`) mean one watch fires on every advice-relevant change.
/// The suppression anti-join IS incrementally maintained in this
/// non-aggregating outer view — see holon-advice
/// `probe_outer_antijoin_is_incrementally_maintained` (contrast the aggregate
/// form, `probe_ivm_shape_findings` FINDING 2, which IVM cannot maintain).
fn advice_watch_sql(view: &str) -> String {
    format!(
        "SELECT v.lesson_id AS id, v.anchor_id, c.content FROM {view} v LEFT JOIN \
         advice_suppressed s ON s.anchor_id = v.anchor_id AND s.lesson_id = v.lesson_id JOIN \
         block_raw c ON c.id = v.lesson_id WHERE s.lesson_id IS NULL"
    )
}

/// PURE CORE (unit-tested): build the woven advice suffix rows for a
/// collection.
///
/// `per_anchor` is `(anchor, ranked_lessons)` where `ranked_lessons` is the
/// canonical read output for that anchor in SCORE order (rank 0 = best),
/// already suppression-filtered and capped at `k`. `content_of` resolves a
/// lesson's LIVE row (its canonical cell) for content — content is taken from
/// the live row, NEVER snapshotted out of the read. When it returns `None`
/// (lesson not yet materialized) the row still renders (empty content),
/// refreshed next recompute.
///
/// Column / key contract of each synthesized row:
/// - `id`        = canonical lesson id → edits/click route to canonical (ADR
///   0015 rule 3)
/// - `parent_id` = anchor id → display-only placement under the anchor (rule 1)
/// - `sort_key`  = [`advice_sort_key`]`(rank)` → score order among advice rows
/// - `content`   = the lesson's live content
/// - `target_id` = canonical lesson id → `op_button` dispatch target
/// - `anchor_id` = anchor id → threaded so `dismiss_advice` binds anchor +
///   lesson
/// - key = `(lesson_uri, Occurrence::Placed(OccurrenceId::for_placement(lesson,
///   anchor)))`
pub fn synthesize_advice_rows(
    per_anchor: &[(EntityUri, Vec<EntityUri>)],
    content_of: impl Fn(&EntityUri) -> Option<Arc<DataRow>>,
) -> Vec<(RowKey, Arc<DataRow>)> {
    let mut out = Vec::new();
    for (anchor, lessons) in per_anchor {
        for (rank, lesson) in lessons.iter().enumerate() {
            // Start from the lesson's LIVE row so content (and any other
            // displayed field) tracks the canonical block; then stamp the
            // display-placement overrides. `id` stays canonical.
            let mut row: DataRow = content_of(lesson).map(|r| (*r).clone()).unwrap_or_default();
            row.insert("id".into(), Value::String(lesson.as_str().to_string()));
            row.insert(
                "parent_id".into(),
                Value::String(anchor.as_str().to_string()),
            );
            row.insert("sort_key".into(), Value::String(advice_sort_key(rank)));
            row.insert(
                "target_id".into(),
                Value::String(lesson.as_str().to_string()),
            );
            row.insert(
                "anchor_id".into(),
                Value::String(anchor.as_str().to_string()),
            );
            row.entry("content".into())
                .or_insert_with(|| Value::String(String::new()));

            let occ = OccurrenceId::for_placement(lesson, anchor);
            out.push(((lesson.clone(), Occurrence::Placed(occ)), Arc::new(row)));
        }
    }
    out
}

/// Recompute the placed-occurrence key of a synthesized advice row from its
/// `id` (canonical lesson) + `anchor_id` columns. Used by the render weave to
/// stamp `ReactiveViewModel::occurrence` — the same `for_placement(lesson,
/// anchor)` coordinate the keystone invariant recomputes to match.
pub fn advice_row_occurrence(row: &DataRow) -> Occurrence {
    let lesson = row.get("id").and_then(|v| v.as_string());
    let anchor = row.get("anchor_id").and_then(|v| v.as_string());
    match (lesson, anchor) {
        (Some(l), Some(a)) => Occurrence::Placed(OccurrenceId::for_placement(
            &holon_api::entity_uri_from_id_str(l),
            &holon_api::entity_uri_from_id_str(a),
        )),
        // A row missing its identity columns is a synthesis bug, not a display
        // state — fail loud rather than silently render it as canonical.
        _ => panic!("advice_row_occurrence: synthesized advice row missing id/anchor_id: {row:?}"),
    }
}

/// The active rule's read parameters (the only slice of the rule the weaver
/// read needs). Discovered by parsing the rule blocks; v1 narrows to ≤1 active
/// rule (matches the keystone).
struct ActiveRule {
    view: String,
    k: u8,
}

/// ONE recompute pass: discover the active rule, run the canonical all-anchor
/// read ONCE, group by anchor, apply the per-anchor top-K, and replace the
/// whole sidecar. No active rule → clear the sidecar (teardown: rule deleted /
/// inactive / unparseable). Read failure → clear + disclose (the reconciler may
/// not have created `advice_rule_{slug}` yet — a real boot race).
pub async fn recompute_sidecar(query_engine: &dyn holon_api::QueryEngine, sidecar: &AdviceSidecar) {
    let Some(rule) = discover_active_rule(query_engine).await else {
        sidecar.lock().unwrap().clear();
        return;
    };

    let sql = advice_read_all_sql(&rule.view);
    let rows = match query_engine
        .execute_query(&sql, QueryLanguage::HolonSql, HashMap::new(), None)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::debug!(
                "advice sidecar: canonical read failed ({e:#}); clearing (rule matview not ready?)"
            );
            sidecar.lock().unwrap().clear();
            return;
        }
    };

    // Group contiguous anchors (the SQL orders by anchor_id first), applying the
    // per-anchor top-K truncation on the score-ordered stream. Content is taken
    // from the joined `content` column of the winning (kept) rows.
    //
    // PERF / TODO(partitioned-top-K): the top-K is applied HERE in Rust because
    // Turso IVM has global `ORDER BY ... LIMIT` but no PARTITION-BY (per-anchor)
    // top-K operator, so the watched `advice_rule_{slug}` matview is per-anchor
    // UNBOUNDED. This is O(all-candidate-advice) per recompute — fine while advice
    // sets are small. If it becomes a latency dominator, push a partitioned top-K
    // operator into the Turso IVM fork (core/incremental/) so K-per-anchor is
    // maintained incrementally and the matview is bounded. Tracked in
    // docs/Testing/BugFunnel.md (deferred-perf).
    let mut per_anchor: Vec<(EntityUri, Vec<EntityUri>)> = Vec::new();
    let mut content_map: HashMap<EntityUri, Arc<DataRow>> = HashMap::new();
    let k = rule.k as usize;
    for r in &rows {
        let (Some(anchor), Some(lesson)) = (
            r.get("anchor_id")
                .and_then(|v| v.as_string())
                .map(holon_api::entity_uri_from_id_str),
            r.get("lesson_id")
                .and_then(|v| v.as_string())
                .map(holon_api::entity_uri_from_id_str),
        ) else {
            continue;
        };
        let kept = match per_anchor.last_mut() {
            Some((a, lessons)) if *a == anchor => {
                if lessons.len() < k {
                    lessons.push(lesson.clone());
                    true
                } else {
                    false
                }
            }
            _ => {
                per_anchor.push((anchor.clone(), vec![lesson.clone()]));
                true
            }
        };
        if kept {
            let mut crow = DataRow::new();
            crow.insert("id".into(), Value::String(lesson.as_str().to_string()));
            crow.insert(
                "content".into(),
                r.get("content")
                    .cloned()
                    .unwrap_or(Value::String(String::new())),
            );
            content_map.insert(lesson, Arc::new(crow));
        }
    }

    let content_of = |lesson: &EntityUri| content_map.get(lesson).cloned();
    let mut map: HashMap<EntityUri, Vec<Arc<DataRow>>> = HashMap::new();
    for (anchor, lessons) in &per_anchor {
        let synthesized = synthesize_advice_rows(&[(anchor.clone(), lessons.clone())], content_of);
        map.insert(
            anchor.clone(),
            synthesized.into_iter().map(|(_, row)| row).collect(),
        );
    }
    *sidecar.lock().unwrap() = map;
}

/// Discover the single active advice rule (v1 narrows to ≤1). Reads the rule
/// blocks via the plain `GET_ADVICE_RULES_SQL` (a one-shot; safe SELECT over
/// the block matview), parses each with the typed boundary parser, and keeps
/// the only `Ok` + `active` rule.
async fn discover_active_rule(query_engine: &dyn holon_api::QueryEngine) -> Option<ActiveRule> {
    let rows = query_engine
        .execute_query(
            holon_advice::GET_ADVICE_RULES_SQL,
            QueryLanguage::HolonSql,
            HashMap::new(),
            None,
        )
        .await
        .map_err(|e| {
            tracing::debug!("advice weaver: rule discovery read failed ({e:#})");
            e
        })
        .ok()?;

    let mut active: Vec<ActiveRule> = rows
        .iter()
        .filter_map(|r| {
            let block_id = r.get("id").and_then(|v| v.as_string())?;
            let content = r.get("content").and_then(|v| v.as_string())?;
            let discovered = holon_advice::parse_discovered_rule(block_id, content);
            match discovered.result {
                Ok(rule) if rule.active => Some(ActiveRule {
                    view: rule.name.matview_name(),
                    k: rule.k.get(),
                }),
                Ok(_) => None,
                Err(e) => {
                    // Rule blocks render their own status (ADR 0022); here we
                    // just skip an unparseable/inactive rule, disclosed.
                    tracing::debug!("advice weaver: skipping unparseable rule {block_id}: {e}");
                    None
                }
            }
        })
        .collect();

    debug_assert!(
        active.len() <= 1,
        "advice v1 expects at most one active rule, found {}",
        active.len()
    );
    active.pop()
}

/// Spawn the session-level reactive weaver: an initial [`recompute_sidecar`],
/// then a recompute on every advice-relevant change. Runs for as long as the
/// session lives; holds only the query engine + the shared sidecar (no engine
/// back-reference), so it is spawned lazily from `advice_children` without a
/// self-referential Arc.
///
/// Two `watch_query` streams, re-established per active-rule generation:
/// - the rule SET (`GET_ADVICE_RULES_SQL`) — a change may swap which matview is
///   active, so it breaks the inner loop to re-establish;
/// - the active rule's canonical read ([`advice_watch_sql`]) — fires on
///   suppression + scoring/lesson changes, so each batch just recomputes.
///
/// Both streams are entity-shaped (rows carry `id`), so enrichment never sees
/// an id-less junction row. The first batch of each is the initial snapshot
/// (already reflected by the preceding recompute) and is skipped.
pub fn spawn_session_weaver(
    runtime: &tokio::runtime::Handle,
    query_engine: Arc<dyn holon_api::QueryEngine>,
    sidecar: AdviceSidecar,
) {
    use futures::StreamExt;
    runtime.spawn(async move {
        // Re-established once per active-rule generation.
        loop {
            recompute_sidecar(query_engine.as_ref(), &sidecar).await;

            let mut rules_stream = match query_engine
                .watch_query(
                    holon_advice::GET_ADVICE_RULES_SQL,
                    QueryLanguage::HolonSql,
                    HashMap::new(),
                    None,
                    RenderRequirements::none(),
                )
                .await
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::debug!(
                        "advice weaver: rule-set watch not established ({e:#}); advice will not \
                         react"
                    );
                    return;
                }
            };
            // Skip the initial snapshot batch (baseline, not a change).
            let _ = rules_stream.next().await;

            let mut data_stream = match discover_active_rule(query_engine.as_ref()).await {
                Some(rule) => match query_engine
                    .watch_query(
                        &advice_watch_sql(&rule.view),
                        QueryLanguage::HolonSql,
                        HashMap::new(),
                        None,
                        RenderRequirements::none(),
                    )
                    .await
                {
                    Ok(mut s) => {
                        let _ = s.next().await; // skip initial snapshot
                        Some(s)
                    }
                    Err(e) => {
                        tracing::debug!(
                            "advice weaver: data watch not established ({e:#}); advice will not \
                             react to suppression/scoring until the rule set changes"
                        );
                        None
                    }
                },
                None => None,
            };

            // Recompute on each data change; re-establish when the rule set changes.
            loop {
                let data_next = async {
                    match data_stream.as_mut() {
                        Some(s) => {
                            s.next().await;
                        }
                        None => std::future::pending::<()>().await,
                    }
                };
                tokio::select! {
                    _ = rules_stream.next() => break, // rule set changed → re-establish
                    _ = data_next => {
                        recompute_sidecar(query_engine.as_ref(), &sidecar).await;
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(id: &str) -> EntityUri {
        EntityUri::block(id)
    }

    fn content_row(id: &str, content: &str) -> Arc<DataRow> {
        let mut row = DataRow::new();
        row.insert("id".into(), Value::String(format!("block:{id}")));
        row.insert("content".into(), Value::String(content.to_string()));
        // A stale parent/sort from the canonical row — the synthesizer MUST
        // override both with the placement's anchor + rank sentinel.
        row.insert(
            "parent_id".into(),
            Value::String("block:real-parent".into()),
        );
        row.insert("sort_key".into(), Value::String("A0".into()));
        Arc::new(row)
    }

    #[test]
    fn sort_keys_are_rank_ordered_and_sort_after_real_keys() {
        // Rank order: 0 < 1 < 2 (string compare of the padded suffix).
        assert!(advice_sort_key(0) < advice_sort_key(1));
        assert!(advice_sort_key(1) < advice_sort_key(2));
        // Sorts AFTER a real fractional-index hex key (leading '2'..'F' < sentinel).
        assert!(advice_sort_key(0).as_str() > "A0");
        assert!(advice_sort_key(9).as_str() > "ZZZZ");
    }

    #[test]
    fn synthesize_stamps_placement_contract_and_score_order() {
        let anchor = uri("task1");
        let lessons = vec![uri("lessonA"), uri("lessonB")];
        let content = |l: &EntityUri| -> Option<Arc<DataRow>> {
            match l.as_str() {
                "block:lessonA" => Some(content_row("lessonA", "advice A")),
                "block:lessonB" => Some(content_row("lessonB", "advice B")),
                _ => None,
            }
        };

        let rows = synthesize_advice_rows(&[(anchor.clone(), lessons.clone())], content);
        assert_eq!(rows.len(), 2);

        for (rank, ((key_uri, occ), row)) in rows.iter().enumerate() {
            let lesson = &lessons[rank];
            // Key: canonical lesson id + Placed(for_placement(lesson, anchor)).
            assert_eq!(key_uri, lesson, "key carries the CANONICAL lesson id");
            assert_eq!(
                *occ,
                Occurrence::Placed(OccurrenceId::for_placement(lesson, &anchor)),
                "occurrence rides the identity key (never the id string)"
            );
            // id stays canonical; parent overridden to anchor; sort = rank sentinel.
            assert_eq!(
                row.get("id").and_then(|v| v.as_string()),
                Some(lesson.as_str())
            );
            assert_eq!(
                row.get("parent_id").and_then(|v| v.as_string()),
                Some(anchor.as_str()),
                "parent_id overridden to the display-local anchor (not the canonical parent)"
            );
            assert_eq!(
                row.get("sort_key").and_then(|v| v.as_string()),
                Some(advice_sort_key(rank).as_str()),
                "rank-encoded sort_key → score order, not lesson-id order"
            );
            // Dispatch columns for the dismiss op_button.
            assert_eq!(
                row.get("target_id").and_then(|v| v.as_string()),
                Some(lesson.as_str())
            );
            assert_eq!(
                row.get("anchor_id").and_then(|v| v.as_string()),
                Some(anchor.as_str()),
                "anchor threaded through so dismiss_advice can bind anchor + lesson"
            );
            // The render weave recomputes the SAME occurrence from the row.
            assert_eq!(advice_row_occurrence(row), *occ);
        }

        // Content comes from the live cell, not the read.
        assert_eq!(
            rows[0].1.get("content").and_then(|v| v.as_string()),
            Some("advice A")
        );
    }

    #[test]
    fn synthesize_tolerates_missing_content_cell() {
        let anchor = uri("task1");
        let lessons = vec![uri("ghost")];
        let rows = synthesize_advice_rows(&[(anchor, lessons)], |_| None);
        assert_eq!(rows.len(), 1);
        // Still renders: id/parent/sort present, content empty.
        assert_eq!(
            rows[0].1.get("content").and_then(|v| v.as_string()),
            Some(""),
            "missing live cell → empty content, row still renders (refreshed next recompute)"
        );
        assert!(rows[0].1.contains_key("sort_key"));
    }

    #[test]
    fn read_all_sql_names_rule_view_and_orders_by_anchor_first() {
        let sql = advice_read_all_sql("advice_rule_lessons_for_tasks");
        assert!(sql.contains("FROM advice_rule_lessons_for_tasks v"));
        assert!(
            sql.contains("s.lesson_id IS NULL"),
            "read-time suppression anti-join"
        );
        assert!(
            sql.contains(
                "ORDER BY v.anchor_id, v.shared_tag_count DESC, c.updated_at DESC, v.lesson_id"
            ),
            "anchor-grouped, then score + recency ordering with the lesson_id tiebreak"
        );
    }
}
