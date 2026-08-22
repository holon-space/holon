//! Flushing the tail into the index trees.
//!
//! The tail holds at most `PINNED_BRANCHING_FACTOR` datoms; the edit that
//! would exceed that is the one LogSeq flushes on. This is the same boundary
//! W1's guard refused at — B is what makes crossing it possible.
//!
//! Measured shape of one LogSeq flush, which these tests hold Holon to:
//! 456 rows -> 458, exactly 2 added (from a split, at max-addr+1 and +2),
//! 27 rewritten in place INCLUDING addr 0 and addr 1, the three index roots
//! keeping their addresses, row 1 reset to `[]`, max-tx advanced and max-eid
//! untouched. See scratchpad/logs/flush-anatomy.log.

use std::path::Path;
use std::path::PathBuf;

use holon_logseq_db::LogseqDbImporter;
use holon_logseq_db::base::ImportBase;
use holon_logseq_db::kvs_writer;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq-db/holontest.sqlite")
}

/// Titles for successive edits, so every one is a real change.
fn title(i: usize) -> String {
    format!("flush probe {i:03}")
}

/// Fill the tail to exactly the branching factor: 16 two-datom edits.
async fn graph_with_a_full_tail() -> (kvs_writer::KvsGraph, Vec<String>) {
    let mut graph = kvs_writer::read_graph(&fixture())
        .await
        .expect("fixture reads");
    let importer = LogseqDbImporter::new();
    let base = ImportBase::from_import(&importer.import(&fixture()).await.expect("imports"));

    let targets: Vec<String> = base
        .uuids()
        .filter(|u| {
            base.get(u)
                .is_some_and(|b| !b.content.is_empty() && !b.parent_id.starts_with("sentinel:"))
        })
        .take(17)
        .map(str::to_string)
        .collect();
    assert_eq!(targets.len(), 17, "need 17 editable blocks");

    for (i, uuid) in targets.iter().take(16).enumerate() {
        let entity = kvs_writer::entity_by_uuid(&graph, uuid)
            .expect("read")
            .expect("entity");
        kvs_writer::replace_block_title(&mut graph, entity, &title(i)).expect("edit");
    }
    assert_eq!(
        graph.tail().expect("tail").datom_count(),
        32,
        "16 two-datom edits fill the tail exactly to the branching factor"
    );
    (graph, targets)
}

/// The flush empties the tail into the trees and leaves the graph consistent.
#[tokio::test]
async fn flushing_moves_the_tail_into_the_trees() {
    let (mut graph, _) = graph_with_a_full_tail().await;
    let rows_before = graph.rows.len();
    let max_addr_before = graph.root.max_addr;
    let roots_before = (graph.root.eavt, graph.root.aevt, graph.root.avet);

    let report = kvs_writer::flush_tail(&mut graph).expect("flush");

    assert_eq!(report.transactions, 16, "16 transactions were in the tail");
    assert_eq!(report.datoms_asserted, 16);
    assert_eq!(report.datoms_retracted, 16);
    assert_eq!(
        graph.tail().expect("tail").datom_count(),
        0,
        "the tail must be empty afterwards"
    );

    // LogSeq keeps its index roots' addresses across a flush, and only
    // split-new nodes take fresh ones.
    assert_eq!(
        (graph.root.eavt, graph.root.aevt, graph.root.avet),
        roots_before,
        "the index roots must not move for an edit of this size"
    );
    // Proof the trees were WRITTEN is rows_modified, not allocation: a new
    // address appears only when a leaf overflows and splits, which depends on
    // where the edited values land. 16 replacements remove as many datoms as
    // they add, so a flush with no split is a legitimate outcome.
    assert!(
        report.rows_modified > 0,
        "a flush that rewrote no rows did not write the trees"
    );
    assert!(
        graph.root.max_addr >= max_addr_before,
        "max-addr must never go backwards"
    );
    println!(
        "flush: {} tx, +{} rows, {} rewritten, orphans {} -> {}, max-addr {} -> {}, max-tx {}",
        report.transactions,
        report.rows_added,
        report.rows_modified,
        report.orphans_before,
        report.orphans_after,
        max_addr_before,
        graph.root.max_addr,
        report.max_tx,
    );
    assert_eq!(
        graph.rows.len() - rows_before,
        report.rows_added,
        "every added row must be accounted for in the report"
    );

    // The census only ever grows: a merged-away node is abandoned, never
    // deleted, and nothing may resurrect one.
    assert!(
        report.orphans_after >= report.orphans_before,
        "orphan rows went from {} to {}; a flush must not delete or revive rows",
        report.orphans_before,
        report.orphans_after
    );

    // The datom count is unchanged: each edit retracts one and asserts one.
    assert_eq!(
        graph.root.eavt_metadata.count, 2609,
        "16 replacements leave the datom count alone"
    );
    assert_eq!(graph.root.avet_metadata.count, 2264);
}

/// The flushed edits are what the importer reads back.
#[tokio::test]
async fn the_flushed_edits_survive_a_re_import() {
    let dir = tempfile::tempdir().expect("temp dir");
    let copy = dir.path().join("db.sqlite");
    let (mut graph, targets) = graph_with_a_full_tail().await;
    kvs_writer::flush_tail(&mut graph).expect("flush");
    kvs_writer::write_graph(&graph, &copy).await.expect("write");

    // Non-vacuity FIRST. Without this the test passes against a flush that
    // did nothing: the tail would still hold the edits and the importer would
    // replay them, so "the edits survived" would say nothing about the trees.
    let written = kvs_writer::read_graph(&copy).await.expect("copy reads");
    assert_eq!(
        written.tail().expect("tail").datom_count(),
        0,
        "the written copy still has a tail, so the assertions below would be \
         measuring tail replay rather than the trees"
    );

    let importer = LogseqDbImporter::new();
    let before = ImportBase::from_import(&importer.import(&fixture()).await.expect("fixture"));
    let after = ImportBase::from_import(&importer.import(&copy).await.expect("copy imports"));

    let diff = before.diff_against(&after);
    assert_eq!(
        (diff.created.len(), diff.changed.len(), diff.removed.len()),
        (0, 16, 0),
        "exactly the 16 edited blocks changed: {diff:?}"
    );
    for (i, uuid) in targets.iter().take(16).enumerate() {
        assert_eq!(
            after.get(uuid).expect("still present").content,
            title(i),
            "block {uuid} did not come back with its flushed title"
        );
    }
}

// ------------------------------------------------------------ the oracles

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

async fn flushed_copy(dir: &Path) -> Vec<String> {
    let (mut graph, targets) = graph_with_a_full_tail().await;
    kvs_writer::flush_tail(&mut graph).expect("flush");
    // Non-vacuity, checked HERE so every leg built on this copy inherits it:
    // under a flush that did nothing the tail would still hold the edits, the
    // validator and the differ would both see them replayed, and leg 1's claim
    // to be judging REBUILT TREES would be false while staying green.
    assert_eq!(
        graph.tail().expect("tail").datom_count(),
        0,
        "the tail is not empty, so the legs below would be judging tail replay \
         rather than the trees"
    );
    std::fs::create_dir_all(dir).expect("dir");
    kvs_writer::write_graph(&graph, &dir.join("db.sqlite"))
        .await
        .expect("write");
    targets
}

/// LEG 1 — LogSeq's validator accepts a graph whose TREES Holon rewrote.
///
/// The sharpest thing this increment can be asked: W1 only ever appended to a
/// log LogSeq replays, whereas this graph's B+-trees were rebuilt by Holon.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn leg1_logseq_validator_accepts_the_flushed_copy() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");
    let copy_dir = dir.path().join("copy");
    flushed_copy(&copy_dir).await;

    let out = oracle.run(
        "script/validate_db.cljs",
        &[&copy_dir.join("db.sqlite")],
        &["--closed-maps", "--group-errors"],
    );
    assert!(
        out.contains("Valid!"),
        "LogSeq's validator rejected the graph whose trees Holon rewrote:\n{out}"
    );
    // Non-vacuity: a validator that read an empty or truncated graph would
    // also not complain about entities it never saw.
    assert!(
        out.contains(":datoms 2609"),
        "the validator must have read the whole graph back:\n{out}"
    );
}

/// LEG 2 — the delta LogSeq sees is exactly the 16 titles and nothing else.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn leg2_logseq_diff_shows_exactly_the_flushed_titles() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");
    let pristine_dir = dir.path().join("pristine");
    let copy_dir = dir.path().join("copy");
    std::fs::create_dir_all(&pristine_dir).expect("dir");
    std::fs::copy(fixture(), pristine_dir.join("db.sqlite")).expect("stage");
    flushed_copy(&copy_dir).await;

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
        32,
        "expected 16 datoms per side and nothing else; got {}:\n{out}",
        entries.len()
    );
    for entry in &entries {
        assert!(
            entry.starts_with("[nil nil "),
            "every difference must be a VALUE change on one block's one \
             attribute: {entry}\n{out}"
        );
    }
    for i in 0..16 {
        assert!(
            out.contains(&title(i)),
            "flushed title {i} is missing from the delta:\n{out}"
        );
    }
}

/// HEAD TO HEAD — the same 17 edits, applied by LogSeq and by Holon, compared
/// row by row.
///
/// The only comparison that can say whether the addressing, split, merge and
/// root-collapse rules are all right at once. Both sides start from the
/// committed fixture and make the IDENTICAL edits in the IDENTICAL order:
/// the entity list is chosen here and handed to LogSeq, so a difference in the
/// output is a difference in the WRITER and nothing else.
///
/// LogSeq's 17th transaction overflows its tail and is flushed together with
/// the 16 before it; Holon appends then flushes on the same rule, so both end
/// with an empty tail and 17 transactions in the trees.
///
/// BYTE IDENTITY IS ASSERTED, not merely reported: the two writers produce the
/// same 458 rows with the same content and the same child pointers. That is the
/// only cheap regression signal for the split, merge and redistribution rules
/// together — each has to be right for this to hold, and a future change that
/// breaks one fails HERE rather than being judged by oracles that pass on a
/// differently-shaped tree. Root collapse is NOT exercised by these 17 edits
/// (the tree never shrinks that far); it has its own test in tree_edit.rs.
#[tokio::test]
#[ignore = "needs HOLON_LOGSEQ_ORACLE — see docs/Testing/LogseqDbOracle.md"]
async fn head_to_head_with_logseqs_own_flush() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");

    // Targets: the first 17 distinct entities carrying :block/title in aevt
    // order — a function of the graph, computed from Holon's own walk.
    let base_graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let tree = holon_logseq_db::tree::Tree::load(&base_graph, holon_logseq_db::tree::Index::Aevt)
        .expect("aevt loads");
    let mut targets: Vec<i64> = Vec::new();
    for datom in tree.datoms().expect("walk") {
        if datom.a == "block/title" && !targets.contains(&datom.e) {
            targets.push(datom.e);
            if targets.len() == 17 {
                break;
            }
        }
    }
    assert_eq!(targets.len(), 17, "need 17 titled entities");

    let edits: Vec<(i64, String)> = targets
        .iter()
        .enumerate()
        .map(|(i, e)| (*e, title(i)))
        .collect();
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
        out.contains("applied 17 edits"),
        "LogSeq did not apply the edits:\n{out}"
    );

    // --- Holon's side
    let holon_dir = dir.path().join("holon");
    std::fs::create_dir_all(&holon_dir).expect("dir");
    let holon_db = holon_dir.join("db.sqlite");
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    for (entity, new_title) in &edits {
        kvs_writer::replace_block_title(&mut graph, *entity, new_title).expect("edit");
    }
    assert_eq!(
        graph.tail().expect("tail").datom_count(),
        0,
        "the 17th edit must have overflowed and flushed, as LogSeq's does"
    );
    kvs_writer::write_graph(&graph, &holon_db)
        .await
        .expect("write");

    // --- Compare, row by row
    let theirs = kvs_writer::read_graph(&logseq_db)
        .await
        .expect("logseq reads");
    let ours = kvs_writer::read_graph(&holon_db)
        .await
        .expect("holon reads");

    let addrs: std::collections::BTreeSet<i64> = theirs
        .rows
        .iter()
        .chain(ours.rows.iter())
        .map(|r| r.addr)
        .collect();
    let (mut identical, mut differing, mut only_theirs, mut only_ours) = (0, 0, 0, 0);
    let mut first_differences = Vec::new();
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
                        // The first differing CHARACTER with context, not the
                        // first 160: these rows share long identical prefixes,
                        // so a prefix tells you nothing about the cause.
                        let at = t
                            .original_content
                            .chars()
                            .zip(o.original_content.chars())
                            .position(|(a, b)| a != b)
                            .unwrap_or(0);
                        let window = |s: &str| {
                            s.chars()
                                .skip(at.saturating_sub(60))
                                .take(130)
                                .collect::<String>()
                        };
                        first_differences.push(format!(
                            "addr {addr} (addresses {:?} vs {:?}), first differs at char {at}:\n                               logseq: …{}…\n  holon : …{}…",
                            t.addresses,
                            o.addresses,
                            window(&t.original_content),
                            window(&o.original_content),
                        ));
                    }
                }
            }
            (Some(_), None) => only_theirs += 1,
            (None, Some(_)) => only_ours += 1,
            (None, None) => unreachable!("addr came from one of the two"),
        }
    }

    println!(
        "HEAD TO HEAD: identical={identical} differing={differing} \
         only-logseq={only_theirs} only-holon={only_ours} \
         (logseq {} rows, holon {} rows)",
        theirs.rows.len(),
        ours.rows.len()
    );
    for d in &first_differences {
        println!("--- {d}");
    }

    // Every differing row must differ ONLY in transaction ids, each exactly
    // one lower on Holon's side. Proven for ALL of them rather than eyeballed
    // for a few: LogSeq's restore consumes one transaction id before its first
    // edit and Holon's does not, so Holon's ids trail by one throughout. If
    // ANY difference survives that normalisation it is structural — a real
    // disagreement about addressing, splitting, merging or collapsing.
    let shift_tx_down = |s: &str| {
        let mut out = String::with_capacity(s.len());
        let mut digits = String::new();
        for ch in s.chars().chain(std::iter::once('\0')) {
            if ch.is_ascii_digit() {
                digits.push(ch);
                continue;
            }
            if !digits.is_empty() {
                match digits.parse::<i64>() {
                    Ok(n) if (536_871_023..=536_871_041).contains(&n) => {
                        out.push_str(&(n - 1).to_string());
                    }
                    _ => out.push_str(&digits),
                }
                digits.clear();
            }
            if ch != '\0' {
                out.push(ch);
            }
        }
        out
    };
    let mut structural = Vec::new();
    for addr in &addrs {
        let (Some(t), Some(o)) = (
            theirs.rows.iter().find(|r| r.addr == *addr),
            ours.rows.iter().find(|r| r.addr == *addr),
        ) else {
            continue;
        };
        if t.original_content != o.original_content
            && shift_tx_down(&t.original_content) != o.original_content
        {
            structural.push(*addr);
        }
    }
    if !structural.is_empty() {
        for addr in structural.iter().take(4) {
            let t = theirs.rows.iter().find(|r| r.addr == *addr).expect("row");
            let o = ours.rows.iter().find(|r| r.addr == *addr).expect("row");
            let normalised = shift_tx_down(&t.original_content);
            let at = normalised
                .chars()
                .zip(o.original_content.chars())
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            let window = |s: &str| {
                s.chars()
                    .skip(at.saturating_sub(70))
                    .take(150)
                    .collect::<String>()
            };
            println!(
                "STRUCTURAL addr {addr} (addresses {:?} vs {:?}), lens {} vs {}, \
                 first differs at {at} AFTER tx-normalisation:\n  logseq: …{}…\n  holon : …{}…",
                t.addresses,
                o.addresses,
                normalised.chars().count(),
                o.original_content.chars().count(),
                window(&normalised),
                window(&o.original_content),
            );
        }
    }
    // Both counts are ZERO today, so this classification exists to make a
    // FAILURE legible rather than to record a known gap: if the bytes ever
    // diverge again, it says whether the cause is transaction NUMBERING (every
    // difference vanishes when LogSeq's ids are shifted down by one) or
    // STRUCTURE (a real disagreement about where datoms sit). Those two want
    // completely different investigations, and telling them apart by eye cost
    // this lane a wrong conclusion once already.
    assert!(
        structural.is_empty(),
        "{} row(s) differ for a reason OTHER than transaction ids: {structural:?}. \
         That is a structural disagreement about addressing, splitting, merging \
         or collapsing.",
        structural.len()
    );
    assert_eq!(
        differing, 0,
        "the two writers no longer produce identical bytes: {differing} row(s) differ"
    );

    // LogSeq's own diff must call the two outputs equal whatever the bytes do.
    let diff = oracle.run("script/diff_graphs.cljs", &[&logseq_db, &holon_db], &["-T"]);
    assert!(
        diff.contains("The two graphs are equal!"),
        "the two writers produced DIFFERENT GRAPHS, not merely different bytes:\n{diff}"
    );
    assert_eq!(
        (only_theirs, only_ours),
        (0, 0),
        "the two writers disagree about which rows exist"
    );
}

/// BISECT — the first edit count at which the two writers' partitions diverge.
///
/// A diagnostic, not a gate: it exists to localise the leaf-boundary
/// divergence rather than to guard against it. LogSeq is forced to store at
/// each N (its tail would otherwise only flush at 17), so both sides have
/// written trees for the same N transactions.
#[tokio::test]
#[ignore = "diagnostic; needs HOLON_LOGSEQ_ORACLE"]
async fn bisect_the_partition_divergence() {
    let oracle = Oracle::find();
    let dir = tempfile::tempdir().expect("temp dir");

    let base_graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    let tree = holon_logseq_db::tree::Tree::load(&base_graph, holon_logseq_db::tree::Index::Aevt)
        .expect("aevt");
    let mut targets: Vec<i64> = Vec::new();
    for d in tree.datoms().expect("walk") {
        if d.a == "block/title" && !targets.contains(&d.e) {
            targets.push(d.e);
            if targets.len() == 17 {
                break;
            }
        }
    }

    let mut previous: Option<Vec<Partition>> = None;
    for n in 1..=17usize {
        let edits: Vec<(i64, String)> = targets
            .iter()
            .take(n)
            .enumerate()
            .map(|(i, e)| (*e, title(i)))
            .collect();
        let edits_json = dir.path().join(format!("edits{n}.json"));
        std::fs::write(&edits_json, serde_json::to_string(&edits).expect("json")).expect("write");

        let theirs_path = dir.path().join(format!("logseq{n}.sqlite"));
        std::fs::copy(fixture(), &theirs_path).expect("stage");
        oracle.run(
            "script/apply_edits.cljs",
            &[&theirs_path, &edits_json],
            &["--store"],
        );

        let ours_path = dir.path().join(format!("holon{n}.sqlite"));
        let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
        for (entity, t) in &edits {
            kvs_writer::replace_block_title(&mut graph, *entity, t).expect("edit");
        }
        kvs_writer::flush_tail(&mut graph).expect("flush");
        kvs_writer::write_graph(&graph, &ours_path)
            .await
            .expect("write");

        let theirs = kvs_writer::read_graph(&theirs_path).await.expect("reads");
        let ours = kvs_writer::read_graph(&ours_path).await.expect("reads");
        let differing = ours
            .rows
            .iter()
            .filter(|o| {
                theirs
                    .rows
                    .iter()
                    .find(|t| t.addr == o.addr)
                    .is_none_or(|t| t.original_content != o.original_content)
            })
            .count();

        // Keep this N's partitions so the divergent step can be shown against
        // the step before it — a partition on its own says nothing about which
        // operation moved the boundary.
        let snapshot: Vec<Partition> = [
            holon_logseq_db::tree::Index::Eavt,
            holon_logseq_db::tree::Index::Aevt,
            holon_logseq_db::tree::Index::Avet,
        ]
        .into_iter()
        .map(|index| {
            let t = holon_logseq_db::tree::Tree::load(&ours, index).expect("o");
            (format!("{index:?}"), t.leaf_lengths().expect("o"))
        })
        .collect();

        if differing > 0 {
            println!("FIRST DIVERGENCE at N={n}: {differing} row(s) differ");
            println!("edit {n} targets entity {}", edits[n - 1].0);
            // Which branch owns the leaves that moved: a node with no LEFT
            // sibling merges rightwards instead, so the grouping decides the
            // direction.
            for index in [
                holon_logseq_db::tree::Index::Eavt,
                holon_logseq_db::tree::Index::Avet,
            ] {
                let t = holon_logseq_db::tree::Tree::load(&theirs, index).expect("t");
                for (addr, kids) in t.branches().expect("branches") {
                    if kids
                        .iter()
                        .any(|k| (1000008..=1000011).contains(k) || (1000333..=1000337).contains(k))
                    {
                        println!("  {index:?} branch {addr} children: {kids:?}");
                    }
                }
            }
            if let Some(prev) = &previous {
                for (name, lengths) in prev {
                    let window: Vec<_> = lengths
                        .iter()
                        .filter(|(a, _)| {
                            (1000008..=1000011).contains(a) || (1000333..=1000337).contains(a)
                        })
                        .collect();
                    println!("  N-1 {name} (both sides identical here): {window:?}");
                }
            }
            for index in [
                holon_logseq_db::tree::Index::Eavt,
                holon_logseq_db::tree::Index::Aevt,
                holon_logseq_db::tree::Index::Avet,
            ] {
                let t = holon_logseq_db::tree::Tree::load(&theirs, index).expect("t");
                let o = holon_logseq_db::tree::Tree::load(&ours, index).expect("o");
                let (tl, ol) = (t.leaf_lengths().expect("t"), o.leaf_lengths().expect("o"));
                if tl != ol {
                    let at = tl.iter().zip(&ol).position(|(a, b)| a != b);
                    println!(
                        "  {index:?} partitions differ at leaf {at:?}\n    logseq: {:?}\n    holon : {:?}",
                        &tl[at.unwrap_or(0).saturating_sub(1)..(at.unwrap_or(0) + 3).min(tl.len())],
                        &ol[at.unwrap_or(0).saturating_sub(1)..(at.unwrap_or(0) + 3).min(ol.len())],
                    );
                } else {
                    println!("  {index:?} partitions IDENTICAL");
                }
            }
            panic!(
                "the writers diverge again, first at N={n}. They agreed at every N \
                 once the redistribution rule was corrected, so this is a regression \
                 in split, merge, redistribution or root collapse."
            );
        }
        previous = Some(snapshot);
        println!("N={n}: identical");
    }
    // Reaching here is the expected outcome: the two writers agree at every
    // edit count. The loop above panics the moment they stop agreeing, so this
    // stays a real test rather than a printout.
    println!("no divergence at any N up to 17");
}

/// One index's leaf partition: its name and each leaf's (address, key count).
type Partition = (String, Vec<(i64, usize)>);

/// Flushing an empty tail reports that NOTHING happened, and touches nothing.
///
/// The report is an observation, so it must not invent one. A default-valued
/// `orphans_after` beside a real `orphans_before` reads as a graph that just
/// lost every unreferenced row — the opposite of the "never shrinks" contract
/// `FlushReport` documents, and precisely the kind of plausible-looking number
/// nobody re-derives.
#[tokio::test]
async fn flushing_an_empty_tail_reports_honestly_and_changes_nothing() {
    let mut graph = kvs_writer::read_graph(&fixture()).await.expect("reads");
    assert_eq!(
        graph.tail().expect("tail").datom_count(),
        0,
        "the fixture starts with an empty tail"
    );
    let before = graph.clone();

    let report = kvs_writer::flush_tail(&mut graph).expect("flush");
    assert_eq!(report.transactions, 0);
    assert_eq!(report.rows_added, 0);
    assert_eq!(report.rows_modified, 0);
    assert_eq!(
        report.orphans_after, report.orphans_before,
        "an empty flush cannot change the orphan census"
    );
    assert_eq!(
        report.orphans_before, 17,
        "and the census must be the REAL one, not a default"
    );
    assert_eq!(report.max_tx, before.root.max_tx);
    assert_eq!(graph.rows, before.rows, "no row may be touched");
    assert_eq!(graph.root, before.root, "addr 0 may not be rewritten");

    // A second flush after a real one is the same no-op.
    let (mut edited, _) = graph_with_a_full_tail().await;
    kvs_writer::flush_tail(&mut edited).expect("first flush");
    let settled = edited.clone();
    let again = kvs_writer::flush_tail(&mut edited).expect("second flush");
    assert_eq!(again.rows_added, 0);
    assert_eq!(again.rows_modified, 0);
    assert_eq!(again.orphans_after, again.orphans_before);
    assert_eq!(
        edited.rows, settled.rows,
        "flushing an already-flushed graph must change 0 rows"
    );
}
