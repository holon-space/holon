//! The org → store → org round trip, end to end through the REAL store.
//!
//! This closes the gap `docs/Reference/CompassConventions.md` discloses in so
//! many words: "no end-to-end org → store → org round-trip test exists, so rule
//! 4 is a mitigation for a real, currently-untested churn path."
//!
//! Each existing test covers one leg and stops.
//! `holon-orgmode/tests/org_block_round_trip_pbt.rs` is org → org (format only)
//! and `turso_block_round_trip_pbt.rs` next door is Block → Turso → Block (no
//! org text). `compass_property_key_probe.rs` does reach for the composite
//! path, but SIMULATES the post-store shape by hand-building blocks that carry
//! no `_drawer_order` — a faithful guess, never a measurement. Here the blocks
//! go through Turso.
//!
//! Measuring the path rather than simulating it REFUTED the premise behind
//! `CompassConventions.md` rule 4. That rule says authored drawer order is lost
//! because `_drawer_order` "never reaches the store"; in fact
//! `drawer_properties()` only hides the `_`-prefixed carrier from the DRAWER
//! (so write-back cannot emit a literal `:_drawer_order:` key — the one job it
//! has), while the store persists the whole properties bag. The carrier comes
//! back and the renderer replays the authored order. The keystone corpus
//! already said as much from the other side: its
//! `org-drawer-order-property-leaks-into-projections` case exists because
//! `_drawer_order` DOES reach `block_raw.properties` and the matview.
//!
//! So the two tests below pin what the real path does:
//!   1. the canonical alphabetical drawer is byte-stable through the store —
//!      rule 4's promise to authors, still worth a gate;
//!   2. a non-alphabetical drawer is byte-stable too, carrier and all — which
//!      is what rule 4 and the CLAUDE.md line quoting it should now be
//!      corrected to say.

use std::path::Path;
use std::sync::Arc;

use holon::core::SqlOperationProvider;
use holon::core::queryable_cache::QueryableCache;
use holon::storage::BLOCK_WRITE_TABLE;
use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon::sync::block_to_params;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::block::Block;
use holon_app::turso_seams::CacheBlockReader;
use holon_core::OperationProvider;
use holon_filesystem::BlockReader;
use holon_org_format::OrgRenderer;
use holon_org_format::parse_org_file;
use holon_turso::schema_modules::BlockSchemaModule;

const ROOT: &str = "/vault";
const FILE: &str = "/vault/compass.org";

/// Rule 4's canonical template: keys in ASCII-alphabetical order after `:ID:`,
/// drawn from the recommended Compass key set (uppercase sorts first,
/// byte-wise).
const ALPHABETICAL: &str = "#+ID: compass-page\n* Reach steady-state ingest\n:PROPERTIES:\n:ID: \
                            compass-anchor\n:TEMPLATE: compass-anchor\n:compass: \
                            north\n:contributes-to: compass-north\n:last-reviewed: \
                            2026-08-11\n:provenance: inferred\n:review-cadence: P30D\n:END:\n";

/// The same keys and values, authored in a deliberately NON-alphabetical order.
const NON_ALPHABETICAL: &str = "#+ID: compass-page\n* Reach steady-state ingest\n:PROPERTIES:\n:ID: \
     compass-anchor\n:provenance: inferred\n:review-cadence: P30D\n:last-reviewed: \
     2026-08-11\n:contributes-to: compass-north\n:compass: north\n:TEMPLATE: \
     compass-anchor\n:END:\n";

async fn setup_production_schema(handle: &holon::storage::turso::DbHandle) {
    use holon_turso::schema_modules::BlockMatviewSchemaModule;
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
    // Parsed org blocks carry link edges the generated-Block PBTs never
    // produce, and the write path fans them into `block_links`.
    LinkSchemaModule
        .ensure_schema(handle)
        .await
        .expect("LinkSchemaModule");
}

/// Which production param builder packs a block for the write.
#[derive(Clone, Copy)]
enum WriteLeg {
    /// The Loro projection writer.
    Loro,
    /// The file-ingest builder `FileSyncController` calls for every parsed
    /// block — the leg a vault boot takes.
    OrgIngest,
}

/// Write `doc` + `blocks` through the production write path (`leg`'s param
/// builder → `SqlOperationProvider`) and read the document's blocks back
/// through the production read path (`CacheBlockReader::get_blocks`).
async fn through_the_store(doc: &Block, blocks: &[Block], leg: WriteLeg) -> Vec<Block> {
    let (_backend, handle) = TursoBackend::new_in_memory()
        .await
        .expect("turso must start in memory");
    setup_production_schema(&handle).await;

    let provider = Arc::new(SqlOperationProvider::with_edge_fields(
        handle.clone(),
        BLOCK_WRITE_TABLE.to_string(),
        "block".to_string(),
        "block".to_string(),
        BlockSchemaModule.edge_fields(),
    ));
    let entity: EntityName = "block".to_string().into();

    // The doc root goes first: the headlines' `parent_id` is the doc, and
    // `block_raw`'s parent FK rejects a child whose parent is absent.
    for (i, block) in std::iter::once(doc).chain(blocks.iter()).enumerate() {
        // `get_blocks` orders by sort_key; minting is the ordering authority's
        // business, so hand out distinct increasing keys to hold sibling order
        // fixed while this test looks at properties.
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
            .unwrap_or_else(|e| panic!("create {}: {e}", block.id));
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
        .expect("get_blocks must read the document back")
}

/// Render `source` twice: straight from the parser (the format-only leg the
/// existing PBTs cover) and after a real store round trip. Returns both, so a
/// caller can attribute any difference to the store leg alone — every
/// org-format quirk applies identically to the two sides.
async fn render_both_ways(source: &str, leg: WriteLeg) -> (String, String) {
    let path = Path::new(FILE);
    let parsed = parse_org_file(path, source, &EntityUri::no_parent(), Path::new(ROOT))
        .expect("the fixture must parse");

    let from_parser =
        OrgRenderer::render_document(&parsed.document, &parsed.blocks, path, &parsed.document.id);
    let restored = through_the_store(&parsed.document, &parsed.blocks, leg).await;
    let after_store =
        OrgRenderer::render_document(&parsed.document, &restored, path, &parsed.document.id);

    (from_parser, after_store)
}

/// The promise CompassConventions rule 4 makes: a drawer authored in
/// ASCII-alphabetical order comes back byte-identical after the document has
/// been through the store. This is the gate — if the store or the renderer
/// drifts, the documented convention stops holding and vault write-back starts
/// churning bytes on every ingest.
#[tokio::test(flavor = "multi_thread")]
async fn alphabetical_compass_drawer_is_byte_stable_through_the_store() {
    let (from_parser, after_store) = render_both_ways(ALPHABETICAL, WriteLeg::Loro).await;

    assert_eq!(
        from_parser, ALPHABETICAL,
        "control: the format-only leg must already reproduce the authored bytes — if this fails \
         the store leg is not what broke"
    );
    assert_eq!(
        after_store, ALPHABETICAL,
        "CompassConventions rule 4: an alphabetically-authored drawer must survive org → store → \
         org byte-identical"
    );
}

/// Authored drawer order DOES survive the store — measured here, against the
/// real one.
///
/// This refutes the rationale `CompassConventions.md` rule 4 rests on
/// ("`_drawer_order` … never reaches the store. After a store round trip the
/// renderer has no authored order and falls back to an alphabetical
/// tiebreak"). `drawer_properties()` hides the `_`-prefixed carrier from the
/// DRAWER, which is all it was ever built to do; the store persists the whole
/// properties bag, so the carrier comes back and the renderer replays the
/// authored order. The keystone corpus says the same thing independently — its
/// `org-drawer-order-property-leaks-into-projections` case exists precisely
/// because `_drawer_order` reaches `block_raw.properties` and the matview.
///
/// The mechanism is asserted, not just the bytes: a byte match alone could also
/// mean the renderer coincidentally sorted into the authored order.
#[tokio::test(flavor = "multi_thread")]
async fn non_alphabetical_drawer_order_survives_the_store() {
    let path = Path::new(FILE);
    let parsed = parse_org_file(
        path,
        NON_ALPHABETICAL,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("the fixture must parse");

    let restored = through_the_store(&parsed.document, &parsed.blocks, WriteLeg::Loro).await;

    let anchor = restored
        .iter()
        .find(|b| b.id.id() == "compass-anchor")
        .expect("the headline must come back from the store");
    assert!(
        anchor
            .get_property(holon_org_format::org_props::DRAWER_ORDER)
            .is_some(),
        "mechanism: the authored-order carrier must come back from the store on the block's \
         properties bag — without it the byte comparison below proves nothing"
    );

    let after_store =
        OrgRenderer::render_document(&parsed.document, &restored, path, &parsed.document.id);
    assert_eq!(
        after_store, NON_ALPHABETICAL,
        "a non-alphabetically authored drawer must round-trip org → store → org byte-identical: \
         the store keeps `_drawer_order` and the renderer replays it"
    );

    // Causality, not correlation: drop the carrier from the restored blocks and
    // the same renderer alphabetizes. This is the shape
    // `compass_property_key_probe.rs::drawer_order_without_authored_order`
    // hand-built and mistook for the post-store shape — it is really the
    // carrier-less shape, which the store does not produce.
    let stripped: Vec<Block> = restored
        .iter()
        .cloned()
        .map(|mut b| {
            b.properties
                .remove(holon_org_format::org_props::DRAWER_ORDER);
            b
        })
        .collect();
    let without_carrier =
        OrgRenderer::render_document(&parsed.document, &stripped, path, &parsed.document.id);
    assert_eq!(
        without_carrier, ALPHABETICAL,
        "without `_drawer_order` the renderer falls back to its alphabetical tiebreak — so the \
         authored order above is replayed FROM the carrier, not a coincidence of sorting"
    );
}

/// The same claim on the leg a vault boot takes: `FileSyncController` packs
/// every parsed block with `FileFormat::build_block_params`, not with the Loro
/// projection writer the two tests above exercise.
#[tokio::test(flavor = "multi_thread")]
async fn non_alphabetical_drawer_order_survives_the_ingest_leg() {
    let path = Path::new(FILE);
    let parsed = parse_org_file(
        path,
        NON_ALPHABETICAL,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("the fixture must parse");

    let restored = through_the_store(&parsed.document, &parsed.blocks, WriteLeg::OrgIngest).await;

    let anchor = restored
        .iter()
        .find(|b| b.id.id() == "compass-anchor")
        .expect("the headline must come back from the store");
    assert!(
        anchor
            .get_property(holon_org_format::org_props::DRAWER_ORDER)
            .is_some(),
        "mechanism: `build_block_params` must carry the authored-order carrier into the store — \
         without it the byte comparison below proves nothing"
    );

    let after_store =
        OrgRenderer::render_document(&parsed.document, &restored, path, &parsed.document.id);
    assert_eq!(
        after_store, NON_ALPHABETICAL,
        "a non-alphabetically authored drawer must round-trip org → ingest → store → org \
         byte-identical"
    );
}

/// `:contributes-to:` and `:REQUIRES:` are the two arc directions of the same
/// Compass relation (docs/Reference/CompassConventions.md), so they must reach
/// the store the same way: as typed edges in their junction tables, not as
/// drawer strings riding along in the properties blob.
///
/// A byte-stable round trip does NOT establish this — a property that survives
/// verbatim renders identically to an edge that was projected. The assertion
/// therefore reads the typed field, and reads the properties blob to confirm
/// the key was consumed at the parse boundary rather than duplicated.
#[tokio::test(flavor = "multi_thread")]
async fn compass_edges_survive_the_store_as_typed_edges() {
    const SOURCE: &str = "#+ID: compass-page\n* Reach steady-state ingest\n:PROPERTIES:\n:ID: \
                          compass-anchor\n:REQUIRES: compass-blocker\n:contributes-to: \
                          compass-north\n:END:\n";

    let path = Path::new(FILE);
    let parsed = parse_org_file(path, SOURCE, &EntityUri::no_parent(), Path::new(ROOT))
        .expect("the fixture must parse");
    let restored = through_the_store(&parsed.document, &parsed.blocks, WriteLeg::Loro).await;

    let anchor = restored
        .iter()
        .find(|b| b.id == EntityUri::block("compass-anchor"))
        .expect("the anchor block must come back from the store");

    assert_eq!(
        anchor.requires,
        vec![EntityUri::block("compass-blocker")],
        "control: the `requires` edge already projects through the junction"
    );
    assert_eq!(
        anchor.contributes_to,
        vec![EntityUri::block("compass-north")],
        "an authored `:contributes-to:` must reach the store as a typed edge, projected to the \
         block_contributes_to junction — parity with `requires`"
    );
    assert!(
        !anchor.properties.contains_key("contributes-to"),
        "`contributes-to` must be consumed at the parse boundary, not also kept as a drawer \
         string: {:?}",
        anchor.properties
    );
}
