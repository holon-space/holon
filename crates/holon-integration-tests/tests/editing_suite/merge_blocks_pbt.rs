//! `merge_blocks` Inc 1 — the duplicate-identity merge, op-level.
//!
//! The motivating shape is the split doc root (see
//! `split_doc_root_idless_duplicates`): two blocks carry the same identity,
//! one holding the real children and content, the other a husk. `merge_blocks`
//! folds the duplicate into the canonical and leaves a REPLICATED redirect so
//! the duplicate's id keeps resolving.
//!
//! Six ratified properties, all asserted per generated case:
//!   (a) resolving the duplicate's id yields the canonical id;
//!   (b) the normalized non-husk content multiset survives, up to dedupe
//!       collapse — every collapsed group keeps at least one member;
//!   (c) child order is deterministic: canonical's children, then the
//!       duplicate's, dedupe keepers in place;
//!   (d) undo restores the exact pre-merge state (one gesture);
//!   (e) every inbound link that resolved to the duplicate resolves to the
//!       canonical afterwards;
//!   (f) merging the same pair twice fails loud;
//!   (g) the DI-resolved PRODUCTION `BlockReader` resolves the merged-away id
//!       to the canonical block;
//!   (h) tags union with the canonical winning conflicts, properties adopted
//!       only for keys the canonical lacks, and the duplicate's authored `ID`
//!       never adopted.
//!
//! Design: docs/Plans/MergeBlocksInc1-2026-07-30.md
//!
//! @pbt kind harness
//! @pbt covers merge-blocks-inc1 — duplicate-identity merge: replicated
//! redirect, one-level dedupe, deterministic order, one-group undo, inbound
//! link re-point

#![cfg(feature = "pbt")]

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_api::inline_mark::EntityRef;
use holon_api::inline_mark::InlineMark;
use holon_api::inline_mark::MarkSpan;
use holon_api::inline_mark::marks_to_json;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::TestEnvironmentBuilder;
use proptest::prelude::*;

const ROOT: &str = "11111111-0000-0000-0000-000000000001";
/// The surviving identity. In the husk case it starts empty.
const CANON: &str = "11111111-0000-0000-0000-000000000002";
/// The identity folded away; its id must keep resolving afterwards.
const DUP: &str = "11111111-0000-0000-0000-000000000003";
/// Carries the inbound link that property (e) follows.
const LINKER: &str = "11111111-0000-0000-0000-000000000004";
/// The duplicate's org-authored `:ID:`. Adopting it onto the survivor would
/// make write-back render `:ID: <merged-away id>` — the split-root shape this
/// operation exists to repair — so property (h) forbids it.
const AUTHORED_DUPLICATE_ID: &str = "authored-duplicate-identity";

const SYNC_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(25);

fn root_org() -> String {
    format!("#+ID: {ROOT}\n#+TITLE: Merge\n")
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build runtime"),
    )
}

/// One generated merge. Empty content on either side is the husk arm; the
/// child lists are drawn from a small alphabet with whitespace decoration so
/// normalization-equal duplicates arise across BOTH sides.
#[derive(Debug, Clone)]
struct MergeCase {
    canonical_content: String,
    duplicate_content: String,
    canonical_children: Vec<ChildSpec>,
    duplicate_children: Vec<ChildSpec>,
    canonical_tags: Vec<String>,
    duplicate_tags: Vec<String>,
    canonical_properties: Vec<(String, String)>,
    duplicate_properties: Vec<(String, String)>,
}

/// A generated child. `grandchildren` is what makes a dedupe LOSER carry a
/// subtree: without it the orphan re-homing loop and its inverse bucket never
/// execute in a green run, so their correctness would be asserted by nothing.
#[derive(Debug, Clone)]
struct ChildSpec {
    content: String,
    grandchildren: Vec<String>,
}

/// Two words joined by at least one space, optionally lead-padded — so
/// trim + whitespace-collapse maps many raw strings onto four normal forms.
fn child_content() -> impl Strategy<Value = String> {
    (
        prop::sample::select(vec!["alpha", "beta"]),
        prop::sample::select(vec!["one", "two"]),
        prop::sample::select(vec!["", " ", "  "]),
        prop::sample::select(vec![" ", "  ", " \t "]),
    )
        .prop_map(|(a, b, lead, mid)| format!("{lead}{a}{mid}{b}"))
}

fn child_spec() -> impl Strategy<Value = ChildSpec> {
    (
        child_content(),
        prop::collection::vec(child_content(), 0..2),
    )
        .prop_map(|(content, grandchildren)| ChildSpec {
            content,
            grandchildren,
        })
}

fn tag_set() -> impl Strategy<Value = Vec<String>> {
    // Deliberately NOT `Page`: a Page tag would make the block a document root
    // and change which operation is under test.
    prop::collection::vec(prop::sample::select(vec!["Alpha", "Beta"]), 0..3)
        .prop_map(|tags| dedup_preserving_order(tags.into_iter().map(str::to_string)))
}

/// A property map drawn from `keys`. Deduped by key so the seed writes each
/// key once and the expected value is unambiguous.
fn property_set(keys: Vec<&'static str>) -> impl Strategy<Value = Vec<(String, String)>> {
    prop::collection::vec(
        (
            prop::sample::select(keys),
            prop::sample::select(vec!["one", "two"]),
        ),
        0..3,
    )
    .prop_map(|pairs| {
        let mut out: Vec<(String, String)> = Vec::new();
        for (k, v) in pairs {
            if !out.iter().any(|(seen, _)| seen == k) {
                out.push((k.to_string(), v.to_string()));
            }
        }
        out
    })
}

fn dedup_preserving_order(items: impl Iterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for item in items {
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn merge_case() -> impl Strategy<Value = MergeCase> {
    (
        prop::sample::select(vec!["", "canonical body"]),
        prop::sample::select(vec!["", "duplicate body"]),
        prop::collection::vec(child_spec(), 0..3),
        prop::collection::vec(child_spec(), 0..4),
        tag_set(),
        tag_set(),
        // The canonical's pool deliberately excludes `ID`, so the duplicate's
        // authored one is always a key the canonical LACKS — i.e. exactly the
        // shape the adoption rule has to refuse. The sibling underscore rule
        // is NOT independently observable here: every dispatched write stamps
        // `_provenance`, so the canonical always already holds it.
        property_set(vec!["author", "status"]),
        property_set(vec!["author", "status", "priority"]),
    )
        .prop_map(
            |(
                canonical_content,
                duplicate_content,
                canonical_children,
                duplicate_children,
                canonical_tags,
                duplicate_tags,
                canonical_properties,
                duplicate_properties,
            )| MergeCase {
                canonical_content: canonical_content.to_string(),
                duplicate_content: duplicate_content.to_string(),
                canonical_children,
                duplicate_children,
                canonical_tags,
                duplicate_tags,
                canonical_properties,
                duplicate_properties,
            },
        )
}

fn canon_child_id(i: usize) -> String {
    format!("11111111-0000-0000-0001-{i:012}")
}

fn dup_child_id(i: usize) -> String {
    format!("11111111-0000-0000-0002-{i:012}")
}

fn canon_grandchild_id(i: usize, j: usize) -> String {
    format!("11111111-0000-0000-0003-{i:06}{j:06}")
}

fn dup_grandchild_id(i: usize, j: usize) -> String {
    format!("11111111-0000-0000-0004-{i:06}{j:06}")
}

/// The dedupe key: trim, then collapse every whitespace run to one space.
/// Mirrors the production normalizer; a divergence here is a real red.
fn normalize(content: &str) -> String {
    content.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn uri(id: &str) -> String {
    format!("block:{id}")
}

/// Every block row, keyed by id — the pre/post snapshot property (d) compares.
///
/// `sort_key` is deliberately EXCLUDED: structural ops recompute the positional
/// columns from the live tree rather than restoring the captured value (the
/// same rule the block→page transform's undo fingerprint follows), so sibling
/// ORDER is what must survive an undo, not the fractional key that encodes it.
/// Order is asserted separately via [`ordered_children`].
async fn snapshot(env: &TestEnvironment) -> Vec<(String, String, String)> {
    let rows = env
        .query_sql("SELECT id, parent_id, content FROM block_raw ORDER BY id")
        .await
        .expect("snapshot query failed");
    rows.iter()
        .map(|r| {
            let field = |name: &str| {
                r.get(name)
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string()
            };
            (field("id"), field("parent_id"), field("content"))
        })
        .collect()
}

/// `parent`'s children in sibling order, as (id, content).
async fn ordered_children(env: &TestEnvironment, parent: &str) -> Vec<(String, String)> {
    let rows = env
        .query_sql(&format!(
            "SELECT id, content FROM block_raw WHERE parent_id = '{}' ORDER BY sort_key",
            uri(parent)
        ))
        .await
        .expect("children query failed");
    rows.iter()
        .map(|r| {
            let field = |name: &str| {
                r.get(name)
                    .and_then(|v| v.as_string())
                    .unwrap_or_default()
                    .to_string()
            };
            (field("id"), field("content"))
        })
        .collect()
}

/// `id`'s tags, sorted.
async fn block_tags(env: &TestEnvironment, id: &str) -> Vec<String> {
    let rows = env
        .query_sql(&format!(
            "SELECT tag FROM block_tags WHERE block_id = '{}' ORDER BY tag",
            uri(id)
        ))
        .await
        .expect("tag query failed");
    rows.iter()
        .filter_map(|r| r.get("tag").and_then(|v| v.as_string()).map(str::to_string))
        .collect()
}

/// `id`'s `properties` blob as a key → rendered-value map. The query layer
/// hands the column back already parsed (`Value::Object`), so values are
/// rendered with `{:?}` — a string and a number can never compare equal.
///
/// `_provenance` is dropped: every dispatched write re-stamps it, so the
/// merge's own constituent ops and the undo's inverses both change it. Keeping
/// it would make every before/after comparison fail for a reason that has
/// nothing to do with the merge.
async fn block_properties(env: &TestEnvironment, id: &str) -> BTreeMap<String, String> {
    let rows = env
        .query_sql(&format!(
            "SELECT properties FROM block_raw WHERE id = '{}'",
            uri(id)
        ))
        .await
        .expect("properties query failed");
    let Some(value) = rows.first().and_then(|r| r.get("properties")) else {
        return BTreeMap::new();
    };
    let map = match value {
        Value::Object(map) => map.clone(),
        Value::Null => return BTreeMap::new(),
        other => panic!("properties of {id} must be an object, got {other:?}"),
    };
    map.into_iter()
        .filter(|(k, _)| k != "_provenance")
        .map(|(k, v)| {
            let text = match v {
                Value::String(s) => s,
                other => format!("{other:?}"),
            };
            (k, text)
        })
        .collect()
}

/// Write one `properties` key through the ordinary dispatched `set_field` —
/// the same op the merge itself uses to adopt a property.
async fn set_property(env: &TestEnvironment, id: &str, key: &str, value: &str) {
    let mut params: HashMap<String, Value> = HashMap::new();
    params.insert("id".into(), Value::String(uri(id)));
    params.insert("field".into(), Value::String(key.to_string()));
    params.insert("value".into(), Value::String(value.to_string()));
    env.execute_operation("block", "set_field", params)
        .await
        .unwrap_or_else(|e| panic!("set property {key} on {id}: {e}"));
}

async fn set_tags(env: &TestEnvironment, id: &str, tags: &[String]) {
    let mut params: HashMap<String, Value> = HashMap::new();
    params.insert("id".into(), Value::String(uri(id)));
    params.insert("field".into(), Value::String("tags".into()));
    params.insert(
        "value".into(),
        Value::Array(tags.iter().map(|t| Value::String(t.clone())).collect()),
    );
    env.execute_operation("block", "set_field", params)
        .await
        .unwrap_or_else(|e| panic!("set tags on {id}: {e}"));
}

/// The `resolved_id` of every inbound link, keyed by source block.
async fn link_resolutions(env: &TestEnvironment) -> Vec<(String, String)> {
    let rows = env
        .query_sql("SELECT source_block_id, resolved_id FROM block_links ORDER BY source_block_id")
        .await
        .expect("block_links query failed");
    rows.iter()
        .filter_map(|r| {
            let source = r.get("source_block_id").and_then(|v| v.as_string())?;
            let resolved = r.get("resolved_id").and_then(|v| v.as_string())?;
            Some((source.to_string(), resolved.to_string()))
        })
        .collect()
}

/// Build the pre-merge tree: canonical + its children (each possibly carrying
/// grandchildren), duplicate + its children, tags and properties on both sides,
/// and a linker block whose only mark is an internal link to the duplicate (so
/// `block_links.resolved_id` starts at the duplicate).
async fn seed_tree(env: &TestEnvironment, case: &MergeCase) {
    env.create_block(CANON, ROOT, &case.canonical_content)
        .await
        .expect("create canonical");
    for (i, child) in case.canonical_children.iter().enumerate() {
        env.create_block(&canon_child_id(i), CANON, &child.content)
            .await
            .expect("create canonical child");
        for (j, content) in child.grandchildren.iter().enumerate() {
            env.create_block(&canon_grandchild_id(i, j), &canon_child_id(i), content)
                .await
                .expect("create canonical grandchild");
        }
    }
    env.create_block(DUP, ROOT, &case.duplicate_content)
        .await
        .expect("create duplicate");
    for (i, child) in case.duplicate_children.iter().enumerate() {
        env.create_block(&dup_child_id(i), DUP, &child.content)
            .await
            .expect("create duplicate child");
        for (j, content) in child.grandchildren.iter().enumerate() {
            env.create_block(&dup_grandchild_id(i, j), &dup_child_id(i), content)
                .await
                .expect("create duplicate grandchild");
        }
    }

    set_tags(env, CANON, &case.canonical_tags).await;
    set_tags(env, DUP, &case.duplicate_tags).await;
    for (key, value) in &case.canonical_properties {
        set_property(env, CANON, key, value).await;
    }
    for (key, value) in &case.duplicate_properties {
        set_property(env, DUP, key, value).await;
    }
    // The org-authored `:ID:` the merge must never copy onto the survivor.
    set_property(env, DUP, "ID", AUTHORED_DUPLICATE_ID).await;

    let label = "see the duplicate";
    env.create_block(LINKER, ROOT, label)
        .await
        .expect("create linker");
    let marks = vec![MarkSpan::new(
        0,
        label.chars().count(),
        InlineMark::Link {
            target: EntityRef::from_uri(&holon_api::EntityUri::block(DUP)),
            label: label.to_string(),
        },
    )];
    let mut params: HashMap<String, Value> = HashMap::new();
    params.insert("id".into(), Value::String(uri(LINKER)));
    params.insert("field".into(), Value::String("marks".into()));
    params.insert("value".into(), Value::String(marks_to_json(&marks)));
    env.execute_operation("block", "set_field", params)
        .await
        .expect("link the linker at the duplicate");

    env.wait_for_loro_quiescence(SYNC_TIMEOUT).await;
    // The seed is authored in Loro; the properties this test reads live in the
    // SQL projection, so wait for the projection to actually carry the seeded
    // content rather than for a fixed interval.
    settle_until(env, |rows| {
        let content_of = |id: &str| {
            rows.iter()
                .find(|(rid, _, _)| rid == &uri(id))
                .map(|(_, _, c)| c.clone())
        };
        let grandchildren_present = |children: &[ChildSpec], id_of: fn(usize, usize) -> String| {
            children.iter().enumerate().all(|(i, child)| {
                (0..child.grandchildren.len()).all(|j| {
                    let gid = uri(&id_of(i, j));
                    rows.iter().any(|(rid, _, _)| rid == &gid)
                })
            })
        };
        content_of(CANON).as_deref() == Some(case.canonical_content.trim_end())
            && content_of(DUP).as_deref() == Some(case.duplicate_content.trim_end())
            && rows.iter().filter(|(_, p, _)| p == &uri(CANON)).count()
                == case.canonical_children.len()
            && rows.iter().filter(|(_, p, _)| p == &uri(DUP)).count()
                == case.duplicate_children.len()
            && grandchildren_present(&case.canonical_children, canon_grandchild_id)
            && grandchildren_present(&case.duplicate_children, dup_grandchild_id)
    })
    .await;

    // Tags and properties land in their own projections; the merge PLANNER
    // reads them, so a merge fired before they projected would test nothing.
    let deadline = tokio::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        let dup_props = block_properties(env, DUP).await;
        let canon_props = block_properties(env, CANON).await;
        let dup_tags = block_tags(env, DUP).await;
        let canon_tags = block_tags(env, CANON).await;
        let seeded = dup_props.get("ID").map(String::as_str) == Some(AUTHORED_DUPLICATE_ID)
            && case
                .duplicate_properties
                .iter()
                .all(|(k, v)| dup_props.get(k).map(String::as_str) == Some(v.as_str()))
            && case
                .canonical_properties
                .iter()
                .all(|(k, v)| canon_props.get(k).map(String::as_str) == Some(v.as_str()))
            && case.duplicate_tags.iter().all(|t| dup_tags.contains(t))
            && case.canonical_tags.iter().all(|t| canon_tags.contains(t));
        if seeded {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            let raw = env
                .query_sql("SELECT id, properties FROM block_raw ORDER BY id")
                .await
                .expect("raw properties query");
            panic!(
                "tags/properties never projected; canonical {canon_tags:?} {canon_props:?}, \
                 duplicate {dup_tags:?} {dup_props:?}; raw {raw:?}"
            );
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Poll the block projection until `done` holds. Fails loud on timeout — a
/// silent proceed would turn a projection stall into a bogus property failure.
async fn settle_until(env: &TestEnvironment, done: impl Fn(&[(String, String, String)]) -> bool) {
    let deadline = tokio::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        let rows = snapshot(env).await;
        if done(&rows) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "block projection never settled; last rows: {rows:?}"
        );
        tokio::time::sleep(POLL).await;
    }
}

async fn merge(env: &TestEnvironment) -> anyhow::Result<()> {
    let mut params: HashMap<String, Value> = HashMap::new();
    params.insert("canonical".into(), Value::String(uri(CANON)));
    params.insert("duplicate".into(), Value::String(uri(DUP)));
    env.execute_operation("block", "merge_blocks", params).await
}

async fn run_case(rt: Arc<tokio::runtime::Runtime>, case: MergeCase) {
    let env = TestEnvironmentBuilder::new()
        .with_org_file("Merge.org", root_org())
        .build(rt)
        .await
        .expect("boot");
    env.wait_for_org_files_stable(25, SYNC_TIMEOUT).await;

    seed_tree(&env, &case).await;

    let before = snapshot(&env).await;
    let root_order_before = ordered_children(&env, ROOT).await;
    let canonical_order_before = ordered_children(&env, CANON).await;
    let duplicate_order_before = ordered_children(&env, DUP).await;
    let links_before = link_resolutions(&env).await;
    let canonical_tags_before = block_tags(&env, CANON).await;
    let canonical_props_before = block_properties(&env, CANON).await;
    let inbound_at_duplicate: Vec<String> = links_before
        .iter()
        .filter(|(_, resolved)| resolved == &uri(DUP))
        .map(|(source, _)| source.clone())
        .collect();
    assert!(
        !inbound_at_duplicate.is_empty(),
        "precondition: the linker must resolve to the duplicate before the merge, got \
         {links_before:?}"
    );

    // The pre-merge content multiset the merge must preserve, husks excluded.
    let mut expected_normalized: Vec<String> = case
        .canonical_children
        .iter()
        .chain(case.duplicate_children.iter())
        .map(|c| normalize(&c.content))
        .filter(|c| !c.is_empty())
        .collect();
    expected_normalized.sort();
    expected_normalized.dedup();

    merge(&env).await.expect("merge_blocks");
    env.wait_for_loro_quiescence(SYNC_TIMEOUT).await;
    // The duplicate's row disappearing is the merge's last projected effect.
    settle_until(&env, |rows| !rows.iter().any(|(id, _, _)| id == &uri(DUP))).await;

    // (a) the duplicate's id still resolves — to the canonical.
    let resolved = env
        .engine()
        .resolve_block_id(&holon_api::EntityUri::block(DUP))
        .await
        .expect("resolve the merged-away id");
    assert_eq!(
        resolved.to_string(),
        uri(CANON),
        "(a) the duplicate's id must resolve to the canonical after the merge"
    );

    // (g) the PRODUCTION `BlockReader` — the DI-resolved seam org write-back
    // and the file-sync controller read every block through — resolves the
    // merged-away id to the canonical block, not to "absent".
    {
        use holon_filesystem::BlockReader;
        let reader = env
            .injector()
            .expect("(g) the environment must expose its container")
            .resolve_async::<dyn BlockReader>()
            .await;
        let block = reader
            .get_block_authoritative(&holon_api::EntityUri::block(DUP))
            .await
            .expect("(g) the BlockReader lookup of a merged-away id must not error")
            .expect("(g) the merged-away id must still resolve through the production BlockReader");
        assert_eq!(
            block.id.to_string(),
            uri(CANON),
            "(g) the production BlockReader must resolve the merged-away id to the canonical block"
        );
    }

    let after_children = ordered_children(&env, CANON).await;
    let after_normalized: Vec<String> = after_children
        .iter()
        .map(|(_, content)| normalize(content))
        .filter(|c| !c.is_empty())
        .collect();

    // (b) every pre-merge normalized content still has a survivor, and the
    // merge invented nothing.
    for expected in &expected_normalized {
        assert!(
            after_normalized.contains(expected),
            "(b) normalized content {expected:?} lost by the merge; survivors \
             {after_normalized:?}, case {case:?}"
        );
    }
    let mut invented: Vec<&String> = after_normalized
        .iter()
        .filter(|c| {
            !expected_normalized.contains(c)
                && *c != &normalize(&case.duplicate_content)
                && *c != &normalize(&case.canonical_content)
        })
        .collect();
    invented.sort();
    assert!(
        invented.is_empty(),
        "(b) the merge invented content {invented:?}; case {case:?}"
    );

    // (b, dedupe) one survivor per collapsed group — no normalization-equal
    // siblings remain.
    let mut seen = after_normalized.clone();
    seen.sort();
    let deduped = {
        let mut d = seen.clone();
        d.dedup();
        d
    };
    assert_eq!(
        seen, deduped,
        "(b) normalization-equal siblings survived the dedupe: {after_children:?}"
    );

    // (c) deterministic order: canonical's own children keep their relative
    // order and precede the duplicate's surviving children.
    let surviving_canonical: Vec<String> = after_children
        .iter()
        .map(|(id, _)| id.clone())
        .filter(|id| (0..case.canonical_children.len()).any(|i| id == &uri(&canon_child_id(i))))
        .collect();
    let expected_canonical_order: Vec<String> = (0..case.canonical_children.len())
        .map(|i| uri(&canon_child_id(i)))
        .filter(|id| surviving_canonical.contains(id))
        .collect();
    assert_eq!(
        surviving_canonical, expected_canonical_order,
        "(c) the canonical's own children must keep their relative order"
    );
    let first_dup_slot = after_children.iter().position(|(id, _)| {
        (0..case.duplicate_children.len()).any(|i| id == &uri(&dup_child_id(i)))
    });
    let last_canonical_slot = after_children.iter().rposition(|(id, _)| {
        (0..case.canonical_children.len()).any(|i| id == &uri(&canon_child_id(i)))
    });
    if let (Some(first_dup), Some(last_canon)) = (first_dup_slot, last_canonical_slot) {
        assert!(
            last_canon < first_dup,
            "(c) surviving duplicate children must follow the canonical's own: {after_children:?}"
        );
    }

    // (e) the inbound link now resolves to the canonical.
    let links_after = link_resolutions(&env).await;
    for source in &inbound_at_duplicate {
        let resolved = links_after
            .iter()
            .find(|(s, _)| s == source)
            .map(|(_, r)| r.clone())
            .unwrap_or_else(|| panic!("(e) inbound link from {source} vanished: {links_after:?}"));
        assert_eq!(
            resolved,
            uri(CANON),
            "(e) inbound link from {source} must resolve to the canonical"
        );
    }

    // (b, subtrees) a dedupe loser is deleted only AFTER its children are
    // re-homed under the keeper, so no generated grandchild is ever orphaned.
    let after_rows = snapshot(&env).await;
    let surviving_parent = |id: &str| {
        after_rows
            .iter()
            .find(|(rid, _, _)| rid == &uri(id))
            .map(|(_, parent, _)| parent.clone())
    };
    for (children, gid) in [
        (
            &case.canonical_children,
            canon_grandchild_id as fn(usize, usize) -> String,
        ),
        (&case.duplicate_children, dup_grandchild_id),
    ] {
        for (i, child) in children.iter().enumerate() {
            for j in 0..child.grandchildren.len() {
                let parent = surviving_parent(&gid(i, j)).unwrap_or_else(|| {
                    panic!(
                        "(b) grandchild {} was orphaned by the merge; rows {after_rows:?}",
                        gid(i, j)
                    )
                });
                assert!(
                    after_rows.iter().any(|(rid, _, _)| rid == &parent),
                    "(b) grandchild {} survived under a parent the merge deleted ({parent})",
                    gid(i, j)
                );
            }
        }
    }

    // (h) tags union with the canonical winning conflicts; properties adopted
    // only for keys the canonical LACKS — and never the duplicate's identity.
    //
    // `merged_from` is written AFTER the tag/property adoption and always, so
    // waiting for it synchronizes on the whole of step 5 without presupposing
    // anything the assertions below are testing.
    let deadline = tokio::time::Instant::now() + SYNC_TIMEOUT;
    loop {
        let props = block_properties(&env, CANON).await;
        if props.contains_key("merged_from") {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the merge's provenance never projected onto the canonical; properties {props:?}"
        );
        tokio::time::sleep(POLL).await;
    }
    let canonical_tags_after = block_tags(&env, CANON).await;
    for tag in case.canonical_tags.iter().chain(case.duplicate_tags.iter()) {
        assert!(
            canonical_tags_after.contains(tag),
            "(h) tag {tag:?} must survive the union; canonical now has {canonical_tags_after:?}"
        );
    }
    let canonical_props_after = block_properties(&env, CANON).await;
    assert!(
        !canonical_props_after.contains_key("ID"),
        "(h) the merge must NEVER adopt the duplicate's authored ID — write-back would then \
         render `:ID: {AUTHORED_DUPLICATE_ID}` on the survivor and re-create the split-root \
         shape; canonical properties {canonical_props_after:?}"
    );
    for (key, value) in &case.canonical_properties {
        assert_eq!(
            canonical_props_after.get(key).map(String::as_str),
            Some(value.as_str()),
            "(h) the canonical must win the conflict on {key:?}"
        );
    }
    for (key, value) in &case.duplicate_properties {
        if canonical_props_before.contains_key(key) {
            assert_eq!(
                canonical_props_after.get(key),
                canonical_props_before.get(key),
                "(h) the duplicate must not overwrite {key:?} on the canonical"
            );
        } else {
            assert_eq!(
                canonical_props_after.get(key).map(String::as_str),
                Some(value.as_str()),
                "(h) the canonical must adopt {key:?}, a key it lacked"
            );
        }
    }

    // (f) the pair is already merged — a second merge must fail loud.
    let second = merge(&env).await;
    assert!(
        second.is_err(),
        "(f) merging an already-merged pair must fail loud, got Ok"
    );

    // (d) one undo gesture restores the exact pre-merge state.
    let outcome = env.engine().undo().await.expect("undo the merge");
    assert!(
        outcome.applied(),
        "(d) the merge must undo as one applied gesture, got {outcome:?}"
    );
    env.wait_for_loro_quiescence(SYNC_TIMEOUT).await;
    settle_until(&env, |rows| rows == before).await;

    let restored = snapshot(&env).await;
    assert_eq!(
        restored, before,
        "(d) undo must restore the exact pre-merge block state"
    );
    assert_eq!(
        ordered_children(&env, ROOT).await,
        root_order_before,
        "(d) undo must restore the pre-merge sibling order under the root"
    );
    assert_eq!(
        ordered_children(&env, CANON).await,
        canonical_order_before,
        "(d) undo must restore the canonical's pre-merge child order"
    );
    assert_eq!(
        ordered_children(&env, DUP).await,
        duplicate_order_before,
        "(d) undo must restore the duplicate's pre-merge child order"
    );
    let restored_links = link_resolutions(&env).await;
    assert_eq!(
        restored_links, links_before,
        "(d) undo must restore the exact pre-merge link resolutions"
    );
    assert_eq!(
        block_tags(&env, CANON).await,
        canonical_tags_before,
        "(d) undo must retract the tags the merge unioned onto the canonical"
    );
    assert_eq!(
        block_properties(&env, CANON).await,
        canonical_props_before,
        "(d) undo must retract the properties the merge adopted onto the canonical"
    );
    let redirects = env
        .query_sql("SELECT from_id FROM block_redirects")
        .await
        .expect("redirect query failed");
    assert!(
        redirects.is_empty(),
        "(d) undo must retract the merge's redirect, got {redirects:?}"
    );
}

fn leaf(content: &str) -> ChildSpec {
    ChildSpec {
        content: content.to_string(),
        grandchildren: vec![],
    }
}

/// The shrunk shape that caught the dedupe-collapse undo defect: two children
/// whose normalized content is IDENTICAL, so the merge collapses one and undo
/// must both re-create it and put the pair back in their original order. The
/// loser carries a child, so the orphan re-homing loop and its inverse bucket
/// run here too. Deterministic, so the regression cannot hide behind
/// generator luck.
#[test]
fn merge_blocks_undo_restores_order_after_identical_child_collapse() {
    let rt = runtime();
    rt.block_on(run_case(
        rt.clone(),
        MergeCase {
            canonical_content: String::new(),
            duplicate_content: String::new(),
            canonical_children: vec![],
            duplicate_children: vec![
                leaf("alpha one"),
                ChildSpec {
                    content: "alpha one".to_string(),
                    grandchildren: vec!["beta two".to_string()],
                },
            ],
            canonical_tags: vec!["Alpha".to_string()],
            duplicate_tags: vec!["Beta".to_string()],
            canonical_properties: vec![("author".to_string(), "one".to_string())],
            duplicate_properties: vec![
                ("author".to_string(), "two".to_string()),
                ("status".to_string(), "two".to_string()),
            ],
        },
    ));
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: std::env::var("PROPTEST_CASES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(6),
        max_shrink_iters: 32,
        failure_persistence: None,
        .. ProptestConfig::default()
    })]

    /// The ratified merge properties over generated husk / both-non-empty
    /// shapes with normalization-colliding children (some carrying subtrees)
    /// and tags/properties on both sides.
    #[test]
    fn merge_blocks_preserves_identity_content_and_order(case in merge_case()) {
        let rt = runtime();
        rt.block_on(run_case(rt.clone(), case));
    }
}
