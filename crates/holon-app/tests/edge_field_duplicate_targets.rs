//! A block carrying the SAME edge target twice must still reach the store.
//!
//! The junction tables key on `(source, target)` (`block_requires.sql`,
//! `block_tags.sql`, …), so a target set is a SET. The write path is not:
//! `SqlOperationProvider::edge_field_replace_sql` emits one plain `INSERT`
//! per element of the params array, so a repeated target raises a primary-key
//! violation that fails the WHOLE block write. In the outbound Loro→SQL
//! reconcile that write is retried against the same unchanged source, so it
//! fails again — the block never lands and the pipeline never converges
//! (silent success upstream, stranded block, wedged pipeline).
//!
//! Live evidence (Martin's vault, 2026-08-29): four blocks in
//! `Projects/Holon/Dogfooding & Agents.org` came back from a Holon write-back
//! with their `:REQUIRES:` target written twice (e.g.
//! `:REQUIRES: handoff-md-migration handoff-md-migration`), so `block.requires`
//! genuinely reaches the write path as a multiset.
//!
//! The parse-side fold is PER FIELD, not universal: `requires` is folded
//! (`holon-org-format/src/parser.rs:948`, `resolve_dependency_edge`), while
//! `contributes_to` (`edge_ids`, `parser.rs:1507`) and `advice_suppressed`
//! (`parser.rs:977`) collect their drawers as multisets. So for those two an
//! authored doubled drawer reaches the junction directly, and for all three
//! any non-org producer — the Loro meta reader, `set_field` over MCP, a
//! hand-built `Block` — does. What every one of them shares is the canonical
//! param builder `EdgeField::param_value`, which is what this test pins, on
//! BOTH production write legs.
//!
//! Recorded as bug-funnel entry
//! `2026-08-30-edge-field-duplicate-target-wedges-write`.

use std::sync::Arc;

use holon::core::SqlOperationProvider;
use holon::core::queryable_cache::QueryableCache;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_app::turso_seams::CacheBlockReader;
use holon_core::OperationProvider;
use holon_filesystem::BlockReader;
use holon_loro::block_to_params;

/// Which production param builder packs a block for the write — the same
/// two legs `org_store_org_round_trip.rs` distinguishes.
#[derive(Clone, Copy)]
enum WriteLeg {
    /// The Loro projection writer.
    Loro,
    /// The file-ingest builder `FileSyncController` calls per parsed block.
    OrgIngest,
}

impl WriteLeg {
    fn name(self) -> &'static str {
        match self {
            Self::Loro => "loro",
            Self::OrgIngest => "org-ingest",
        }
    }
}

async fn setup_production_schema(handle: &holon::storage::turso::DbHandle) {
    use holon_turso::schema_modules::BlockMatviewSchemaModule;
    use holon_turso::schema_modules::BlockSchemaModule;
    use holon_turso::schema_modules::CoreSchemaModule;
    use holon_turso::schema_modules::LinkSchemaModule;

    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("FKs");
    CoreSchemaModule
        .ensure_schema(handle)
        .await
        .expect("CoreSchemaModule");
    BlockSchemaModule
        .ensure_schema(handle)
        .await
        .expect("BlockSchemaModule");
    BlockMatviewSchemaModule
        .ensure_schema(handle)
        .await
        .expect("BlockMatviewSchemaModule");
    LinkSchemaModule
        .ensure_schema(handle)
        .await
        .expect("LinkSchemaModule");
}

/// The doc root every fixture block hangs under.
fn doc() -> Block {
    Block::new_text(
        EntityUri::block("dupe-page"),
        EntityUri::no_parent(),
        "Duplicate edge targets",
    )
}

/// One headline whose every edge field names the same target twice — the
/// shape a Loro read, an MCP `set_field`, or a stale in-memory block can
/// hand to the write path.
fn anchor() -> Block {
    let mut b = Block::new_text(EntityUri::block("dupe-anchor"), doc().id, "Anchor");
    b.tags = vec!["agent".to_string(), "agent".to_string()].into();
    b.requires = vec![EntityUri::block("dep"), EntityUri::block("dep")];
    b.advice_suppressed = vec![EntityUri::block("lesson"), EntityUri::block("lesson")];
    b.contributes_to = vec![EntityUri::block("goal"), EntityUri::block("goal")];
    b
}

/// Write `doc` + `blocks` through `leg`'s production param builder and read
/// the document back through the production read path. Write errors are
/// returned rather than panicked on — a failed write IS the defect, and the
/// caller reports it with the leg name attached.
async fn through_the_store(
    doc: &Block,
    blocks: &[Block],
    leg: WriteLeg,
) -> Result<Vec<Block>, String> {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("turso must start in memory");
    setup_production_schema(&handle).await;

    let provider = Arc::new(SqlOperationProvider::with_edge_fields(
        handle.clone(),
        BLOCK_WRITE_TABLE.to_string(),
        "block".to_string(),
        "block".to_string(),
        holon_turso::schema_modules::BlockSchemaModule.edge_fields(),
    ));
    let entity: EntityName = "block".to_string().into();

    for (i, block) in std::iter::once(doc).chain(blocks.iter()).enumerate() {
        let sort_key = format!("{i:010}");
        let params = match leg {
            WriteLeg::Loro => block_to_params(&holon::api::SnapshotBlock {
                block: block.clone(),
                sort_key,
            }),
            WriteLeg::OrgIngest => {
                let mut params =
                    holon_orgmode::build_block_params(block, &block.parent_id, &doc.id, None);
                params.insert("sort_key".into(), holon_api::Value::String(sort_key));
                params
            }
        };
        provider
            .execute_operation(&entity, "create", params)
            .await
            .map_err(|e| format!("create {} on the {} leg: {e}", block.id, leg.name()))?;
    }

    let cache: Arc<QueryableCache<Block>> = Arc::new(
        QueryableCache::<Block>::new(handle.clone(), Block::type_definition())
            .await
            .expect("block cache"),
    );
    let reader: Arc<dyn BlockReader> = Arc::new(CacheBlockReader::new(cache));
    reader
        .get_blocks(&doc.id)
        .await
        .map_err(|e| format!("get_blocks on the {} leg: {e}", leg.name()))
}

/// The write must SUCCEED and every junction must hold the target exactly
/// once — the set the schema's primary key already says it is.
///
/// Asserted on both write legs because the two param builders are separate
/// code paths into the same junctions; a fold in only one of them leaves the
/// other wedging on the same vault content.
#[tokio::test(flavor = "multi_thread")]
async fn duplicate_edge_targets_collapse_to_one_junction_row() {
    for leg in [WriteLeg::OrgIngest, WriteLeg::Loro] {
        let doc = doc();
        let restored = through_the_store(&doc, &[anchor()], leg)
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "a repeated edge target must not fail the block write — the junction key \
                     already makes the target set a SET: {e}"
                )
            });

        let a = restored
            .iter()
            .find(|b| b.id == EntityUri::block("dupe-anchor"))
            .unwrap_or_else(|| panic!("leg {}: the anchor must come back", leg.name()));

        assert_eq!(
            a.requires,
            vec![EntityUri::block("dep")],
            "leg {}: a doubled `requires` target must reach block_requires once",
            leg.name()
        );
        // `tags` is the control, and the model: `Tags` is a `BTreeSet`,
        // so the duplicate is already unrepresentable by the time the
        // param builder sees it. The three `Vec<EntityUri>` edges below
        // are the ones with no such type.
        assert_eq!(
            a.tags.iter().collect::<Vec<_>>(),
            vec!["agent"],
            "leg {}: a doubled tag must reach block_tags once",
            leg.name()
        );
        assert_eq!(
            a.advice_suppressed,
            vec![EntityUri::block("lesson")],
            "leg {}: a doubled `advice_suppressed` target must reach its junction once",
            leg.name()
        );
        assert_eq!(
            a.contributes_to,
            vec![EntityUri::block("goal")],
            "leg {}: a doubled `contributes_to` target must reach its junction once",
            leg.name()
        );
    }
}
