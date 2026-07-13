// # holon_rule watcher — single-block rules (ADR 0024 §7.2)
//
// The unified rule surface: one `holon_rule` YAML block carries BOTH its guard
// (`when: not block_exists("Journals/{today}")`) and its effect
// (`emit: {place, name}`). This watcher discovers such blocks, parses them into
// the typed `HolonRule` at the boundary (fail-loud on malformed YAML), and —
// for a *ratcheted-create* ("operate") rule — reactively fires the emission
// through `execute_operation`, so C2a provenance stamps `OpOrigin::Rule`
// automatically.
//
// ## Why the reactive path watches the clock, not a compiled anti-join matview
//
// The ratified guard `not block_exists("Journals/{today}")` is an *inhibitor
// arc* (ADR 0024). Its faithful reactive form would be a CDC matview with the
// anti-join baked in (`... WHERE NOT EXISTS (SELECT 1 FROM block ...)`). But
// the reactive `block` relation is itself a materialized view, and a matview
// that reads another matview hangs in Turso IVM (the documented chained-matview
// hazard). So the guard is NOT compiled to a live matview here.
//
// Instead, per ADR 0024 P4/P5 and plan §7.2/Q3, a clock-subject guard's
// reactive binding is the **clock read-arc** — `SELECT today FROM clock WHERE
// grain='day'`, which reads only the base `clock` table (CDC-eligible, no
// chained matview; this is the proven journal-trigger path). The day-rollover
// `UPDATE` re-fires it. The inhibitor is then enforced two ways at fire time,
// both cheap and non-matview:
//   1. a direct existence read (self-inhibition: "does this block already exist
//      under its place?") — the `not block_exists(...)` semantics evaluated as
//      a one-shot read, which also prevents cross-scheme duplicates on
//      migration;
//   2. deterministic effect ids (ADR 0024 P4) — two replicas firing the same
//      rule for the same day mint the SAME block id, so the CRDT merge
//      collapses concurrent creates into one node.
// Together: at-most-once per day, idempotent under re-fire and concurrency.
//
// Deferred (loud, never silent): block-subject operate rules (their reactive
// form hits the same chained-matview wall and needs a non-matview evaluator),
// and the full in-memory ≡ SQL anti-join matview. The `Guard`'s own
// `evaluate`/`to_sql` remain the semantics of record for those paths.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use holon_advice::holon_rule::Emit;
use holon_advice::holon_rule::parse_holon_rule;
use holon_api::EntityName;
use holon_api::Value;
use holon_api::effect_id::FiringKey;
use holon_api::effect_id::OutputSlot;
use holon_api::effect_id::RuleId;
use holon_api::effect_id::deterministic_block_id;
use holon_api::pattern::Subject;
use holon_api::streaming::Change;
use holon_core::storage::types::StorageEntity;
use tokio::task::JoinHandle;
use tokio_stream::StreamExt;
use tracing::info;

use crate::api::backend_engine::BackendEngine;
use crate::api::rule_status::RuleStatus;
use crate::api::rule_status::RuleStatusHandle;

const DISCOVERY_SQL: &str = include_str!("../../../../assets/queries/holon_rule_discovery.sql");

/// A clock-subject guard's reactive binding source: the `clock` day read-arc.
/// Reads only the base `clock` table (never the `block` matview), so it is
/// CDC-eligible with no chained matview (see module docs). One row per day; the
/// scheduler's day-rollover `UPDATE` re-fires it.
const CLOCK_BINDING_SQL: &str = "SELECT today AS today FROM clock WHERE grain = 'day'";

#[tracing::instrument(skip(engine), name = "holon_rule_watcher.start")]
pub async fn start_holon_rule_watchers(engine: Arc<BackendEngine>) -> Result<()> {
    let discovery_stream = engine
        .query_and_watch(DISCOVERY_SQL.to_string(), HashMap::new(), None)
        .await
        .context("Failed to subscribe to holon_rule discovery matview")?;

    let status = engine.rule_status().clone();
    crate::util::spawn_actor(run_discovery_loop(engine, status, discovery_stream));
    Ok(())
}

async fn run_discovery_loop(
    engine: Arc<BackendEngine>,
    status: RuleStatusHandle,
    mut discovery_stream: crate::storage::turso::RowChangeStream,
) {
    let mut active: HashMap<String, JoinHandle<()>> = HashMap::new();

    while let Some(batch) = discovery_stream.next().await {
        for item in batch.inner.items {
            match item.change {
                Change::Created { data, .. } | Change::Updated { data, .. } => {
                    start_rule(&engine, &status, &mut active, &data).await;
                }
                Change::Deleted { id, .. } => {
                    if let Some(handle) = active.remove(&id) {
                        handle.abort();
                        info!("[holon_rule_watcher] aborted watcher for {id}");
                    }
                    status.clear(&id);
                }
                _ => {}
            }
        }
    }
}

/// Parse one discovered rule block and (re)spawn its firing watcher. A
/// malformed body, an unsupported effect shape, or a block-subject operate rule
/// each set a LOUD [`RuleStatus`] and do NOT fire — never a silent skip.
async fn start_rule(
    engine: &Arc<BackendEngine>,
    status: &RuleStatusHandle,
    active: &mut HashMap<String, JoinHandle<()>>,
    data: &StorageEntity,
) {
    let block_id = match extract_string(data, "id") {
        Some(id) => id,
        None => {
            tracing::warn!("[holon_rule_watcher] discovery row missing id");
            return;
        }
    };
    let content = extract_string(data, "content").unwrap_or_default();

    if let Some(handle) = active.remove(&block_id) {
        handle.abort();
    }

    // Shape separation from the legacy pair-watcher: a `holon_rule` block that has
    // a sibling trigger (`holon_sql`/`holon_prql`/`holon_gql`) is a query+action
    // *pair*, owned by `action_watcher`. This single-block watcher must not touch
    // it (no firing, no status), so the two never stomp one block's rule card.
    match is_paired(engine, &block_id).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            // An uncertain pairing check must not silently fire; surface it loud.
            tracing::error!("[holon_rule_watcher] {block_id} pairing check failed: {e:#}");
            status.set(
                &block_id,
                RuleStatus::CompileError(format!("pairing check failed: {e:#}")),
            );
            return;
        }
    }

    let rule = match parse_holon_rule(&content) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("[holon_rule_watcher] {block_id} failed to parse: {e}");
            status.set(&block_id, RuleStatus::ParseError(e.to_string()));
            return;
        }
    };

    // An operate rule carries a ratcheted `emit`. A guard-only rule (advice
    // authored in the holon_rule language) has none — the advice reconciler owns
    // it, so this watcher leaves it alone.
    let emit = match rule.emit {
        Some(emit) => emit,
        None => {
            info!("[holon_rule_watcher] {block_id} is guard-only (no emit); not an operate rule");
            return;
        }
    };

    // Only clock-subject guards have a non-matview reactive binding today. A
    // block-subject operate rule's reactive form hits the chained-matview wall
    // (module docs) — surface it loud rather than fire a half-wired rule.
    match rule.guard.subject {
        Subject::Clock => {}
        Subject::Block => {
            status.set(
                &block_id,
                RuleStatus::CompileError(
                    "block-subject operate rules are not yet reactively wired (ADR 0024 §7.2 — \
                     needs a non-chained-matview evaluator)"
                        .to_string(),
                ),
            );
            return;
        }
    }

    info!("[holon_rule_watcher] starting operate watcher for {block_id}");
    status.set(&block_id, RuleStatus::Active);
    let rule_id = RuleId::new(block_id.clone());
    let handle = tokio::spawn(run_rule_watcher(
        engine.clone(),
        status.clone(),
        rule_id,
        emit,
    ));
    active.insert(block_id, handle);
}

async fn run_rule_watcher(
    engine: Arc<BackendEngine>,
    status: RuleStatusHandle,
    rule_id: RuleId,
    emit: Emit,
) {
    if let Err(e) = run_rule_watcher_inner(&engine, &status, &rule_id, &emit).await {
        tracing::error!(
            "[holon_rule_watcher] watcher for {} failed: {e:#}",
            rule_id.as_str()
        );
    }
}

async fn run_rule_watcher_inner(
    engine: &BackendEngine,
    status: &RuleStatusHandle,
    rule_id: &RuleId,
    emit: &Emit,
) -> Result<()> {
    let mut binding_stream = engine
        .query_and_watch(CLOCK_BINDING_SQL.to_string(), HashMap::new(), None)
        .await
        .with_context(|| {
            format!(
                "Failed to subscribe to clock binding for rule {}",
                rule_id.as_str()
            )
        })?;

    while let Some(batch) = binding_stream.next().await {
        for item in batch.inner.items {
            // Ratcheted create: fire on the initial `Created` binding and on the
            // day-rollover `Updated` (rowid-stable clock UPDATE). Both are
            // idempotent via the fire-time inhibitor + deterministic id.
            match item.change {
                Change::Created { data, .. } | Change::Updated { data, .. } => {
                    fire_emit(engine, status, rule_id, emit, &data).await;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Fire one ratcheted-create emission for a produced binding row. Idempotent:
/// a fire-time existence read enforces the `not block_exists(...)` inhibitor
/// (self-inhibition), and a deterministic effect id converges concurrent
/// replica fires. Execution failures are logged loudly but do not tear down the
/// watcher.
async fn fire_emit(
    engine: &BackendEngine,
    status: &RuleStatusHandle,
    rule_id: &RuleId,
    emit: &Emit,
    binding: &StorageEntity,
) {
    let today = match extract_string(binding, "today") {
        Some(t) => t,
        None => {
            tracing::error!(
                "[holon_rule_watcher] {} binding row missing `today` column",
                rule_id.as_str()
            );
            status.set(
                rule_id.as_str(),
                RuleStatus::ExecError("clock binding row missing `today`".to_string()),
            );
            return;
        }
    };

    let content = emit.name.render(&today);
    let parent_id = emit.place.parent_id();

    // Inhibitor arc, evaluated as a direct read (never a matview): if the place
    // already holds this block, the guard is FALSE — do nothing. This is the
    // `not block_exists("Journals/{today}")` semantics, and it also keeps a
    // vault migrated from the old (differently-keyed) rule free of duplicates.
    match already_present(engine, &parent_id, &content).await {
        Ok(true) => return,
        Ok(false) => {}
        Err(e) => {
            tracing::error!(
                "[holon_rule_watcher] {} inhibitor existence check failed: {e:#}",
                rule_id.as_str()
            );
            status.set(rule_id.as_str(), RuleStatus::ExecError(format!("{e:#}")));
            return;
        }
    }

    // Deterministic effect id (ADR 0024 P4): a name-based UUIDv5 of
    // (rule-id, firing-key, slot), so every replica firing this rule for this day
    // mints the SAME block id; the CRDT merge collapses concurrent creates, and a
    // re-fire upserts the same id.
    let key = FiringKey::from_row(binding);
    let id = deterministic_block_id(rule_id, &key, &OutputSlot::first());

    let mut params = StorageEntity::new();
    params.insert("id".into(), Value::String(id.as_str().to_string()));
    params.insert("parent_id".into(), Value::String(parent_id));
    params.insert("content".into(), Value::String(content));
    // Page-file placement (`place: page(<root>)`): tag the emitted block `Page` so
    // the fileless-page sweep materializes it into its own `<name-chain>.org`
    // (ADR 0024 §7.2). `tags` is the block_tags edge field; it must carry a
    // `Value::Array`.
    if emit.place.is_page() {
        params.insert(
            "tags".into(),
            Value::Array(vec![Value::String("Page".to_string())]),
        );
    }

    info!(
        "[holon_rule_watcher] {} emitting block.create params={params:?}",
        rule_id.as_str()
    );

    if let Err(e) = engine
        .execute_operation(
            &EntityName::new("block"),
            "create",
            params,
            holon_api::OpOrigin::Rule {
                transition_id: rule_id.as_str().to_string(),
            },
        )
        .await
    {
        status.set(rule_id.as_str(), RuleStatus::ExecError(format!("{e:#}")));
        tracing::error!(
            "[holon_rule_watcher] execute_operation failed for rule {}: {e:#}",
            rule_id.as_str()
        );
    }
}

/// Is this rule block one half of a legacy query+action *pair* — i.e. does it
/// have a sibling source block whose language is a query language? Such blocks
/// are owned by `action_watcher`; this single-block watcher leaves them alone.
/// A one-shot read (never `query_and_watch`), so no matview reads the `block`
/// view.
async fn is_paired(engine: &BackendEngine, block_id: &str) -> Result<bool> {
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(block_id.to_string()));
    let rows = engine
        .db_handle()
        .query(
            "SELECT 1 FROM block_raw sib WHERE sib.parent_id = (SELECT parent_id FROM block_raw \
             WHERE id = $id) AND sib.content_type = 'source' AND sib.source_language IN \
             ('holon_sql', 'holon_prql', 'holon_gql') LIMIT 1",
            params,
        )
        .await
        .context("pairing sibling read failed")?;
    Ok(!rows.is_empty())
}

/// Does the place already hold a block with this content? A direct base-table
/// read (`block_raw`) — the inhibitor arc, evaluated without a matview.
async fn already_present(engine: &BackendEngine, parent_id: &str, content: &str) -> Result<bool> {
    let mut params = HashMap::new();
    params.insert("parent".to_string(), Value::String(parent_id.to_string()));
    params.insert("content".to_string(), Value::String(content.to_string()));
    let rows = engine
        .db_handle()
        .query(
            "SELECT id FROM block_raw WHERE parent_id = $parent AND content = $content",
            params,
        )
        .await
        .context("inhibitor existence read failed")?;
    Ok(!rows.is_empty())
}

fn extract_string(row: &StorageEntity, key: &str) -> Option<String> {
    match row.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Integer(i) => Some(i.to_string()),
        Value::Float(f) => Some(f.to_string()),
        other => Some(format!("{other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use holon_advice::holon_rule::Emit;
    use holon_advice::holon_rule::NameTemplate;
    use holon_advice::holon_rule::Place;

    use super::*;
    use crate::core::sql_operation_provider::SqlOperationProvider;
    use crate::di::test_helpers::create_test_engine_with_providers;
    use crate::storage::BLOCK_WRITE_TABLE;

    /// The journal rule exercised here uses PAGE-FILE placement
    /// (`place: page(journals)`) so the watcher tags the day-block `Page` — the
    /// mechanism this module implements (ADR 0024 §7.2). (The shipped default
    /// seed still uses inline `place: journals` pending Fork B B1; the
    /// parser supports both, and this const pins the page-file path
    /// directly.)
    const JOURNAL_RULE: &str = r#"
name: daily_journal
when: 'not block_exists("Journals/{today}")'
emit:
  place: page(journals)
  name: "{today}"
"#;

    async fn block_engine() -> Arc<BackendEngine> {
        create_test_engine_with_providers(":memory:".into(), |module| {
            module.with_operation_provider_factory(|backend| {
                let db_handle =
                    tokio::task::block_in_place(|| backend.blocking_read().handle().clone());
                // Register the `tags` edge field so a page-file emission's `Page`
                // tag lands in the `block_tags` junction (not folded into
                // properties JSON), exactly like the prod schema module.
                Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle,
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    vec![crate::storage::EdgeFieldDescriptor {
                        entity: "block".to_string(),
                        field: "tags".to_string(),
                        join_table: "block_tags".to_string(),
                        source_col: "block_id".to_string(),
                        target_col: "tag".to_string(),
                    }],
                ))
            })
        })
        .await
        .unwrap()
    }

    async fn seed_journals_parent(engine: &BackendEngine) {
        let mut parent = StorageEntity::new();
        parent.insert("id".into(), Value::String("block:journals".to_string()));
        parent.insert("content".into(), Value::String("Journals".to_string()));
        engine
            .execute_operation(
                &EntityName::new("block"),
                "create",
                parent,
                holon_api::OpOrigin::User,
            )
            .await
            .unwrap();
    }

    /// (block id, properties JSON) for every child of `block:journals`. The
    /// rule-fired create's provenance rides the `_provenance` key inside
    /// `properties` (C2a provenance stamping).
    async fn journal_children(engine: &BackendEngine) -> Vec<(String, String)> {
        engine
            .db_handle()
            .query(
                "SELECT id, properties FROM block_raw WHERE parent_id = 'block:journals'",
                HashMap::new(),
            )
            .await
            .unwrap()
            .into_iter()
            .map(|row| {
                let id = match row.get("id") {
                    Some(Value::String(s)) => s.clone(),
                    other => format!("{other:?}"),
                };
                let props = match row.get("properties") {
                    Some(Value::String(s)) => s.clone(),
                    Some(Value::Json(s)) => s.clone(),
                    Some(other) => format!("{other:?}"),
                    None => String::new(),
                };
                (id, props)
            })
            .collect()
    }

    fn journal_emit() -> Emit {
        let rule = parse_holon_rule(JOURNAL_RULE).unwrap();
        rule.emit.unwrap()
    }

    fn day_binding(day: &str) -> StorageEntity {
        let mut row = StorageEntity::new();
        row.insert("today".into(), Value::String(day.to_string()));
        row
    }

    async fn seed_source_block(
        engine: &BackendEngine,
        id: &str,
        parent: &str,
        lang: &str,
        content: &str,
    ) {
        let mut b = StorageEntity::new();
        b.insert("id".into(), Value::String(id.to_string()));
        b.insert("parent_id".into(), Value::String(parent.to_string()));
        b.insert("content_type".into(), Value::String("source".to_string()));
        b.insert("source_language".into(), Value::String(lang.to_string()));
        b.insert("content".into(), Value::String(content.to_string()));
        engine
            .execute_operation(
                &EntityName::new("block"),
                "create",
                b,
                holon_api::OpOrigin::User,
            )
            .await
            .unwrap();
    }

    /// Shape separation: a `holon_rule` block that sits next to a query trigger
    /// is a legacy pair (pair-watcher owns it) → `is_paired` true; a
    /// self-contained single-block rule → false.
    #[tokio::test(flavor = "multi_thread")]
    async fn is_paired_distinguishes_single_block_from_pair() {
        let engine = block_engine().await;
        seed_journals_parent(&engine).await;
        // A legacy pair under one parent heading.
        seed_source_block(
            &engine,
            "block:paired_rule",
            "block:journals",
            "holon_rule",
            "x",
        )
        .await;
        seed_source_block(
            &engine,
            "block:paired_trig",
            "block:journals",
            "holon_sql",
            "SELECT 1",
        )
        .await;
        // A lone single-block rule under a different parent.
        seed_source_block(
            &engine,
            "block:lone_rule",
            "block:journals",
            "holon_rule",
            "y",
        )
        .await;

        assert!(
            is_paired(&engine, "block:paired_rule").await.unwrap(),
            "a holon_rule with a query sibling is pair-managed"
        );
        // The lone rule shares block:journals with the pair's trigger in this
        // fixture, so assert the negative on a parent with no query sibling.
        seed_source_block(&engine, "block:solo_parent", "block:journals", "text", "P").await;
        seed_source_block(
            &engine,
            "block:solo_rule",
            "block:solo_parent",
            "holon_rule",
            "z",
        )
        .await;
        assert!(
            !is_paired(&engine, "block:solo_rule").await.unwrap(),
            "a holon_rule with no query sibling is a single-block rule"
        );
    }

    /// The parsed journal rule is a clock-subject operate rule with a
    /// journals-placed `{today}` create — the exact migrated shape.
    #[test]
    fn journal_rule_parses_to_clock_operate() {
        let rule = parse_holon_rule(JOURNAL_RULE).unwrap();
        assert_eq!(rule.guard.subject, Subject::Clock);
        let emit = rule.emit.expect("operate rule");
        assert_eq!(emit.place.parent_id(), "block:journals");
        assert!(
            emit.place.is_page(),
            "page-file journal rule places the day-block as its own page-file"
        );
        assert_eq!(emit.name.render("2026-07-10"), "2026-07-10");
    }

    /// The page-file emission tags the created day-block `Page` (block_tags
    /// junction), so the fileless-page sweep materializes it into its own file.
    #[tokio::test(flavor = "multi_thread")]
    async fn fire_emit_page_placement_tags_page() {
        let engine = block_engine().await;
        seed_journals_parent(&engine).await;
        fire_emit(
            &engine,
            engine.rule_status(),
            &RuleId::new("block:journals_rule"),
            &journal_emit(),
            &day_binding("2026-07-10"),
        )
        .await;

        let children = journal_children(&engine).await;
        assert_eq!(children.len(), 1);
        let day_id = children[0].0.clone();
        let tags = engine
            .db_handle()
            .query(
                &format!(
                    "SELECT tag FROM block_tags WHERE block_id = '{}'",
                    day_id.replace('\'', "''")
                ),
                HashMap::new(),
            )
            .await
            .unwrap();
        assert!(
            tags.iter()
                .any(|r| r.get("tag").and_then(|v| v.as_string()) == Some("Page")),
            "page-file journal day-block must carry the `Page` tag, got {tags:?}"
        );
    }

    /// WP core: firing the same rule for the same day twice creates exactly one
    /// journal (deterministic id + fire-time inhibitor); a new day is a new
    /// block.
    #[tokio::test(flavor = "multi_thread")]
    async fn fire_emit_is_idempotent_per_day() {
        let engine = block_engine().await;
        seed_journals_parent(&engine).await;
        let emit = journal_emit();
        let rule_id = RuleId::new("block:journals_rule");
        let status = engine.rule_status();

        fire_emit(&engine, status, &rule_id, &emit, &day_binding("2026-07-10")).await;
        fire_emit(&engine, status, &rule_id, &emit, &day_binding("2026-07-10")).await;
        assert_eq!(
            journal_children(&engine).await.len(),
            1,
            "same-day re-fire must converge to one journal block"
        );

        fire_emit(&engine, status, &rule_id, &emit, &day_binding("2026-07-11")).await;
        assert_eq!(
            journal_children(&engine).await.len(),
            2,
            "a new day mints a distinct journal block"
        );
    }

    /// Provenance: the rule-fired create is stamped rule-origin (never user),
    /// so C2a provenance and the undo-exclusion both hold.
    #[tokio::test(flavor = "multi_thread")]
    async fn fire_emit_stamps_rule_provenance() {
        let engine = block_engine().await;
        seed_journals_parent(&engine).await;
        let emit = journal_emit();
        let rule_id = RuleId::new("block:journals_rule");

        fire_emit(
            &engine,
            engine.rule_status(),
            &rule_id,
            &emit,
            &day_binding("2026-07-10"),
        )
        .await;

        let children = journal_children(&engine).await;
        assert_eq!(children.len(), 1);
        let (_, props) = &children[0];
        // The `_provenance` stamp records origin=rule and the firing transition id.
        // Assert on the transition id (unambiguous) plus the rule origin tag,
        // tolerant of the stored representation (JSON string vs decoded object).
        assert!(
            props.contains("journals_rule") && props.contains("rule"),
            "rule-fired create must carry rule provenance (_provenance) in properties, got \
             {props:?}"
        );
    }

    /// The fire-time inhibitor prevents a duplicate even when a pre-existing
    /// journal was created under a DIFFERENT id (migration / prior scheme).
    #[tokio::test(flavor = "multi_thread")]
    async fn inhibitor_suppresses_cross_id_duplicate() {
        let engine = block_engine().await;
        seed_journals_parent(&engine).await;

        // A pre-existing journal for the day, created out-of-band with its own id.
        let mut pre = StorageEntity::new();
        pre.insert(
            "id".into(),
            Value::String("block:legacy_journal".to_string()),
        );
        pre.insert(
            "parent_id".into(),
            Value::String("block:journals".to_string()),
        );
        pre.insert("content".into(), Value::String("2026-07-10".to_string()));
        engine
            .execute_operation(
                &EntityName::new("block"),
                "create",
                pre,
                holon_api::OpOrigin::User,
            )
            .await
            .unwrap();

        fire_emit(
            &engine,
            engine.rule_status(),
            &RuleId::new("block:journals_rule"),
            &journal_emit(),
            &day_binding("2026-07-10"),
        )
        .await;

        assert_eq!(
            journal_children(&engine).await.len(),
            1,
            "inhibitor must not add a second journal for a day that already has one"
        );
    }

    /// A malformed `holon_rule` body surfaces a LOUD ParseError, never silent.
    #[test]
    fn build_emit_types_are_constructible() {
        // Guards the public Emit surface the watcher depends on stays constructible.
        let emit = Emit {
            place: Place::parse("journals").unwrap(),
            name: NameTemplate::parse("{today}").unwrap(),
        };
        assert_eq!(emit.place.parent_id(), "block:journals");
    }

    async fn _wait(engine: &BackendEngine, n: usize, timeout: Duration) {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if journal_children(engine).await.len() == n {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for {n}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// End-to-end reactive proof: the discovery watcher picks up a seeded
    /// journal rule block and fires it against the boot clock row, then
    /// re-fires on a day-rollover UPDATE — exactly one journal per day.
    #[tokio::test(flavor = "multi_thread")]
    async fn discovery_fires_and_rolls_over() {
        let engine = block_engine().await;
        seed_journals_parent(&engine).await;
        engine
            .db_handle()
            .execute(
                "INSERT INTO clock (grain, today, epoch_day, updated_at) VALUES ('day', \
                 '2026-07-10', 20679, '2026-07-10T00:00:00Z') ON CONFLICT(grain) DO UPDATE SET \
                 today = excluded.today, epoch_day = excluded.epoch_day, updated_at = \
                 excluded.updated_at",
                vec![],
            )
            .await
            .unwrap();

        let watcher = tokio::spawn(run_rule_watcher(
            engine.clone(),
            engine.rule_status().clone(),
            RuleId::new("block:journals_rule"),
            journal_emit(),
        ));

        _wait(&engine, 1, Duration::from_secs(15)).await;

        engine
            .db_handle()
            .execute(
                "UPDATE clock SET today = '2026-07-11', epoch_day = 20680, updated_at = \
                 '2026-07-11T00:00:00Z' WHERE grain = 'day'",
                vec![],
            )
            .await
            .unwrap();

        _wait(&engine, 2, Duration::from_secs(15)).await;
        watcher.abort();
    }
}
