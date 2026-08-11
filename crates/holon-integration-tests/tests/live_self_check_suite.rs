//! The live self-check suite over hand-built snapshots: it must CONVICT a
//! seeded class-1 violation, and it must DISCLOSE — never omit — every
//! invariant it could not run.
//!
//! @pbt kind infra
//! @pbt covers live-self-check — the class-1 suite answering `run_self_checks`

use holon_api::Block;
use holon_api::EntityUri;
use holon_integration_tests::pbt::live_self_check::LiveSelfCheck;
use holon_mcp::self_check::CheckOutcome;
use holon_mcp::self_check::LiveSelfCheckSuite;
use holon_mcp::self_check::LiveSnapshot;
use holon_mcp::self_check::SelfCheckReport;

fn uri(s: &str) -> EntityUri {
    EntityUri::parse(s).expect("valid test EntityUri")
}

fn run(snapshot: LiveSnapshot) -> SelfCheckReport {
    LiveSelfCheck.run(snapshot).expect("suite runs")
}

fn outcome_of<'a>(report: &'a SelfCheckReport, id: &str) -> &'a CheckOutcome {
    &report
        .checks
        .iter()
        .find(|c| c.id == id)
        .unwrap_or_else(|| {
            panic!(
                "{id} must appear in the report; present ids: {:?}",
                report.checks.iter().map(|c| &c.id).collect::<Vec<_>>()
            )
        })
        .outcome
}

/// A block whose parent is not in the snapshot — a dangling parent pointer in
/// the live CDC mirror, exactly what `inv-no-orphan-blocks` reads through
/// `live_block_snapshot`.
fn orphaned_snapshot() -> LiveSnapshot {
    LiveSnapshot {
        live_blocks: vec![
            Block::new_text(uri("local://root"), EntityUri::no_parent(), "root"),
            Block::new_text(uri("local://orphan"), uri("local://vanished"), "orphan"),
        ],
        focus_roots: Vec::new(),
    }
}

/// The convicting case: the suite must FAIL `inv-no-orphan-blocks` and name the
/// offending block. Seeded through `live_block_snapshot` because that is the
/// only block surface the running app can answer truthfully — the write-side
/// `block_raw_snapshot` has no live source, so the mark-bounds class can only
/// ever Skip here.
#[test]
fn seeded_orphan_is_convicted() {
    let report = run(orphaned_snapshot());

    match outcome_of(&report, "inv-no-orphan-blocks") {
        CheckOutcome::Fail { detail } => {
            assert!(
                detail.contains("local://orphan") && detail.contains("local://vanished"),
                "the failure detail must name the orphan and its missing parent; got: {detail}"
            );
        }
        other => panic!("inv-no-orphan-blocks must FAIL on a dangling parent; got {other:?}"),
    }
    assert!(report.failed >= 1, "report: {report:?}");
    assert_eq!(report.live_block_count, 2);
}

/// A clean snapshot must not manufacture the same conviction — the red above
/// comes from the seeded orphan, not from the suite failing everything.
#[test]
fn clean_snapshot_passes_the_orphan_check() {
    let report = run(LiveSnapshot {
        live_blocks: vec![Block::new_text(
            uri("local://root"),
            EntityUri::no_parent(),
            "root",
        )],
        focus_roots: Vec::new(),
    });
    assert!(
        matches!(
            outcome_of(&report, "inv-no-orphan-blocks"),
            CheckOutcome::Pass
        ),
        "report: {report:?}"
    );
}

/// Skipped is LOUD: an invariant with no live source is present in the report
/// with a reason that names what is missing.
#[test]
fn unavailable_checks_are_present_and_reasoned() {
    let report = run(orphaned_snapshot());

    // Needs `SutOrgRender`, which the live snapshot does not host.
    match outcome_of(&report, "inv-org-render-fixed-point") {
        CheckOutcome::Skipped { reason } => assert!(
            reason.contains("SutOrgRender"),
            "the skip reason must name the missing cap; got: {reason}"
        ),
        other => panic!("inv-org-render-fixed-point must be Skipped; got {other:?}"),
    }

    // The write-side surface is refused, not faked — the refusal reaches the
    // report as its reason.
    match outcome_of(&report, "inv-mark-bounds-within-content") {
        CheckOutcome::Skipped { reason } => assert!(
            reason.contains("block_raw_snapshot"),
            "the skip reason must name the refused live source; got: {reason}"
        ),
        other => panic!("inv-mark-bounds-within-content must be Skipped; got {other:?}"),
    }

    for id in [
        "inv-sql-budget",
        "inv-settle-budget",
        "inv-complexity-class-trend",
        "inv-matview-consistent-with-recompute",
        "inv-no-steady-reseed-leak",
    ] {
        match outcome_of(&report, id) {
            CheckOutcome::Skipped { reason } => {
                assert!(!reason.is_empty(), "{id} skipped with an empty reason")
            }
            other => panic!("{id} is class-3 and must be Skipped; got {other:?}"),
        }
    }
}

#[test]
fn counts_agree_with_the_rows() {
    let report = run(orphaned_snapshot());
    println!(
        "live self-check: {} total = {} pass / {} fail / {} skip",
        report.total, report.passed, report.failed, report.skipped
    );
    for c in &report.checks {
        println!("  {} => {:?}", c.id, c.outcome);
    }
    assert_eq!(report.total, report.checks.len());
    assert_eq!(
        report.passed + report.failed + report.skipped,
        report.total,
        "report: {report:?}"
    );
    assert!(report.total > 0, "the suite must consider some invariants");
}
