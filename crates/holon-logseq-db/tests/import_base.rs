//! The import base (Inc 1): what LogSeq last looked like, per block, per field.
//!
//! The base is transport-independent — it is the third side of every later
//! three-way merge and the whole of echo suppression — so it survived the
//! 2026-08-22 reversal from the HTTP API to a closed-file writer intact.
//!
//! The headline test is deliberately a perturbation round trip rather than a
//! plain "importing twice changes nothing": the latter asserts only
//! `diff.is_empty()`, which any broken diff satisfies. A done-criterion that
//! cannot go red for the logic it names is not a done-criterion.

use std::path::PathBuf;

use holon_logseq_db::LogseqDbImporter;
use holon_logseq_db::base::BASE_FORMAT_VERSION;
use holon_logseq_db::base::BaseBlock;
use holon_logseq_db::base::BaseError;
use holon_logseq_db::base::ImportBase;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq-db/holontest.sqlite")
}

fn seeded(pairs: &[(&str, &str)]) -> ImportBase {
    let mut base = ImportBase::default();
    for (uuid, content) in pairs {
        base.witness_create(
            uuid,
            BaseBlock {
                content: (*content).to_string(),
                ..BaseBlock::default()
            },
        )
        .expect("a fresh uuid");
    }
    base
}

fn content_of(base: &ImportBase, uuid: &str) -> String {
    base.get(uuid).expect("present").content.clone()
}

// ------------------------------------------------------------ diff semantics

#[test]
fn two_identical_bases_differ_in_nothing() {
    let diff = seeded(&[("u1", "hello"), ("u2", "world")])
        .diff_against(&seeded(&[("u1", "hello"), ("u2", "world")]));
    assert!(diff.is_empty(), "unchanged must mean no diff, got {diff:?}");
}

#[test]
fn a_changed_block_is_reported_as_changed() {
    let diff = seeded(&[("u1", "hello"), ("u2", "world")])
        .diff_against(&seeded(&[("u1", "hello"), ("u2", "WORLD")]));
    assert_eq!(diff.changed, vec!["u2".to_string()], "diff was {diff:?}");
    assert_eq!(diff.len(), 1, "diff was {diff:?}");
}

#[test]
fn a_new_block_is_created_and_a_gone_block_is_removed() {
    let diff = seeded(&[("u1", "hello")]).diff_against(&seeded(&[("u2", "fresh")]));
    assert_eq!(diff.created, vec!["u2".to_string()], "diff was {diff:?}");
    assert_eq!(diff.removed, vec!["u1".to_string()], "diff was {diff:?}");
    assert!(diff.changed.is_empty(), "diff was {diff:?}");
}

/// H1: a LogSeq-side re-parent must be visible. A base that stores only content
/// reports nothing here, and Inc 5 would then keep Holon's stale parent without
/// ever surfacing a conflict.
#[test]
fn a_reparent_is_a_diff() {
    let before = seeded(&[("u1", "hello")]);
    let mut after = before.clone();
    after
        .advance(
            "u1",
            BaseBlock {
                content: "hello".to_string(),
                parent_id: "block:somewhere-else".to_string(),
                ..BaseBlock::default()
            },
        )
        .expect("u1 is tracked");
    let diff = before.diff_against(&after);
    assert_eq!(
        diff.changed,
        vec!["u1".to_string()],
        "a re-parent must be visible to the merge, got {diff:?}"
    );
}

/// H1, second half: sibling order. A move within a parent changes nothing else.
#[test]
fn a_reorder_among_siblings_is_a_diff() {
    let before = seeded(&[("u1", "hello")]);
    let mut after = before.clone();
    after
        .advance(
            "u1",
            BaseBlock {
                content: "hello".to_string(),
                position: Some(3),
                ..BaseBlock::default()
            },
        )
        .expect("u1 is tracked");
    assert_eq!(
        before.diff_against(&after).changed,
        vec!["u1".to_string()],
        "a re-order must be visible to the merge"
    );
}

/// H2: an edge or tag change carries no content change, so a content-only base
/// cannot see it.
#[test]
fn an_edge_or_tag_change_is_a_diff() {
    for mutate in [
        |b: &mut BaseBlock| b.contributes_to.push("block:other".to_string()),
        |b: &mut BaseBlock| b.tags.push("Page".to_string()),
        |b: &mut BaseBlock| b.requires.push("block:blocker".to_string()),
        |b: &mut BaseBlock| b.advice_suppressed.push("block:lesson".to_string()),
    ] {
        let before = seeded(&[("u1", "hello")]);
        let mut observed = before.get("u1").expect("present").clone();
        mutate(&mut observed);
        let mut after = before.clone();
        after.advance("u1", observed).expect("u1 is tracked");
        assert_eq!(
            before.diff_against(&after).changed,
            vec!["u1".to_string()],
            "an edge/tag change must be visible to the merge"
        );
    }
}

#[test]
fn a_property_change_is_a_diff() {
    let before = seeded(&[("u1", "hello")]);
    let mut observed = before.get("u1").expect("present").clone();
    observed.properties.insert(
        "TODO".to_string(),
        holon_api::Value::String("DONE".to_string()),
    );
    let mut after = before.clone();
    after.advance("u1", observed).expect("u1 is tracked");
    assert_eq!(
        before.diff_against(&after).changed,
        vec!["u1".to_string()],
        "a property change must be visible to the merge"
    );
}

// --------------------------------------------------- advance / retract rules

/// H3: a delete Holon pushed must stop echoing back as a LogSeq-side removal.
#[test]
fn a_retracted_block_does_not_echo_back_as_a_logseq_removal() {
    let mut base = seeded(&[("u1", "hello"), ("u2", "world")]);
    let logseq_after_our_delete = seeded(&[("u1", "hello")]);

    assert_eq!(
        base.diff_against(&logseq_after_our_delete).removed,
        vec!["u2".to_string()],
        "before retracting, our own delete looks like a LogSeq-side removal"
    );

    base.retract("u2").expect("u2 is tracked");
    assert!(
        base.diff_against(&logseq_after_our_delete).is_empty(),
        "after retracting, the delete must be invisible to the next import"
    );
}

/// H4: the base may lag reality, never lead it. Advancing a uuid the base does
/// not hold would claim LogSeq confirmed something it never saw.
#[test]
fn advancing_an_unknown_block_is_a_loud_error() {
    let mut base = seeded(&[("u1", "hello")]);
    let err = base
        .advance("phantom", BaseBlock::default())
        .expect_err("the base must refuse to lead reality");
    assert_eq!(
        err,
        BaseError::AdvanceUnknown {
            uuid: "phantom".to_string()
        }
    );
    assert_eq!(base.len(), 1, "a refused advance must not grow the base");
}

#[test]
fn retracting_an_unknown_block_is_a_loud_error() {
    let mut base = seeded(&[("u1", "hello")]);
    assert_eq!(
        base.retract("phantom")
            .expect_err("retracting what was never tracked is a disagreement"),
        BaseError::RetractUnknown {
            uuid: "phantom".to_string()
        }
    );
}

#[test]
fn witnessing_a_create_twice_is_a_loud_error() {
    let mut base = seeded(&[("u1", "hello")]);
    assert_eq!(
        base.witness_create("u1", BaseBlock::default())
            .expect_err("u1 already exists"),
        BaseError::CreateExisting {
            uuid: "u1".to_string()
        }
    );
}

#[test]
fn a_witnessed_create_becomes_advanceable() {
    let mut base = ImportBase::default();
    base.witness_create("fresh", BaseBlock::default())
        .expect("new uuid");
    base.advance(
        "fresh",
        BaseBlock {
            content: "now tracked".to_string(),
            ..BaseBlock::default()
        },
    )
    .expect("a witnessed block is advanceable");
    assert_eq!(content_of(&base, "fresh"), "now tracked");
}

// ------------------------------------------------------------- persistence

#[test]
fn a_saved_base_round_trips() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("base.json");
    let base = seeded(&[("u1", "hello"), ("u2", "world")]);
    base.save(&path).expect("save");
    assert_eq!(ImportBase::load(&path).expect("load"), base);
}

/// A base written by a different layout must be refused, not silently misread
/// as "every block changed".
#[test]
fn a_base_from_another_format_version_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("base.json");
    seeded(&[("u1", "hello")]).save(&path).expect("save");

    let doctored = std::fs::read_to_string(&path).expect("read").replace(
        &format!("\"version\": {BASE_FORMAT_VERSION}"),
        "\"version\": 999",
    );
    std::fs::write(&path, doctored).expect("write");

    let err = ImportBase::load(&path).expect_err("an unknown layout must be refused");
    assert!(
        err.to_string().contains("format version 999"),
        "the error must name the version it found, got: {err}"
    );
}

/// The pre-versioning layout: no `version` key, and a `BaseBlock` carrying only
/// `content`. It must fail LOUD. If `version` ever gained a serde default this
/// file would load as version 0 with every widened field defaulted, and the
/// first diff would report every block as changed — a silent, total false
/// positive that looks exactly like a LogSeq-side rewrite of the whole graph.
#[test]
fn an_old_content_only_base_is_refused_not_silently_defaulted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("base.json");
    std::fs::write(
        &path,
        r#"{"blocks":{"u1":{"content":"hello"},"u2":{"content":"world"}}}"#,
    )
    .expect("write");

    let err = ImportBase::load(&path).expect_err("a pre-versioning base must be refused");
    let msg = err.to_string();
    assert!(
        msg.contains("carries no format version"),
        "the error must name the missing version, got: {msg}"
    );
    assert!(
        msg.contains("Re-import"),
        "the error must say what to do about it, got: {msg}"
    );
}

// ------------------------------------------------------- the done-criterion

/// Inc 1's done-criterion, stated so it can actually go red.
///
/// A plain "import twice reports nothing" asserts only `diff.is_empty()`, which
/// a diff that always returns nothing satisfies. This drives the full cycle
/// against the real fixture: import, perturb one block, require the diff to SEE
/// it, advance the base, require the echo to vanish.
#[tokio::test]
async fn the_fixture_round_trips_and_a_perturbation_is_seen_then_absorbed() {
    let importer = LogseqDbImporter::new();
    let first = ImportBase::from_import(&importer.import(&fixture()).await.expect("first import"));
    let second =
        ImportBase::from_import(&importer.import(&fixture()).await.expect("second import"));

    assert!(
        !first.is_empty(),
        "the fixture must produce a non-empty base, or this test proves nothing"
    );
    assert_eq!(first.version(), BASE_FORMAT_VERSION);
    assert!(
        first.diff_against(&second).is_empty(),
        "re-importing an unchanged graph must report nothing: {:?}",
        first.diff_against(&second)
    );

    // A LogSeq-side edit to one real block.
    let victim = first.uuids().next().expect("non-empty").to_string();
    let mut perturbed = second.clone();
    let mut edited = perturbed.get(&victim).expect("present").clone();
    edited.content.push_str(" — edited in LogSeq");
    perturbed.advance(&victim, edited.clone()).expect("tracked");

    let diff = first.diff_against(&perturbed);
    assert_eq!(
        diff.changed,
        vec![victim.clone()],
        "the diff must SEE a real perturbation, got {diff:?}"
    );

    // Holon observes it; the echo disappears.
    let mut absorbed = first.clone();
    absorbed.advance(&victim, edited).expect("tracked");
    assert!(
        absorbed.diff_against(&perturbed).is_empty(),
        "after advancing, the perturbation must be absorbed: {:?}",
        absorbed.diff_against(&perturbed)
    );
}

/// The base keys on the bare LogSeq uuid, which is also the Holon block id, and
/// mirrors every projected field.
#[tokio::test]
async fn the_base_mirrors_the_projection_keyed_by_bare_uuid() {
    let importer = LogseqDbImporter::new();
    let result = importer.import(&fixture()).await.expect("import");
    let base = ImportBase::from_import(&result);

    assert_eq!(base.len(), result.blocks.len());
    for block in &result.blocks {
        let uuid = block.id.id();
        let based = base
            .get(uuid)
            .unwrap_or_else(|| panic!("block {} is missing from the base", block.id));
        assert_eq!(based.content, block.content, "content drifted for {uuid}");
        assert_eq!(
            based.parent_id,
            block.parent_id.to_string(),
            "parent drifted for {uuid}"
        );
        assert!(
            uuid::Uuid::parse_str(uuid).is_ok(),
            "base key {uuid:?} is not a bare uuid — a scheme prefix leaked in"
        );
    }

    // The fixture has ordered siblings, so at least one block must carry a
    // position — otherwise `position` is dead weight and a re-order is still
    // invisible in practice.
    assert!(
        base.uuids()
            .filter_map(|u| base.get(u))
            .any(|b| b.position.is_some()),
        "no block carried a sibling position; a re-order would be invisible"
    );
}

// ------------------------------------------------- the persisted form is data

/// Two imports of the SAME graph must persist to the SAME bytes.
///
/// The base is a file that lives next to the graph and gets re-written after
/// every push. If re-importing unchanged data produces different bytes, every
/// push shows a spurious VCS diff and the file stops being reviewable — which
/// is exactly what `save`'s own contract promises it will not do.
///
/// Two SEPARATE imports are required to see it. Serializing one base twice
/// cannot: a `HashMap` iterates consistently within its own lifetime, so the
/// disorder only shows between two independently built maps.
/// See bugfunnel 2026-08-22-importbase-serialization-is-not-byte-stable.
#[tokio::test]
async fn two_imports_of_one_graph_persist_to_identical_bytes() {
    let importer = LogseqDbImporter::new();
    let first = ImportBase::from_import(&importer.import(&fixture()).await.expect("first import"));
    let second =
        ImportBase::from_import(&importer.import(&fixture()).await.expect("second import"));

    let dir = tempfile::tempdir().expect("temp dir");
    let a = dir.path().join("a.json");
    let b = dir.path().join("b.json");
    first.save(&a).expect("first base saves");
    second.save(&b).expect("second base saves");

    let bytes_a = std::fs::read_to_string(&a).expect("read a");
    let bytes_b = std::fs::read_to_string(&b).expect("read b");

    if bytes_a != bytes_b {
        let at = bytes_a
            .chars()
            .zip(bytes_b.chars())
            .position(|(x, y)| x != y)
            .expect("differing strings share no prefix only if one is empty");
        let window = |s: &str| {
            s.chars()
                .skip(at.saturating_sub(80))
                .take(200)
                .collect::<String>()
        };
        panic!(
            "the same graph persisted to different bytes, first differing at char {at}\n\
             A: …{}…\nB: …{}…",
            window(&bytes_a),
            window(&bytes_b)
        );
    }
}

/// The canonical form is what `save` writes, so a caller can compare bases
/// without going through the filesystem.
#[tokio::test]
async fn the_canonical_form_is_what_save_persists() {
    let importer = LogseqDbImporter::new();
    let base = ImportBase::from_import(&importer.import(&fixture()).await.expect("import"));

    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("base.json");
    base.save(&path).expect("saves");

    assert_eq!(
        std::fs::read_to_string(&path).expect("read"),
        base.to_canonical_json().expect("canonical form"),
        "save must write exactly the canonical form, or the two can drift apart"
    );
}
