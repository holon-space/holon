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

    assert_eq!(
        canonical_json(&base_original),
        canonical_json(&base_copied),
        "the serialized import bases must be byte-identical"
    );
}

/// `value` serialized with every object's keys in sorted order.
///
/// A plain `to_string` cannot be compared here: a `_logseq_raw/*` property
/// holding a nested map reaches the base as `holon_api::Value::Object`, which
/// is a `HashMap`, so its emission order varies between processes and even
/// between two imports of the SAME file. Sorting makes the comparison a
/// statement about content instead of about hash seeds. Nothing is weakened by
/// it: a re-encode that reordered a map's keys is caught by
/// [`every_row_re_encodes_to_the_value_it_decoded_from`], whose `TransitNode`
/// maps are ordered, and by leg 2's datom-level diff.
fn canonical_json<T: serde::Serialize>(value: &T) -> String {
    fn sort(v: serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(map) => {
                let mut entries: Vec<_> = map.into_iter().map(|(k, v)| (k, sort(v))).collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                serde_json::Value::Object(entries.into_iter().collect())
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.into_iter().map(sort).collect())
            }
            other => other,
        }
    }
    sort(serde_json::to_value(value).expect("base serializes")).to_string()
}

/// The storage parameters this writer is pinned to, asserted on the real graph.
#[tokio::test]
async fn fixture_declares_the_pinned_storage_parameters() {
    let graph = kvs_writer::read_graph(&fixture())
        .await
        .expect("fixture reads");

    assert_eq!(graph.root.branching_factor, PINNED_BRANCHING_FACTOR);
    assert_eq!(graph.root.ref_type, PINNED_REF_TYPE);
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
