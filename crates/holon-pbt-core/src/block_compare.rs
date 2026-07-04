//! Shared block-equivalence core (normalization + non-panicking comparison),
//! lifted to `holon-pbt-core` (co-location Phase 1a follow-on) so every
//! companion `*-testing` crate can compare a store's block snapshot against the
//! reference without depending on `holon-integration-tests`.
//!
//! Block equivalence in Holon is *defined by* the org round-trip: two blocks
//! are "equal" when their org-normalized forms match. [`normalize_block`]
//! hand-replicates that round-trip (trim trailing whitespace, strip
//! internal/null/empty properties, unify the document root) using only
//! `holon-api` — so the floor stays org-crate-free.
//!
//! The `inv-blocks-match-ref` composite checks the SAME comparison against
//! every store that holds blocks (Loro, Org, `block_raw`, the `block` matview):
//! each store normalises to a `Vec<holon_api::Block>` and this module compares
//! it to the reference's snapshot. Two facets, both derived from the normalised
//! `Vec<Block>`:
//!
//! - **fields** — [`normalize_block`] + sort-by-id + `==`. `normalize_block`
//!   zeroes timestamps and strips internal/null/empty properties, so this
//!   compares content, content_type, parent, properties, tags, edge fields,
//!   task_state, source_language — everything *except* sibling order.
//! - **order** — per-parent sibling order under the renderer's canonical sort
//!   (source/image before text, then `sequence`, then id).
//!
//! Bodies stay dumb: they pick a store snapshot, a readiness gate, and which
//! facets apply, then call [`compare_blocks`]. The per-store `RunMode` and
//! CDC-lag → `Skipped` decision live in the body/runner, not here.

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::block::Block;

use crate::invariant::InvariantResult;
use crate::sibling_order::compare_sibling_order;

/// Properties that are internal bookkeeping (never part of an org round-trip)
/// and must be stripped before comparing a reference block against a
/// store-projected one.
///
/// `_provenance` (ADR 0024 P8 / C2a) is the engine's authorship stamp
/// (`origin`/`at_millis`/firing ids), written onto every create/update block by
/// `DispatchingOperationEngine`. Like `created_at`/`updated_at`/`id` it is
/// system-authored metadata, not user content: the org drawer serializer strips
/// all `_`-prefixed keys so it never round-trips through org, and the reference
/// model does not (and should not) re-derive it. Stripping it here is the same
/// normalization class as the timestamps — stamping *correctness* is owned by
/// the dedicated C2a unit/integration tests (`provenance_stamp_tests`,
/// `provenance::tests`), not by this oracle.
pub const INTERNAL_PROPS: &[&str] = &[
    "sequence",
    "level",
    "ID",
    "id",
    "created_at",
    "updated_at",
    "document_id",
    "todo_keywords",
    holon_api::PROVENANCE_PROPERTY,
];

/// The block's ordering key, read straight off its `properties` map (pure
/// `holon-api`). This is the inlined equivalent of
/// `holon_org_format::OrgBlockExt::sequence` — kept here so block comparison
/// never drags an org crate onto the pbt-core floor. The `sequence` property is
/// the canonical intra-parent ordering key; absent → `0`.
pub fn block_sequence(block: &Block) -> i64 {
    block
        .get_property("sequence")
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
}

pub fn normalize_block(block: &Block) -> Block {
    let mut normalized = block.clone();
    normalized.created_at = 0;
    normalized.updated_at = 0;
    // sort_key is no longer a field of the domain Block (ADR 0005) — ordering is
    // validated separately via the order facet below.
    // Trim overall content and normalize internal trailing whitespace per line
    // (org round-trip strips trailing whitespace from source block lines)
    normalized.content = normalized
        .content
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    // The `__default__` page is prod's layout-owning root container (a real
    // block, not the sentinel — see `default_doc_block_uri`). The reference
    // model represents the document root as `__document_root__` and parents the
    // layout straight to it, so unify the two roots here.
    if normalized.parent_id.is_no_parent()
        || normalized.parent_id.is_sentinel()
        || normalized.parent_id == holon_api::default_doc_block_uri()
    {
        normalized.parent_id = holon_api::EntityUri::block("__document_root__");
    }
    // document_id removed from Block struct; no normalization needed
    for prop in INTERNAL_PROPS {
        normalized.properties.remove(*prop);
    }
    // `_`-prefixed keys are prod's declared internal-property namespace, not
    // user content: the org drawer serializer never emits them (`OrgBlockExt::
    // drawer_properties`) and `effect_id` excludes them from effect identity.
    // The reference model does not re-derive them, so the whole namespace is
    // stripped here rather than key-by-key (`_provenance`, `_drawer_order`, …).
    normalized.properties.retain(|k, _| !k.starts_with('_'));
    // Strip Null-valued and empty-string properties: the org parser stores
    // task_state=Null explicitly in the DB but the reference model omits absent
    // properties. Empty-string task_state means "no state" and is lost during
    // org round-trip (not written as a keyword, so not parsed back).
    normalized.properties.retain(|_, v| match v {
        holon_api::Value::Null => false,
        holon_api::Value::String(s) if s.is_empty() => false,
        _ => true,
    });
    // `task_state_category` is `task_state`'s sidecar — without a (non-empty)
    // keyword it carries no information, and the org round-trip drops the PAIR
    // (no keyword rendered → neither parsed back). The retain above already
    // dropped an empty/Null keyword; drop its orphaned sidecar with it, or the
    // ref (which stores ""+"active" after cycling to Clear, exactly like
    // block_raw) diverges from the org-parsed side on a phantom property.
    if !normalized.properties.contains_key("task_state") {
        normalized.properties.remove("task_state_category");
    }
    // Marks canonicalization: stores emit equal mark SETS in different orders
    // (Loro Peritext closes overlapping runs in HashMap order; the SQL JSON
    // column preserves insertion order) — sort into the canonical order on
    // both sides. An empty set is semantically "no marks": unify with None so
    // `Some([])` vs `None` never diverges spuriously.
    if let Some(marks) = &mut normalized.marks {
        if marks.is_empty() {
            normalized.marks = None;
        } else {
            holon_api::canonicalize_marks(marks);
        }
    }
    normalized
}

/// Field-equality facet: `Ok(())` when the two snapshots are
/// [`normalize_block`]-equivalent (id-set + every non-order field), else an
/// `Err` carrying the normalized diff.
pub fn compare_block_fields(
    label: &str,
    actual: &[Block],
    expected: &[Block],
) -> Result<(), String> {
    let mut actual_sorted: Vec<_> = actual.iter().map(normalize_block).collect();
    let mut expected_sorted: Vec<_> = expected.iter().map(normalize_block).collect();
    actual_sorted.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
    expected_sorted.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));

    if actual_sorted == expected_sorted {
        return Ok(());
    }
    Err(format!(
        "[{label}] fields diverge from reference\n{}",
        render_block_diff(label, &actual_sorted, &expected_sorted),
    ))
}

/// How many divergent ids / field deltas a diff spells out before summarising
/// the rest as a count. A whole-snapshot dump of 30+ blocks is ~100 KB on one
/// line and has to be decoded with a script before it says anything; a
/// divergence wider than this is a wholesale mismatch, for which the exact
/// membership of the tail adds nothing.
const DIFF_LIST_CAP: usize = 12;

fn id_list(ids: &[&str]) -> String {
    if ids.len() <= DIFF_LIST_CAP {
        return format!("{ids:?}");
    }
    format!(
        "{:?} … and {} more",
        &ids[..DIFF_LIST_CAP],
        ids.len() - DIFF_LIST_CAP
    )
}

/// Structured, human-readable divergence report: which ids only one side holds,
/// then the per-field deltas for the ids both sides hold.
///
/// Both inputs must already be [`normalize_block`]-normalized and sorted by id.
fn render_block_diff(label: &str, actual: &[Block], expected: &[Block]) -> String {
    let actual_ids: Vec<&str> = actual.iter().map(|b| b.id.as_str()).collect();
    let expected_ids: Vec<&str> = expected.iter().map(|b| b.id.as_str()).collect();

    let only_actual: Vec<&str> = actual_ids
        .iter()
        .filter(|id| !expected_ids.contains(id))
        .copied()
        .collect();
    let only_expected: Vec<&str> = expected_ids
        .iter()
        .filter(|id| !actual_ids.contains(id))
        .copied()
        .collect();

    let mut out = format!(
        "  {label}: {} blocks, reference: {} blocks\n  only in {label} ({}): {}\n  \
         only in reference ({}): {}\n",
        actual.len(),
        expected.len(),
        only_actual.len(),
        id_list(&only_actual),
        only_expected.len(),
        id_list(&only_expected),
    );

    // A duplicate id makes `find` compare the wrong pair and can report "no
    // deltas" for genuinely divergent snapshots. Ids are a primary key on both
    // sides, so a repeat is a store/reference bug in its own right — say so
    // instead of emitting a diff that silently understates the divergence.
    let mut dup = String::new();
    for (side, blocks) in [("actual", actual), ("reference", expected)] {
        let ids: Vec<&str> = blocks.iter().map(|b| b.id.as_str()).collect();
        let mut seen = std::collections::BTreeSet::new();
        let repeats: Vec<&str> = ids
            .iter()
            .filter(|id| !seen.insert(**id))
            .copied()
            .collect();
        if !repeats.is_empty() {
            dup.push_str(&format!(
                "  !! {side} holds DUPLICATE ids {} — the per-id deltas below \
                 compare arbitrary pairs and may understate the divergence\n",
                id_list(&repeats),
            ));
        }
    }
    out.push_str(&dup);

    let mut deltas = Vec::new();
    for a in actual {
        let Some(e) = expected.iter().find(|e| e.id == a.id) else {
            continue;
        };
        if a == e {
            continue;
        }
        deltas.push(format!("    {}: {}", a.id.as_str(), field_deltas(a, e)));
    }

    out.push_str(&format!("  field deltas ({}):\n", deltas.len()));
    for d in deltas.iter().take(DIFF_LIST_CAP) {
        out.push_str(d);
        out.push('\n');
    }
    if deltas.len() > DIFF_LIST_CAP {
        out.push_str(&format!(
            "    … and {} more\n",
            deltas.len() - DIFF_LIST_CAP
        ));
    }
    out
}

/// The per-field difference between two normalized blocks with the same id.
///
/// The named-field sweep is exhaustive over `Block` today. If a field is added
/// and not listed here, two unequal blocks would report "no named field
/// differs" — so that case falls back to dumping both sides rather than
/// silently reporting an empty delta.
fn field_deltas(a: &Block, e: &Block) -> String {
    let mut parts = Vec::new();
    macro_rules! delta {
        ($field:ident) => {
            if a.$field != e.$field {
                parts.push(format!(
                    "{}: sut={:?} ref={:?}",
                    stringify!($field),
                    a.$field,
                    e.$field
                ));
            }
        };
    }
    delta!(parent_id);
    delta!(tags);
    delta!(requires);
    delta!(advice_suppressed);
    delta!(contributes_to);
    delta!(content);
    delta!(content_type);
    delta!(source_language);
    delta!(source_name);
    delta!(properties);
    delta!(marks);
    delta!(collapsed);
    delta!(widget_only);
    delta!(created_at);
    delta!(updated_at);

    if parts.is_empty() {
        return format!(
            "blocks differ but no named field does — `field_deltas` is missing a \
             `Block` field. sut={a:#?} ref={e:#?}"
        );
    }
    parts.join("; ")
}

/// Ordering facet: per-parent sibling order under the renderer's canonical
/// sort (source/image first, then `sequence`, then id), only comparing when
/// both sides hold the same id set and skipping all-source sibling groups.
/// Returns the first divergent parent as an `Err`.
pub fn compare_block_order(
    label: &str,
    actual: &[Block],
    expected: &[Block],
) -> Result<(), String> {
    let parent_ids: std::collections::HashSet<EntityUri> =
        actual.iter().map(|b| b.parent_id.clone()).collect();

    let render_group = |ct: ContentType| -> u8 {
        match ct {
            ContentType::Source | ContentType::Image => 0,
            ContentType::Text => 1,
        }
    };
    let canonical_sort = |children: &mut Vec<&Block>| {
        children.sort_by(|a, b| {
            render_group(a.content_type)
                .cmp(&render_group(b.content_type))
                .then_with(|| block_sequence(a).cmp(&block_sequence(b)))
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });
    };

    for parent_id in &parent_ids {
        let mut actual_children: Vec<&Block> = actual
            .iter()
            .filter(|b| b.parent_id.as_raw_str() == parent_id.as_str())
            .collect();
        canonical_sort(&mut actual_children);
        let actual_order: Vec<&str> = actual_children.iter().map(|b| b.id.as_str()).collect();

        let mut ref_children: Vec<&Block> = expected
            .iter()
            .filter(|b| {
                if parent_id.is_no_parent() || parent_id.is_sentinel() {
                    b.parent_id.is_no_parent() || b.parent_id.is_sentinel()
                } else {
                    b.parent_id.as_raw_str() == parent_id.as_str()
                }
            })
            .collect();
        canonical_sort(&mut ref_children);
        let ref_order: Vec<&str> = ref_children.iter().map(|b| b.id.as_str()).collect();

        // Only compare when both sides hold the same id set under this parent.
        if actual_order.len() != ref_order.len()
            || !actual_order.iter().all(|id| ref_order.contains(id))
        {
            continue;
        }
        // Exact order comparison. Both sides are pre-sorted by the same
        // canonical key (render group, then `sequence`, then id); the ref's
        // `sequence` now reproduces the parser's `Source < Image < Text`
        // order, so no render-artifact exemption is needed.
        compare_sibling_order(label, parent_id, &ref_order, &actual_order)?;
    }
    Ok(())
}

/// Run the field facet (always) and, when `check_order`, the ordering facet.
/// Returns `Fail` on the first divergence, else `Ok`. Stores with a CDC-lag /
/// readiness gate convert their own "not ready" into `Skipped` *before*
/// calling this — the comparison itself never `Skip`s.
pub fn compare_blocks(
    label: &str,
    actual: &[Block],
    expected: &[Block],
    check_order: bool,
) -> InvariantResult {
    if let Err(msg) = compare_block_fields(label, actual, expected) {
        return InvariantResult::Fail(msg);
    }
    if check_order {
        if let Err(msg) = compare_block_order(label, actual, expected) {
            return InvariantResult::Fail(msg);
        }
    }
    InvariantResult::Ok
}

/// A single comparable facet of a block. Used by [`compare_block_subset`] for
/// stores that natively hold only some fields — e.g. `block_raw` has
/// `content`/`properties`/`parent` columns but NOT the junction-derived
/// `tags`/`requires`, so comparing those would always diverge.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockFacet {
    Content,
    Properties,
    Parent,
    ContentType,
    SourceLanguage,
    /// Inline rich-text marks (`block_raw` has a `marks` column). Compared
    /// canonicalized (see [`normalize_block`]) — losing this facet is the
    /// 2026-07-10 on-disk link-destruction class.
    Marks,
}

/// Compare only `facets` (plus the id-set, always) between two snapshots, both
/// [`normalize_block`]-normalised. For stores that don't natively carry every
/// `Block` field — comparing the full struct would false-fail on fields the
/// store can't represent. Returns `Fail` on the first divergence.
pub fn compare_block_subset(
    label: &str,
    actual: &[Block],
    expected: &[Block],
    facets: &[BlockFacet],
) -> InvariantResult {
    use std::collections::BTreeMap;

    let a: BTreeMap<EntityUri, Block> = actual
        .iter()
        .map(normalize_block)
        .map(|b| (b.id.clone(), b))
        .collect();
    let e: BTreeMap<EntityUri, Block> = expected
        .iter()
        .map(normalize_block)
        .map(|b| (b.id.clone(), b))
        .collect();

    let a_ids: std::collections::BTreeSet<&EntityUri> = a.keys().collect();
    let e_ids: std::collections::BTreeSet<&EntityUri> = e.keys().collect();
    if a_ids != e_ids {
        let missing: Vec<&&EntityUri> = e_ids.difference(&a_ids).collect();
        let spurious: Vec<&&EntityUri> = a_ids.difference(&e_ids).collect();
        return InvariantResult::Fail(format!(
            "[{label}] block id set diverges from reference\n  missing in {label}: {missing:?}\n  \
             spurious in {label}: {spurious:?}"
        ));
    }

    for (id, eb) in &e {
        let ab = &a[id];
        for facet in facets {
            let diverged = match facet {
                BlockFacet::Content => ab.content != eb.content,
                BlockFacet::Properties => ab.properties != eb.properties,
                BlockFacet::Parent => ab.parent_id != eb.parent_id,
                BlockFacet::ContentType => ab.content_type != eb.content_type,
                BlockFacet::SourceLanguage => ab.source_language != eb.source_language,
                BlockFacet::Marks => ab.marks != eb.marks,
            };
            if diverged {
                return InvariantResult::Fail(format!(
                    "[{label}] block {id} diverges from reference on {facet:?}\n  {label}: \
                     {ab:#?}\n  reference: {eb:#?}"
                ));
            }
        }
    }
    InvariantResult::Ok
}

#[cfg(test)]
mod tests {
    use holon_api::block::Block;

    use super::*;

    fn blk(id: &str, parent: &str, content: &str) -> Block {
        Block::new_text(
            EntityUri::block(id),
            EntityUri::block(parent),
            content.to_string(),
        )
    }

    /// The diff must name the divergence, not merely report one. These assert
    /// on the rendered text because that text IS the artifact a triager reads —
    /// the whole point of the structured diff over a `{:#?}` dump.
    #[test]
    fn diff_names_missing_extra_and_changed_ids() {
        let actual = vec![blk("1", "root", "hello"), blk("extra", "root", "x")];
        let expected = vec![blk("1", "root", "CHANGED"), blk("only-ref", "root", "y")];
        let msg = compare_block_fields("t", &actual, &expected).unwrap_err();

        assert!(msg.contains("only in t (1): [\"block:extra\"]"), "{msg}");
        assert!(
            msg.contains("only in reference (1): [\"block:only-ref\"]"),
            "{msg}"
        );
        assert!(msg.contains("field deltas (1)"), "{msg}");
        assert!(
            msg.contains("content: sut=\"hello\" ref=\"CHANGED\""),
            "{msg}"
        );
    }

    /// A repeated id would make the per-id pairing arbitrary, so it must be
    /// called out rather than silently producing an understated diff.
    #[test]
    fn duplicate_ids_are_flagged() {
        let actual = vec![blk("1", "root", "a"), blk("1", "root", "b")];
        let expected = vec![blk("1", "root", "a")];
        let msg = compare_block_fields("t", &actual, &expected).unwrap_err();

        assert!(msg.contains("DUPLICATE ids"), "{msg}");
        assert!(msg.contains("block:1"), "{msg}");
    }

    #[test]
    fn identical_snapshots_match() {
        let a = vec![blk("1", "root", "hello"), blk("2", "root", "world")];
        let b = vec![blk("2", "root", "world"), blk("1", "root", "hello")];
        // order-insensitive on the field facet (sorted by id)
        assert!(matches!(
            compare_blocks("test", &a, &b, false),
            InvariantResult::Ok
        ));
    }

    #[test]
    fn content_divergence_fails() {
        let a = vec![blk("1", "root", "hello")];
        let b = vec![blk("1", "root", "HELLO")];
        assert!(matches!(
            compare_blocks("test", &a, &b, false),
            InvariantResult::Fail(_)
        ));
    }

    #[test]
    fn missing_block_fails() {
        let a = vec![blk("1", "root", "hello")];
        let b = vec![blk("1", "root", "hello"), blk("2", "root", "world")];
        assert!(matches!(
            compare_blocks("test", &a, &b, false),
            InvariantResult::Fail(_)
        ));
    }

    #[test]
    fn subset_ignores_unlisted_facets() {
        // Same id+content, different parent. Subset on [Content] only → Ok,
        // even though parent diverges (the facet isn't compared).
        let mut a = blk("1", "parentA", "hello");
        let mut b = blk("1", "parentB", "hello");
        a.parent_id = EntityUri::block("parentA");
        b.parent_id = EntityUri::block("parentB");
        assert!(matches!(
            compare_block_subset("test", &[a], &[b], &[BlockFacet::Content]),
            InvariantResult::Ok
        ));
    }

    #[test]
    fn subset_catches_listed_facet_and_id_set() {
        let a = vec![blk("1", "root", "hello")];
        let b = vec![blk("1", "root", "WORLD")];
        assert!(matches!(
            compare_block_subset("test", &a, &b, &[BlockFacet::Content]),
            InvariantResult::Fail(_)
        ));
        // id-set always checked even with empty facets
        let a2 = vec![blk("1", "root", "x")];
        let b2 = vec![blk("1", "root", "x"), blk("2", "root", "y")];
        assert!(matches!(
            compare_block_subset("test", &a2, &b2, &[]),
            InvariantResult::Fail(_)
        ));
    }

    #[test]
    fn trailing_whitespace_normalized_away() {
        let a = vec![blk("1", "root", "hello   ")];
        let b = vec![blk("1", "root", "hello")];
        assert!(matches!(
            compare_blocks("test", &a, &b, false),
            InvariantResult::Ok
        ));
    }
}
