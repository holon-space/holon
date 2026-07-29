//! A Loro-only (`StorageSelector::LoroMemory`) session must carry the same
//! entity lookups the Turso session's DI wiring registers (`query_source`,
//! `rule_sibling`).
//!
//! Without them the bundled `block` profile's lookup-dependent computed fields
//! (`has_query_source`, `is_program`) evaluate against a Rhai engine that has
//! no such function — every one of them lands at `Null`, so a no-Turso session
//! silently loses the query-page and rule-machinery routing the Turso session
//! gets.
//!
//! @pbt kind harness
//! @pbt covers loro-live-entity-wiring — the Turso-free profile resolver
//! carries the entity lookups, refreshed from the block source

#![cfg(feature = "pbt")]

use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::SourceLanguage;
use holon_api::Value;
use holon_api::block::Block;
use holon_core::storage::BlockQuerySource;
use holon_core::storage::BlockSnapshot;
use holon_core::storage::FocusRoot;
use holon_core::storage::from_sync;
use holon_frontend::FrontendSession;
use holon_integration_tests::pbt::reference_state::block_to_data_row;

/// A block source reading a mutable block set, so a test can graft a block
/// after the session booted (what an edit does in a live Loro session).
type Blocks = Arc<Mutex<Vec<Block>>>;

fn source_over(blocks: Blocks) -> Arc<dyn BlockQuerySource> {
    Arc::new(from_sync(move || {
        Ok(BlockSnapshot::from_ordered(
            blocks.lock().unwrap().clone(),
            Vec::<FocusRoot>::new(),
        ))
    })) as Arc<dyn BlockQuerySource>
}

fn prql() -> String {
    SourceLanguage::Query(QueryLanguage::HolonPrql).to_string()
}

fn page(id: &str) -> Block {
    Block::new_text(EntityUri::block(id), EntityUri::no_parent(), "heading")
}

fn source_child(id: &str, parent: &str, language: &str) -> Block {
    Block::new_source(
        EntityUri::block(id),
        EntityUri::block(parent),
        language,
        "from block",
    )
}

fn boot(blocks: &Blocks) -> FrontendSession {
    holon_app::from_block_query_source(source_over(Arc::clone(blocks)), None)
}

fn computed(session: &FrontendSession, block: &Block) -> Option<Value> {
    session
        .profiles()
        .resolve_computed_only(&block_to_data_row(block))
        .get("has_query_source")
        .cloned()
}

/// Await a computed field reaching `expected` — the Turso-free resolver reads
/// its entities from block-source snapshots on a poll, so a just-grafted block
/// becomes visible within a poll interval, not instantly.
fn await_field(session: &FrontendSession, block: &Block, field: &str, expected: &Value) -> Value {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let value = session
            .profiles()
            .resolve_computed_only(&block_to_data_row(block))
            .get(field)
            .cloned()
            .unwrap_or(Value::Null);
        if &value == expected || Instant::now() >= deadline {
            return value;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `has_query_source` = "owns a query-source child, and is not rule
/// machinery" — a `query_source(id)` lookup the no-Turso resolver must answer.
#[tokio::test(flavor = "multi_thread")]
async fn loro_session_computes_has_query_source() {
    let owner = page("loro-qs-owner");
    let blocks: Blocks = Arc::new(Mutex::new(vec![
        owner.clone(),
        source_child("loro-qs-owner-src", "loro-qs-owner", &prql()),
    ]));
    let session = boot(&blocks);

    assert_eq!(
        await_field(&session, &owner, "has_query_source", &Value::Boolean(true)),
        Value::Boolean(true),
        "a Loro-only session must see the query-source child through the \
         `query_source` lookup"
    );
}

/// `is_program` clause (b): a source block whose parent owns a rule head is the
/// rule's trigger sibling — a `rule_sibling(parent_id)` lookup.
#[tokio::test(flavor = "multi_thread")]
async fn loro_session_computes_is_program_for_rule_trigger_sibling() {
    let trigger = source_child("loro-rule-trigger", "loro-rule-owner", &prql());
    let blocks: Blocks = Arc::new(Mutex::new(vec![
        page("loro-rule-owner"),
        source_child("loro-rule-head", "loro-rule-owner", "holon_rule"),
        trigger.clone(),
    ]));
    let session = boot(&blocks);

    assert_eq!(
        await_field(&session, &trigger, "is_program", &Value::Boolean(true)),
        Value::Boolean(true),
        "the trigger sibling of a rule head is program machinery in a \
         Loro-only session too"
    );
}

/// The lookups track the live block set: grafting a query source under a page
/// after boot must flip the field, or the answer is frozen at session start —
/// which is always "no content yet".
#[tokio::test(flavor = "multi_thread")]
async fn loro_session_lookups_track_grafted_query_source() {
    let owner = page("loro-qs-late");
    let blocks: Blocks = Arc::new(Mutex::new(vec![owner.clone()]));
    let session = boot(&blocks);

    assert_eq!(
        await_field(&session, &owner, "has_query_source", &Value::Boolean(false)),
        Value::Boolean(false),
        "before the graft the page owns no query source"
    );

    blocks
        .lock()
        .unwrap()
        .push(source_child("loro-qs-late-src", "loro-qs-late", &prql()));

    assert_eq!(
        await_field(&session, &owner, "has_query_source", &Value::Boolean(true)),
        Value::Boolean(true),
        "the grafted query source must reach the lookup"
    );
}

/// `content_type` is part of what the lookups read: a block that stops being a
/// source block stops populating the entity, even though its id, parent and
/// language never change. A refresh keyed on the language triple alone would
/// hold the stale `true` until some unrelated edit happened to move the key.
#[tokio::test(flavor = "multi_thread")]
async fn loro_session_lookups_track_content_type_flip() {
    let owner = page("loro-qs-flip");
    let blocks: Blocks = Arc::new(Mutex::new(vec![
        owner.clone(),
        source_child("loro-qs-flip-src", "loro-qs-flip", &prql()),
    ]));
    let session = boot(&blocks);

    assert_eq!(
        await_field(&session, &owner, "has_query_source", &Value::Boolean(true)),
        Value::Boolean(true),
        "the query-source child is live before the flip"
    );

    // Same id, same parent, same source_language — only the content type moves.
    blocks.lock().unwrap()[1].content_type = ContentType::Text;

    assert_eq!(
        await_field(&session, &owner, "has_query_source", &Value::Boolean(false)),
        Value::Boolean(false),
        "a block that is no longer a source block must leave the entity"
    );
}

/// Guards the assertion above from passing vacuously: `computed` reads the
/// field the profile actually declares.
#[tokio::test(flavor = "multi_thread")]
async fn computed_field_is_present_at_all() {
    let owner = page("loro-qs-present");
    let blocks: Blocks = Arc::new(Mutex::new(vec![owner.clone()]));
    let session = boot(&blocks);
    assert!(
        computed(&session, &owner).is_some(),
        "the bundled block profile declares `has_query_source`"
    );
}
