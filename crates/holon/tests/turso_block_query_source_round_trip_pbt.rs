//! Round-trip PBT for the Turso [`BlockQuerySource`] (ADR 0004 Phase 9).
//!
//! Locks the equivalence the read seam promises: blocks written through the
//! production path and read back via `TursoBlockQuerySource::snapshot()` must
//! reproduce the generated blocks — field-for-field (id-keyed) **and** in
//! canonical sibling order.
//!
//! ```text
//! Vec<Block> --[execute_operation("block","create")]--> Turso (block matview)
//!            <--[TursoBlockQuerySource::snapshot()]----- Turso  (CDC LiveData mirror)
//! reference BlockSnapshot  ==  turso BlockSnapshot
//! ```
//!
//! Generators + the `NormalizedDocument` comparison come from
//! `holon-block-roundtrip-testing`, shared with the org / cache round-trip
//! PBTs.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Barrier;
use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::PoisonError;
use std::time::Duration;

use holon::api::QueryEngine;
use holon::api::backend_engine::BackendEngine;
use holon::core::SqlOperationProvider;
use holon::di::test_helpers::create_test_engine_with_path;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::schema_module::SchemaModule;
use holon::sync::turso_block_query_source::BlockRowParser;
use holon::sync::turso_block_query_source::TursoBlockQuerySource;
use holon_api::BlockContent;
use holon_api::ContentType;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::SourceBlock;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::delivery_ledger::Owed;
use holon_api::repository::Lifecycle;
use holon_block_roundtrip_testing::HeadlineSpec;
use holon_block_roundtrip_testing::NormalizedDocument;
use holon_block_roundtrip_testing::assert_normalized_docs_equal;
use holon_block_roundtrip_testing::assert_sibling_order_matches;
use holon_block_roundtrip_testing::build_blocks;
use holon_block_roundtrip_testing::root_headlines_strategy;
use holon_core::OperationProvider;
use holon_core::storage::BlockQuery;
use holon_core::storage::BlockQuerySource;
use holon_core::storage::types::StorageEntity;
use holon_loro::LoroBackend;
use holon_loro::block_to_params;
use holon_loro_wiring::loro_block_query_source::LoroBlockQuerySource;
use holon_turso::schema_modules::BlockSchemaModule;
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use tokio::runtime::Runtime;
use uuid::Uuid;

fn unique_db_path() -> std::path::PathBuf {
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering;
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let id = COUNTER.fetch_add(1, Ordering::SeqCst);
    std::env::temp_dir().join(format!(
        "holon_bqs_round_trip_{}_{}.db",
        std::process::id(),
        id
    ))
}

/// Write `blocks` through the production create path, assigning each a
/// document-order `sort_key` so canonical sibling order round-trips. `blocks`
/// arrives in pre-order from `build_blocks`, so the enumeration index is a
/// monotonic per-parent order key.
async fn write_blocks(
    engine: &BackendEngine,
    doc_id: &EntityUri,
    blocks: &[Block],
) -> Result<(), TestCaseError> {
    // Write through the production block provider directly on the engine's
    // handle (the core test engine doesn't auto-register a "block" provider);
    // the matview projects incrementally, so the engine's `watch_view` sees the
    // rows. Edge fields fan `tags`/`requires` into the junction tables.
    let provider = SqlOperationProvider::with_edge_fields(
        engine.db_handle().clone(),
        BLOCK_WRITE_TABLE.to_string(),
        "block".to_string(),
        "block".to_string(),
        BlockSchemaModule.edge_fields(),
        std::sync::Arc::new(holon_api::SystemClock),
    );
    let entity: EntityName = "block".to_string().into();
    // Seed the document root the generated roots hang under (their `parent_id`
    // is `doc_id`), mirroring `seed_loro_backend`. Its own parent is the
    // `sentinel:no_parent` root anchor. Without it the generated roots would
    // reference a nonexistent parent and the block_raw parent FK rejects them.
    let mut root_params = StorageEntity::new();
    root_params.insert("id".into(), Value::String(doc_id.as_str().to_string()));
    root_params.insert(
        "parent_id".into(),
        Value::String(EntityUri::no_parent().as_str().to_string()),
    );
    root_params.insert("content".into(), Value::String("doc".to_string()));
    root_params.insert("sort_key".into(), Value::String("0000000000".to_string()));
    provider
        .execute_operation(&entity, "create", root_params)
        .await
        .map_err(|e| TestCaseError::fail(format!("create doc root {doc_id}: {e}")))?;
    for (i, b) in blocks.iter().enumerate() {
        let params = block_to_params(&holon::api::SnapshotBlock {
            block: b.clone(),
            sort_key: format!("{i:010}"),
        });
        provider
            .execute_operation(&entity, "create", params)
            .await
            .map_err(|e| TestCaseError::fail(format!("create {}: {e}", b.id)))?;
    }
    Ok(())
}

/// The one production engine of this test binary, booted once and shared by
/// EVERY case of BOTH properties. A `BackendEngine` boot costs ~2.3 s of CPU in
/// a debug build (full schema DDL parse, FTS index, script engine), which at 20
/// cases per property was the whole runtime of the test.
///
/// It is one store, so only [`exclusive_case`] may hand it out: cases are
/// isolated by running one at a time and purging the case's document at the
/// end, which leaves every case a store holding exactly its own blocks.
fn shared_engine() -> &'static (Runtime, Arc<BackendEngine>) {
    static SHARED: OnceLock<(Runtime, Arc<BackendEngine>)> = OnceLock::new();
    SHARED.get_or_init(|| {
        let rt = Runtime::new().unwrap();
        let engine = rt.block_on(async {
            create_test_engine_with_path(unique_db_path())
                .await
                .expect("engine init")
        });
        (rt, engine)
    })
}

/// Run one case body with exclusive use of [`shared_engine`]. `cargo test` runs
/// the two properties on two threads of one process, where without this lock
/// each case's `snapshot()` reads the other property's blocks too; nextest runs
/// each property in its own process and never did.
fn exclusive_case<T>(case: impl FnOnce(&Runtime, &BackendEngine) -> T) -> T {
    static CASE_LOCK: Mutex<()> = Mutex::new(());

    let (rt, engine) = shared_engine();
    // Keep a poisoning case's real counterexample, rather than reporting the
    // poison of every case after it.
    let _held = CASE_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
    case(rt, engine)
}

/// Purge `doc_id` and, by cascade, the case's whole generated subtree, so the
/// next case sees an empty store. Without it the store would keep every earlier
/// case's blocks, and the generator's ids repeat as proptest shrinks: the
/// second write of an id another entity still holds is refused by the identity
/// policy, which would replace the counterexample with a collision error.
async fn purge_document(engine: &BackendEngine, doc_id: &EntityUri) -> Result<(), TestCaseError> {
    let provider = SqlOperationProvider::with_edge_fields(
        engine.db_handle().clone(),
        BLOCK_WRITE_TABLE.to_string(),
        "block".to_string(),
        "block".to_string(),
        BlockSchemaModule.edge_fields(),
        std::sync::Arc::new(holon_api::SystemClock),
    );
    let mut params = StorageEntity::new();
    params.insert("id".into(), Value::String(doc_id.as_str().to_string()));
    provider
        .execute_operation(&"block".to_string().into(), "purge", params)
        .await
        .map_err(|e| TestCaseError::fail(format!("purge doc {doc_id}: {e}")))?;
    Ok(())
}

/// One case of [`block_round_trips_through_turso_query_source`]: write the
/// generated blocks into `engine`, read them back through the query source,
/// compare, then purge the case's document.
fn turso_round_trip_case(
    rt: &Runtime,
    engine: &BackendEngine,
    headlines: &[HeadlineSpec],
) -> Result<(), TestCaseError> {
    rt.block_on(async {
        let doc_id = EntityUri::block(&Uuid::new_v4().to_string());
        let blocks = build_blocks(&doc_id, headlines);

        let outcome = async {
            write_blocks(engine, &doc_id, &blocks).await?;

            let source = TursoBlockQuerySource::watch_default(engine)
                .await
                .map_err(|e| TestCaseError::fail(format!("watch: {e}")))?;
            let snapshot = source
                .snapshot()
                .await
                .map_err(|e| TestCaseError::fail(format!("snapshot: {e}")))?;

            // Drop the physical doc root; the reference store has no doc block.
            let actual_blocks: Vec<Block> = snapshot
                .iter_blocks()
                .filter(|b| b.id != doc_id)
                .cloned()
                .collect();
            let expected = NormalizedDocument::from_blocks(None, &blocks);
            let actual = NormalizedDocument::from_blocks(None, &actual_blocks);
            assert_normalized_docs_equal(&expected, &actual, "turso_block_query_round_trip")?;
            assert_sibling_order_matches(&blocks, &snapshot, "turso_block_query_round_trip")?;

            Ok(())
        }
        .await;

        outcome.and(purge_document(engine, &doc_id).await)
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 20,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// Blocks round-trip through `TursoBlockQuerySource::snapshot()` —
    /// fields (id-keyed) and per-parent sibling order.
    #[test]
    fn block_round_trips_through_turso_query_source(headlines in root_headlines_strategy()) {
        exclusive_case(|rt, engine| turso_round_trip_case(rt, engine, &headlines))?;
    }
}

/// Seed a fresh, Turso-free [`LoroBackend`] with `blocks` (pre-order),
/// preserving each block's id, content, source fields, properties (which carry
/// `level`, `sequence`, and `_source_header_args`), tags, and requires — the
/// same fields `block_to_params` fans into Turso. `doc_id` is materialized as a
/// physical root node so the generated roots (whose `parent_id` is `doc_id`)
/// resolve to a tree parent and read back with `parent_id == doc_id`, matching
/// the Turso arm.
async fn seed_loro_backend(
    doc_id: &EntityUri,
    blocks: &[Block],
) -> Result<Arc<LoroBackend>, TestCaseError> {
    let backend = LoroBackend::create_new(
        "bqs-equivalence".to_string(),
        std::sync::Arc::new(holon_api::SystemClock),
    )
    .await
    .map_err(|e| TestCaseError::fail(format!("loro create_new: {e}")))?;
    backend
        .create_block_with_properties(
            EntityUri::no_parent(),
            BlockContent::text("doc"),
            Some(doc_id.clone()),
            &HashMap::new(),
            &holon_api::BlockEdges::default(),
        )
        .await
        .map_err(|e| TestCaseError::fail(format!("loro seed doc root: {e}")))?;
    for b in blocks {
        // `to_block_content` drops source `header_args`; rebuild them from the
        // block so the Loro content meta agrees with the `_source_header_args`
        // property carried in `properties_map`.
        let content = match b.content_type {
            ContentType::Source => BlockContent::Source(SourceBlock {
                language: b.source_language.as_ref().map(|l| l.to_string()),
                source: b.content.clone(),
                name: b.source_name.clone(),
                header_args: b.get_source_header_args(),
            }),
            _ => b.to_block_content(),
        };
        backend
            .create_block_with_properties(
                b.parent_id.clone(),
                content,
                Some(b.id.clone()),
                &b.properties_map(),
                &holon_api::BlockEdges::of(b),
            )
            .await
            .map_err(|e| TestCaseError::fail(format!("loro seed {}: {e}", b.id)))?;
    }
    Ok(Arc::new(backend))
}

/// One case of [`loro_and_turso_query_sources_agree`]: seed both arms with the
/// same generated store, compare them, then purge the case's document from
/// `engine`.
fn query_sources_agree_case(
    rt: &Runtime,
    engine: &BackendEngine,
    headlines: &[HeadlineSpec],
) -> Result<(), TestCaseError> {
    rt.block_on(async {
        let doc_id = EntityUri::block(&Uuid::new_v4().to_string());
        let blocks = build_blocks(&doc_id, headlines);

        let outcome = async {
            // Turso arm: production create path → matview → CDC mirror read.
            write_blocks(engine, &doc_id, &blocks).await?;
            let turso_source = TursoBlockQuerySource::watch_default(engine)
                .await
                .map_err(|e| TestCaseError::fail(format!("turso watch: {e}")))?;
            let turso_snapshot = turso_source
                .snapshot()
                .await
                .map_err(|e| TestCaseError::fail(format!("turso snapshot: {e}")))?;
            // Drop the physical doc root; the reference store has no doc block.
            let turso_blocks: Vec<Block> = turso_snapshot
                .iter_blocks()
                .filter(|b| b.id != doc_id)
                .cloned()
                .collect();
            let turso_doc = NormalizedDocument::from_blocks(None, &turso_blocks);

            // Loro arm: Turso-free tree walk over the same generated store.
            let backend = seed_loro_backend(&doc_id, &blocks).await?;
            let loro_snapshot = LoroBlockQuerySource::new(backend)
                .snapshot()
                .await
                .map_err(|e| TestCaseError::fail(format!("loro snapshot: {e}")))?;
            // Drop the physical doc root; the reference store has no doc block.
            let loro_blocks: Vec<Block> = loro_snapshot
                .iter_blocks()
                .filter(|b| b.id != doc_id)
                .cloned()
                .collect();
            let loro_doc = NormalizedDocument::from_blocks(None, &loro_blocks);

            let expected = NormalizedDocument::from_blocks(None, &blocks);
            // Each arm reproduces the generated store, hence agree with each
            // other.
            assert_normalized_docs_equal(&expected, &turso_doc, "query_source_equivalence[turso]")?;
            assert_normalized_docs_equal(&expected, &loro_doc, "query_source_equivalence[loro]")?;
            assert_normalized_docs_equal(
                &turso_doc,
                &loro_doc,
                "query_source_equivalence[loro==turso]",
            )?;

            // Per-parent sibling order under each generated parent (`doc_id` →
            // roots included); the Loro doc root lives under `no_parent`, which
            // is not a generated parent, so it is not inspected here.
            assert_sibling_order_matches(
                &blocks,
                &turso_snapshot,
                "query_source_equivalence[turso]",
            )?;
            assert_sibling_order_matches(
                &blocks,
                &loro_snapshot,
                "query_source_equivalence[loro]",
            )?;

            Ok(())
        }
        .await;

        outcome.and(purge_document(engine, &doc_id).await)
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 20,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// H10 (BlockEventStorm) — query-source equivalence: for the same generated
    /// store, the Turso and Loro `BlockQuerySource` arms return equal blocks.
    ///
    /// Compares BLOCKS only (id-keyed fields + per-parent sibling order). The
    /// Loro arm returns empty `focus_roots` by disclosed design (navigation focus
    /// is a Turso matview with no Loro-native source, see
    /// `loro_block_query_source.rs`), so `focus_roots` is a recorded, disclosed
    /// asymmetry and is out of scope for this equivalence.
    #[test]
    fn loro_and_turso_query_sources_agree(headlines in root_headlines_strategy()) {
        exclusive_case(|rt, engine| query_sources_agree_case(rt, engine, &headlines))?;
    }
}

/// `cargo test` runs the two properties on two threads of ONE process, so both
/// case bodies meet on the one store behind [`exclusive_case`]; nextest gives
/// each property its own process and never does. Each body must still read back
/// only its own blocks.
#[test]
fn shared_engine_isolates_the_two_properties() {
    // Cases per thread — one pair can miss each other, several cannot.
    const ROUNDS: usize = 3;

    let mut runner = TestRunner::deterministic();
    let mut draw = || {
        root_headlines_strategy()
            .new_tree(&mut runner)
            .expect("draw headlines")
            .current()
    };
    let round_trip_headlines = draw();
    let agree_headlines = draw();

    let start = Barrier::new(2);
    std::thread::scope(|scope| {
        let round_trip = scope.spawn(|| {
            start.wait();
            (0..ROUNDS).try_for_each(|_| {
                exclusive_case(|rt, engine| {
                    turso_round_trip_case(rt, engine, &round_trip_headlines)
                })
            })
        });
        let agree = scope.spawn(|| {
            start.wait();
            (0..ROUNDS).try_for_each(|_| {
                exclusive_case(|rt, engine| query_sources_agree_case(rt, engine, &agree_headlines))
            })
        });
        round_trip
            .join()
            .expect("round-trip thread")
            .expect("round-trip case reads only its own blocks");
        agree
            .join()
            .expect("agreement thread")
            .expect("agreement case reads only its own blocks");
    });
}

/// How long the slow mirror takes to apply one row: far beyond the quiet
/// window `snapshot()` waits for, so a read that trusts quiet reads stale.
const SLOW_APPLY: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Apply {
    Open,
    Slow,
    Held,
}

/// Switches how fast the block mirror applies, by gating its injected parser.
struct ApplyGate {
    mode: Mutex<Apply>,
    changed: Condvar,
}

impl ApplyGate {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            mode: Mutex::new(Apply::Open),
            changed: Condvar::new(),
        })
    }

    fn set(&self, mode: Apply) {
        *self.mode.lock().unwrap() = mode;
        self.changed.notify_all();
    }

    fn pass(&self) {
        let mut mode = self.mode.lock().unwrap();
        while *mode == Apply::Held {
            mode = self.changed.wait(mode).unwrap();
        }
        if *mode == Apply::Slow {
            drop(mode);
            std::thread::sleep(SLOW_APPLY);
        }
    }

    fn parser(self: &Arc<Self>) -> BlockRowParser {
        let gate = Arc::clone(self);
        Arc::new(move |row| {
            gate.pass();
            Block::try_from(row.clone())
        })
    }
}

/// The engine settle: poll what the engine still owes until nothing is owed on
/// two reads in a row. Returns the reads it took, or the owed stages when
/// `hang_after` passes first.
async fn settle(engine: &BackendEngine, hang_after: Duration) -> Result<u32, Vec<Owed>> {
    let deadline = tokio::time::Instant::now() + hang_after;
    let mut reads = 0;
    let mut clear_in_a_row = 0;
    loop {
        reads += 1;
        let owed = engine.delivery_owed().await.expect("read delivery_owed");
        if owed.is_empty() {
            clear_in_a_row += 1;
            if clear_in_a_row == 2 {
                return Ok(reads);
            }
        } else {
            clear_in_a_row = 0;
            if tokio::time::Instant::now() >= deadline {
                return Err(owed);
            }
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
}

/// One write of the case. `Ui` and `Sql` are the app and raw-SQL writes of the
/// org-suite settle PBT; its external org edit needs the org ingest, which
/// this crate does not run. A target index picks a live block modulo the
/// live count; with no live block the write creates one instead.
#[derive(Clone, Debug)]
enum Write {
    Create(String),
    Ui(usize, String),
    Sql(usize, String),
    Delete(usize),
    Navigate(usize),
}

fn write() -> impl Strategy<Value = Write> {
    prop_oneof![
        "[a-z]{3,10}".prop_map(Write::Create),
        (any::<usize>(), "[a-z]{3,10}").prop_map(|(i, c)| Write::Ui(i, c)),
        (any::<usize>(), "[a-z]{3,10}").prop_map(|(i, c)| Write::Sql(i, c)),
        any::<usize>().prop_map(Write::Delete),
        any::<usize>().prop_map(Write::Navigate),
    ]
}

/// What the case wrote: live blocks in creation order, and the focus roots it
/// opened in its own region.
struct WriteModel {
    doc_id: EntityUri,
    region: String,
    blocks: Vec<(EntityUri, String)>,
    roots: std::collections::BTreeSet<String>,
}

async fn apply_write(
    engine: &BackendEngine,
    model: &mut WriteModel,
    write: &Write,
) -> Result<(), TestCaseError> {
    let provider = SqlOperationProvider::with_edge_fields(
        engine.db_handle().clone(),
        BLOCK_WRITE_TABLE.to_string(),
        "block".to_string(),
        "block".to_string(),
        BlockSchemaModule.edge_fields(),
        std::sync::Arc::new(holon_api::SystemClock),
    );
    let entity: EntityName = "block".to_string().into();
    let live = model.blocks.len();
    match write {
        Write::Ui(i, content) if live > 0 => {
            let id = model.blocks[i % live].0.clone();
            let mut params = StorageEntity::new();
            params.insert("id".into(), Value::String(id.as_str().to_string()));
            params.insert("content".into(), Value::String(content.clone()));
            provider
                .execute_operation(&entity, "update", params)
                .await
                .map_err(failed(format!("update {id}")))?;
            model.blocks[i % live].1 = content.clone();
        }
        Write::Sql(i, content) if live > 0 => {
            let id = model.blocks[i % live].0.clone();
            engine
                .db_handle()
                .execute(
                    &format!("UPDATE {BLOCK_WRITE_TABLE} SET content = ? WHERE id = ?"),
                    vec![
                        turso::Value::Text(content.clone()),
                        turso::Value::Text(id.as_str().to_string()),
                    ],
                )
                .await
                .map_err(failed(format!("raw update {id}")))?;
            model.blocks[i % live].1 = content.clone();
        }
        Write::Delete(i) if live > 0 => {
            let (id, _) = model.blocks.remove(i % live);
            let mut params = StorageEntity::new();
            params.insert("id".into(), Value::String(id.as_str().to_string()));
            provider
                .execute_operation(&entity, "delete", params)
                .await
                .map_err(failed(format!("delete {id}")))?;
        }
        Write::Navigate(i) => {
            let root = if live > 0 {
                model.blocks[i % live].0.clone()
            } else {
                model.doc_id.clone()
            };
            engine
                .db_handle()
                .execute(
                    "INSERT INTO navigation_history (region, block_id) VALUES (?, ?)",
                    vec![
                        turso::Value::Text(model.region.clone()),
                        turso::Value::Text(root.as_str().to_string()),
                    ],
                )
                .await
                .map_err(failed(format!("navigate to {root}")))?;
            model.roots.insert(root.as_str().to_string());
        }
        Write::Create(content) | Write::Ui(_, content) | Write::Sql(_, content) => {
            create_child(&provider, model, content).await?;
        }
        Write::Delete(_) => create_child(&provider, model, "recreated").await?,
    }
    Ok(())
}

fn failed<E: std::fmt::Display>(what: String) -> impl FnOnce(E) -> TestCaseError {
    move |e| TestCaseError::fail(format!("{what}: {e}"))
}

async fn create_child(
    provider: &SqlOperationProvider,
    model: &mut WriteModel,
    content: &str,
) -> Result<(), TestCaseError> {
    let id = EntityUri::block(&Uuid::new_v4().to_string());
    let mut params = StorageEntity::new();
    params.insert("id".into(), Value::String(id.as_str().to_string()));
    params.insert(
        "parent_id".into(),
        Value::String(model.doc_id.as_str().to_string()),
    );
    params.insert("content".into(), Value::String(content.to_string()));
    params.insert(
        "sort_key".into(),
        Value::String(format!("{:010}", model.blocks.len() + 1)),
    );
    provider
        .execute_operation(&"block".to_string().into(), "create", params)
        .await
        .map_err(|e| TestCaseError::fail(format!("create {id}: {e}")))?;
    model.blocks.push((id, content.to_string()));
    Ok(())
}

/// One case of [`settled_snapshot_reads_every_write`]: with a block mirror
/// that applies far slower than the snapshot's quiet window, each write,
/// then a settle, then ONE snapshot read must show every write.
fn settled_snapshot_case(
    rt: &Runtime,
    engine: &BackendEngine,
    writes: &[Write],
) -> Result<(), TestCaseError> {
    rt.block_on(async {
        let doc_id = EntityUri::block(&Uuid::new_v4().to_string());
        let gate = ApplyGate::new();

        let outcome = async {
            write_blocks(engine, &doc_id, &[]).await?;
            let source = TursoBlockQuerySource::watch(engine, gate.parser())
                .await
                .map_err(|e| TestCaseError::fail(format!("watch: {e}")))?;
            let hang_after = TursoBlockQuerySource::DEFAULT_SETTLE_TIMEOUT;
            settle(engine, hang_after)
                .await
                .map_err(|owed| TestCaseError::fail(format!("settle hung: {owed:?}")))?;

            gate.set(Apply::Slow);
            let mut model = WriteModel {
                region: format!("settle-probe-{}", Uuid::new_v4()),
                doc_id: doc_id.clone(),
                blocks: Vec::new(),
                roots: Default::default(),
            };
            for (i, w) in writes.iter().enumerate() {
                apply_write(engine, &mut model, w).await?;
                if i % 2 == 1 {
                    settle(engine, hang_after)
                        .await
                        .map_err(|owed| TestCaseError::fail(format!("settle hung: {owed:?}")))?;
                }
            }
            let settled = settle(engine, hang_after).await;
            let snapshot = source.snapshot().await;
            gate.set(Apply::Open);
            settled.map_err(|owed| TestCaseError::fail(format!("settle hung: {owed:?}")))?;
            let snapshot = snapshot.map_err(|e| TestCaseError::fail(format!("snapshot: {e}")))?;

            let expected: HashMap<String, String> = model
                .blocks
                .iter()
                .map(|(id, content)| (id.as_str().to_string(), content.clone()))
                .collect();
            let actual: HashMap<String, String> = snapshot
                .iter_blocks()
                .filter(|b| b.parent_id == doc_id)
                .map(|b| (b.id.as_str().to_string(), b.content.clone()))
                .collect();
            prop_assert_eq!(actual, expected, "blocks under the case document");
            let roots: std::collections::BTreeSet<String> = snapshot
                .focus_roots()
                .into_iter()
                .filter(|r| r.region == model.region)
                .map(|r| r.root_id)
                .collect();
            prop_assert_eq!(roots, model.roots, "focus roots of the case region");

            let reads = settle(engine, hang_after)
                .await
                .map_err(|owed| TestCaseError::fail(format!("second settle hung: {owed:?}")))?;
            prop_assert_eq!(reads, 2, "a settle with no write since the last one");
            Ok(())
        }
        .await;

        gate.set(Apply::Open);
        outcome.and(purge_document(engine, &doc_id).await)
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 6,
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    /// A settle waits for the block-query mirrors to apply, not for a quiet
    /// period: the ONE snapshot read after it shows every write, and a second
    /// settle with no write between returns at once.
    #[test]
    fn settled_snapshot_reads_every_write(writes in prop::collection::vec(write(), 1..5)) {
        exclusive_case(|rt, engine| settled_snapshot_case(rt, engine, &writes))?;
    }
}

/// A mirror that never applies makes the settle fail, naming the stage and
/// the seq the mirror applied last. An example, not a property: the input is
/// one state, a held mirror with one write pending.
#[test]
fn settle_names_a_mirror_that_never_applies() {
    exclusive_case(|rt, engine| {
        rt.block_on(async {
            let doc_id = EntityUri::block(&Uuid::new_v4().to_string());
            let gate = ApplyGate::new();
            write_blocks(engine, &doc_id, &[]).await.expect("doc root");
            let _source = TursoBlockQuerySource::watch(engine, gate.parser())
                .await
                .expect("watch");
            let hang_after = TursoBlockQuerySource::DEFAULT_SETTLE_TIMEOUT;
            settle(engine, hang_after)
                .await
                .expect("settle before the hold");

            gate.set(Apply::Held);
            let mut model = WriteModel {
                region: format!("settle-probe-{}", Uuid::new_v4()),
                doc_id: doc_id.clone(),
                blocks: Vec::new(),
                roots: Default::default(),
            };
            apply_write(engine, &mut model, &Write::Create("held".into()))
                .await
                .expect("write");
            let owed = settle(engine, hang_after).await;
            gate.set(Apply::Open);
            let owed = owed.expect_err("a held mirror must keep the settle owing");
            assert!(
                owed.iter().any(|o| matches!(o, Owed::Applied { .. })),
                "owed stages name the mirror's stage and applied seq: {owed:?}"
            );
            settle(engine, hang_after)
                .await
                .expect("settle after release");
            purge_document(engine, &doc_id).await.expect("purge");
        })
    });
}
