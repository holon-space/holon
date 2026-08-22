//! W0: pure round-trip identity of a LogSeq DB graph through Rust.
//!
//! Holon opens the committed fixture read-only, decodes every `kvs` row,
//! re-encodes it, and writes a COPY. The copy must be the same graph by three
//! independent measures:
//!
//! 1. LogSeq's own validator says `Valid!` on it,
//! 2. LogSeq's own graph diff reports no datom delta against the original,
//! 3. Holon's importer reads it to an `ImportBase` equal to the original's.
//!
//! Legs 1 and 2 need a LogSeq checkout with its JS toolchain installed, which
//! no Rust build can assume, so both are `#[ignore]`d: a plain `cargo test`
//! reports them as ignored and never as passing. `just lsqdb-oracle` runs them
//! with `--ignored` and the checkout wired up — that recipe is the gate, and a
//! W-lane is not green without it. Leg 3 always runs, so the fixture is never
//! unguarded. Setup lives in `docs/Testing/LogseqDbOracle.md`.
//!
//! There is deliberately no skip-when-absent path. An oracle leg that passes
//! by not running is the one failure mode this file cannot tolerate, so
//! [`Oracle::find`] panics with the setup instructions instead.

use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

use holon_logseq_db::LogseqDbImporter;
use holon_logseq_db::TransitNode;
use holon_logseq_db::base::ImportBase;
use holon_logseq_db::decode_document;
use holon_logseq_db::encode_document;
use holon_logseq_db::kvs_writer;
use holon_logseq_db::kvs_writer::PINNED_BRANCHING_FACTOR;
use holon_logseq_db::kvs_writer::PINNED_REF_TYPE;
use holon_logseq_db::kvs_writer::ROOT_KEYS;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq-db/holontest.sqlite")
}

/// Read the fixture, re-encode it, and write the copy into `dir` as a LogSeq
/// graph directory (`<dir>/db.sqlite`, the layout `open-db!` expects).
async fn write_copy(dir: &Path) -> (kvs_writer::KvsGraph, kvs_writer::WriteReport) {
    let graph = kvs_writer::read_graph(&fixture())
        .await
        .expect("the committed fixture reads and passes the storage guards");
    std::fs::create_dir_all(dir).expect("copy directory is creatable");
    let report = kvs_writer::write_graph(&graph, &dir.join("db.sqlite"))
        .await
        .expect("writing the copy succeeds");
    (graph, report)
}

/// Where the LogSeq oracle lives.
///
/// Panics rather than skipping when the checkout is absent. These tests are
/// reached only through `--ignored`, so being here IS the statement that the
/// oracle was meant to run; degrading to a pass would turn the strongest legs
/// into the most misleading ones.
struct Oracle {
    deps_db: PathBuf,
}

impl Oracle {
    fn find() -> Self {
        const SETUP: &str = "see docs/Testing/LogseqDbOracle.md, or run `just lsqdb-oracle`";
        let root = std::env::var_os("HOLON_LOGSEQ_ORACLE").unwrap_or_else(|| {
            panic!(
                "HOLON_LOGSEQ_ORACLE is not set; it must name a LogSeq checkout at schema \
                 65.33 with deps/db dependencies installed ({SETUP})"
            )
        });
        let deps_db = PathBuf::from(root).join("deps/db");
        assert!(
            deps_db.join("node_modules").is_dir(),
            "{} has no node_modules ({SETUP})",
            deps_db.display()
        );
        for script in ["script/validate_db.cljs", "script/diff_graphs.cljs"] {
            assert!(
                deps_db.join(script).is_file(),
                "{} is missing {script}; LogSeq deleted both scripts upstream and they must be \
                 restored from its history ({SETUP})",
                deps_db.display()
            );
        }
        Self { deps_db }
    }

    /// Run one oracle script under nbb-logseq, returning its combined output.
    ///
    /// Graphs are named by an ABSOLUTE path to their `db.sqlite`: LogSeq's
    /// `->open-db-args` passes an absolute path straight through as the db
    /// file, while it treats a relative one as a graph directory to resolve.
    fn run(&self, script: &str, graphs: &[&Path], flags: &[&str]) -> String {
        let nbb = self.deps_db.join("node_modules/.bin/nbb-logseq");
        let mut cmd = Command::new(&nbb);
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

/// LEG 3 — Holon's own importer reads the copy to the same `ImportBase`.
///
/// The strongest leg that needs no external toolchain, and the one that would
/// catch a re-encode that changed a datom's value rather than the tree's shape.
#[tokio::test]
async fn leg3_importer_reads_the_copy_to_an_identical_base() {
    let dir = tempfile::tempdir().expect("temp dir");
    let copy_dir = dir.path().join("copy");
    let (graph, report) = write_copy(&copy_dir).await;

    assert_eq!(
        report.rows_written,
        graph.rows.len(),
        "every row must reach the copy"
    );
    // The headline W0 result, pinned rather than merely printed: writing the
    // graph back unchanged reproduces LogSeq's own bytes for EVERY row. It
    // holds because the Transit write cache is invertible, so any drop here is
    // a real divergence in the encoder, not expected noise.
    assert_eq!(
        report.rows_byte_identical, report.rows_written,
        "re-encoding an unchanged graph must reproduce LogSeq's bytes for every row"
    );

    let importer = LogseqDbImporter::new();
    let original = importer.import(&fixture()).await.expect("fixture imports");
    let copied = importer
        .import(&copy_dir.join("db.sqlite"))
        .await
        .expect("the copy imports");

    let base_original = ImportBase::from_import(&original);
    let base_copied = ImportBase::from_import(&copied);

    let diff = base_original.diff_against(&base_copied);
    assert!(
        diff.is_empty(),
        "the copy's import base differs from the fixture's: {} created, {} changed, {} removed \
         (fixture {} blocks, copy {} blocks; {} rows written)",
        diff.created.len(),
        diff.changed.len(),
        diff.removed.len(),
        base_original.len(),
        base_copied.len(),
        report.rows_written,
    );
    assert_eq!(
        base_original, base_copied,
        "the two import bases must be equal, not merely diff-free"
    );

    // The canonical form is the one the base is persisted in, so this is a
    // statement about the artifact rather than about hash seeds. Nothing is
    // weakened: a re-encode that reordered a nested map's keys is still caught
    // by `every_row_re_encodes_to_the_value_it_decoded_from`, whose
    // `TransitNode` maps are ordered, and by leg 2's datom-level diff.
    assert_eq!(
        base_original.to_canonical_json().expect("canonical form"),
        base_copied.to_canonical_json().expect("canonical form"),
        "the persisted import bases must be byte-identical"
    );
}

/// The storage parameters this writer is pinned to, asserted on the real graph.
#[tokio::test]
async fn fixture_declares_the_pinned_storage_parameters() {
    let graph = kvs_writer::read_graph(&fixture())
        .await
        .expect("fixture reads");

    assert_eq!(graph.root.branching_factor, PINNED_BRANCHING_FACTOR);
    assert_eq!(graph.root.ref_type, PINNED_REF_TYPE);

    // The version guard proven against a REAL graph, not only the synthetic
    // rows its unit tests build. The fixture also carries
    // `:logseq.kv/graph-initial-schema-version`, so reading the right one here
    // is a genuine discrimination.
    assert_eq!(
        kvs_writer::schema_version(&graph.rows).expect("the fixture declares its schema version"),
        kvs_writer::PINNED_SCHEMA_VERSION,
        "the committed fixture must be the version this build is pinned to"
    );
    kvs_writer::assert_pinned_schema_version(&graph.rows).expect("the fixture is writable");
    assert_eq!(
        graph.rows.len(),
        456,
        "the committed fixture is a 456-row graph"
    );
    assert_eq!(graph.rows[0].addr, 0, "addr 0 sorts first");

    // Every root key is a known one and every known one is present — the
    // unknown-key stop is what `RootNode::parse` already enforced to get here.
    let mut declared: Vec<&str> = graph
        .root
        .declared_keys()
        .iter()
        .map(String::as_str)
        .collect();
    declared.sort_unstable();
    let mut known: Vec<&str> = ROOT_KEYS.to_vec();
    known.sort_unstable();
    assert_eq!(declared, known, "addr-0 key set is exactly the known set");

    // Addr 0 is the one row rebuilt from parsed fields rather than replayed,
    // so it is the row where a misread storage parameter would show up.
    assert_eq!(
        encode_document(&graph.root.to_node()),
        graph.rows[0].original_content,
        "the root re-emitted from its parsed fields must reproduce LogSeq's own bytes"
    );

    // Child pointers live in the column alone. `read_graph` refuses a node
    // carrying both, so reaching here proves no row double-writes them.
    let with_addresses = graph.rows.iter().filter(|r| r.addresses.is_some()).count();
    assert_eq!(
        with_addresses, 23,
        "23 branch nodes carry an addresses column"
    );
    for row in &graph.rows {
        if let Some(addrs) = &row.addresses {
            assert!(
                !addrs.is_empty(),
                "addr {}: an addresses column that parses to nothing is not a branch node",
                row.addr
            );
        }
    }
}

/// Re-encoding every row of the real graph reproduces the value it held.
///
/// This is the property the Transit write cache can break silently: a
/// back-reference emitted one slot off still parses, into a different graph.
#[tokio::test]
async fn every_row_re_encodes_to_the_value_it_decoded_from() {
    let graph = kvs_writer::read_graph(&fixture())
        .await
        .expect("fixture reads");

    let mut byte_identical = 0usize;
    for row in &graph.rows {
        let encoded = encode_document(&row.node);
        let redecoded = decode_document(&encoded)
            .unwrap_or_else(|e| panic!("addr {}: re-encoded row does not decode: {e}", row.addr));
        assert_eq!(
            redecoded, row.node,
            "addr {}: re-encoding changed the value",
            row.addr
        );
        if encoded == row.original_content {
            byte_identical += 1;
        }
    }
    println!("byte-identical rows: {byte_identical}/{}", graph.rows.len());
    assert_eq!(
        byte_identical,
        graph.rows.len(),
        "the encoder must reproduce LogSeq's own bytes for every row, not merely the same value"
    );
}

/// LEG 1 — LogSeq's own validator accepts the graph Rust wrote.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn leg1_logseq_validator_accepts_the_copy() {
    let oracle = Oracle::find();

    let dir = tempfile::tempdir().expect("temp dir");
    let copy_dir = dir.path().join("copy");
    write_copy(&copy_dir).await;

    let out = oracle.run(
        "script/validate_db.cljs",
        &[&copy_dir.join("db.sqlite")],
        &["--closed-maps", "--group-errors"],
    );
    assert!(
        out.contains("Valid!"),
        "LogSeq's validator rejected the graph Rust wrote:\n{out}"
    );
}

/// LEG 2 — LogSeq's own graph diff reports no datom delta.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn leg2_logseq_diff_reports_no_datom_delta() {
    let oracle = Oracle::find();

    let dir = tempfile::tempdir().expect("temp dir");
    let pristine_dir = dir.path().join("pristine");
    let copy_dir = dir.path().join("copy");
    std::fs::create_dir_all(&pristine_dir).expect("pristine dir");
    std::fs::copy(fixture(), pristine_dir.join("db.sqlite")).expect("stage pristine");
    write_copy(&copy_dir).await;

    // `-T` keeps timestamps in the comparison; without it a re-encode that
    // dropped every :block/updated-at would still read as "equal".
    let out = oracle.run(
        "script/diff_graphs.cljs",
        &[&pristine_dir.join("db.sqlite"), &copy_dir.join("db.sqlite")],
        &["-T"],
    );
    assert!(
        out.contains("The two graphs are equal!"),
        "LogSeq's diff found a datom delta between the fixture and Rust's copy:\n{out}"
    );
}

// ------------------------------------------------------- W1: one title edit

const NEW_TITLE: &str = "W1 replaced this title";

/// The block this increment edits.
///
/// A function of the fixture, not a uuid pasted into the test: the smallest
/// uuid among blocks that have content AND a real parent. The parent condition
/// matters — without it the smallest uuid is a journal DAY PAGE, whose title
/// is date-derived and coupled to `:block/journal-day`. Editing an ordinary
/// nested block is both more representative of what Holon will push and free of
/// that coupling.
async fn pick_target() -> (String, String) {
    let importer = LogseqDbImporter::new();
    let imported = importer.import(&fixture()).await.expect("fixture imports");
    let base = ImportBase::from_import(&imported);
    let uuid = base
        .uuids()
        .filter(|u| {
            base.get(u)
                .is_some_and(|b| !b.content.is_empty() && !b.parent_id.starts_with("sentinel:"))
        })
        .min()
        .expect("the fixture has a titled block with a parent")
        .to_string();
    let content = base.get(&uuid).expect("just found it").content.clone();
    (uuid, content)
}

/// Apply the W1 edit to a fresh copy of the fixture in `dir`.
async fn write_edited_copy(dir: &Path) -> (kvs_writer::TitleEdit, kvs_writer::WriteReport, String) {
    let (uuid, _) = pick_target().await;
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let entity = kvs_writer::entity_by_uuid(&graph, &uuid).expect("the target block has an entity");

    let edit = kvs_writer::replace_block_title(&mut graph, entity, NEW_TITLE).expect("title edit");
    std::fs::create_dir_all(dir).expect("copy dir");
    let report = kvs_writer::write_graph(&graph, &dir.join("db.sqlite"))
        .await
        .expect("writes");
    (edit, report, uuid)
}

/// The edit touches EXACTLY the tail row — every other row is byte-identical.
///
/// This is the sharpest cheap signal that the tail path does what it claims:
/// if any tree row or addr 0 moved, the write was not the edit we described.
#[tokio::test]
async fn the_title_edit_changes_only_the_tail_row() {
    let dir = tempfile::tempdir().expect("temp dir");
    let (edit, report, _) = write_edited_copy(&dir.path().join("copy")).await;

    assert_ne!(edit.old_title, NEW_TITLE, "the edit must be a real change");
    assert_eq!(report.rows_written, 456);
    assert_eq!(
        report.rows_byte_identical, 455,
        "exactly one row may differ: the tail at addr 1. Addr 0 must NOT move, because \
         LogSeq's own `ldb/transact!` writes this same tail shape and leaves the root \
         alone — rewriting it would be a divergence from LogSeq, not tidiness"
    );
}

/// The tail says exactly "retract the old title, assert the new one", once.
#[tokio::test]
async fn the_tail_holds_one_retract_and_one_assert_under_a_new_tx() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    assert_eq!(
        graph.tail().expect("fixture tail parses").datom_count(),
        0,
        "the fixture starts with an empty tail"
    );
    let root_max_tx = graph.root.max_tx;

    let (uuid, _) = pick_target().await;
    let entity = kvs_writer::entity_by_uuid(&graph, &uuid).expect("entity");
    let edit = kvs_writer::replace_block_title(&mut graph, entity, NEW_TITLE).expect("edit");

    let tail = graph.tail().expect("edited tail parses");
    assert_eq!(tail.transactions().len(), 1, "one transaction");
    let tx = &tail.transactions()[0];
    assert_eq!(tx.len(), 2, "one retract and one assert, nothing else");

    assert_eq!(tx[0].op, kvs_writer::DatomOp::Retract);
    assert_eq!(tx[0].value, TransitNode::Str(edit.old_title.clone()));
    assert_eq!(tx[1].op, kvs_writer::DatomOp::Assert);
    assert_eq!(tx[1].value, TransitNode::Str(NEW_TITLE.to_string()));

    for datom in tx {
        assert_eq!(datom.entity, entity);
        assert_eq!(datom.attribute, "block/title");
        assert_eq!(datom.tx, edit.tx, "both halves share ONE new transaction");
    }
    // root + 2, not merely "greater than root": RESTORING a graph spends one
    // transaction id before any edit, so LogSeq's first edit on a pristine
    // graph takes root + 2 and so must this one. Measured on a copy at root
    // 536871022, where both take 536871024. Asserting only ">" would let the
    // seeding drift back to root + 1 — which it once was — without a red here.
    assert_eq!(
        edit.tx.get(),
        root_max_tx + 2,
        "the first edit on a pristine graph must take the id LogSeq would give \
         it (root {root_max_tx} + 2), not merely a larger one"
    );
}

/// LEG 3 for the edit — Holon's importer sees exactly one changed block.
#[tokio::test]
async fn the_edit_shows_up_as_exactly_one_changed_block() {
    let dir = tempfile::tempdir().expect("temp dir");
    let copy_dir = dir.path().join("copy");
    let (edit, _, uuid) = write_edited_copy(&copy_dir).await;

    let importer = LogseqDbImporter::new();
    let before = ImportBase::from_import(&importer.import(&fixture()).await.expect("fixture"));
    let after = ImportBase::from_import(
        &importer
            .import(&copy_dir.join("db.sqlite"))
            .await
            .expect("the edited copy imports"),
    );

    let diff = before.diff_against(&after);
    assert_eq!(
        (diff.created.len(), diff.changed.len(), diff.removed.len()),
        (0, 1, 0),
        "exactly one block changed, none created or removed: {diff:?}"
    );
    assert_eq!(diff.changed[0], uuid, "and it is the block we edited");
    assert_eq!(
        after.get(&uuid).expect("still present").content,
        NEW_TITLE,
        "the new title must be what the importer reads back"
    );
    assert_eq!(
        before.get(&uuid).expect("was present").content,
        edit.old_title
    );
}

/// LEG 1 for the edit — LogSeq's validator accepts the edited graph.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn leg1_logseq_validator_accepts_the_edited_copy() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");
    let copy_dir = dir.path().join("copy");
    write_edited_copy(&copy_dir).await;

    let out = oracle.run(
        "script/validate_db.cljs",
        &[&copy_dir.join("db.sqlite")],
        &["--closed-maps", "--group-errors"],
    );
    assert!(
        out.contains("Valid!"),
        "LogSeq's validator rejected the edited graph:\n{out}"
    );
}

/// LEG 2 for the edit — the delta is one value change and nothing else.
///
/// Names the SIZE and SHAPE of what LogSeq sees, not which block it happened
/// to; see the comment on the entry assertion.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn leg2_logseq_diff_reports_exactly_the_title_change() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");
    let pristine_dir = dir.path().join("pristine");
    let copy_dir = dir.path().join("copy");
    std::fs::create_dir_all(&pristine_dir).expect("pristine dir");
    std::fs::copy(fixture(), pristine_dir.join("db.sqlite")).expect("stage pristine");
    let (edit, _, _) = write_edited_copy(&copy_dir).await;

    let out = oracle.run(
        "script/diff_graphs.cljs",
        &[&pristine_dir.join("db.sqlite"), &copy_dir.join("db.sqlite")],
        &["-T"],
    );

    assert!(
        !out.contains("The two graphs are equal!"),
        "the edit must be visible to LogSeq at all:\n{out}"
    );

    // `diff_graphs` prints a clojure.data/diff: a datom vector per side where
    // every UNCHANGED slot is `nil`. So the only non-nil entries are the parts
    // that actually differ, and "exactly the retract+assert pair" means exactly
    // two of them — one per side.
    //
    // What this pins is the SHAPE of the delta: one datom per side, and a
    // value-only change. It does NOT pin WHICH entity changed — editing a
    // different block produces the identical `[nil nil …]` shape, because the
    // entity slot is nil precisely when it is the same on both sides. Identity
    // is leg 3's job (`the_edit_shows_up_as_exactly_one_changed_block` asserts
    // the changed uuid), and the two legs are only jointly sufficient.
    // A complete entry both opens and closes on one line; the pretty-printer
    // also wraps the enclosing vector's own `[`, which is not an entry.
    let entries: Vec<&str> = out
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('[') && line.contains(']'))
        .collect();
    assert_eq!(
        entries.len(),
        2,
        "exactly one datom may differ on each side; got {:#?}\nfull diff:\n{out}",
        entries
    );
    assert!(
        entries.iter().any(|e| e.contains(&edit.old_title)),
        "one side must hold the retracted title:\n{out}"
    );
    assert!(
        entries.iter().any(|e| e.contains(NEW_TITLE)),
        "the other side must hold the asserted title:\n{out}"
    );
    // `[nil nil "…"]` — the entity and attribute slots are nil, i.e. unchanged
    // between the two sides, so the delta is a VALUE change rather than a
    // different entity or a different attribute. Which entity it was is not
    // observable here; leg 3 names it.
    for entry in &entries {
        assert!(
            entry.starts_with("[nil nil "),
            "the delta must be a value change, not a different entity or \
             attribute: {entry}\nfull diff:\n{out}"
        );
    }
}
