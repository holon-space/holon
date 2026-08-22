//! Keystone: read-only import of the committed HolonTest LogSeq-DB fixture.
//!
//! Acceptance gates on the **identity check**, not regex counts (spike
//! reshape): deduped-datom count stable, and `#(:block/uuid datoms) ==
//! #(uuid-bearing entities)`, and the block projection preserves that 206. Plus
//! three spot-checks (page / journals / task) from the spike REPORT.
//!
//! Red-first (holon-feature): with the increment-0 stub `import` returning an
//! empty result, every count/identity/spot-check assertion below fails on its
//! value (0 != 206), NOT on a missing symbol — red for the right reason.
//!
//! Store-level assertions that need the running IVM pipeline — that Holon
//! *re-derives* `:block/refs` (dropped on import) so Project Alpha's backlinks
//! are non-empty (amendment A5), and that re-minted sibling order matches the
//! fracdex sequence — live in the increment-4 integration keystone under
//! `crates/holon-integration-tests/`, where a real store exists.

use std::collections::BTreeSet;
use std::path::PathBuf;

use holon_api::Block;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_logseq_db::LogseqDbImporter;

/// Constants derived from the committed fixture. ONE place (amendment A3).
///
/// Provenance: `~/.claude/plans/logseq-db-spike-2026-08-20/REPORT.md`
/// §Objective 1, re-measured directly from the fixture on 2026-08-21 with the
/// spike's own Python decoder — independently of the Rust under test, so these
/// cannot be satisfied by the implementation agreeing with itself.
///
/// The identity counts (2631 / 215 / 206) match REPORT exactly. Three numbers
/// do NOT come from REPORT and are the direct recount:
///  - `DISTINCT_ATTRS` is 57; REPORT's prose says 58.
///  - `KV_SINGLETONS` is 7, not the 9 the plan assumed — see `ORPHAN_ENTITIES`
///    and amendment A6 in `plan-lsqdb-import.md`.
///  - `JOURNAL_AUG20_UPDATED_AT` pins max-tx resolution (amendment A8).
mod fixture_facts {
    pub const UNIQUE_DATOMS: usize = 2631;
    pub const LEAF_DATOMS: usize = 8210;
    pub const DISTINCT_ENTITIES: usize = 215;
    pub const BLOCKS_WITH_UUID: usize = 206;
    /// Of the 206, the ones a person authored. The other 189 are LogSeq's own
    /// property, class and system pages — 185 flagged or identified as
    /// built-in, plus 4 per-view UI records that sit on the built-in
    /// `$$$views` page. See LW-7.a and the bugfunnel entry
    /// 2026-08-22-importer-materializes-logseq-built-ins-as-blocks.
    pub const BLOCKS_PROJECTED: usize = 17;
    /// Excluded by the PAGE leg rather than by evidence of their own.
    pub const EXCLUDED_UNDER_BUILT_IN_PAGE: usize = 4;
    /// Uuid-less entities carrying `:db/ident :logseq.kv/*`.
    pub const KV_SINGLETONS: usize = 7;
    /// Uuid-less entities that are NOT config singletons: e197 (a bare
    /// `:block/created-at` plus an empty `:block/title`) and e199 (a bare
    /// `:block/created-at`) — LogSeq's own half-created remnants. Measured
    /// 2026-08-21; the plan assumed all 9 uuid-less entities were kv
    /// singletons, which the fixture refutes.
    pub const ORPHAN_ENTITIES: usize = 2;
    pub const DISTINCT_ATTRS: usize = 57;

    /// The Aug-20 journal carries TWO `:block/updated-at` datoms even though
    /// the attribute is declared cardinality-one — one per transaction that
    /// touched it. The current value is the one from the higher transaction
    /// (tx 536871019), not the other (tx 536870916).
    pub const JOURNAL_AUG20_UPDATED_AT: i64 = 1787221153038;
    /// The value the LOSING datom holds. It equals the block's `created-at`,
    /// so resolving the attribute wrongly yields `updated_at == created_at` —
    /// entirely plausible-looking, which is why this needs a named tripwire
    /// rather than trusting the spot-checks to notice.
    pub const JOURNAL_AUG20_STALE_UPDATED_AT: i64 = 1787218310305;

    // Spot-check entities, by bare LogSeq uuid.
    pub const PROJECT_ALPHA_UUID: &str = "6a86cf74-3882-4ebd-a19d-c1fa46f58380";
    pub const JOURNAL_AUG20_UUID: &str = "00000001-2026-0820-0000-000000000000";
    pub const JOURNAL_AUG19_UUID: &str = "00000001-2026-0819-0000-000000000000";
    pub const JOURNAL_AUG22_UUID: &str = "00000001-2026-0822-0000-000000000000";
    pub const PROBE_TASK_UUID: &str = "6a86ce9d-9fe6-434e-b07f-bd629bb68ae9";
    /// e206, the ONE entity whose `:block/refs` include Project Alpha. Its
    /// title is `Link to [[6a86cf74-…]]` — LogSeq DB carries references inside
    /// the title text, which is what makes the dropped `:block/refs`
    /// recoverable.
    pub const LINKING_BLOCK_UUID: &str = "6a86cf5f-2cc4-4f32-b6ba-9496235db709";

    /// The six blocks that carry NO `:block/created-at` and NO
    /// `:block/updated-at` datom — entities 10 and 180-184. They are where
    /// amendment A9's epoch-0 sentinel actually fires, so they are what pins
    /// it: a `now()` implementation fabricates plausible timestamps here and
    /// nothing else in the suite would notice.
    pub const EPOCH_ZERO_UUIDS: &[&str] = &[
        "00000004-1595-0218-3700-000000000000", // e10
        "00000004-3919-3813-3000-000000000000", // e180
        "00000004-1713-4660-3800-000000000000", // e181
        "00000004-1335-6485-2300-000000000000", // e182
        "00000004-4049-4381-0000-000000000000", // e183
        "00000004-2116-2824-5200-000000000000", // e184
    ];
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq-db/holontest.sqlite")
}

#[tokio::test]
async fn holontest_db_imports_with_identity_gate() {
    use fixture_facts::*;

    let result = LogseqDbImporter::new()
        .import(&fixture_path())
        .await
        .expect("import HolonTest fixture");

    // --- Identity gate (the ruled acceptance criterion) ---
    assert_eq!(
        result.stats.unique_datoms, UNIQUE_DATOMS,
        "deduped (e,a,v,tx) datom count"
    );
    assert_eq!(
        result.stats.distinct_entities, DISTINCT_ENTITIES,
        "distinct entities"
    );
    assert_eq!(
        result.stats.distinct_attrs, DISTINCT_ATTRS,
        "distinct datom attributes"
    );
    assert_eq!(
        result.stats.uuid_datoms, result.stats.uuid_entities,
        "IDENTITY: #(:block/uuid datoms) must equal #(uuid-bearing entities)"
    );
    assert_eq!(
        result.stats.uuid_entities, BLOCKS_WITH_UUID,
        "uuid-bearing entities"
    );
    // Of those, the ones a person authored. The rest are LogSeq's own
    // property, class and system pages, read for schema knowledge and never
    // materialized as blocks (LW-7.a).
    assert_eq!(
        result.stats.block_entities, BLOCKS_PROJECTED,
        "uuid-bearing entities that become Holon blocks"
    );
    assert_eq!(
        result.stats.built_in_entities + result.stats.block_entities,
        result.stats.uuid_entities,
        "PARTITION: every uuid-bearing entity is either LogSeq's or the user's"
    );
    assert_eq!(
        result.stats.excluded_under_built_in_page, EXCLUDED_UNDER_BUILT_IN_PAGE,
        "excluded because their containing PAGE is built-in, disclosed here"
    );
    assert_eq!(
        result.stats.leaf_datoms, LEAF_DATOMS,
        "leaf tuples before dedup (index-tree redundancy)"
    );
    assert_eq!(
        result.stats.kv_singletons, KV_SINGLETONS,
        "uuid-less :logseq.kv/* config singletons"
    );
    assert_eq!(
        result.stats.orphan_entities, ORPHAN_ENTITIES,
        "uuid-less non-config remnants"
    );
    // The anti-silent-drop invariant: the three entity kinds partition the
    // entity set exactly, so no entity can be quietly classified into nothing.
    assert_eq!(
        result.stats.uuid_entities + result.stats.kv_singletons + result.stats.orphan_entities,
        result.stats.distinct_entities,
        "TOTALITY: every entity is a block, a config singleton, or a recorded orphan"
    );
    // No silent loss: every uuid-bearing entity is accounted for as EITHER a
    // projected block OR a disclosed built-in. The old form of this assertion
    // said "every uuid-bearing entity projects to a Block", which stopped
    // being true when LW-7.a excluded LogSeq's own pages — but "fewer blocks
    // than uuids" must never be allowed to mean "some vanished", so the
    // invariant is restated rather than relaxed.
    assert_eq!(
        result.blocks.len(),
        BLOCKS_PROJECTED,
        "every non-built-in uuid-bearing entity projects to exactly one Block"
    );
    assert_eq!(
        result.blocks.len() + result.stats.built_in_entities,
        BLOCKS_WITH_UUID,
        "NO SILENT LOSS: projected blocks plus disclosed built-ins account for \
         every uuid-bearing entity"
    );

    // --- Spot-check 1: the Project Alpha page ---
    let alpha = result
        .block_by_uuid(PROJECT_ALPHA_UUID)
        .expect("Project Alpha block present");
    assert!(alpha.is_page(), "Project Alpha is tagged Page");
    assert_eq!(alpha.content, "Project Alpha", "Project Alpha title");

    // --- Spot-check 2: the three journal days ---
    for uuid in [JOURNAL_AUG20_UUID, JOURNAL_AUG19_UUID, JOURNAL_AUG22_UUID] {
        let journal = result
            .block_by_uuid(uuid)
            .unwrap_or_else(|| panic!("journal block {uuid} present"));
        assert!(
            journal.tags.contains("Journal"),
            "journal {uuid} is tagged Journal"
        );
    }

    // --- Cardinality-one resolution: the CURRENT value, not any value ---
    // The Aug-20 journal holds two `:block/updated-at` datoms, one per
    // transaction. Reading the wrong one is invisible to every assertion
    // above, so it gets its own tripwire.
    let aug20_block = result
        .block_by_uuid(JOURNAL_AUG20_UUID)
        .expect("Aug-20 journal present");
    assert_eq!(
        aug20_block.updated_at, JOURNAL_AUG20_UPDATED_AT,
        "a cardinality-one attribute resolves to its highest-transaction datom"
    );
    assert_ne!(
        aug20_block.updated_at, JOURNAL_AUG20_STALE_UPDATED_AT,
        "the superseded :block/updated-at must not win — it equals created-at, \
         so a wrong resolution looks entirely plausible"
    );

    // --- The reference graph survives the drop of `:block/refs` ---
    // `:block/refs` is dropped on import because Holon derives its own
    // references — but that derivation reads a block's MARKS, so the marks
    // have to exist. Without them the whole link graph vanishes silently.
    let linking = result
        .block_by_uuid(LINKING_BLOCK_UUID)
        .expect("the block that links to Project Alpha is present");
    let marks = linking
        .marks
        .as_ref()
        .expect("a title carrying [[uuid]] projects as rich text with marks");
    let targets: Vec<EntityUri> = marks
        .iter()
        .filter_map(|span| match &span.mark {
            InlineMark::Link { target, .. } => target.entity_uri(),
            _ => None,
        })
        .collect();
    assert!(
        targets.contains(&EntityUri::block(PROJECT_ALPHA_UUID)),
        "block {LINKING_BLOCK_UUID} must carry a link mark naming Project Alpha \
         (its title is `Link to [[{PROJECT_ALPHA_UUID}]]`); got {targets:?}"
    );

    // --- A9: those six are LogSeq's, and are gone from the base ---
    // All six carried no timestamp datom, and all six are LogSeq's own:
    // `:logseq.property/empty-placeholder` and the five shipped files
    // (config.edn, custom.css, custom.js, publish.css, publish.js). Under
    // LW-7.a none of them is a block any more, so this pins the exclusion at
    // named entities rather than only at a count.
    for uuid in EPOCH_ZERO_UUIDS {
        assert!(
            result.block_by_uuid(uuid).is_none(),
            "{uuid} is one of LogSeq's own entities and must not be a block"
        );
    }

    // COVERAGE LOST, recorded rather than quietly dropped: this loop used to
    // guard the epoch-0 sentinel — a block with no timestamp datom keeps 0
    // rather than a fabricated import time, which a `now()` implementation
    // would violate while passing every other assertion in this file. Its
    // only six subjects were exactly the entities just excluded, and MEASURED
    // (probe, 2026-08-22) none of the 17 remaining blocks has a missing
    // timestamp. So the sentinel path is no longer reachable from this
    // fixture and nothing here drives it. Named in LogseqDbPush.md's W3 list;
    // closing it needs a constructed datom set, not a fixture edit.
    assert!(
        result
            .blocks
            .iter()
            .all(|b| b.created_at != 0 && b.updated_at != 0),
        "every remaining block carries a real timestamp — the premise of the \
         paragraph above; if this ever fails, the sentinel guard is reachable \
         again and should be restored"
    );

    // --- Sibling order keys are unambiguous on this corpus ---
    // The projection breaks equal `:block/order` keys by uuid, which is
    // deterministic but arbitrary. That tie-break is NOT exercised here: within
    // every parent's sibling group the keys are distinct. Asserting it keeps
    // the arbitrary path honest — if a future corpus introduces real ambiguity
    // this goes red instead of silently picking an order.
    let aug20 = EntityUri::block(JOURNAL_AUG20_UUID);
    let children = result.ordered_children(&aug20);
    assert_eq!(children.len(), 9, "the Aug-20 journal has nine children");
    let unique: std::collections::BTreeSet<&EntityUri> = children.iter().collect();
    assert_eq!(unique.len(), children.len(), "no child appears twice");

    // --- Spot-check 3: the probe task ---
    let task = result
        .block_by_uuid(PROBE_TASK_UUID)
        .expect("probe task block present");
    assert!(task.tags.contains("Task"), "probe task is tagged Task");
    assert!(
        task.content.is_empty(),
        "probe task has an empty :block/title"
    );

    // --- Sibling order populated for a known parent (fracdex-sorted; the
    // precise sequence + re-mint is asserted store-side at increment 4) ---
    let aug20 = EntityUri::block(JOURNAL_AUG20_UUID);
    assert!(
        !result.ordered_children(&aug20).is_empty(),
        "the Aug-20 journal has ordered children"
    );
}

/// Excluding LogSeq's own entities does NOT sever what the importer derives
/// from them.
///
/// LW-7.a says built-ins are still READ — property definitions, class names —
/// and only never MATERIALIZED as blocks. That distinction is the whole
/// design, and until this test existed nothing asserted it: the datoms stay in
/// the set and `class_index` walks every entity regardless of kind, so tags
/// resolve to names by construction. But a plausible future refactor ("we only
/// project Block-kind entities, so index only those") would break every tag,
/// and before this test nothing named that as the property at stake.
///
/// The names below are LogSeq's own classes. Every one of them belongs to an
/// entity that is NOT a block any more, so a green here is exactly the claim:
/// knowledge survived the exclusion.
///
/// WHERE THIS TEST IS ACTUALLY LOAD-BEARING: the `import` call below, not the
/// assertions. Measured by neutralising the class index: the projection
/// refuses a tag reference it cannot resolve, so the import returns
/// `DanglingReference` and `.expect("imports")` dies BEFORE any assertion
/// runs. Losing built-in-derived knowledge is therefore a loud refusal, not a
/// silent degradation to raw ids — which is the opposite of what I assumed
/// when writing this. The assertions below are the secondary check: they
/// would catch a future world where the import succeeds with degraded tags,
/// which today's code cannot produce.
#[tokio::test]
async fn schema_knowledge_survives_the_exclusion() {
    let result = LogseqDbImporter::new()
        .import(&fixture_path())
        .await
        .expect("imports");

    assert_eq!(
        result.blocks.len(),
        fixture_facts::BLOCKS_PROJECTED,
        "the exclusion is in force for this test to mean anything"
    );

    let tagged: Vec<&Block> = result
        .blocks
        .iter()
        .filter(|b| !b.tags.is_empty())
        .collect();
    assert_eq!(
        tagged.len(),
        7,
        "7 of the surviving blocks carry tags; got {:?}",
        tagged.iter().map(|b| &b.content).collect::<Vec<_>>()
    );

    let names: BTreeSet<&str> = tagged
        .iter()
        .flat_map(|b| b.tags.iter().map(String::as_str))
        .collect();
    assert_eq!(
        names,
        ["Journal", "Page", "Query", "Task"].into_iter().collect(),
        "tags must resolve to LogSeq's CLASS NAMES, which live on entities the \
         importer no longer materializes — a raw id or an empty set here means \
         the class index stopped seeing built-ins"
    );

    // Property definitions are the other half of "still read": every surviving
    // block carries properties, whose keys are resolved through the same
    // built-in entities.
    assert!(
        result.blocks.iter().all(|b| !b.properties.is_empty()),
        "every surviving block carries properties resolved via built-in \
         property definitions"
    );
}
