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

use std::collections::HashMap;
use std::path::Path;
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
use holon_loro::LoroBackend;
use holon_loro::LoroDocument;
use holon_loro::block_to_params;
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

impl WriteLeg {
    fn name(self) -> &'static str {
        match self {
            Self::Loro => "loro",
            Self::OrgIngest => "org-ingest",
        }
    }
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
        OrgRenderer::render_document(&parsed.document, &parsed.blocks, path, &parsed.document.id)
            .expect("org render")
            .text;
    let restored = through_the_store(&parsed.document, &parsed.blocks, leg).await;
    let after_store =
        OrgRenderer::render_document(&parsed.document, &restored, path, &parsed.document.id)
            .expect("org render")
            .text;

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
        OrgRenderer::render_document(&parsed.document, &restored, path, &parsed.document.id)
            .expect("org render")
            .text;
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
        OrgRenderer::render_document(&parsed.document, &stripped, path, &parsed.document.id)
            .expect("org render")
            .text;
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
        OrgRenderer::render_document(&parsed.document, &restored, path, &parsed.document.id)
            .expect("org render")
            .text;
    assert_eq!(
        after_store, NON_ALPHABETICAL,
        "a non-alphabetically authored drawer must round-trip org → ingest → store → org \
         byte-identical"
    );
}

/// `:COLLAPSED: t` is the fold marker, and `collapsed` is DOCUMENT state
/// (Martin ruling 2026-07-11) — it is shared, synced, and survives a restart.
/// So an org file that declares it must come back out of the store with the
/// typed field set, on BOTH write legs, and must NOT leave the uppercase drawer
/// key behind in the untyped properties bag (the parser already consumed it).
#[tokio::test(flavor = "multi_thread")]
async fn collapsed_drawer_marker_survives_both_write_legs() {
    const SOURCE: &str = "#+ID: fold-page\n* Folded parent\n:PROPERTIES:\n:ID: \
                          folded-parent\n:COLLAPSED: t\n:END:\n* Open sibling\n:PROPERTIES:\n:ID: \
                          open-sibling\n:END:\n";

    let path = Path::new(FILE);
    let parsed = parse_org_file(path, SOURCE, &EntityUri::no_parent(), Path::new(ROOT))
        .expect("the fixture must parse");

    let folded = parsed
        .blocks
        .iter()
        .find(|b| b.id.id() == "folded-parent")
        .expect("the fixture must parse a folded headline");
    assert!(
        folded.collapsed,
        "control: the PARSER must already lift `:COLLAPSED: t` into the typed field — if this \
         fails the store leg is not what broke"
    );

    for leg in [WriteLeg::OrgIngest, WriteLeg::Loro] {
        let restored = through_the_store(&parsed.document, &parsed.blocks, leg).await;
        let folded = restored
            .iter()
            .find(|b| b.id.id() == "folded-parent")
            .expect("the folded headline must come back from the store");
        let open = restored
            .iter()
            .find(|b| b.id.id() == "open-sibling")
            .expect("the open headline must come back from the store");

        assert!(
            folded.collapsed,
            "leg {}: an authored `:COLLAPSED: t` must reach the typed `collapsed` column, not be \
             lost on import",
            leg.name()
        );
        assert!(
            !open.collapsed,
            "leg {}: negative control — a headline with no `:COLLAPSED:` must come back open",
            leg.name()
        );
        assert!(
            !folded.properties.contains_key("COLLAPSED"),
            "leg {}: `COLLAPSED` is consumed at the parse boundary into the typed field — it must \
             not ALSO ride along as an untyped property: {:?}",
            leg.name(),
            folded.properties
        );
    }
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

/// Doubled emphasis wrapping a markdown link, an org link, and plain words —
/// the shapes bugfunnel
/// `2026-09-02-org-write-back-halves-bold-markers-around-a-link` writes back
/// with half their delimiters.
const MARKED_INLINE: &str = "#+ID: marks-page\n* Inline marks\n:PROPERTIES:\n:ID: \
                             marks-anchor\n:END:\nOpen: **[pr#128](https://example.test/128)** \
                             (fixture) and **plain** and /italic/ and *[[https://example.test/1][org link]]* \
                             tail.\n";

/// The Loro seam, which the SQL legs of `render_both_ways` do not reach:
/// `marks` travels to SQL as a JSON array, duplicates and all, while the CRDT
/// keeps a Peritext attribute set that cannot hold them (see `MarkSpan`).
#[tokio::test(flavor = "multi_thread")]
async fn inline_marks_around_a_link_survive_the_loro_text_seam() {
    let path = Path::new(FILE);
    let parsed = parse_org_file(
        path,
        MARKED_INLINE,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("the fixture must parse");

    let doc = Arc::new(LoroDocument::new("marks".to_string()).expect("loro doc"));
    let backend = LoroBackend::from_document(doc);
    for block in std::iter::once(&parsed.document).chain(parsed.blocks.iter()) {
        backend
            .create_block_with_properties(
                block.parent_id.clone(),
                block.to_block_content(),
                Some(block.id.clone()),
                &HashMap::new(),
                &holon_api::BlockEdges::default(),
            )
            .await
            .unwrap_or_else(|e| panic!("create {}: {e}", block.id));
    }

    let snapshot = backend.snapshot_blocks().await;
    for block in parsed.blocks.iter() {
        let back = snapshot
            .get(&block.id.to_string())
            .unwrap_or_else(|| panic!("{} must come back from the Loro doc", block.id));
        assert_eq!(
            back.block.to_block_content(),
            block.to_block_content(),
            "block {}: the Loro doc gave back a different mark set than the parser minted",
            block.id
        );
    }
}

/// A link whose label is its target is written back without the label: the
/// one authored form whose bytes change on write-back.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_labelled_with_its_target_is_written_back_bare() {
    const AUTHORED: &str = "#+ID: bare-page\n* Links\n:PROPERTIES:\n:ID: bare-anchor\n:END:\nSee \
                            [[https://example.test/1][https://example.test/1]] and \
                            [[https://example.test/2][two]].\n";
    const WRITTEN: &str = "#+ID: bare-page\n* Links\n:PROPERTIES:\n:ID: bare-anchor\n:END:\nSee \
                           [[https://example.test/1]] and [[https://example.test/2][two]].\n";
    for leg in [WriteLeg::OrgIngest, WriteLeg::Loro] {
        let (from_parser, after_store) = render_both_ways(AUTHORED, leg).await;
        assert_eq!(
            from_parser,
            WRITTEN,
            "leg {}: the format-only leg writes the bare form",
            leg.name()
        );
        assert_eq!(
            after_store,
            WRITTEN,
            "leg {}: the store round trip writes the bare form",
            leg.name()
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn inline_marks_around_a_link_survive_both_write_legs() {
    for leg in [WriteLeg::OrgIngest, WriteLeg::Loro] {
        let (from_parser, after_store) = render_both_ways(MARKED_INLINE, leg).await;
        assert_eq!(
            from_parser,
            MARKED_INLINE,
            "control (leg {}): the format-only leg must already reproduce the authored bytes",
            leg.name()
        );
        assert_eq!(
            after_store,
            MARKED_INLINE,
            "leg {}: write-back halved the authored `**…**` delimiters",
            leg.name()
        );
    }
}

// ---------------------------------------------------------------------------
// Org priority (D101.a)
// ---------------------------------------------------------------------------

/// The three ways a vault authors a priority. All three name the SAME priority,
/// so all three must reach the store as the same canonical rank AND come back
/// out spelled the way they went in — the cookie stays a cookie, the drawer key
/// stays a drawer key, and a headline carrying both keeps both.
const PRIORITY_COOKIE: &str = "#+ID: prio-page\n* TODO [#A] Cookie carries it\n:PROPERTIES:\n:ID: \
                               prio-cookie\n:END:\n";
const PRIORITY_DRAWER: &str = "#+ID: prio-page\n* TODO Drawer carries it\n:PROPERTIES:\n:ID: \
                               prio-drawer\n:priority: A\n:END:\n";
const PRIORITY_BOTH: &str = "#+ID: prio-page\n* TODO [#A] Both carry it\n:PROPERTIES:\n:ID: \
                             prio-both\n:priority: A\n:END:\n";

/// Write-back data loss: with the drawer spelling in play the typed priority is
/// destroyed at parse, so the renderer emits NEITHER the cookie NOR the drawer
/// line and the authored priority leaves the file altogether.
#[tokio::test(flavor = "multi_thread")]
async fn every_authored_priority_carrier_survives_the_store_byte_identical() {
    for (name, source) in [
        ("cookie", PRIORITY_COOKIE),
        ("drawer", PRIORITY_DRAWER),
        ("both", PRIORITY_BOTH),
    ] {
        for leg in [WriteLeg::Loro, WriteLeg::OrgIngest] {
            let (from_parser, after_store) = render_both_ways(source, leg).await;
            assert_eq!(
                from_parser,
                source,
                "[{name}/{}] control: the format-only leg must already reproduce the authored \
                 bytes",
                leg.name()
            );
            assert_eq!(
                after_store,
                source,
                "[{name}/{}] the authored priority carrier must survive org → store → org \
                 byte-identical",
                leg.name()
            );
        }
    }
}

/// Same page, same block id, no priority carrier at all — the shape a block has
/// after the erasure bug stripped its authored priority from disk.
const PRIORITY_NONE: &str = "#+ID: prio-page\n* TODO Drawer carries it\n:PROPERTIES:\n:ID: \
                             prio-drawer\n:END:\n";

/// The priority page before its headline was written.
const PRIORITY_DOC_ONLY: &str = "#+ID: prio-page\n";

/// A headline that never carried a priority, edited in the file.
const UNPRIORITIZED: &str = "#+ID: prio-page\n* TODO Buy milk\n:PROPERTIES:\n:ID: \
                             prio-none\n:END:\n";
const UNPRIORITIZED_EDITED: &str = "#+ID: prio-page\n* TODO Buy oat milk\n:PROPERTIES:\n:ID: \
                                    prio-none\n:END:\n";

fn parse_fixture(source: &str) -> holon_org_format::ParseResult {
    parse_org_file(
        Path::new(FILE),
        source,
        &EntityUri::no_parent(),
        Path::new(ROOT),
    )
    .expect("the fixture must parse")
}

/// What [`ingest_in_sequence`] left behind for one headline.
struct Ingested {
    /// The stored `properties` bag.
    bag: HashMap<String, holon_api::Value>,
    /// Every create/update op the ingest dispatched for it, in order.
    ops: Vec<holon_api::StorageEntity>,
}

/// Ingest `sources` in order into one fresh store the way `FileSyncController`
/// does: a block absent from the previous file is created, a block present in
/// it is updated when `content_differs` flags it, with the previous parse as
/// `previous`.
async fn ingest_in_sequence(sources: &[&str], headline: &str) -> Ingested {
    use holon_core::file_format::FileFormatAdapter;

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
    let adapter = holon_orgmode::OrgFormatAdapter::new();

    let mut previous: Option<holon_org_format::ParseResult> = None;
    let mut ops = Vec::new();
    for source in sources {
        let parsed = parse_fixture(source);
        let doc = &parsed.document;
        for (i, block) in std::iter::once(doc).chain(parsed.blocks.iter()).enumerate() {
            let old = previous.as_ref().map(|p| {
                std::iter::once(&p.document)
                    .chain(p.blocks.iter())
                    .find(|b| b.id == block.id)
            });
            let (op, mut params) = match old.flatten() {
                None => (
                    "create",
                    adapter.build_block_params(block, &block.parent_id, &doc.id, None),
                ),
                Some(old) if adapter.content_differs(old, block) => (
                    "update",
                    adapter.build_block_params(block, &block.parent_id, &doc.id, Some(old)),
                ),
                Some(_) => continue,
            };
            params.insert(
                "sort_key".into(),
                holon_api::Value::String(format!("{i:010}")),
            );
            if block.id.id() == headline {
                ops.push(params.clone());
            }
            provider
                .execute_operation(&entity, op, params)
                .await
                .unwrap_or_else(|e| panic!("{op} {}: {e}", block.id));
        }
        previous = Some(parsed);
    }

    let rows = handle
        .query(
            &format!("SELECT properties FROM block_raw WHERE id = 'block:{headline}'"),
            HashMap::new(),
        )
        .await
        .expect("read the stored properties bag");
    assert_eq!(rows.len(), 1, "exactly one stored row for {headline}");
    let bag = match rows[0].get("properties") {
        Some(holon_api::Value::Object(bag)) => bag.clone(),
        other => panic!("{headline}: `properties` is not an object: {other:?}"),
    };
    Ingested { bag, ops }
}

/// The migration hinges on this: `RENDERER_VERSION` forces a re-ingest, but a
/// re-ingest only repairs a stale stored rank if an ingest that finds NO
/// priority carrier CLEARS it. Without the clear, a block the erasure bug
/// already stripped on disk keeps its legacy (inverted, or raw-string) rank
/// forever, and nothing in the pipeline can ever notice.
#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_lost_its_priority_clears_the_stored_rank_on_re_ingest() {
    use holon_org_format::models::OrgBlockExt;

    assert_eq!(
        parse_fixture(PRIORITY_DRAWER).blocks[0].priority(),
        Some(holon_api::Priority::A),
        "control: the first ingest must actually carry a priority"
    );
    let bag = ingest_in_sequence(&[PRIORITY_DRAWER, PRIORITY_NONE], "prio-drawer")
        .await
        .bag;
    assert_eq!(
        bag.get("priority"),
        None,
        "a file with no priority carrier must remove the stored rank, not leave it or a null: \
         {bag:?}"
    );
    for carrier in ["_priority_drawer_only", "_drawer_order"] {
        assert_eq!(
            bag.get(carrier),
            None,
            "the file no longer authors a drawer priority, so its `{carrier}` carrier must go \
             too: {bag:?}"
        );
    }
}

/// `priority` lives in the `properties` bag, so a Null written to "clear" it
/// is a real JSON null key on every block without one, and each
/// `TaskEntity::priority()` read of that row warns.
#[tokio::test(flavor = "multi_thread")]
async fn a_block_that_never_had_a_priority_stores_no_priority_key() {
    let created = ingest_in_sequence(&[UNPRIORITIZED], "prio-none").await;
    let edited = ingest_in_sequence(&[UNPRIORITIZED, UNPRIORITIZED_EDITED], "prio-none").await;
    assert_eq!(
        edited.ops.len(),
        2,
        "one create and one update: {:?}",
        edited.ops
    );
    for Ingested { bag, ops } in [created, edited] {
        assert_eq!(bag.get("priority"), None, "{bag:?}");
        // An eraser for a key the block never had leaves the bag unchanged, so
        // only the op can show it.
        for op in &ops {
            assert_eq!(op.get("priority"), None, "{op:?}");
        }
    }
}

/// A headline that appears in a later file is created there, with no
/// previous state to clear against.
#[tokio::test(flavor = "multi_thread")]
async fn a_headline_new_in_a_later_file_is_created_without_erasers() {
    let Ingested { bag, ops } =
        ingest_in_sequence(&[PRIORITY_DOC_ONLY, PRIORITY_DRAWER], "prio-drawer").await;
    assert_eq!(
        bag.get("priority"),
        Some(&holon_api::Value::Integer(1)),
        "{bag:?}"
    );
    assert_eq!(ops.len(), 1, "{ops:?}");
    assert!(
        !ops[0].values().any(holon_api::Value::is_removed),
        "a create carries no eraser: {:?}",
        ops[0]
    );
}

/// The sort contract behind the vault's `Now.org` query: the stored value is a
/// RANK that ascends with importance, so `ORDER BY priority ASC` puts A first
/// with no `CASE` expression. Asserted on the value that actually lands in the
/// store, for every authored carrier, because the carrier is exactly what used
/// to change the stored shape.
#[tokio::test(flavor = "multi_thread")]
async fn every_authored_carrier_stores_the_same_canonical_rank() {
    use holon_api::Value;
    use holon_org_format::models::OrgBlockExt;

    for (name, source) in [
        ("cookie", PRIORITY_COOKIE),
        ("drawer", PRIORITY_DRAWER),
        ("both", PRIORITY_BOTH),
    ] {
        for leg in [WriteLeg::Loro, WriteLeg::OrgIngest] {
            let parsed = parse_org_file(
                Path::new(FILE),
                source,
                &EntityUri::no_parent(),
                Path::new(ROOT),
            )
            .expect("the fixture must parse");
            let restored = through_the_store(&parsed.document, &parsed.blocks, leg).await;
            let headline = restored
                .iter()
                .find(|b| b.level() == 1)
                .expect("the fixture has one headline");

            assert_eq!(
                headline.properties_map().get("priority"),
                Some(&Value::Integer(1)),
                "[{name}/{}] `[#A]` must store as rank 1 — an integer, never the letter (SQLite \
                 sorts every string after every integer, which is how 41 vault blocks fell to the \
                 bottom of `Now.org`)",
                leg.name()
            );
        }
    }
}

/// A page and its blocks.
type Page = (Block, Vec<Block>);

/// Pages as the store returned them, served to the write-back controller.
#[derive(Clone, Default)]
struct StoredPages(Arc<std::sync::Mutex<HashMap<EntityUri, Page>>>);

#[async_trait::async_trait]
impl BlockReader for StoredPages {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        Ok(self
            .0
            .lock()
            .unwrap()
            .get(doc_id)
            .map(|(_, kids)| kids.clone())
            .unwrap_or_default())
    }

    async fn doc_block_topology(
        &self,
        doc_id: &EntityUri,
    ) -> anyhow::Result<Vec<(EntityUri, EntityUri)>> {
        Ok(self
            .get_blocks(doc_id)
            .await?
            .into_iter()
            .map(|b| (b.id, b.parent_id))
            .collect())
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        let pages = self.0.lock().unwrap();
        Ok(pages.values().find_map(|(page, kids)| {
            std::iter::once(page)
                .chain(kids)
                .find(|b| &b.id == id)
                .cloned()
        }))
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        let mut docs: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|(id, (_, kids))| (id.clone(), kids.clone()))
            .collect();
        docs.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        Ok(docs)
    }
}

#[async_trait::async_trait]
impl holon_filesystem::DocumentManager for StoredPages {
    async fn find_by_parent_and_name(
        &self,
        _: &EntityUri,
        _: &str,
    ) -> anyhow::Result<Option<Block>> {
        Ok(None)
    }
    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        Ok(doc)
    }
    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        Ok(self.0.lock().unwrap().get(id).map(|(page, _)| page.clone()))
    }
    async fn update_metadata(&self, _: &Block) -> anyhow::Result<()> {
        Ok(())
    }
}

struct NoOrdering;

#[async_trait::async_trait]
impl holon_core::block_ordering::BlockOrdering for NoOrdering {
    async fn place(
        &self,
        _: &EntityUri,
        _: &EntityUri,
        _: Option<&EntityUri>,
    ) -> holon_core::traits::Result<()> {
        Ok(())
    }
    async fn prev_sibling(&self, _: &EntityUri) -> holon_core::traits::Result<Option<EntityUri>> {
        Ok(None)
    }
    async fn next_sibling(&self, _: &EntityUri) -> holon_core::traits::Result<Option<EntityUri>> {
        Ok(None)
    }
    async fn first_child(&self, _: &EntityUri) -> holon_core::traits::Result<Option<EntityUri>> {
        Ok(None)
    }
    async fn last_child(&self, _: &EntityUri) -> holon_core::traits::Result<Option<EntityUri>> {
        Ok(None)
    }
    async fn children(&self, _: &EntityUri) -> holon_core::traits::Result<Vec<EntityUri>> {
        Ok(Vec::new())
    }
    async fn update_in_tree(&self, _: holon_api::StorageEntity) -> holon_core::traits::Result<()> {
        Ok(())
    }
    async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> holon_core::traits::Result<()> {
        Ok(())
    }
}

/// A block whose value its file cannot hold, on a page of its own.
fn lossy_page(title: &str, kid_id: EntityUri, lossy: impl FnOnce(&mut Block)) -> (Block, Block) {
    let mut page = Block::new_text(
        EntityUri::block(&format!("{}-page", title.to_lowercase())),
        EntityUri::no_parent(),
        title,
    );
    page.set_page(true);
    let mut kid = Block::new_text(kid_id, page.id.clone(), "Kid");
    lossy(&mut kid);
    (page, kid)
}

/// Each value write-back cannot put in the file raises a `WritebackLossy`
/// condition for that file, naming the block; a render that holds every value
/// lifts it.
#[tokio::test(flavor = "multi_thread")]
async fn a_value_the_file_cannot_hold_raises_the_files_writeback_condition() {
    let cases = [
        lossy_page("Tagged", EntityUri::block("tagged-kid"), |kid| {
            kid.content = "Meeting :urgent:".to_string();
        }),
        lossy_page("Keyed", EntityUri::block("keyed-kid"), |kid| {
            kid.set_property("a b", holon_api::Value::String("v".into()));
        }),
        lossy_page(
            "Refused",
            EntityUri::parse("block:a#b").expect("a block URI"),
            |_| {},
        ),
    ];

    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let pages = StoredPages::default();
    for (page, kid) in &cases {
        let restored = through_the_store(page, std::slice::from_ref(kid), WriteLeg::Loro).await;
        let back = restored
            .iter()
            .find(|b| b.id == kid.id)
            .unwrap_or_else(|| panic!("{} must come back from the store", kid.id));
        assert_eq!(
            (&back.content, back.properties_map().get("a b")),
            (&kid.content, kid.properties_map().get("a b")),
            "mechanism: the store keeps the value the file cannot hold"
        );
        pages
            .0
            .lock()
            .unwrap()
            .insert(page.id.clone(), (page.clone(), restored));
    }

    let bus = Arc::new(holon_api::ConditionBus::new());
    let mut sync = holon_orgmode::file_sync_controller::new_org_sync_controller(
        Arc::new(pages.clone()),
        Arc::new(pages.clone()),
        root.clone(),
        Arc::new(NoOrdering),
        Arc::new(holon_filesystem::RealFileSystem),
    )
    .with_writeback_disclosure(Arc::new(
        holon_app::loro_seams::WritebackDegradedDisclosure { bus: bus.clone() },
    ));
    sync.materialize_missing_page_files()
        .await
        .unwrap_or_else(|e| panic!("{e:#}"));

    let degraded = |file: &str| {
        let subject = holon_core::CanonicalPath::new(root.join(file))
            .display()
            .to_string();
        bus.current().into_iter().find_map(|c| match c.reason {
            holon_api::ConditionKind::WritebackLossy { detail } if c.subject == subject => {
                Some(detail)
            }
            _ => None,
        })
    };
    let mut missing = Vec::new();
    for ((page, kid), file) in cases.iter().zip(["Tagged.org", "Keyed.org", "Refused.org"]) {
        match degraded(file) {
            Some(detail) if detail.contains(kid.id.as_str()) && detail.contains(file) => {}
            Some(detail) => missing.push(format!("{file}: does not name {}: {detail}", kid.id)),
            None => missing.push(format!(
                "{file}: no condition for {} of {}",
                kid.id, page.id
            )),
        }
    }
    assert!(
        missing.is_empty(),
        "each lossy file raises a condition naming the block and the file: {missing:#?}\n\
         conditions: {:?}",
        bus.current()
    );

    for (page, kid) in &cases {
        let mut fixed = Block::new_text(
            EntityUri::block(&format!("{}-kid-fixed", page.content.to_lowercase())),
            page.id.clone(),
            "Kid",
        );
        if kid.id.as_str() != "block:a#b" {
            fixed.id = kid.id.clone();
        }
        pages
            .0
            .lock()
            .unwrap()
            .insert(page.id.clone(), (page.clone(), vec![fixed]));
    }
    sync.re_render_all_tracked(&std::collections::HashSet::new())
        .await
        .unwrap_or_else(|e| panic!("{e:#}"));
    sync.materialize_missing_page_files()
        .await
        .unwrap_or_else(|e| panic!("{e:#}"));
    for file in ["Tagged.org", "Keyed.org", "Refused.org"] {
        assert_eq!(
            degraded(file),
            None,
            "{file}: a render that holds every value lifts the condition"
        );
    }
}

/// A render the org parser would refuse is not written: a new page gets no
/// file, an existing page keeps its old, readable bytes, and each file's
/// `WritebackLossy` condition says why.
#[tokio::test(flavor = "multi_thread")]
async fn a_render_the_parser_would_refuse_leaves_the_page_readable() {
    let (page, kid) = lossy_page("Cookie", EntityUri::block("cookie-kid"), |_| {});
    let (fresh, fresh_kid) = lossy_page("Fresh", EntityUri::block("fresh-kid"), |kid| {
        kid.content = "[#a] lowercase cookie".to_string();
    });
    let tmp = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(tmp.path()).unwrap();
    let pages = StoredPages::default();
    pages
        .0
        .lock()
        .unwrap()
        .insert(page.id.clone(), (page.clone(), vec![kid.clone()]));

    let bus = Arc::new(holon_api::ConditionBus::new());
    let mut sync = holon_orgmode::file_sync_controller::new_org_sync_controller(
        Arc::new(pages.clone()),
        Arc::new(pages.clone()),
        root.clone(),
        Arc::new(NoOrdering),
        Arc::new(holon_filesystem::RealFileSystem),
    )
    .with_writeback_disclosure(Arc::new(
        holon_app::loro_seams::WritebackDegradedDisclosure { bus: bus.clone() },
    ));
    sync.materialize_missing_page_files()
        .await
        .unwrap_or_else(|e| panic!("{e:#}"));
    let file = root.join("Cookie.org");
    let before = std::fs::read_to_string(&file).expect("the page file is written");

    let mut cookie = kid.clone();
    cookie.content = "[#1] numeric cookie".to_string();
    pages
        .0
        .lock()
        .unwrap()
        .insert(page.id.clone(), (page.clone(), vec![cookie]));
    pages
        .0
        .lock()
        .unwrap()
        .insert(fresh.id.clone(), (fresh.clone(), vec![fresh_kid]));
    let rerender = sync
        .re_render_all_tracked(&std::collections::HashSet::new())
        .await;
    let materialize = sync.materialize_missing_page_files().await;
    let outcomes = format!("re-render {rerender:?}, materialize {materialize:?}");

    let after = std::fs::read_to_string(&file).unwrap();
    assert_eq!(
        after, before,
        "the refused render must not reach disk ({outcomes})"
    );
    let fresh_file = root.join("Fresh.org");
    if let Ok(written) = std::fs::read_to_string(&fresh_file) {
        holon_orgmode::parser::parse_org_file(
            &fresh_file,
            &written,
            &EntityUri::no_parent(),
            &root,
        )
        .unwrap_or_else(|e| panic!("a written page must be readable: {e:#}\n{written}"));
    }
    let condition = |file: &Path| {
        let subject = holon_core::CanonicalPath::new(file).display().to_string();
        bus.current().into_iter().find_map(|c| match c.reason {
            holon_api::ConditionKind::WritebackLossy { detail } if c.subject == subject => {
                Some(detail)
            }
            _ => None,
        })
    };
    for f in [&file, &fresh_file] {
        let detail = condition(f);
        assert!(
            detail.as_deref().is_some_and(|d| d.contains("refused")),
            "{}: the condition names the refusal: {detail:?} ({outcomes})",
            f.display()
        );
    }
}

const BLANK_LINES: &str = "#+ID: blank-page\n\n* Topic\n:PROPERTIES:\n:ID: \
     blank-topic\n:END:\n\nbody\n\n\n** Kid\n:PROPERTIES:\n:ID: blank-kid\n:END:\n\n* Next\n\
     :PROPERTIES:\n:ID: blank-next\n:END:\nend\n";

/// The blank lines an author put around a headline's text come back after the
/// store, on both write legs.
#[tokio::test(flavor = "multi_thread")]
async fn blank_lines_survive_the_store() {
    for leg in [WriteLeg::Loro, WriteLeg::OrgIngest] {
        let (from_parser, after_store) = render_both_ways(BLANK_LINES, leg).await;
        assert_eq!(
            (from_parser.as_str(), after_store.as_str()),
            (BLANK_LINES, BLANK_LINES),
            "{}",
            leg.name()
        );
    }
}

const KEYWORD_LINES: &str = "#+ID: keyword-page\n* Topic\n:PROPERTIES:\n:ID: keyword-topic\n:END:\n\
     text\n#+STARTUP: fold\n#+CAPTION: cap\n| a |\n";

/// A keyword line written in Emacs in a block's text comes back raw, in its
/// place, after the store, on both write legs.
#[tokio::test(flavor = "multi_thread")]
async fn keyword_lines_in_block_text_survive_the_store() {
    for leg in [WriteLeg::Loro, WriteLeg::OrgIngest] {
        let (from_parser, after_store) = render_both_ways(KEYWORD_LINES, leg).await;
        assert_eq!(
            (from_parser.as_str(), after_store.as_str()),
            (KEYWORD_LINES, KEYWORD_LINES),
            "{}",
            leg.name()
        );
    }
}

const DRAWER_SPACING: &str = "#+ID: spacing-page\n* Topic\n:PROPERTIES:\n:ID: spacing-topic\n:NOTE: two \
     \n:LEAD:  lead\n:EMPTY: \n:END:\ntext\n";

/// A drawer value keeps its authored spacing after the store, on both write
/// legs.
#[tokio::test(flavor = "multi_thread")]
async fn drawer_value_spacing_survives_the_store() {
    for leg in [WriteLeg::Loro, WriteLeg::OrgIngest] {
        let (from_parser, after_store) = render_both_ways(DRAWER_SPACING, leg).await;
        assert_eq!(
            (from_parser.as_str(), after_store.as_str()),
            (DRAWER_SPACING, DRAWER_SPACING),
            "{}",
            leg.name()
        );
    }
}

/// Source-block delimiters as authored, a source block with no `:id`, and a
/// headline deeper than one level below its parent keep their bytes through
/// the store on both write legs.
#[tokio::test(flavor = "multi_thread")]
async fn authored_source_lines_and_star_counts_survive_the_store() {
    let head = "#+ID: shapes-page\n* H\n:PROPERTIES:\n:ID: shapes-h\n:END:\n";
    let deep = "**** deep\n:PROPERTIES:\n:ID: shapes-d\n:END:\n";
    for source in [
        format!("{head}#+begin_src sh :id shapes-s1\nx\n#+end_src\n"),
        format!("{head}#+name: n1\n#+begin_src sh\n\nx\n#+end_src\n"),
        format!("{head}{deep}"),
    ] {
        for leg in [WriteLeg::Loro, WriteLeg::OrgIngest] {
            let (from_parser, after_store) = render_both_ways(&source, leg).await;
            assert_eq!(
                from_parser,
                source,
                "[{}] control: format-only leg",
                leg.name()
            );
            assert_eq!(after_store, source, "[{}] after the store", leg.name());
        }
    }
}
