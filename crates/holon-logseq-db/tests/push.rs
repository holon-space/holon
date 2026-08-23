//! W2 — pushing a `BaseDiff` into a LogSeq graph as tail transactions.
//!
//! `push` is the end-to-end leg: Holon hands it the base it last observed and
//! the base it now wants, and it turns the difference into the same tail
//! transactions LogSeq's own transactor would write, flushing through B when
//! the tail overflows.
//!
//! Scope is title/content ONLY. Everything else a `BaseDiff` can report —
//! creation, removal, re-parent, re-order, edges, properties — is REFUSED by
//! name rather than silently dropped, because a push that reports success
//! having applied half of what it was handed is the failure this whole layer
//! exists to prevent.
//!
//! Two measured facts about the committed fixture shape these tests:
//!
//! 1. LogSeq counts 192 of the graph's 213 entities as built-in — its own
//!    property, class and kv pages, which the importer currently admits as
//!    blocks. Push refuses them, and
//!    [`logseqs_own_built_in_verdict_matches_ours`] holds Holon's predicate to
//!    LogSeq's answer entity for entity rather than to a count.
//! 2. That leaves 12 editable blocks, 24 datoms — under the branching factor.
//!    One push therefore CANNOT overflow the tail, so the overflow leg drives
//!    two successive pushes and crosses the boundary mid-push.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use holon_api::Value;
use holon_logseq_db::LogseqDbImporter;
use holon_logseq_db::TransitNode;
use holon_logseq_db::base::BaseBlock;
use holon_logseq_db::base::ImportBase;
use holon_logseq_db::kvs_writer;
use holon_logseq_db::kvs_writer::KvsGraph;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq-db/holontest.sqlite")
}

async fn base_of(path: &Path) -> ImportBase {
    let importer = LogseqDbImporter::new();
    ImportBase::from_import(&importer.import(path).await.expect("imports"))
}

/// Every entity id the graph LIVES — reachable from the eavt root, not the
/// raw rows.
///
/// The distinction is load-bearing. The fixture carries 17 UNREFERENCED rows
/// (B measured them: LogSeq abandons merged-away nodes and its storage layer
/// discards its delete list), and those rows still hold datoms. Scanning
/// `graph.rows` therefore reports entities 197 and 199, which LogSeq's own
/// `d/datoms :eavt` does not — they are garbage, not deletions, and the tail
/// is empty so nothing retracted them.
fn all_entities(graph: &KvsGraph) -> BTreeSet<i64> {
    kvs_writer::datoms_now(graph)
        .expect("the graph reads")
        .into_iter()
        .map(|d| d.e)
        .collect()
}

/// The entities Holon considers built-in.
/// The entities Holon considers built-in — via the PRODUCTION predicate.
///
/// Deliberately not a second implementation. The test used to run its own
/// recursive walk while production used another traversal, which is precisely
/// why neither the missing predicate legs nor the tail blindness could show up
/// as a test failure: two readers cannot disagree with each other if only one
/// of them is ever asserted about.
fn built_in_entities(graph: &KvsGraph) -> BTreeSet<i64> {
    all_entities(graph)
        .into_iter()
        .filter(|e| kvs_writer::is_built_in(graph, *e).expect("read"))
        .collect()
}

/// LogSeq's own verdict, recorded by `oracle/probe_built_in.cljs`.
#[derive(serde::Deserialize)]
struct LogseqVerdict {
    entities: usize,
    built_in: Vec<i64>,
    by_leg: std::collections::BTreeMap<String, usize>,
    non_built_in: Vec<i64>,
}

fn logseq_verdict() -> LogseqVerdict {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/logseq-db/built-in-entities.json");
    serde_json::from_str(
        &std::fs::read_to_string(&path).expect("the recorded verdict is committed"),
    )
    .expect("the recorded verdict parses")
}

/// The blocks a push may legitimately target: in the base, carrying a title,
/// not one of LogSeq's own built-in pages.
fn editable_uuids(graph: &KvsGraph, base: &ImportBase) -> Vec<String> {
    let built_in = built_in_entities(graph);
    let mut out: Vec<String> = base
        .uuids()
        .filter(|u| {
            let block = base.get(u).expect("present");
            if block.content.is_empty() || block.parent_id.starts_with("sentinel:") {
                return false;
            }
            kvs_writer::entity_by_uuid(graph, u)
                .expect("read")
                .is_some_and(|e| !built_in.contains(&e))
        })
        .map(str::to_string)
        .collect();
    out.sort();
    out
}

/// The uuid LogSeq gave `entity`, read from the live graph.
fn uuid_of(graph: &KvsGraph, entity: i64) -> String {
    kvs_writer::datoms_now(graph)
        .expect("read")
        .into_iter()
        .find_map(|d| match (d.e == entity, d.a.as_str(), d.v) {
            (true, "block/uuid", TransitNode::Uuid(u)) => Some(u),
            _ => None,
        })
        .expect("the entity carries a uuid")
}

/// Add a base entry for an entity the importer deliberately does NOT
/// materialize.
///
/// Push's built-in refusal is a BACKSTOP: since LW-7.a the base cannot
/// normally carry a built-in block at all, so the only way to reach the guard
/// is to hand push a base that claims one exists — which is exactly the
/// Holon-side bug the backstop is there to stop. Constructing it is the point,
/// not a workaround.
fn base_claiming(graph: &KvsGraph, base: &ImportBase, entity: i64) -> (ImportBase, String) {
    let uuid = uuid_of(graph, entity);
    let mut next = base.clone();
    next.witness_create(
        &uuid,
        BaseBlock {
            content: kvs_writer::datoms_now(graph)
                .expect("read")
                .into_iter()
                .find_map(|d| match (d.e == entity, d.a.as_str(), d.v) {
                    (true, "block/title", TransitNode::Str(t)) => Some(t),
                    _ => None,
                })
                .expect("the entity carries a title"),
            ..BaseBlock::default()
        },
    )
    .expect("a uuid the base does not hold");
    (next, uuid)
}

/// A titled entity LogSeq owns, by its own evidence (not the page leg).
fn a_titled_built_in(graph: &KvsGraph) -> i64 {
    let datoms = kvs_writer::datoms_now(graph).expect("read");
    let titled: BTreeSet<i64> = datoms
        .iter()
        .filter(|d| d.a == "block/title")
        .map(|d| d.e)
        .collect();
    titled
        .into_iter()
        .find(|e| kvs_writer::is_built_in(graph, *e).expect("read"))
        .expect("the fixture has 192 built-ins, many titled")
}

/// The editable blocks push will actually accept today.
///
/// `editable_uuids` is the census fact — what the base holds that carries a
/// title. This is the subset push can WRITE, which excludes
/// titles carrying ref syntax: 8 editable, 2 ref-bearing, 6 pushable. Kept as
/// a separate helper rather than folded into `editable_uuids` so the cost of
/// the fail-closed ruling stays visible as a number, not hidden in a filter.
fn pushable_uuids(graph: &KvsGraph, base: &ImportBase) -> Vec<String> {
    editable_uuids(graph, base)
        .into_iter()
        .filter(|u| {
            let c = &base.get(u).expect("present").content;
            !(c.contains("[[") || c.contains("((") || c.contains('#'))
        })
        .collect()
}

/// `base` with `uuid`'s content replaced.
fn retitled(base: &ImportBase, uuid: &str, content: &str) -> ImportBase {
    let mut next = base.clone();
    let block = BaseBlock {
        content: content.to_string(),
        ..base.get(uuid).expect("present").clone()
    };
    next.advance(uuid, block).expect("a known uuid");
    next
}

// --------------------------------------------------------------- the pinning

/// Holon's built-in verdict is LogSeq's, entity for entity.
///
/// A count would not do. Holon's first attempt at this predicate checked only
/// `:logseq.property/built-in?` and found 179 of LogSeq's 192 — the 13 it
/// missed being 5 file entities (`logseq/config.edn` and friends, which carry
/// no flag at all) and 8 `:logseq.kv/*` entries. Every one of the 13 is
/// untitled, so `current_title` refused them first and no behaviour test could
/// tell. Only comparing the SETS catches that, which is what this does.
///
/// The verdict is committed rather than computed, so the pin holds without the
/// oracle installed and moves only when LogSeq's own answer moves. Regenerate
/// with `oracle/probe_built_in.cljs` — see docs/Testing/LogseqDbOracle.md.
#[tokio::test]
async fn logseqs_own_built_in_verdict_matches_ours() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let logseq = logseq_verdict();

    let live = all_entities(&graph);
    assert_eq!(
        live.len(),
        logseq.entities,
        "Holon and LogSeq must be looking at the same set of entities"
    );

    assert_eq!(
        logseq.by_leg,
        [
            ("flag".to_string(), 179),
            ("file-path".to_string(), 5),
            ("internal-ident".to_string(), 8),
        ]
        .into_iter()
        .collect::<std::collections::BTreeMap<_, _>>(),
        "the recorded verdict must still be the three-legged one this predicate \
         was written against"
    );

    let ours = built_in_entities(&graph);
    let theirs: BTreeSet<i64> = logseq.built_in.iter().copied().collect();
    let missed: Vec<i64> = theirs.difference(&ours).copied().collect();
    let over: Vec<i64> = ours.difference(&theirs).copied().collect();
    assert_eq!(
        (missed.as_slice(), over.as_slice()),
        (&[][..], &[][..]),
        "built-in verdicts differ: LogSeq says built-in and Holon does not for \
         {missed:?}; Holon says built-in and LogSeq does not for {over:?}"
    );
    assert_eq!(ours.len(), 192, "LogSeq counts 192 built-in entities");

    // Non-vacuity: the two sets agreeing means nothing if either is everything.
    assert_eq!(
        logseq.non_built_in.len(),
        21,
        "21 entities are NOT built-in, so the predicate is discriminating"
    );
}

/// The numbers every later test in this file rests on.
///
/// They are not decoration: if a future importer change stops admitting
/// LogSeq's built-in property pages as blocks, the editable set grows, one
/// push starts being able to overflow the tail, and the two-push overflow leg
/// below silently stops testing overflow. This is where that shows up.
#[tokio::test]
async fn the_fixtures_built_in_share_is_pinned() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;

    // The base holds ONLY what a person authored. LogSeq's own property,
    // class and kv pages are read for schema knowledge and never materialized
    // as blocks (LW-7.a).
    assert_eq!(
        built_in_entities(&graph).len(),
        192,
        "LogSeq still counts 192 of the graph's 213 entities as built-in — the \
         exclusion is about the BASE, not the graph"
    );
    let built_in = built_in_entities(&graph);
    let in_base: Vec<&str> = base
        .uuids()
        .filter(|u| {
            kvs_writer::entity_by_uuid(&graph, u)
                .expect("read")
                .is_some_and(|e| built_in.contains(&e))
        })
        .collect();
    assert_eq!(
        in_base,
        Vec::<&str>::new(),
        "no built-in entity may be materialized as a block in the base"
    );
    assert_eq!(
        base.len(),
        17,
        "the base holds only what a person authored: 213 entities, 192 LogSeq \
         built-ins, 4 more excluded because they sit on a built-in PAGE"
    );

    // The page-leg exclusion is DISCLOSED, not inferred from a count that
    // came out lower than expected.
    let importer = LogseqDbImporter::new();
    let result = importer.import(&fixture()).await.expect("imports");
    assert_eq!(
        result.stats.excluded_under_built_in_page, 4,
        "the four per-view UI records on $$$views, named in the import summary"
    );
    assert_eq!(
        result.stats.built_in_entities, 189,
        "185 built-in-by-own-evidence blocks plus the 4 page-owned ones"
    );

    let editable = editable_uuids(&graph, &base);
    assert_eq!(
        editable.len(),
        8,
        "8 blocks are titled, non-sentinel and pushable; got {editable:?}"
    );
    assert!(
        editable.len() * 2 <= 32,
        "one push over every editable block cannot exceed the branching factor"
    );
}

// ------------------------------------------------------------ the happy path

/// Two title edits go in as two tail transactions and come back out of a
/// re-import.
///
/// The assertion is `re-imported base == base_after`, not `diff is smaller`:
/// a push that applied one of the two edits, or applied both to the wrong
/// blocks, fails here. A push that did nothing fails at the tail count first.
#[tokio::test]
async fn a_push_of_two_title_edits_survives_a_re_import() {
    let dir = tempfile::tempdir().expect("tempdir");
    let copy = dir.path().join("pushed.sqlite");

    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let before = base_of(&fixture()).await;
    let targets = editable_uuids(&graph, &before);

    let after = retitled(
        &retitled(&before, &targets[0], "pushed title one"),
        &targets[1],
        "pushed title two",
    );

    let report = kvs_writer::push(&mut graph, &before, &after).expect("push");
    assert_eq!(report.transactions, 2, "one transaction per changed block");
    assert_eq!(
        report.datoms, 4,
        "a retract and an assert per changed block"
    );
    assert_eq!(report.flushes, 0, "four datoms do not overflow the tail");
    assert_eq!(
        graph.tail().expect("tail").datom_count(),
        4,
        "the edits are IN the tail, not merely reported"
    );

    kvs_writer::write_graph(&graph, &copy).await.expect("write");
    let reimported = base_of(&copy).await;
    assert_eq!(
        reimported.diff_against(&after),
        Default::default(),
        "a re-import must observe exactly the base that was pushed"
    );
}

/// Pushing a base against itself writes nothing at all.
#[tokio::test]
async fn a_no_op_push_touches_nothing() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let rows_before = graph.rows.clone();
    let base = base_of(&fixture()).await;

    let report = kvs_writer::push(&mut graph, &base, &base).expect("push");
    assert_eq!(report.transactions, 0, "nothing changed");
    assert_eq!(report.datoms, 0, "nothing changed");
    assert_eq!(report.flushes, 0, "nothing changed");
    assert_eq!(
        graph.rows, rows_before,
        "a no-op push must leave every row exactly as it found it"
    );
}

// ---------------------------------------------------------------- refusals

/// Push `make`'s two bases and require a refusal that changed nothing.
///
/// The "changed nothing" half is the point of the helper: every refusal below
/// would still read as a refusal if push had already appended half the
/// transactions before hitting it, and that graph is the one nobody can
/// recover from.
async fn refused(
    make: impl FnOnce(&KvsGraph, &ImportBase) -> (ImportBase, ImportBase),
) -> kvs_writer::RowError {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let rows_before = graph.rows.clone();

    let tail_before = graph.tail().expect("tail").datom_count();

    let (before, after) = make(&graph, &base);
    let err = kvs_writer::push(&mut graph, &before, &after).expect_err("must be refused");
    assert_eq!(
        graph.rows, rows_before,
        "a refused push must leave every row exactly as it found it"
    );
    assert_eq!(
        graph.tail().expect("tail").datom_count(),
        tail_before,
        "a refused push must not leave a transaction in the tail either"
    );
    err
}

#[tokio::test]
async fn creating_a_block_is_refused_by_name() {
    let err = refused(|_, base| {
        let mut after = base.clone();
        after
            .witness_create("11111111-2222-3333-4444-555555555555", BaseBlock::default())
            .expect("a fresh uuid");
        (base.clone(), after)
    })
    .await;
    assert!(
        matches!(&err, kvs_writer::RowError::PushOutOfScope { shape, .. } if *shape == "block creation"),
        "got {err}"
    );
}

#[tokio::test]
async fn removing_a_block_is_refused_by_name() {
    let err = refused(|graph, base| {
        let mut after = base.clone();
        after
            .retract(&editable_uuids(graph, base)[0])
            .expect("a known uuid");
        (base.clone(), after)
    })
    .await;
    assert!(
        matches!(&err, kvs_writer::RowError::PushOutOfScope { shape, .. } if *shape == "block removal"),
        "got {err}"
    );
}

#[tokio::test]
async fn re_parenting_a_block_is_refused_by_name() {
    let err = refused(|graph, base| {
        let targets = editable_uuids(graph, base);
        let mut after = base.clone();
        let moved = BaseBlock {
            parent_id: format!("block:{}", targets[1]),
            ..base.get(&targets[0]).expect("present").clone()
        };
        after.advance(&targets[0], moved).expect("a known uuid");
        (base.clone(), after)
    })
    .await;
    assert!(
        matches!(&err, kvs_writer::RowError::PushOutOfScope { shape, .. } if *shape == "re-parent"),
        "got {err}"
    );
}

#[tokio::test]
async fn re_ordering_a_block_is_refused_by_name() {
    let err = refused(|graph, base| {
        let targets = editable_uuids(graph, base);
        let mut after = base.clone();
        let current = base.get(&targets[0]).expect("present").clone();
        let moved = BaseBlock {
            position: Some(current.position.unwrap_or(0) + 7),
            ..current
        };
        after.advance(&targets[0], moved).expect("a known uuid");
        (base.clone(), after)
    })
    .await;
    assert!(
        matches!(&err, kvs_writer::RowError::PushOutOfScope { shape, .. } if *shape == "re-order"),
        "got {err}"
    );
}

/// EVERY field push cannot write is refused, each by its own name.
///
/// All 206 of the fixture's blocks carry empty `requires`, `contributes_to`,
/// `advice_suppressed` and `properties`, so these four arms are unreachable
/// from fixture data alone and would sit in the source undriven. Each case
/// below perturbs the field directly, which is the only way to reach them.
///
/// TAGS are not here: push writes them, and the ways a tag edit is refused
/// are more specific than "out of scope" — an unresolvable class name and a
/// stale observation, each with its own test.
#[tokio::test]
async fn changing_any_non_title_field_is_refused_by_its_own_name() {
    /// The perturbation one case applies to a block.
    type Perturb = fn(BaseBlock) -> BaseBlock;

    let cases: Vec<(&str, Perturb)> = vec![
        ("requires-edge change", |b| BaseBlock {
            requires: vec!["block:11111111-2222-3333-4444-555555555555".to_string()],
            ..b
        }),
        ("contributes-to-edge change", |b| BaseBlock {
            contributes_to: vec!["block:11111111-2222-3333-4444-555555555555".to_string()],
            ..b
        }),
        ("advice-suppression change", |b| BaseBlock {
            advice_suppressed: vec!["some-advice".to_string()],
            ..b
        }),
        ("property change", |b| {
            let mut properties = b.properties.clone();
            properties.insert("invented".to_string(), Value::String("value".to_string()));
            BaseBlock { properties, ..b }
        }),
    ];

    for (expected, perturb) in cases {
        let err = refused(|graph, base| {
            let target = editable_uuids(graph, base)[0].clone();
            let mut after = base.clone();
            after
                .advance(
                    &target,
                    perturb(base.get(&target).expect("present").clone()),
                )
                .expect("a known uuid");
            (base.clone(), after)
        })
        .await;
        assert!(
            matches!(&err, kvs_writer::RowError::PushOutOfScope { shape, .. } if *shape == expected),
            "expected a {expected:?} refusal, got {err}"
        );
    }
}

/// LogSeq's storage layer would accept this edit; its outliner would not.
/// Holon writes the storage layer directly, so this is the only thing standing
/// between a Holon-side bug and a rewritten schema.
#[tokio::test]
async fn editing_one_of_logseqs_built_in_pages_is_refused() {
    let err = refused(|graph, base| {
        let entity = a_titled_built_in(graph);
        let (claimed, uuid) = base_claiming(graph, base, entity);
        (
            claimed.clone(),
            retitled(&claimed, &uuid, "Holon rewrote your schema"),
        )
    })
    .await;
    assert!(
        matches!(&err, kvs_writer::RowError::PushBuiltIn { .. }),
        "got {err}"
    );
}

/// A push whose diff touches ONLY built-ins is a refusal, not a quiet no-op.
///
/// The distinction matters because "there was nothing I could legally do" and
/// "there was nothing to do" must not look the same to a caller: the first
/// means Holon is holding edits LogSeq will never receive.
#[tokio::test]
async fn a_push_of_nothing_but_built_ins_is_refused_rather_than_a_no_op() {
    let err = refused(|graph, base| {
        let datoms = kvs_writer::datoms_now(graph).expect("read");
        let titled: BTreeSet<i64> = datoms
            .iter()
            .filter(|d| d.a == "block/title")
            .map(|d| d.e)
            .collect();
        let victims: Vec<i64> = titled
            .into_iter()
            .filter(|e| kvs_writer::is_built_in(graph, *e).expect("read"))
            .take(3)
            .collect();
        assert_eq!(victims.len(), 3, "need three titled built-ins");

        let mut before = base.clone();
        let mut uuids = Vec::new();
        for entity in victims {
            let (next, uuid) = base_claiming(graph, &before, entity);
            before = next;
            uuids.push(uuid);
        }
        let mut after = before.clone();
        for (i, uuid) in uuids.iter().enumerate() {
            after = retitled(&after, uuid, &format!("schema rewrite {i}"));
        }
        (before, after)
    })
    .await;
    assert!(
        matches!(&err, kvs_writer::RowError::PushBuiltIn { .. }),
        "got {err}"
    );
}

/// The base must describe the graph. If LogSeq moved underneath it, the
/// retract half of the replacement would name a value that is no longer there.
///
/// Driven with FOUR changed blocks, one of them stale, because that is the
/// case the all-or-nothing rule exists for: the three legal edits must not be
/// written, and with a single-block push that claim is vacuous. The stale
/// block is deliberately the LAST in push order, so a writer that validated
/// and applied block by block would already have written three transactions
/// by the time it noticed.
#[tokio::test]
async fn one_stale_block_refuses_the_whole_push_by_name() {
    let mut stale_uuid = String::new();
    let mut truth = String::new();
    let err = refused(|graph, base| {
        let targets = editable_uuids(graph, base);
        assert!(targets.len() >= 4, "need four user-authored blocks");
        // `diff.changed` follows the base's uuid order, so the last of these
        // four is the last push would reach.
        let victim = targets[3].clone();
        stale_uuid = victim.clone();
        truth = base.get(&victim).expect("present").content.clone();

        // `before` misdescribes ONLY the victim; the other three agree with
        // the graph and are therefore perfectly pushable on their own.
        let before = retitled(base, &victim, "what Holon wrongly believes LogSeq holds");
        let mut after = before.clone();
        for (i, uuid) in targets.iter().take(4).enumerate() {
            after = retitled(&after, uuid, &format!("would-be title {i}"));
        }
        (before, after)
    })
    .await;

    match &err {
        kvs_writer::RowError::PushBaseStale {
            uuid,
            expected,
            found,
        } => {
            assert_eq!(
                *uuid, stale_uuid,
                "the error must name the block that is actually stale"
            );
            // Both sides of the disagreement, because an error that says only
            // "stale" leaves the reader to re-derive what LogSeq actually holds.
            assert_eq!(
                expected, "what Holon wrongly believes LogSeq holds",
                "the error must quote what the base recorded"
            );
            assert_eq!(
                *found, truth,
                "the error must quote what the graph actually holds"
            );
            assert_ne!(truth, "", "the block under test must have a real title");
        }
        other => panic!("got {other}"),
    }
    // `refused` has already asserted rows AND tail are untouched, which is the
    // "the other three were not written" half of the claim.
}

#[tokio::test]
async fn a_block_the_graph_does_not_have_is_refused() {
    let err = refused(|_, base| {
        let ghost = "99999999-8888-7777-6666-555555555555";
        let mut before = base.clone();
        before
            .witness_create(
                ghost,
                BaseBlock {
                    content: "not in the graph".to_string(),
                    ..BaseBlock::default()
                },
            )
            .expect("a fresh uuid");
        (before.clone(), retitled(&before, ghost, "still not there"))
    })
    .await;
    assert!(
        matches!(&err, kvs_writer::RowError::PushUnknownBlock { .. }),
        "got {err}"
    );
}

// ----------------------------------------------------------------- overflow

/// THREE pushes over the 8 editable blocks cross the branching factor
/// MID-push and flush through B.
///
/// REDESIGNED, not re-pinned. Under LW-7.a the editable set is 8 blocks, not
/// 12, and 8 blocks is 16 datoms — so push 1 reaches 16 and push 2 reaches
/// exactly 32, which does NOT exceed the branching factor and therefore does
/// NOT flush. Reaching an overflow at all now takes a THIRD push, which
/// crosses on its very first block. Changing the numbers alone would have
/// produced a test that no longer exercised a flush while still looking like
/// it did.
#[tokio::test]
async fn a_third_push_overflows_the_tail_and_flushes_through_the_trees() {
    let dir = tempfile::tempdir().expect("tempdir");
    let copy = dir.path().join("overflowed.sqlite");

    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let mut base = base_of(&fixture()).await;
    let targets = pushable_uuids(&graph, &base);
    assert_eq!(targets.len(), 6, "the PUSHABLE set this leg is sized for");

    let mut flushes = Vec::new();
    let mut tails = Vec::new();
    for round in 0..3 {
        let mut next = base.clone();
        for (i, uuid) in targets.iter().enumerate() {
            next = retitled(&next, uuid, &format!("overflow round {round} {i:02}"));
        }
        let report = kvs_writer::push(&mut graph, &base, &next).expect("push");
        assert_eq!(report.transactions, 6, "every pushable block changed");
        flushes.push(report.flushes);
        tails.push(graph.tail().expect("tail").datom_count());
        base = next;
    }

    // MEASURED, not predicted: 16 after push 1, 32 after push 2 (at the limit
    // and still not over it), and after push 3 the first block crosses to 34,
    // flushes all of them, and the remaining seven leave 14 behind.
    // MEASURED at 6 pushable blocks (12 datoms per push): 12, then 24, then
    // push 3 crosses on its FIFTH block (24+10=34), flushes all 34, and the
    // sixth block leaves 2 behind.
    assert_eq!(
        (flushes.as_slice(), tails.as_slice()),
        (&[0, 0, 1][..], &[12, 24, 2][..]),
        "the flush must land on push THREE, and the tail lengths pin exactly \
         where — a `< 32` style assertion would also pass a flush a push early"
    );
    assert!(
        graph.rows.len() > 456,
        "a flush that rebuilt the trees added rows; got {}",
        graph.rows.len()
    );

    kvs_writer::write_graph(&graph, &copy).await.expect("write");
    let reimported = base_of(&copy).await;
    assert_eq!(
        reimported.diff_against(&base),
        Default::default(),
        "the flushed transactions must survive into the trees, not be dropped \
         by the flush"
    );
}

// ------------------------------------------------------------- the oracles

struct Oracle {
    deps_db: PathBuf,
}

impl Oracle {
    fn find() -> Self {
        const SETUP: &str = "see docs/Testing/LogseqDbOracle.md, or run `just lsqdb-oracle`";
        let root = std::env::var_os("HOLON_LOGSEQ_ORACLE")
            .unwrap_or_else(|| panic!("HOLON_LOGSEQ_ORACLE is not set ({SETUP})"));
        let deps_db = PathBuf::from(root).join("deps/db");
        assert!(
            deps_db.join("node_modules").is_dir(),
            "{} has no node_modules ({SETUP})",
            deps_db.display()
        );
        Self { deps_db }
    }

    fn run(&self, script: &str, graphs: &[&Path], flags: &[&str]) -> String {
        let nbb = self.deps_db.join("node_modules/.bin/nbb-logseq");
        let mut cmd = std::process::Command::new(&nbb);
        cmd.current_dir(&self.deps_db).arg(script);
        for graph in graphs {
            cmd.arg(graph);
        }
        cmd.args(flags);
        let out = cmd
            .output()
            .unwrap_or_else(|e| panic!("running {}: {e}", nbb.display()));
        let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&out.stderr));
        text
    }
}

/// The title round `r` gives the block at index `i`.
fn pushed_title(round: usize, i: usize) -> String {
    format!("push oracle r{round} b{i:02}")
}

/// Drive the two-push sequence and hand back the graph plus the edits it made,
/// as `(entity, title)` in push order — the exact list LogSeq is asked to
/// apply in the head-to-head below.
async fn three_pushes() -> (KvsGraph, Vec<(i64, String)>) {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let mut base = base_of(&fixture()).await;
    let targets = pushable_uuids(&graph, &base);

    let mut edits = Vec::new();
    let mut flushes = 0;
    for round in 0..3 {
        let mut next = base.clone();
        for (i, uuid) in targets.iter().enumerate() {
            next = retitled(&next, uuid, &pushed_title(round, i));
        }
        let report = kvs_writer::push(&mut graph, &base, &next).expect("push");
        // The head-to-head hands LogSeq this order; if push chose another one
        // the two writers would be making different edits and a byte
        // difference would say nothing about the writer.
        let expected: Vec<i64> = targets
            .iter()
            .map(|u| {
                kvs_writer::entity_by_uuid(&graph, u)
                    .expect("read")
                    .expect("entity")
            })
            .collect();
        assert_eq!(
            report.blocks, expected,
            "push must edit blocks in the base's uuid order"
        );
        for (i, entity) in expected.iter().enumerate() {
            edits.push((*entity, pushed_title(round, i)));
        }
        flushes += report.flushes;
        base = next;
    }
    assert_eq!(edits.len(), 18, "6 blocks over three rounds");
    // THREE rounds, not two: 8 blocks is 16 datoms, so two pushes reach
    // exactly 32 and never cross it. Without a third the legs below would
    // compare two tail-only files and could not see a flush at all.
    assert_eq!(flushes, 1, "the third push must have flushed");
    (graph, edits)
}

async fn pushed_copy(dir: &Path) -> Vec<(i64, String)> {
    let (graph, edits) = three_pushes().await;
    std::fs::create_dir_all(dir).expect("dir");
    kvs_writer::write_graph(&graph, &dir.join("db.sqlite"))
        .await
        .expect("write");
    edits
}

/// LEG 1 — LogSeq's own validator accepts a graph Holon pushed into.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn leg1_logseq_validator_accepts_a_pushed_graph() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");
    let copy_dir = dir.path().join("pushed");
    pushed_copy(&copy_dir).await;

    let out = oracle.run(
        "script/validate_db.cljs",
        &[&copy_dir.join("db.sqlite")],
        &["--closed-maps", "--group-errors"],
    );
    assert!(
        out.contains("Valid!"),
        "LogSeq's validator rejected a graph Holon pushed into:\n{out}"
    );
    // Non-vacuity: a validator that read a truncated graph would also not
    // complain about the entities it never saw.
    assert!(
        out.contains(":datoms 2609"),
        "the validator must have read the whole graph back:\n{out}"
    );
}

/// LEG 2 — the delta LogSeq sees is exactly the 6 final titles, nothing else.
///
/// Six, not eighteen: each round supersedes the last, so a correct push
/// leaves no trace of either set of intermediate titles. A push that
/// asserted without retracting would show them, and this is where that shows.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn leg2_logseq_diff_shows_exactly_the_pushed_titles() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");
    let pristine_dir = dir.path().join("pristine");
    let copy_dir = dir.path().join("pushed");
    std::fs::create_dir_all(&pristine_dir).expect("dir");
    std::fs::copy(fixture(), pristine_dir.join("db.sqlite")).expect("stage");
    pushed_copy(&copy_dir).await;

    let out = oracle.run(
        "script/diff_graphs.cljs",
        &[&pristine_dir.join("db.sqlite"), &copy_dir.join("db.sqlite")],
        &["-T"],
    );

    let entries: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('[') && l.contains(']'))
        .collect();
    assert_eq!(
        entries.len(),
        12,
        "expected 6 datoms per side and nothing else; got {}:\n{out}",
        entries.len()
    );
    for entry in &entries {
        assert!(
            entry.starts_with("[nil nil "),
            "every difference must be a VALUE change on one block's one \
             attribute: {entry}\n{out}"
        );
    }
    for i in 0..6 {
        assert!(
            out.contains(&pushed_title(2, i)),
            "final title {i} is missing from the delta:\n{out}"
        );
        for superseded in [0, 1] {
            assert!(
                !out.contains(&pushed_title(superseded, i)),
                "round-{superseded} title {i} is still in the graph, so a \
                 retract was skipped:\n{out}"
            );
        }
    }
}

/// HEAD TO HEAD — the same 18 edits, applied by LogSeq and by Holon's `push`,
/// compared row by row.
///
/// B established byte identity for the tail-and-flush machinery, but every one
/// of its 17 targets was a LogSeq built-in page. Push refuses those, so this
/// re-establishes the same claim on the blocks push can actually reach — and
/// through the `push` entry point rather than a hand-picked entity list.
///
/// LogSeq is handed exactly the edit list push made, in push's order, so any
/// difference in the output is a difference in the WRITER and nothing else.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn head_to_head_with_logseq_applying_the_same_pushes() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");

    // --- Holon's side, first: the edit list is whatever push decided to make.
    let holon_dir = dir.path().join("holon");
    let edits = pushed_copy(&holon_dir).await;
    let holon_db = holon_dir.join("db.sqlite");

    let edits_json = dir.path().join("edits.json");
    std::fs::write(
        &edits_json,
        serde_json::to_string(&edits).expect("edits serialize"),
    )
    .expect("write edits");

    // --- LogSeq's side
    let logseq_dir = dir.path().join("logseq");
    std::fs::create_dir_all(&logseq_dir).expect("dir");
    let logseq_db = logseq_dir.join("db.sqlite");
    std::fs::copy(fixture(), &logseq_db).expect("stage");
    let out = oracle.run("script/apply_edits.cljs", &[&logseq_db, &edits_json], &[]);
    assert!(
        out.contains("applied 18 edits"),
        "LogSeq did not apply the edits:\n{out}"
    );

    // --- Compare, row by row
    let theirs = kvs_writer::read_graph(&logseq_db)
        .await
        .expect("logseq reads");
    let ours = kvs_writer::read_graph(&holon_db)
        .await
        .expect("holon reads");

    let addrs: BTreeSet<i64> = theirs
        .rows
        .iter()
        .chain(ours.rows.iter())
        .map(|r| r.addr)
        .collect();
    let (mut identical, mut differing, mut only_theirs, mut only_ours) = (0, 0, 0, 0);
    let mut first_differences: Vec<String> = Vec::new();
    for addr in &addrs {
        let t = theirs.rows.iter().find(|r| r.addr == *addr);
        let o = ours.rows.iter().find(|r| r.addr == *addr);
        match (t, o) {
            (Some(t), Some(o)) => {
                if t.original_content == o.original_content && t.addresses == o.addresses {
                    identical += 1;
                } else {
                    differing += 1;
                    if first_differences.len() < 4 {
                        let at = t
                            .original_content
                            .chars()
                            .zip(o.original_content.chars())
                            .position(|(a, b)| a != b)
                            .unwrap_or(0);
                        let from = at.saturating_sub(60);
                        let slice = |s: &str| s.chars().skip(from).take(160).collect::<String>();
                        first_differences.push(format!(
                            "addr {addr} diverges at char {at}\n  logseq: {}\n  holon:  {}\n  \
                             addresses: {:?} vs {:?}",
                            slice(&t.original_content),
                            slice(&o.original_content),
                            t.addresses,
                            o.addresses
                        ));
                    }
                }
            }
            (Some(_), None) => only_theirs += 1,
            (None, Some(_)) => only_ours += 1,
            (None, None) => unreachable!("addr came from one of the two"),
        }
    }

    assert_eq!(
        (differing, only_theirs, only_ours),
        (0, 0, 0),
        "the two writers disagree: {identical} identical, {differing} differing, \
         {only_theirs} only LogSeq's, {only_ours} only Holon's\n{}",
        first_differences.join("\n")
    );
    assert_eq!(
        identical,
        addrs.len(),
        "every row must have been compared, not skipped"
    );
    assert_eq!(
        (identical, addrs.len(), ours.rows.len(), theirs.rows.len()),
        (459, 459, 459, 459),
        "both writers grow the fixture's 456 rows to 459 for these 18 edits — \
         and every row must have been compared, not skipped. The count is \
         MEASURED per edit-set, not a constant: at 24 edits both writers \
         produced 458. What is invariant is that they AGREE, not the number."
    );
}

/// BISECT — the two writers agree at EVERY prefix of the 24 pushed edits, not
/// only at the end.
///
/// A single end-state comparison can be green because two divergences
/// cancelled. This walks N = 1..24 and compares the files.
///
/// NO forced store and NO unconditional flush on either side. That is the
/// whole point and an earlier version of this test got it wrong: it called
/// `flush_tail` unconditionally and passed `--store`, which made both sides
/// write trees at every N and left the test unable to notice a SKIPPED flush —
/// it stayed green under a no-flush mutation that reddened the head-to-head.
/// It also drove `replace_block_title` in a loop, so it exercised none of
/// push's planning, guards, or flush decision despite claiming to. Both sides
/// now flush only when their own rule says to, which is the behaviour under
/// test.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn bisect_the_pushed_edits_prefix_by_prefix() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");
    let (_, edits) = three_pushes().await;
    assert_eq!(edits.len(), 18, "the bisect walks the same 18 edits");

    let pristine = base_of(&fixture()).await;
    let base_graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let targets = pushable_uuids(&base_graph, &pristine);

    let mut first_divergence: Option<(usize, usize)> = None;
    let mut agreed = 0usize;
    let mut flushed_at = Vec::new();
    for n in 1..=edits.len() {
        let prefix: Vec<(i64, String)> = edits.iter().take(n).cloned().collect();
        let edits_json = dir.path().join(format!("edits{n}.json"));
        std::fs::write(&edits_json, serde_json::to_string(&prefix).expect("json")).expect("write");

        // LogSeq applies the same prefix and flushes only on its OWN rule.
        let theirs_path = dir.path().join(format!("logseq{n}.sqlite"));
        std::fs::copy(fixture(), &theirs_path).expect("stage");
        oracle.run("script/apply_edits.cljs", &[&theirs_path, &edits_json], &[]);

        // Holon reaches the same prefix through `push` — planning, guards and
        // flush decision included — by pushing whole rounds and then a partial
        // one, which is how a caller would actually get there.
        let ours_path = dir.path().join(format!("holon{n}.sqlite"));
        let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
        let mut base = pristine.clone();
        let mut applied = 0usize;
        for round in 0..3 {
            if applied >= n {
                break;
            }
            let take = (n - applied).min(targets.len());
            let mut next = base.clone();
            for (i, uuid) in targets.iter().take(take).enumerate() {
                next = retitled(&next, uuid, &pushed_title(round, i));
            }
            let report = kvs_writer::push(&mut graph, &base, &next).expect("push");
            if report.flushes > 0 {
                flushed_at.push(n);
            }
            applied += take;
            base = next;
        }
        assert_eq!(applied, n, "the prefix was applied in full");
        kvs_writer::write_graph(&graph, &ours_path)
            .await
            .expect("write");

        let theirs = kvs_writer::read_graph(&theirs_path).await.expect("reads");
        let ours = kvs_writer::read_graph(&ours_path).await.expect("reads");
        let addrs: BTreeSet<i64> = theirs
            .rows
            .iter()
            .chain(ours.rows.iter())
            .map(|r| r.addr)
            .collect();
        let differing = addrs
            .iter()
            .filter(|addr| {
                let t = theirs.rows.iter().find(|r| r.addr == **addr);
                let o = ours.rows.iter().find(|r| r.addr == **addr);
                match (t, o) {
                    (Some(t), Some(o)) => {
                        t.original_content != o.original_content || t.addresses != o.addresses
                    }
                    _ => true,
                }
            })
            .count();

        if differing == 0 {
            agreed += 1;
        } else if first_divergence.is_none() {
            first_divergence = Some((n, differing));
        }
    }

    assert_eq!(
        first_divergence,
        None,
        "the writers first disagree at (N, differing rows); {agreed} of {} prefixes agreed",
        edits.len()
    );
    assert_eq!(agreed, 18, "every prefix must have been compared");
    // Non-vacuity: if neither side ever flushed, this compared 24 tail-only
    // files and would stay green under a writer that cannot flush at all.
    assert!(
        !flushed_at.is_empty(),
        "no prefix triggered a flush, so the flush path was never compared"
    );
}

// ------------------------------------------- the guard must read the TAIL

/// Mark `entity` built-in in the TAIL, the way LogSeq's transactor does before
/// its tail overflows: the trees are untouched and the marker lives only in
/// the transaction log at addr 1.
fn mark_built_in_in_the_tail(graph: &mut KvsGraph, entity: i64) {
    let tx = graph.allocate_tx().expect("a tx id");
    let mut tail = graph.tail().expect("tail");
    tail.push_transaction(vec![kvs_writer::TailDatom {
        entity,
        attribute: "logseq.property/built-in?".to_string(),
        value: TransitNode::Bool(true),
        tx,
        op: kvs_writer::DatomOp::Assert,
    }])
    .expect("room in the tail");
    graph.set_tail(&tail).expect("set tail");
}

/// Retract `entity`'s built-in flag in the TAIL, leaving the trees untouched.
fn retract_built_in_in_the_tail(graph: &mut KvsGraph, entity: i64) {
    let tx = graph.allocate_tx().expect("a tx id");
    let mut tail = graph.tail().expect("tail");
    tail.push_transaction(vec![kvs_writer::TailDatom {
        entity,
        attribute: "logseq.property/built-in?".to_string(),
        value: TransitNode::Bool(true),
        tx,
        op: kvs_writer::DatomOp::Retract,
    }])
    .expect("room in the tail");
    graph.set_tail(&tail).expect("set tail");
}

/// A built-in marker that has not been flushed yet still refuses the push.
///
/// This is the shape that made the guard wrong: `is_built_in` read the stored
/// TREES while `current_title` replayed the TAIL, so the two halves of push
/// disagreed about what the graph currently said — and disagreed in the
/// direction that WRITES. LogSeq's own transactor leaves a marker in the tail
/// for up to 32 datoms before it flushes, so this is an ordinary state of a
/// live graph, not a contrived one.
#[tokio::test]
async fn a_built_in_marker_that_is_still_in_the_tail_refuses_the_push() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let target = editable_uuids(&graph, &base)[0].clone();
    let entity = kvs_writer::entity_by_uuid(&graph, &target)
        .expect("read")
        .expect("entity");

    // Precondition: the block is pushable BEFORE the marker exists, so a
    // refusal afterwards can only be the marker's doing.
    assert!(
        !kvs_writer::is_built_in(&graph, entity).expect("read"),
        "the block must start out not built-in"
    );

    mark_built_in_in_the_tail(&mut graph, entity);
    let rows_before = graph.rows.clone();

    let err = kvs_writer::push(
        &mut graph,
        &base,
        &retitled(&base, &target, "pushed anyway"),
    )
    .expect_err("a tail-resident built-in marker must refuse the push");
    assert!(
        matches!(&err, kvs_writer::RowError::PushBuiltIn { entity: e, .. } if *e == entity),
        "got {err}"
    );
    assert_eq!(
        graph.rows, rows_before,
        "the refused push must not have written a row"
    );
}

/// The MIRROR: a built-in flag RETRACTED in the tail stops counting.
///
/// LogSeq's answer, measured (`oracle/probe_mirror.cljs`): entity 40 is
/// built-in by the flag alone — no `:file/path`, no internal `:db/ident` — and
/// after `[:db/retract 40 :logseq.property/built-in? true]` with no forced
/// store, `built-in-entity?` returns nil. So the retraction counts, and Holon
/// must accept the push rather than refuse on a flag the graph no longer
/// asserts. A reader that honoured tail asserts but not tail retractions would
/// pass the test above and fail this one.
///
/// Sixteen of the fixture's built-ins are flag-only, so this is reachable
/// rather than hypothetical.
#[tokio::test]
async fn a_built_in_flag_retracted_in_the_tail_makes_the_block_pushable() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;

    // A FLAG-ONLY built-in that carries a title. Most built-ins also carry an
    // internal `:db/ident`, and for those the flag's retraction changes
    // nothing — leg 3 still fires, which is what LogSeq answers for entity 61.
    // The candidate is chosen by asking the production predicate whether the
    // retraction actually clears the verdict, rather than by hard-coding an
    // entity id that a fixture change could silently repurpose.
    //
    // Since LW-7.a the base no longer carries built-ins, so the base entry is
    // CONSTRUCTED — reaching push's backstop is the whole point of the test.
    let datoms = kvs_writer::datoms_now(&graph).expect("read");
    let titled: BTreeSet<i64> = datoms
        .iter()
        .filter(|d| d.a == "block/title")
        .map(|d| d.e)
        .collect();
    let entity = titled
        .into_iter()
        .filter(|e| kvs_writer::is_built_in(&graph, *e).expect("read"))
        .find(|e| {
            let mut probe = graph.clone();
            retract_built_in_in_the_tail(&mut probe, *e);
            !kvs_writer::is_built_in(&probe, *e).expect("read")
        })
        .expect("the fixture has 16 flag-only built-ins");
    let (base, target) = base_claiming(&graph, &base, entity);
    assert!(
        kvs_writer::is_built_in(&graph, entity).expect("read"),
        "precondition: the block starts out built-in"
    );

    retract_built_in_in_the_tail(&mut graph, entity);

    assert!(
        !kvs_writer::is_built_in(&graph, entity).expect("read"),
        "a tail retraction of the only leg that applied must clear the verdict"
    );
    let report = kvs_writer::push(&mut graph, &base, &retitled(&base, &target, "now editable"))
        .expect("the block is no longer built-in, so the push is legal");
    assert_eq!(report.blocks, vec![entity], "the push edited that block");
}

/// The stale-base guard reads the TAIL too.
///
/// Same family as the built-in blindness: if the guard compared the base
/// against the TREES only, a LogSeq edit sitting in an unflushed tail would be
/// invisible and push would overwrite it — the exact loss the guard exists to
/// prevent. It shares `current_title`, and `current_title` reads
/// [`datoms_now`], so it sees the tail; this is what holds that true.
#[tokio::test]
async fn the_stale_base_guard_sees_a_title_that_is_only_in_the_tail() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let target = editable_uuids(&graph, &base)[0].clone();
    let entity = kvs_writer::entity_by_uuid(&graph, &target)
        .expect("read")
        .expect("entity");

    // LogSeq retitled the block and has not flushed. The base still records
    // what the TREES say, so it is stale in exactly the way that matters.
    kvs_writer::replace_block_title(&mut graph, entity, "LogSeq changed this")
        .expect("stage a tail-resident edit");
    let rows_before = graph.rows.clone();

    let err = kvs_writer::push(
        &mut graph,
        &base,
        &retitled(&base, &target, "Holon's title"),
    )
    .expect_err("the base no longer describes the graph");
    match &err {
        kvs_writer::RowError::PushBaseStale { uuid, found, .. } => {
            assert_eq!(*uuid, target, "the error names the block");
            assert_eq!(
                found, "LogSeq changed this",
                "the guard must quote the TAIL's value, not the tree's"
            );
        }
        other => panic!("got {other}"),
    }
    assert_eq!(graph.rows, rows_before, "nothing was written");
}

/// A title attribute declared cardinality-MANY refuses the push.
///
/// An assert on a cardinality-many attribute ADDS to the set instead of
/// superseding, so the retract-then-assert pair push writes would leave the
/// old title in place beside the new one. Refusing is the only correct answer
/// until someone means "remove this particular value".
///
/// The fixture declares `:block/title` cardinality-one and no test built a
/// graph that says otherwise, so this guard sat undriven — neutralising it
/// left the whole file green. Found by verify-w2, not by my flip sweep, whose
/// guard list simply never named it.
#[tokio::test]
async fn a_cardinality_many_title_refuses_the_push() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let target = editable_uuids(&graph, &base)[0].clone();

    // Redeclare :block/title as cardinality-many in the root schema, which is
    // where push reads the declaration from.
    let TransitNode::Map(attrs) = &mut graph.root.schema else {
        panic!("the root schema is a map");
    };
    let definition = attrs
        .iter_mut()
        .find(|(k, _)| matches!(k, TransitNode::Keyword(k) if k == "block/title"))
        .map(|(_, v)| v)
        .expect("the schema declares :block/title");
    let TransitNode::Map(fields) = definition else {
        panic!("an attribute definition is a map");
    };
    fields.retain(|(k, _)| !matches!(k, TransitNode::Keyword(k) if k == "db/cardinality"));
    fields.push((
        TransitNode::Keyword("db/cardinality".to_string()),
        TransitNode::Keyword("db.cardinality/many".to_string()),
    ));

    let rows_before = graph.rows.clone();
    let err = kvs_writer::push(
        &mut graph,
        &base,
        &retitled(&base, &target, "two titles now?"),
    )
    .expect_err("a cardinality-many title must be refused");
    assert!(
        matches!(&err, kvs_writer::RowError::NotCardinalityOne { attribute } if attribute == "block/title"),
        "got {err}"
    );
    assert_eq!(graph.rows, rows_before, "nothing was written");
}

/// A push that fails PART WAY THROUGH leaves the graph completely untouched —
/// `next_tx` included.
///
/// The refusal tests all fail during PRE-VALIDATION, before the write loop is
/// entered, so none of them can see a partial write. Reaching the loop takes
/// care: the failure must be invisible to the plan phase and fatal to the
/// flush. So the tail is filled to exactly the branching factor (the next edit
/// must therefore flush) and only the AVET root is broken — `datoms_now` reads
/// EAVT, so pre-validation passes, and `flush_tail` rebuilds all three trees,
/// so it does not.
///
/// Without copy-and-swap the pushed block's transaction would already be in
/// the tail when the flush failed. `next_tx` is compared too: it is not part
/// of `rows`, and `allocate_tx` increments it BEFORE it can reject an id, so a
/// comparison over rows alone would call such a graph untouched.
#[tokio::test]
async fn a_push_that_fails_mid_loop_leaves_the_graph_completely_untouched() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let targets = editable_uuids(&graph, &base);

    // Fill the tail to exactly 32 datoms with edits to a block the push will
    // NOT touch, so the pushed block's title is still what the base recorded.
    let filler = kvs_writer::entity_by_uuid(&graph, &targets[1])
        .expect("read")
        .expect("entity");
    // 30, not 32: the FIRST pushed block must succeed and the SECOND must
    // flush. A failure on the first iteration cannot distinguish copy-and-swap
    // from a writer that swaps eagerly at the end of each iteration, because
    // neither has written anything yet.
    for i in 0..15 {
        kvs_writer::replace_block_title(&mut graph, filler, &format!("filler {i:02}"))
            .expect("edit");
    }
    assert_eq!(
        graph.tail().expect("tail").datom_count(),
        30,
        "one more block fits; the one after it must flush"
    );

    // Break a tree the plan phase does not read and the flush must rebuild.
    graph.root.avet = 9_999_999;

    let before = graph.clone();
    // TWO blocks: the first lands in the tail, the second overflows and dies.
    let after = retitled(
        &retitled(&base, &targets[0], "this must not survive"),
        &targets[2],
        "nor must this",
    );
    let err = kvs_writer::push(&mut graph, &base, &after)
        .expect_err("the flush cannot rebuild a tree whose root is missing");
    assert!(
        matches!(&err, kvs_writer::RowError::MissingNode { .. }),
        "the failure must come from the FLUSH, not from pre-validation; got {err}"
    );

    assert_eq!(graph.rows, before.rows, "no row may have changed");
    assert_eq!(
        graph.tail().expect("tail").datom_count(),
        30,
        "neither pushed block's transaction may be in the tail — including the \
         FIRST one, which had already succeeded when the second failed"
    );
    assert_eq!(
        graph.next_tx(),
        before.next_tx(),
        "the transaction counter must not have advanced either"
    );
}

/// A graph at an unexpected LogSeq schema version refuses the push.
///
/// The whole writer is calibrated to 65.33 — the tail shape, the branching
/// factor, the flush rule were all measured there. Writing into a graph
/// LogSeq has since migrated would be writing by guess.
#[tokio::test]
async fn a_graph_at_an_unpinned_schema_version_refuses_the_push() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let target = editable_uuids(&graph, &base)[0].clone();

    // Bump the minor version wherever the graph declares it.
    let mut bumped = 0;
    for row in &mut graph.rows {
        bump_schema_minor(&mut row.node, &mut bumped);
    }
    // More than one: branch nodes repeat their leaves' datoms as separators,
    // so the same datom is rewritten wherever it appears.
    assert!(bumped > 0, "a schema-version datom was actually rewritten");

    let rows_before = graph.rows.clone();
    let err = kvs_writer::push(
        &mut graph,
        &base,
        &retitled(&base, &target, "from the future"),
    )
    .expect_err("an unpinned schema version must be refused");
    assert!(
        matches!(&err, kvs_writer::RowError::SchemaVersionMismatch { .. }),
        "got {err}"
    );
    assert_eq!(graph.rows, rows_before, "nothing was written");
}

/// Rewrite a `:logseq.kv/schema-version` value's `:minor` to something the
/// build does not pin.
fn bump_schema_minor(node: &mut TransitNode, count: &mut usize) {
    match node {
        TransitNode::Map(pairs) => {
            let is_version = pairs
                .iter()
                .any(|(k, _)| matches!(k, TransitNode::Keyword(k) if k == "minor"));
            if is_version {
                for (k, v) in pairs.iter_mut() {
                    if matches!(k, TransitNode::Keyword(k) if k == "minor") {
                        *v = TransitNode::Int(9999);
                        *count += 1;
                    }
                }
                return;
            }
            for (k, v) in pairs.iter_mut() {
                bump_schema_minor(k, count);
                bump_schema_minor(v, count);
            }
        }
        TransitNode::List(items) => {
            for item in items {
                bump_schema_minor(item, count);
            }
        }
        TransitNode::Tagged(_, inner) => bump_schema_minor(inner, count),
        _ => {}
    }
}

/// A block LogSeq holds with NO title cannot have its title "replaced".
///
/// Push writes a retract of the stored value paired with the assert of the
/// new one. With nothing to retract there is no such pair, and inventing a
/// bare assert would be writing a shape LogSeq's transactor never writes. The
/// fixture has four such blocks — LogSeq creates them empty — so a Holon user
/// typing into one reaches this, and it is refused by name rather than
/// half-written.
#[tokio::test]
async fn a_block_with_no_title_at_all_is_refused_by_name() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;

    let built_in = built_in_entities(&graph);
    let untitled = base
        .uuids()
        .find(|u| {
            base.get(u).expect("present").content.is_empty()
                && kvs_writer::entity_by_uuid(&graph, u)
                    .expect("read")
                    .is_some_and(|e| {
                        !built_in.contains(&e)
                            && kvs_writer::datoms_now(&graph)
                                .expect("read")
                                .iter()
                                .all(|d| !(d.e == e && d.a == "block/title"))
                    })
        })
        .expect("the fixture has blocks LogSeq created with no title")
        .to_string();

    let rows_before = graph.rows.clone();
    let err = kvs_writer::push(
        &mut graph,
        &base,
        &retitled(&base, &untitled, "Holon typed here"),
    )
    .expect_err("a block with no title has nothing to retract");
    assert!(
        matches!(&err, kvs_writer::RowError::NoTitle { .. }),
        "got {err}"
    );
    assert_eq!(graph.rows, rows_before, "nothing was written");
}

// ------------------------------------------------- the ident rule's honesty

/// LogSeq's own `internal-ident?` verdicts, obtained by CALLING it.
#[derive(serde::Deserialize)]
struct IdentReference {
    schema_version: String,
    source: String,
    /// Every measured ident -> LogSeq's answer.
    verdicts: std::collections::BTreeMap<String, bool>,
    /// The idents this graph actually instantiates.
    graph_idents: Vec<String>,
}

/// Holon's ident rule against LogSeq's, EVERY entry, no skips.
///
/// The rule is a namespace approximation of a membership test, so "it passes"
/// is not the claim. The claim is stronger and more specific: it agrees with
/// LogSeq on 182 of 191 measured idents, and every one of the nine
/// disagreements is an OVER-refusal — Holon calls internal something LogSeq
/// does not, so push declines an edit LogSeq would allow. Never the reverse.
///
/// An earlier version of this test SKIPPED `block/*` idents on the grounds
/// that the arm was unreachable. That was wrong twice: unreachable described
/// the fixture's data, not the predicate, and the skip hid the fact that
/// dropping the arm under-refuses 13 idents LogSeq calls internal. There are
/// no skips now.
#[tokio::test]
async fn the_ident_rule_matches_logseq_except_for_a_recorded_over_refusal_set() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/logseq-db/internal-ident-reference.json");
    let reference: IdentReference =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("the recording is committed"))
            .expect("it parses");

    // The recording is only valid for the schema version this build pins, and
    // W1 hard-refuses a graph of any other version, so the two cannot drift
    // apart unnoticed.
    assert_eq!(
        reference.schema_version, "65.33",
        "the recording must describe the schema version this build pins"
    );
    assert!(
        reference.source.contains("internal-ident?"),
        "the verdicts must come from calling LogSeq's own predicate, not from \
         reading its source: {}",
        reference.source
    );
    assert_eq!(reference.verdicts.len(), 191, "191 measured idents");
    assert_eq!(reference.graph_idents.len(), 171, "171 in the graph itself");

    // Holon's rule is only observable through the predicate, so each ident is
    // driven by a constructed entity whose ONLY built-in evidence is that
    // ident — no flag, no file path. This is also the only thing that drives
    // the `block` arm at all.
    let base_graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let ours = |ident: &str| {
        let mut graph = base_graph.clone();
        let tx = graph.allocate_tx().expect("tx");
        let mut tail = graph.tail().expect("tail");
        tail.push_transaction(vec![kvs_writer::TailDatom {
            entity: 8_000_001,
            attribute: "db/ident".to_string(),
            value: TransitNode::Keyword(ident.trim_start_matches(':').to_string()),
            tx,
            op: kvs_writer::DatomOp::Assert,
        }])
        .expect("room in the tail");
        graph.set_tail(&tail).expect("stored");
        kvs_writer::is_built_in(&graph, 8_000_001).expect("read")
    };

    let mut agreed = 0;
    let mut divergences: Vec<&str> = Vec::new();
    for (ident, theirs) in &reference.verdicts {
        let mine = ours(ident);
        if mine == *theirs {
            agreed += 1;
            continue;
        }
        assert!(
            mine && !theirs,
            "every divergence must be an OVER-refusal — Holon stricter than \
             LogSeq, never more permissive. {ident}: LogSeq={theirs}, Holon={mine}"
        );
        divergences.push(ident);
    }

    assert_eq!(
        divergences,
        vec![
            ":block/name",
            ":block/tx-id",
            ":block/uuid",
            ":logseq-plugin/x",
            ":logseq.thirdparty/x",
            ":logseq/foo",
            ":logseq_x/y",
            ":logseqfoo/bar",
            ":logseqified/x",
        ],
        "the divergence set must not change silently"
    );
    assert_eq!(agreed, 182, "182 of the 191 measured idents agree exactly");

    // The `block` arm's whole justification, asserted rather than asserted
    // ABOUT: LogSeq calls 13 of the 16 declared block/* idents internal, and
    // Holon agrees on every one. Deleting the arm would flip all 13 to
    // under-refusals.
    let block_internal: Vec<&str> = reference
        .verdicts
        .iter()
        .filter(|(ident, theirs)| ident.starts_with(":block/") && **theirs)
        .map(|(ident, _)| ident.as_str())
        .collect();
    assert_eq!(
        block_internal.len(),
        13,
        "LogSeq calls 13 block/* idents internal; got {block_internal:?}"
    );
    for ident in &block_internal {
        assert!(
            ours(ident),
            "{ident} is one of the 13 the block arm exists for"
        );
    }

    // And every ident the graph actually instantiates is covered by the
    // recording — otherwise the agreement above is over a set that misses the
    // population that matters.
    for ident in &reference.graph_idents {
        assert!(
            reference.verdicts.contains_key(ident),
            "the graph instantiates {ident} but the recording has no verdict \
             for it, so nothing here pins Holon's answer"
        );
    }
}

// ------------------------------------------- tail replay, against the oracle

/// Append one transaction to the tail and hand back the graph.
fn with_tail_transaction(graph: &KvsGraph, datoms: Vec<kvs_writer::TailDatom>) -> KvsGraph {
    let mut next = graph.clone();
    let mut tail = next.tail().expect("tail");
    tail.push_transaction(datoms).expect("room in the tail");
    next.set_tail(&tail).expect("stored");
    next
}

fn tail_datom(
    entity: i64,
    attribute: &str,
    value: TransitNode,
    tx: i64,
    op: kvs_writer::DatomOp,
) -> kvs_writer::TailDatom {
    kvs_writer::TailDatom {
        entity,
        attribute: attribute.to_string(),
        value,
        tx: kvs_writer::TxId::new(tx).expect("a valid tx id"),
        op,
    }
}

fn values_of(graph: &KvsGraph, entity: i64, attribute: &str) -> Vec<TransitNode> {
    kvs_writer::datoms_now(graph)
        .expect("read")
        .into_iter()
        .filter(|d| d.e == entity && d.a == attribute)
        .map(|d| d.v)
        .collect()
}

/// A cardinality-MANY assert ADDS; it does not supersede.
///
/// LogSeq's answer, measured: entity 144 holds `:block/tags #{3}`;
/// `[:db/add 144 :block/tags 5]` left unflushed restores as `(3 5)`. A reader
/// that superseded on `(e, a)` would silently drop tag 3 — and 149 of this
/// graph's entities carry tags, so this is routine, not a corner.
#[tokio::test]
async fn a_cardinality_many_assert_in_the_tail_adds_rather_than_supersedes() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let before = values_of(&graph, 144, "block/tags");
    assert_eq!(
        before,
        vec![TransitNode::Int(3)],
        "the fixture's entity 144 holds exactly one tag"
    );

    let with_five = with_tail_transaction(
        &graph,
        vec![tail_datom(
            144,
            "block/tags",
            TransitNode::Int(5),
            536_871_024,
            kvs_writer::DatomOp::Assert,
        )],
    );
    assert_eq!(
        values_of(&with_five, 144, "block/tags"),
        vec![TransitNode::Int(3), TransitNode::Int(5)],
        "both tags must survive, as LogSeq restores them"
    );
}

/// A cardinality-MANY retract removes ONLY the value it names.
///
/// Measured: tags `{3}` with an unflushed assert of 5 and retract of 3
/// restores as `(5)`. The retract datom carries the value it removes, so a
/// value-matching removal is well defined.
#[tokio::test]
async fn a_cardinality_many_retract_removes_only_the_named_value() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let staged = with_tail_transaction(
        &graph,
        vec![tail_datom(
            144,
            "block/tags",
            TransitNode::Int(5),
            536_871_024,
            kvs_writer::DatomOp::Assert,
        )],
    );
    let staged = with_tail_transaction(
        &staged,
        vec![tail_datom(
            144,
            "block/tags",
            TransitNode::Int(3),
            536_871_024,
            kvs_writer::DatomOp::Retract,
        )],
    );
    assert_eq!(
        values_of(&staged, 144, "block/tags"),
        vec![TransitNode::Int(5)],
        "only the named value is removed; the added one stays"
    );
}

/// A cardinality-ONE assert supersedes — resolved by tail POSITION, because
/// two tail transactions can carry the SAME transaction id.
///
/// Measured across two LogSeq sessions with no store between them: nothing
/// updates the root's max-tx while edits sit in the tail, so the second
/// session allocates the same id again. The tail then reads
/// `[[retract "This is today" -T, assert "S1" +T], [retract "S1" -T, assert
/// "S2" +T]]` with ONE T, and LogSeq restores "S2". A merge that resolved by
/// tx magnitude is ambiguous exactly here and can keep the superseded value.
#[tokio::test]
async fn later_wins_when_two_tail_transactions_share_a_transaction_id() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let target = editable_uuids(&graph, &base)[0].clone();
    let entity = kvs_writer::entity_by_uuid(&graph, &target)
        .expect("read")
        .expect("entity");
    let original = base.get(&target).expect("present").content.clone();

    const SHARED: i64 = 536_871_024;
    let session_one = with_tail_transaction(
        &graph,
        vec![
            tail_datom(
                entity,
                "block/title",
                TransitNode::Str(original.clone()),
                SHARED,
                kvs_writer::DatomOp::Retract,
            ),
            tail_datom(
                entity,
                "block/title",
                TransitNode::Str("session one".to_string()),
                SHARED,
                kvs_writer::DatomOp::Assert,
            ),
        ],
    );
    let session_two = with_tail_transaction(
        &session_one,
        vec![
            tail_datom(
                entity,
                "block/title",
                TransitNode::Str("session one".to_string()),
                SHARED,
                kvs_writer::DatomOp::Retract,
            ),
            tail_datom(
                entity,
                "block/title",
                TransitNode::Str("session two".to_string()),
                SHARED,
                kvs_writer::DatomOp::Assert,
            ),
        ],
    );

    assert_eq!(
        values_of(&session_two, entity, "block/title"),
        vec![TransitNode::Str("session two".to_string())],
        "the LATER transaction wins even though both carry tx {SHARED}"
    );
}

/// Every cardinality-many attribute LogSeq declares is visible in the root
/// schema this reader consults.
///
/// If the root carried only a subset, the missing attributes would fall into
/// the cardinality-ONE supersede path — the LOSSY direction, silently
/// discarding values. LogSeq reports 19 for this graph.
#[tokio::test]
async fn the_root_schema_declares_every_cardinality_many_attribute() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let TransitNode::Map(attrs) = &graph.root.schema else {
        panic!("the root schema is a map");
    };
    let many: Vec<&str> = attrs
        .iter()
        .filter_map(|(k, v)| {
            let TransitNode::Keyword(name) = k else {
                return None;
            };
            let TransitNode::Map(fields) = v else {
                return None;
            };
            fields
                .iter()
                .any(|(fk, fv)| {
                    matches!(fk, TransitNode::Keyword(k) if k == "db/cardinality")
                        && matches!(fv, TransitNode::Keyword(v) if v == "db.cardinality/many")
                })
                .then_some(name.as_str())
        })
        .collect();

    assert_eq!(
        many.len(),
        19,
        "LogSeq reports 19 cardinality-many attributes for this graph; the \
         root schema must declare all of them or the missing ones silently \
         take the lossy supersede path. Got {many:?}"
    );
    for expected in [
        "block/tags",
        "block/refs",
        "block/alias",
        "logseq.property/classes",
    ] {
        assert!(
            many.contains(&expected),
            "{expected} must be among them; got {many:?}"
        );
    }
}

/// LogSeq writes an EMPTY transaction when it drops a datom, and Holon reads
/// and pushes over one.
///
/// Measured: `[:db/retract 195 :block/title "<never held>"]` never reaches the
/// tail — the transactor drops the datom and writes `[[]]`. So a mismatched
/// retract cannot arrive from LogSeq at all, but the empty transaction it
/// leaves behind is a real input.
#[tokio::test]
async fn an_empty_transaction_in_the_tail_reads_and_pushes_fine() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let mut staged = with_tail_transaction(&graph, vec![]);

    assert_eq!(
        staged.tail().expect("tail").datom_count(),
        0,
        "an empty transaction contributes no datoms"
    );
    assert_eq!(
        kvs_writer::datoms_now(&staged).expect("read").len(),
        kvs_writer::datoms_now(&graph).expect("read").len(),
        "and changes nothing about what the graph holds"
    );

    let target = editable_uuids(&staged, &base)[0].clone();
    let report = kvs_writer::push(
        &mut staged,
        &base,
        &retitled(&base, &target, "over an empty tx"),
    )
    .expect("a graph carrying an empty transaction is pushable");
    assert_eq!(report.transactions, 1, "the push went through normally");
}

/// Datoms living in UNREFERENCED rows are not part of the graph.
///
/// The fixture carries 17 rows LogSeq merged away and abandoned; they still
/// hold datoms, and entities 197 and 199 exist ONLY there. The tail is empty,
/// so nothing retracted them — they are garbage, not deletions, and LogSeq's
/// own `d/datoms` does not report them.
///
/// Excluding them is a property of construction now (`datoms_now` walks the
/// reachable eavt tree), not of the data happening to be harmless. Without it
/// a uuid found only in garbage would resolve to an entity, and a built-in
/// marker stranded in garbage would refuse a legitimate push.
#[tokio::test]
async fn datoms_stranded_in_unreferenced_rows_are_not_part_of_the_graph() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    assert_eq!(
        graph.tail().expect("tail").datom_count(),
        0,
        "the fixture's tail is empty, so nothing here is a retraction"
    );

    // A raw row scan sees them; that is what makes this worth pinning.
    let mut in_rows = BTreeSet::new();
    for row in &graph.rows {
        if let TransitNode::Map(pairs) = &row.node {
            for (k, v) in pairs {
                if matches!(k, TransitNode::Keyword(k) if k == "keys") {
                    if let TransitNode::List(tuples) = v {
                        for tuple in tuples {
                            if let TransitNode::List(slots) = tuple {
                                if let Some(TransitNode::Int(e)) = slots.first() {
                                    in_rows.insert(*e);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let live: BTreeSet<i64> = kvs_writer::datoms_now(&graph)
        .expect("read")
        .into_iter()
        .map(|d| d.e)
        .collect();

    let stranded: Vec<i64> = in_rows.difference(&live).copied().collect();
    assert_eq!(
        stranded,
        vec![197, 199],
        "exactly the two entities that exist only in abandoned rows"
    );
    for entity in &stranded {
        assert!(
            !kvs_writer::is_built_in(&graph, *entity).expect("read"),
            "a stranded entity must not be reachable by the built-in predicate"
        );
        assert!(
            kvs_writer::datoms_now(&graph)
                .expect("read")
                .iter()
                .all(|d| d.e != *entity),
            "entity {entity} must contribute no datoms at all"
        );
    }

    // And a uuid that lives only in garbage resolves to nothing, so a push
    // naming it is refused rather than silently aimed at a dead entity.
    let mut base = base_of(&fixture()).await;
    let ghost = "00000000-dead-dead-dead-000000000000";
    base.witness_create(
        ghost,
        BaseBlock {
            content: "only in an abandoned row".to_string(),
            ..BaseBlock::default()
        },
    )
    .expect("a fresh uuid");
    let mut graph_mut = graph.clone();
    let err = kvs_writer::push(&mut graph_mut, &base, &retitled(&base, ghost, "edited"))
        .expect_err("a uuid the live graph does not hold is refused");
    assert!(
        matches!(&err, kvs_writer::RowError::PushUnknownBlock { .. }),
        "got {err}"
    );
}

/// A BARE assert on a cardinality-one attribute leaves ONE value, not two.
///
/// This is the shape that isolates the supersede rule. Every tail LogSeq
/// writes pairs a retract with its assert, so on well-formed input the retract
/// already removes the old value and the supersede is redundant — which is
/// exactly why a mutation of it stays silent unless something drives THIS.
///
/// NOT LogSeq-measured, and it cannot be: LogSeq's transactor will not emit a
/// bare assert on a cardinality-one attribute, so there is no graph to read
/// the answer off. The rule here comes from what cardinality-one MEANS — at
/// most one value per (entity, attribute) — not from an observation, and that
/// is the honest basis. Without it a bare assert would leave two titles for
/// one block and `current_title` would be picking between them by position
/// alone.
#[tokio::test]
async fn a_bare_cardinality_one_assert_supersedes_rather_than_accumulating() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let target = editable_uuids(&graph, &base)[0].clone();
    let entity = kvs_writer::entity_by_uuid(&graph, &target)
        .expect("read")
        .expect("entity");
    let original = base.get(&target).expect("present").content.clone();

    assert_eq!(
        values_of(&graph, entity, "block/title"),
        vec![TransitNode::Str(original.clone())],
        "the block starts with exactly one title"
    );

    // An assert with NO accompanying retract.
    let staged = with_tail_transaction(
        &graph,
        vec![tail_datom(
            entity,
            "block/title",
            TransitNode::Str("bare assert".to_string()),
            536_871_024,
            kvs_writer::DatomOp::Assert,
        )],
    );

    assert_eq!(
        values_of(&staged, entity, "block/title"),
        vec![TransitNode::Str("bare assert".to_string())],
        "cardinality-one means at most one value survives; the old title must \
         be gone even though nothing retracted it"
    );
}

// ------------------------------- excluding a page-owned parent with children

/// Excluding a page-owned entity that HAS CHILDREN refuses the import.
///
/// The page leg drops LogSeq's per-view UI records. Today every one of them is
/// childless (measured), so nothing is lost. If a future LogSeq grows content
/// beneath one, dropping the parent would silently take the children with it —
/// so the importer refuses by name instead.
///
/// Constructed, because the fixture cannot supply the case: a child is added
/// under `$$$views` panel e=198 through the tail, exactly as LogSeq's own
/// transactor would, and the graph is written out and imported for real.
#[tokio::test]
async fn excluding_a_page_owned_parent_that_has_children_refuses_the_import() {
    let dir = tempfile::tempdir().expect("tempdir");
    let grown = dir.path().join("grown.sqlite");

    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");

    // Precondition: e=198 is page-excluded and childless, so a refusal
    // afterwards can only be the new child's doing.
    let base_before = base_of(&fixture()).await;
    assert!(
        base_before
            .uuids()
            .all(|u| kvs_writer::entity_by_uuid(&graph, u).expect("read") != Some(198)),
        "e=198 is excluded from the base to begin with"
    );

    const CHILD: i64 = 9_000_001;
    let tx = graph.allocate_tx().expect("tx");
    let mut tail = graph.tail().expect("tail");
    tail.push_transaction(vec![
        kvs_writer::TailDatom {
            entity: CHILD,
            attribute: "block/uuid".to_string(),
            value: TransitNode::Uuid("00000009-0000-0000-0000-000000000001".to_string()),
            tx,
            op: kvs_writer::DatomOp::Assert,
        },
        kvs_writer::TailDatom {
            entity: CHILD,
            attribute: "block/title".to_string(),
            value: TransitNode::Str("a note somebody wrote under a system page".to_string()),
            tx,
            op: kvs_writer::DatomOp::Assert,
        },
        kvs_writer::TailDatom {
            entity: CHILD,
            attribute: "block/parent".to_string(),
            value: TransitNode::Int(198),
            tx,
            op: kvs_writer::DatomOp::Assert,
        },
    ])
    .expect("room in the tail");
    graph.set_tail(&tail).expect("stored");
    kvs_writer::write_graph(&graph, &grown)
        .await
        .expect("write");

    let err = LogseqDbImporter::new()
        .import(&grown)
        .await
        .expect_err("a page-excluded parent with a child must refuse the import");
    let message = err.to_string();
    assert!(
        message.contains("198") && message.contains("would be dropped"),
        "the refusal must name the parent and say what would be lost; got: {message}"
    );
}

/// The PARENT-leg shape: not built-in, page NOT built-in, but parent IS.
///
/// The page leg excludes an entity whose `:block/page` is LogSeq's. It says
/// nothing about `:block/parent`, so an entity could in principle sit on a
/// user page while its PARENT is a built-in — and be projected with a parent
/// that is not a block. Zero such entities exist in the fixture, which makes
/// it data-closed, and data-closed is the shape that has hidden two real
/// defects in this lane already.
///
/// Constructed, and the answer is that it is CONSTRUCTION-closed after all:
/// the importer already refuses any reference to a non-projectable entity
/// (`DanglingReference`), so the shape cannot produce a dangling parent — it
/// produces a loud refusal naming both entities and the attribute. This test
/// pins that, so the guarantee survives a future change to either rule.
#[tokio::test]
async fn an_entity_whose_parent_is_built_in_refuses_the_import() {
    let dir = tempfile::tempdir().expect("tempdir");
    let grown = dir.path().join("parent-leg.sqlite");

    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    const CHILD: i64 = 9_000_001;
    const BUILT_IN_PARENT: i64 = 22;
    const USER_PAGE: i64 = 193;
    assert!(
        kvs_writer::is_built_in(&graph, BUILT_IN_PARENT).expect("read"),
        "precondition: entity {BUILT_IN_PARENT} is one LogSeq owns"
    );
    assert!(
        !kvs_writer::is_built_in(&graph, USER_PAGE).expect("read"),
        "precondition: entity {USER_PAGE} is a user page, so the PAGE leg \
         cannot be what refuses this"
    );

    let tx = graph.allocate_tx().expect("tx");
    let mut tail = graph.tail().expect("tail");
    tail.push_transaction(vec![
        kvs_writer::TailDatom {
            entity: CHILD,
            attribute: "block/uuid".to_string(),
            value: TransitNode::Uuid("00000009-0000-0000-0000-000000000002".to_string()),
            tx,
            op: kvs_writer::DatomOp::Assert,
        },
        kvs_writer::TailDatom {
            entity: CHILD,
            attribute: "block/title".to_string(),
            value: TransitNode::Str("a user block under a built-in parent".to_string()),
            tx,
            op: kvs_writer::DatomOp::Assert,
        },
        kvs_writer::TailDatom {
            entity: CHILD,
            attribute: "block/parent".to_string(),
            value: TransitNode::Int(BUILT_IN_PARENT),
            tx,
            op: kvs_writer::DatomOp::Assert,
        },
        kvs_writer::TailDatom {
            entity: CHILD,
            attribute: "block/page".to_string(),
            value: TransitNode::Int(USER_PAGE),
            tx,
            op: kvs_writer::DatomOp::Assert,
        },
    ])
    .expect("room in the tail");
    graph.set_tail(&tail).expect("stored");
    kvs_writer::write_graph(&graph, &grown)
        .await
        .expect("write");

    let err = LogseqDbImporter::new()
        .import(&grown)
        .await
        .expect_err("a parent that is not a block must refuse the import");
    let message = err.to_string();
    for expected in [
        &CHILD.to_string()[..],
        &BUILT_IN_PARENT.to_string()[..],
        ":block/parent",
    ] {
        assert!(
            message.contains(expected),
            "the refusal must name {expected}; got: {message}"
        );
    }
}

// ------------------------------------------- ref-bearing titles, fail-closed

/// A title carrying ref syntax is REFUSED, because who maintains
/// `:block/refs` on an edit is unmeasured.
///
/// Byte-identity is established on titles WITHOUT refs. LogSeq may derive
/// `:block/refs` from a title at edit time; `db_pipeline` documents that IT
/// does not, and `save-block!` — the layer that might — is not drivable under
/// nbb. So whether a ref-bearing edit leaves the graph internally consistent
/// is UNMEASURED. Writing one anyway would be guessing with the user's data,
/// and the failure would be invisible: the title would be right and the
/// backlinks silently wrong.
///
/// Fail closed. Both directions are refused — a title that HAD a ref and a
/// title that GAINS one — because either changes what refs should exist.
#[tokio::test]
async fn a_ref_bearing_title_is_refused_until_refs_are_measured() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;

    // The fixture carries exactly two, both `[[...]]`, measured.
    let existing: Vec<String> = base
        .uuids()
        .filter(|u| base.get(u).expect("present").content.contains("[["))
        .map(str::to_string)
        .collect();
    assert_eq!(existing.len(), 2, "the fixture's ref-bearing blocks");

    // (a) the OLD title carries a ref: editing it away is still refused.
    let mut g = graph.clone();
    let err = kvs_writer::push(&mut g, &base, &retitled(&base, &existing[0], "plain now"))
        .expect_err("an existing ref-bearing title must be refused");
    assert!(
        matches!(&err, kvs_writer::RowError::PushRefBearingTitle { .. }),
        "got {err}"
    );

    // (b) the NEW title GAINS a ref, on a block that had none.
    let clean = editable_uuids(&graph, &base)
        .into_iter()
        .find(|u| !base.get(u).expect("present").content.contains("[["))
        .expect("a ref-free editable block");
    for gaining in [
        "see [[Project Alpha]]",
        "a #tag appears",
        "a ((block-ref)) appears",
    ] {
        let mut g = graph.clone();
        let err = kvs_writer::push(&mut g, &base, &retitled(&base, &clean, gaining))
            .expect_err("a title gaining ref syntax must be refused");
        assert!(
            matches!(&err, kvs_writer::RowError::PushRefBearingTitle { .. }),
            "{gaining:?} was not refused; got {err}"
        );
    }

    // Non-vacuity: a plain-to-plain edit on that same block still works.
    let mut g = graph.clone();
    kvs_writer::push(&mut g, &base, &retitled(&base, &clean, "still plain"))
        .expect("a title with no ref syntax on either side is unaffected");
}

// ------------------------------------------ what the root schema decides

/// Re-asserting a cardinality-MANY value the graph already holds changes
/// nothing — not even the datom's transaction id.
///
/// MEASURED (`oracle/probe_w3_cardinality.cljs`): LogSeq's own transactor
/// drops the redundant add, writes an EMPTY transaction into the tail, and
/// leaves the held datom at its original tx. A reader that appended a second
/// copy would report a graph LogSeq never restores — and for a many-attribute
/// nothing else would notice, because two identical tags read like one.
#[tokio::test]
async fn a_redundant_cardinality_many_assert_in_the_tail_changes_nothing() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let held = kvs_writer::datoms_now(&graph)
        .expect("read")
        .into_iter()
        .find(|d| d.a == "block/tags")
        .expect("the fixture carries tags");

    let next = with_tail_transaction(
        &graph,
        vec![tail_datom(
            held.e,
            "block/tags",
            held.v.clone(),
            held.tx + 1,
            kvs_writer::DatomOp::Assert,
        )],
    );

    let copies: Vec<i64> = kvs_writer::datoms_now(&next)
        .expect("read")
        .into_iter()
        .filter(|d| d.e == held.e && d.a == "block/tags" && d.v == held.v)
        .map(|d| d.tx)
        .collect();
    assert_eq!(
        copies,
        vec![held.tx],
        "one copy at its original tx; a second copy or a bumped tx is a \
         reading LogSeq's transactor never produces"
    );
}

/// The same for cardinality-ONE, where a superseding reader would keep one
/// datom but move it to the tail's transaction id.
#[tokio::test]
async fn a_redundant_cardinality_one_assert_in_the_tail_changes_nothing() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let held = kvs_writer::datoms_now(&graph)
        .expect("read")
        .into_iter()
        .find(|d| d.a == "block/title")
        .expect("the fixture carries titles");

    let next = with_tail_transaction(
        &graph,
        vec![tail_datom(
            held.e,
            "block/title",
            held.v.clone(),
            held.tx + 1,
            kvs_writer::DatomOp::Assert,
        )],
    );

    let copies: Vec<i64> = kvs_writer::datoms_now(&next)
        .expect("read")
        .into_iter()
        .filter(|d| d.e == held.e && d.a == "block/title" && d.v == held.v)
        .map(|d| d.tx)
        .collect();
    assert_eq!(
        copies,
        vec![held.tx],
        "re-asserting the value already held leaves the datom untouched"
    );
}

/// An attribute the ROOT SCHEMA does not declare is refused by name.
///
/// Reading its cardinality is what decides whether an existing value is kept
/// or superseded away, and the root is the authority on that: MEASURED, LogSeq
/// restores its connection from the schema stored at addr 0, so a graph whose
/// root omits an attribute is a graph whose transactor never treated that
/// attribute as many either. Defaulting to cardinality-one would therefore be
/// RIGHT and still wrong to do quietly — the datom names a vocabulary the
/// graph does not have, and the value it drops would leave no trace.
#[tokio::test]
async fn an_attribute_the_root_schema_does_not_declare_refuses_the_read() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let next = with_tail_transaction(
        &graph,
        vec![tail_datom(
            1,
            "my.test/undeclared",
            TransitNode::Str("a".to_string()),
            536_871_024,
            kvs_writer::DatomOp::Assert,
        )],
    );

    let error = kvs_writer::datoms_now(&next)
        .expect_err("an undeclared attribute must not be read as cardinality-one in silence");
    assert!(
        matches!(
            &error,
            kvs_writer::RowError::UndeclaredAttribute { attribute } if attribute == "my.test/undeclared"
        ),
        "the refusal must name the attribute; got {error:?}"
    );
}

/// DataScript's own meta-vocabulary is admitted, because it describes the
/// schema and so is never listed inside it.
///
/// `:db/cardinality` is the subject rather than `:db/ident` because the root
/// DOES declare `:db/ident`, which would reach the schema lookup and never
/// exercise the namespace admission at all. The meta-attributes proper appear
/// as datoms on property-definition entities and nowhere in the schema map,
/// which is exactly the case the admission exists for.
#[tokio::test]
async fn the_db_meta_vocabulary_is_admitted_though_the_schema_never_lists_it() {
    let graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let next = with_tail_transaction(
        &graph,
        vec![tail_datom(
            8_000_001,
            "db/cardinality",
            TransitNode::Keyword("db.cardinality/one".to_string()),
            536_871_024,
            kvs_writer::DatomOp::Assert,
        )],
    );
    assert!(
        kvs_writer::datoms_now(&next)
            .expect("a :db/* meta-attribute reads")
            .iter()
            .any(|d| d.e == 8_000_001 && d.a == "db/cardinality"),
        "the constructed :db/cardinality datom must reach the reading"
    );
}

// ----------------------------------------------------------- pushing tags

/// The classes this graph has, in the order the tag legs prefer them.
const TAG_CANDIDATES: &[&str] = &["Task", "Query", "Page", "Journal"];

/// `base` with `uuid`'s tags replaced.
fn retagged(base: &ImportBase, uuid: &str, tags: Vec<String>) -> ImportBase {
    let mut next = base.clone();
    let mut tags = tags;
    tags.sort();
    let block = BaseBlock {
        tags,
        ..base.get(uuid).expect("present").clone()
    };
    next.advance(uuid, block).expect("a known uuid");
    next
}

/// Six rounds of TAG-ONLY pushes, and the edit list they made.
///
/// Each round attaches (even) or detaches (odd) ONE class per block, so every
/// block's transaction holds exactly one datom — which is what lets the
/// head-to-head compare bytes without either side having to guess an order
/// LogSeq's own save path would use for a multi-datom transaction. That path
/// (`outliner-core/save-block`) is unreachable under nbb, so the order INSIDE
/// a transaction is Holon's own choice and is pinned by a unit test, not by
/// the oracle.
///
/// Six rounds rather than two because 6 blocks x 6 rounds is 36 datoms, which
/// crosses the branching factor and makes the leg exercise a FLUSH of
/// reference-valued datoms into the trees — the one thing tags bring that
/// titles never did, since a tag's value is an integer and the index
/// comparator orders numbers against the strings already there.
async fn tag_pushes() -> (KvsGraph, Vec<(i64, String, i64)>) {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let mut base = base_of(&fixture()).await;
    let targets = pushable_uuids(&graph, &base);

    // Per block, a class it does NOT already carry, so round 0 is a real add.
    let chosen: Vec<(String, String, i64)> = targets
        .iter()
        .map(|uuid| {
            let held = &base.get(uuid).expect("present").tags;
            let name = TAG_CANDIDATES
                .iter()
                .find(|c| !held.contains(&c.to_string()))
                .unwrap_or_else(|| panic!("{uuid} already carries every candidate class"));
            let entity = kvs_writer::class_entity_by_name(&graph, name)
                .expect("read")
                .unwrap_or_else(|| panic!("the fixture has no class called {name}"));
            (uuid.clone(), (*name).to_string(), entity)
        })
        .collect();

    let mut edits = Vec::new();
    let mut flushes = 0;
    for round in 0..6 {
        let attaching = round % 2 == 0;
        let mut next = base.clone();
        for (uuid, name, _) in &chosen {
            let mut tags = base.get(uuid).expect("present").tags.clone();
            if attaching {
                tags.push(name.clone());
            } else {
                tags.retain(|t| t != name);
            }
            next = retagged(&next, uuid, tags);
        }
        let report = kvs_writer::push(&mut graph, &base, &next).expect("push");
        let expected: Vec<i64> = chosen
            .iter()
            .map(|(uuid, _, _)| {
                kvs_writer::entity_by_uuid(&graph, uuid)
                    .expect("read")
                    .expect("entity")
            })
            .collect();
        assert_eq!(
            report.blocks, expected,
            "push must edit blocks in the base's uuid order"
        );
        for (entity, (_, _, tag)) in expected.iter().zip(&chosen) {
            edits.push((
                *entity,
                if attaching { "add" } else { "retract" }.to_string(),
                *tag,
            ));
        }
        flushes += report.flushes;
        base = next;
    }
    assert_eq!(edits.len(), 36, "6 blocks over six rounds");
    assert_eq!(
        flushes, 1,
        "36 datoms must cross the branching factor exactly once"
    );
    (graph, edits)
}

/// HEAD TO HEAD — the same 36 tag edits, applied by LogSeq and by `push`.
///
/// The claim the title leg makes, re-made for reference-valued datoms. It is
/// a different claim: a tag's value is an entity id, so the index comparator
/// has to order integers against the strings and uuids already in the trees,
/// and the flush this leg forces is where that happens.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn head_to_head_with_logseq_applying_the_same_tag_pushes() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");

    let holon_dir = dir.path().join("holon");
    let (graph, edits) = tag_pushes().await;
    std::fs::create_dir_all(&holon_dir).expect("dir");
    let holon_db = holon_dir.join("db.sqlite");
    kvs_writer::write_graph(&graph, &holon_db)
        .await
        .expect("write");

    let edits_json = dir.path().join("tag-edits.json");
    std::fs::write(
        &edits_json,
        serde_json::to_string(&edits).expect("edits serialize"),
    )
    .expect("write edits");

    let logseq_dir = dir.path().join("logseq");
    std::fs::create_dir_all(&logseq_dir).expect("dir");
    let logseq_db = logseq_dir.join("db.sqlite");
    std::fs::copy(fixture(), &logseq_db).expect("stage");
    let out = oracle.run(
        "script/apply_tag_edits.cljs",
        &[&logseq_db, &edits_json],
        &[],
    );
    assert!(
        out.contains("applied 36 tag edits"),
        "LogSeq did not apply the tag edits:\n{out}"
    );

    let theirs = kvs_writer::read_graph(&logseq_db)
        .await
        .expect("logseq reads");
    let ours = kvs_writer::read_graph(&holon_db)
        .await
        .expect("holon reads");

    let addrs: BTreeSet<i64> = theirs
        .rows
        .iter()
        .chain(ours.rows.iter())
        .map(|r| r.addr)
        .collect();
    let (mut identical, mut differing, mut only_theirs, mut only_ours) = (0, 0, 0, 0);
    let mut first_differences: Vec<String> = Vec::new();
    for addr in &addrs {
        let t = theirs.rows.iter().find(|r| r.addr == *addr);
        let o = ours.rows.iter().find(|r| r.addr == *addr);
        match (t, o) {
            (Some(t), Some(o)) => {
                if t.original_content == o.original_content && t.addresses == o.addresses {
                    identical += 1;
                } else {
                    differing += 1;
                    if first_differences.len() < 4 {
                        let at = t
                            .original_content
                            .chars()
                            .zip(o.original_content.chars())
                            .position(|(a, b)| a != b)
                            .unwrap_or(0);
                        let from = at.saturating_sub(60);
                        let slice = |s: &str| s.chars().skip(from).take(160).collect::<String>();
                        first_differences.push(format!(
                            "addr {addr} diverges at char {at}\n  logseq: {}\n  holon:  {}\n  \
                             addresses: {:?} vs {:?}",
                            slice(&t.original_content),
                            slice(&o.original_content),
                            t.addresses,
                            o.addresses
                        ));
                    }
                }
            }
            (Some(_), None) => only_theirs += 1,
            (None, Some(_)) => only_ours += 1,
            (None, None) => unreachable!("addr came from one of the two"),
        }
    }

    assert_eq!(
        (differing, only_theirs, only_ours),
        (0, 0, 0),
        "the two writers disagree on tag edits: {identical} identical, \
         {differing} differing, {only_theirs} only LogSeq's, {only_ours} only \
         Holon's\n{}",
        first_differences.join("\n")
    );
    assert_eq!(
        identical,
        addrs.len(),
        "every row must have been compared, not skipped"
    );
    assert!(
        ours.rows.len() >= 456,
        "the pushed graph must still hold the fixture's rows; got {}",
        ours.rows.len()
    );
}

/// LogSeq's validator accepts a graph Holon pushed TAGS into.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn logseqs_validator_accepts_a_tag_pushed_graph() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");
    let copy_dir = dir.path().join("tagged");
    std::fs::create_dir_all(&copy_dir).expect("dir");
    let (graph, _) = tag_pushes().await;
    kvs_writer::write_graph(&graph, &copy_dir.join("db.sqlite"))
        .await
        .expect("write");

    let out = oracle.run(
        "script/validate_db.cljs",
        &[&copy_dir.join("db.sqlite")],
        &["--closed-maps", "--group-errors"],
    );
    assert!(
        out.contains("Valid!"),
        "LogSeq's validator rejected a graph Holon pushed tags into:\n{out}"
    );
    assert!(
        out.contains(":datoms 2609"),
        "the validator must have read the whole graph back:\n{out}"
    );
}

/// A tag naming no class in this graph is refused BY NAME.
///
/// The hazard is measured, not imagined: `[:db/add e :block/tags 987654]` is
/// ACCEPTED by LogSeq's transactor and leaves a dangling reference
/// (`oracle/probe_tag_edits.cljs`). Nothing downstream would complain until
/// the importer read the graph back and refused the whole import.
#[tokio::test]
async fn a_tag_naming_no_class_is_refused_by_name() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let uuid = pushable_uuids(&graph, &base)
        .first()
        .expect("a pushable block")
        .clone();

    let mut tags = base.get(&uuid).expect("present").tags.clone();
    tags.push("NoSuchClassExists".to_string());
    let next = retagged(&base, &uuid, tags);

    let error = kvs_writer::push(&mut graph, &base, &next)
        .expect_err("a tag naming no class must be refused");
    assert!(
        matches!(
            &error,
            kvs_writer::RowError::PushUnknownTag { uuid: u, tag } if *u == uuid && tag == "NoSuchClassExists"
        ),
        "the refusal must name the block and the tag; got {error:?}"
    );
}

/// A base whose tags no longer match the graph refuses the whole push.
///
/// The tag counterpart of `PushBaseStale`, and it is what makes a redundant
/// add unreachable through `push`: LogSeq's transactor treats one as a no-op
/// (measured), but a base that asks for a tag the block already carries is a
/// base describing a state that exists nowhere, and applying half of it is
/// the failure this layer exists to prevent.
#[tokio::test]
async fn a_base_whose_tags_are_stale_refuses_the_whole_push() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let uuid = pushable_uuids(&graph, &base)
        .first()
        .expect("a pushable block")
        .clone();

    // Claim the block was observed carrying a class it does not carry, then
    // ask for it to be dropped — a REMOVE of a tag that is not there.
    let absent = TAG_CANDIDATES
        .iter()
        .find(|c| {
            !base
                .get(&uuid)
                .expect("present")
                .tags
                .contains(&c.to_string())
        })
        .expect("some class the block lacks");
    let mut observed_tags = base.get(&uuid).expect("present").tags.clone();
    observed_tags.push((*absent).to_string());
    let stale = retagged(&base, &uuid, observed_tags);
    let wanted = retagged(&base, &uuid, base.get(&uuid).expect("present").tags.clone());

    let error = kvs_writer::push(&mut graph, &stale, &wanted)
        .expect_err("a stale tag observation must refuse the push");
    assert!(
        matches!(&error, kvs_writer::RowError::PushBaseStaleTags { uuid: u, .. } if *u == uuid),
        "the refusal must name the block; got {error:?}"
    );
}

/// One datom per attach, one per detach, and nothing else moves.
#[tokio::test]
async fn a_tag_change_writes_one_datom_and_touches_nothing_else() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let uuid = pushable_uuids(&graph, &base)
        .first()
        .expect("a pushable block")
        .clone();
    let entity = kvs_writer::entity_by_uuid(&graph, &uuid)
        .expect("read")
        .expect("entity");
    let name = TAG_CANDIDATES
        .iter()
        .find(|c| {
            !base
                .get(&uuid)
                .expect("present")
                .tags
                .contains(&c.to_string())
        })
        .expect("some class the block lacks");
    let tag = kvs_writer::class_entity_by_name(&graph, name)
        .expect("read")
        .expect("the class exists");

    let untouched = |g: &KvsGraph| -> Vec<(String, TransitNode)> {
        kvs_writer::datoms_now(g)
            .expect("read")
            .into_iter()
            .filter(|d| {
                d.e == entity
                    && matches!(
                        d.a.as_str(),
                        "block/title" | "block/updated-at" | "block/refs" | "block/created-at"
                    )
            })
            .map(|d| (d.a, d.v))
            .collect()
    };
    let before = untouched(&graph);

    let mut tags = base.get(&uuid).expect("present").tags.clone();
    tags.push((*name).to_string());
    let attached = retagged(&base, &uuid, tags);
    let report = kvs_writer::push(&mut graph, &base, &attached).expect("push attaches the tag");
    assert_eq!(
        (report.transactions, report.datoms),
        (1, 1),
        "attaching one tag is ONE datom in ONE transaction"
    );
    assert!(
        kvs_writer::datoms_now(&graph)
            .expect("read")
            .iter()
            .any(|d| d.e == entity && d.a == "block/tags" && d.v == TransitNode::Int(tag)),
        "the tag datom must be in the graph, and its value must be the class ENTITY"
    );
    assert_eq!(
        untouched(&graph),
        before,
        "a tag edit must leave title, refs and both timestamps exactly as they were"
    );

    let report = kvs_writer::push(&mut graph, &attached, &base).expect("push detaches the tag");
    assert_eq!(
        (report.transactions, report.datoms),
        (1, 1),
        "detaching one tag is ONE datom in ONE transaction"
    );
    assert!(
        !kvs_writer::datoms_now(&graph)
            .expect("read")
            .iter()
            .any(|d| d.e == entity && d.a == "block/tags" && d.v == TransitNode::Int(tag)),
        "the detach must have removed exactly that reference"
    );
    assert_eq!(
        untouched(&graph),
        before,
        "and the detach must leave the same four attributes alone"
    );
}

/// A block that gains one tag and loses another does BOTH in one transaction,
/// detach first.
///
/// The order is Holon's choice and is NOT measured: LogSeq's own save path is
/// unreachable under nbb, so nothing here can say what it would emit. It
/// mirrors the title leg's measured retract-then-assert shape, and it is
/// pinned so the choice is visible rather than incidental.
#[tokio::test]
async fn a_simultaneous_attach_and_detach_is_one_transaction_detach_first() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let uuid = pushable_uuids(&graph, &base)
        .first()
        .expect("a pushable block")
        .clone();
    let entity = kvs_writer::entity_by_uuid(&graph, &uuid)
        .expect("read")
        .expect("entity");

    let absent: Vec<&str> = TAG_CANDIDATES
        .iter()
        .filter(|c| {
            !base
                .get(&uuid)
                .expect("present")
                .tags
                .contains(&c.to_string())
        })
        .take(2)
        .copied()
        .collect();
    assert_eq!(absent.len(), 2, "need two classes the block lacks");
    let (first, second) = (absent[0], absent[1]);

    let mut with_first = base.get(&uuid).expect("present").tags.clone();
    with_first.push(first.to_string());
    let attached = retagged(&base, &uuid, with_first.clone());
    kvs_writer::push(&mut graph, &base, &attached).expect("attach the first");

    let mut swapped = with_first.clone();
    swapped.retain(|t| t != first);
    swapped.push(second.to_string());
    let next = retagged(&attached, &uuid, swapped);

    let report = kvs_writer::push(&mut graph, &attached, &next).expect("swap the tags");
    assert_eq!(
        (report.transactions, report.datoms),
        (1, 2),
        "one transaction, two datoms"
    );

    let tail = graph.tail().expect("tail");
    let last = tail
        .transactions()
        .last()
        .expect("the swap is the last transaction")
        .clone();
    let ops: Vec<(i64, kvs_writer::DatomOp)> = last
        .iter()
        .map(|d| match d.value {
            TransitNode::Int(target) => (target, d.op),
            ref other => panic!("a tag datom's value must be a reference; got {other:?}"),
        })
        .collect();
    let dropped = kvs_writer::class_entity_by_name(&graph, first)
        .expect("read")
        .expect("class");
    let gained = kvs_writer::class_entity_by_name(&graph, second)
        .expect("read")
        .expect("class");
    assert_eq!(
        ops,
        vec![
            (dropped, kvs_writer::DatomOp::Retract),
            (gained, kvs_writer::DatomOp::Assert)
        ],
        "detach before attach, both under one transaction, for entity {entity}"
    );
}

/// A DECLARED attribute whose cardinality cannot be read is refused by name.
///
/// The dangerous case is not the missing declaration — that one is already
/// refused — but the present-and-unreadable one, which a default would send
/// down the cardinality-ONE supersede path and drop a value with nothing to
/// show for it. Driven by rewriting the root schema, because no real graph
/// offers a broken declaration.
#[tokio::test]
async fn a_declaration_whose_cardinality_cannot_be_read_is_refused_by_name() {
    let base_graph = kvs_writer::read_graph(&fixture()).await.expect("reads");

    let redeclared = |cardinality: Option<TransitNode>| {
        let mut graph = base_graph.clone();
        let TransitNode::Map(attrs) = &mut graph.root.schema else {
            panic!("the fixture's root schema is a map");
        };
        let definition = match cardinality {
            Some(value) => TransitNode::Map(vec![(
                TransitNode::Keyword("db/cardinality".to_string()),
                value,
            )]),
            None => TransitNode::Str("not a declaration at all".to_string()),
        };
        attrs.push((
            TransitNode::Keyword("probe/attribute".to_string()),
            definition,
        ));
        with_tail_transaction(
            &graph,
            vec![tail_datom(
                1,
                "probe/attribute",
                TransitNode::Str("a".to_string()),
                536_871_024,
                kvs_writer::DatomOp::Assert,
            )],
        )
    };

    for (case, cardinality) in [
        (
            "an unrecognised cardinality keyword",
            Some(TransitNode::Keyword("db.cardinality/plenty".to_string())),
        ),
        (
            "a cardinality that is not a keyword",
            Some(TransitNode::Str("many".to_string())),
        ),
        ("a declaration that is not a map", None),
    ] {
        let graph = redeclared(cardinality);
        let error = kvs_writer::datoms_now(&graph)
            .expect_err(&format!("{case} must be refused, not defaulted to one"));
        assert!(
            matches!(
                &error,
                kvs_writer::RowError::MalformedAttributeDeclaration { attribute, .. }
                    if attribute == "probe/attribute"
            ),
            "{case}: the refusal must name the attribute; got {error:?}"
        );
    }
}

/// A base naming one tag twice is refused, not panicked on.
///
/// `push` is public, so its input is whatever the caller built. A list holding
/// `Task` twice cancels out — the attach side filters it because the block
/// already carries it, the detach side because it is still wanted — leaving a
/// block the diff reported as changed with nothing to write.
#[tokio::test]
async fn a_base_naming_one_tag_twice_is_refused_by_name() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let uuid = pushable_uuids(&graph, &base)
        .first()
        .expect("a pushable block")
        .clone();

    // Attach a class legitimately first, so the duplicate names a tag the
    // block really carries — the case that cancels on both sides.
    let name = TAG_CANDIDATES
        .iter()
        .find(|c| {
            !base
                .get(&uuid)
                .expect("present")
                .tags
                .contains(&c.to_string())
        })
        .expect("some class the block lacks");
    let mut tags = base.get(&uuid).expect("present").tags.clone();
    tags.push((*name).to_string());
    let attached = retagged(&base, &uuid, tags.clone());
    kvs_writer::push(&mut graph, &base, &attached).expect("the honest attach succeeds");

    let mut doubled = tags.clone();
    doubled.push((*name).to_string());
    doubled.sort();
    let next = retagged(&attached, &uuid, doubled);

    let error = kvs_writer::push(&mut graph, &attached, &next)
        .expect_err("a duplicate tag name must be refused, not asserted on");
    assert!(
        matches!(
            &error,
            kvs_writer::RowError::PushDuplicateTag { uuid: u, tag } if *u == uuid && tag == name
        ),
        "the refusal must name the block and the tag; got {error:?}"
    );
}

/// A base whose tags are not in the order the graph reports them is refused.
///
/// The same cancellation as a duplicate, reached differently: two lists
/// holding the same names in a different order are a diff to `BaseDiff` and no
/// change at all to the writer. A base carries tags SORTED — that is what
/// `ImportBase::observe` produces and what the graph is compared against — so
/// an unsorted one is not an observation this graph ever made.
#[tokio::test]
async fn a_base_whose_tags_are_unsorted_is_refused_by_name() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let uuid = pushable_uuids(&graph, &base)
        .first()
        .expect("a pushable block")
        .clone();

    let absent: Vec<&str> = TAG_CANDIDATES
        .iter()
        .filter(|c| {
            !base
                .get(&uuid)
                .expect("present")
                .tags
                .contains(&c.to_string())
        })
        .take(2)
        .copied()
        .collect();
    assert_eq!(absent.len(), 2, "need two classes the block lacks");

    let mut sorted = base.get(&uuid).expect("present").tags.clone();
    sorted.push(absent[0].to_string());
    sorted.push(absent[1].to_string());
    sorted.sort();
    let attached = retagged(&base, &uuid, sorted.clone());
    kvs_writer::push(&mut graph, &base, &attached).expect("the honest attach succeeds");

    // Same set, reversed. `retagged` sorts, so build the block by hand.
    let mut reversed = sorted.clone();
    reversed.reverse();
    assert_ne!(reversed, sorted, "the reversal must actually differ");
    let mut next = attached.clone();
    next.advance(
        &uuid,
        BaseBlock {
            tags: reversed,
            ..attached.get(&uuid).expect("present").clone()
        },
    )
    .expect("a known uuid");

    let error = kvs_writer::push(&mut graph, &attached, &next)
        .expect_err("an unsorted tag list must be refused, not asserted on");
    assert!(
        matches!(&error, kvs_writer::RowError::PushUnsortedTags { uuid: u, .. } if *u == uuid),
        "the refusal must name the block; got {error:?}"
    );
}

/// A malformed list is named as malformed on the OBSERVED side too.
///
/// Both checks are load-bearing and they fail differently. Without the
/// observed-side one the block still refuses — but as `PushBaseStaleTags`,
/// because a list the graph cannot equal reads as a graph disagreement. That
/// is the wrong story: the graph is fine and the base is not, and a caller
/// told its observation is stale will go re-import instead of fixing the list
/// it built. So this asserts the VARIANT, not merely that something failed.
#[tokio::test]
async fn a_malformed_observed_list_is_named_malformed_not_stale() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let base = base_of(&fixture()).await;
    let uuid = pushable_uuids(&graph, &base)
        .first()
        .expect("a pushable block")
        .clone();

    let absent: Vec<&str> = TAG_CANDIDATES
        .iter()
        .filter(|c| {
            !base
                .get(&uuid)
                .expect("present")
                .tags
                .contains(&c.to_string())
        })
        .take(2)
        .copied()
        .collect();
    assert_eq!(absent.len(), 2, "need two classes the block lacks");

    // Attach both honestly, so the graph really holds the sorted pair.
    let mut held = base.get(&uuid).expect("present").tags.clone();
    held.push(absent[0].to_string());
    held.push(absent[1].to_string());
    held.sort();
    let attached = retagged(&base, &uuid, held.clone());
    kvs_writer::push(&mut graph, &base, &attached).expect("the honest attach succeeds");

    // `retagged` sorts and dedupes nothing, so build the malformed OBSERVED
    // sides by hand.
    let malformed = |tags: Vec<String>| {
        let mut before = attached.clone();
        before
            .advance(
                &uuid,
                BaseBlock {
                    tags,
                    ..attached.get(&uuid).expect("present").clone()
                },
            )
            .expect("a known uuid");
        before
    };

    let mut reversed = held.clone();
    reversed.reverse();
    let mut doubled = held.clone();
    doubled.push(held[0].clone());
    doubled.sort();

    for (case, before) in [
        ("unsorted", malformed(reversed)),
        ("duplicated", malformed(doubled)),
    ] {
        // `after` is the list the graph actually holds, so the ONLY thing
        // wrong with this push is the observation it starts from.
        let error = kvs_writer::push(&mut graph, &before, &attached)
            .expect_err("a malformed observed list must be refused");
        assert!(
            matches!(
                &error,
                kvs_writer::RowError::PushUnsortedTags { uuid: u, .. }
                    | kvs_writer::RowError::PushDuplicateTag { uuid: u, .. } if *u == uuid
            ),
            "a malformed OBSERVED list ({case}) must be named malformed, not reported as a \
             stale observation; got {error:?}"
        );
    }
}
