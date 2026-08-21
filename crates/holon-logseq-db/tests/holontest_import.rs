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

use std::path::PathBuf;

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
    assert_eq!(
        result.blocks.len(),
        BLOCKS_WITH_UUID,
        "every uuid-bearing entity projects to exactly one Block, no silent loss"
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

    // --- A9: the epoch-0 sentinel is a sentinel, not a fabricated time ---
    // A block with no timestamp datom keeps 0 rather than the import time. A
    // `now()` implementation passes every other assertion in this file.
    for uuid in EPOCH_ZERO_UUIDS {
        let block = result
            .block_by_uuid(uuid)
            .unwrap_or_else(|| panic!("timestamp-less block {uuid} present"));
        assert_eq!(
            (block.created_at, block.updated_at),
            (0, 0),
            "block {uuid} carries no timestamp datom, so both stamps must stay \
             the visibly-absent epoch — never a fabricated import time"
        );
    }

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
