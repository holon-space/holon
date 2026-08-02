//! Substrate-corruption fault RED GUARDS — `CorruptTurso` / `CorruptLoro`
//! exercised through `TestEnvironment` at both timings (`MidRun` /
//! `PreRestart`), documenting the failure modes they expose TODAY, before the
//! BootLadder recovery increments exist.
//!
//! These live BESIDE the one composed keystone (not woven into `E2ETransition`)
//! because the keystone boots pre-started and has no true storage reboot — see
//! `holon_integration_tests::faults` for the full rationale, and
//! `docs/Testing/CorruptionFailureModes-2026-07-18.md` for the observed→desired
//! ledger. Every guard is `#[ignore]`d with its OBSERVED mode so
//! `just pbt general` (the keystone) stays green; run one-flag with
//! `--ignored`. When a BootLadder increment lands, flip the guard's assertion
//! to the DESIRED outcome and un-ignore it.
//!
//! @pbt kind harness
//! @pbt covers substrate-corruption-recovery — Turso/Loro corruption × timing
//! @pbt overlaps general_e2e_composed_pbt — kept: no reboot transition in
//! keystone, corruption breaks steady invariants by construction

use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

use holon_integration_tests::TestEnvironment;
use holon_integration_tests::faults::CorruptionOutcome;
use holon_integration_tests::faults::CorruptionTiming;
use holon_integration_tests::faults::LoroCorruption;
use holon_integration_tests::faults::ScenarioReport;
use holon_integration_tests::faults::TursoCorruption;
use holon_integration_tests::faults::run_loro_scenario;
use holon_integration_tests::faults::run_turso_scenario;
use holon_integration_tests::test_tracing::SpanCollector;
use holon_integration_tests::test_tracing::attach_scope_to_runtime;
use holon_integration_tests::test_tracing::begin_test_scope;

/// Install the process-global collector and open this case's observability
/// scope BEFORE the SUT runtime is built, so background-worker panics and
/// ERROR logs are attributed to this case — `faults::captured_problems`
/// panics on a thread that owns no scope.
fn runtime() -> Arc<tokio::runtime::Runtime> {
    SpanCollector::global();
    let scope = begin_test_scope();
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder.worker_threads(2).enable_all();
    attach_scope_to_runtime(&mut builder, scope);
    Arc::new(builder.build().expect("build runtime"))
}

/// Turn a caught panic into a verdict, using the injection flag as the phase
/// boundary. A panic BEFORE the damage landed means the scenario never tested
/// anything, so it is re-raised and fails the test — the `Panic` rung is only
/// available to panics the corruption itself provoked.
fn verdict_from_panic(
    e: Box<dyn std::any::Any + Send>,
    injected: &AtomicBool,
    layer: &str,
    shape: &str,
    timing: &str,
) -> ScenarioReport {
    if !injected.load(Ordering::SeqCst) {
        eprintln!(
            "[corruption] {layer}/{shape}/{timing}: panic BEFORE injection — this is a setup \
             failure, not a corruption outcome; re-raising."
        );
        std::panic::resume_unwind(e);
    }
    ScenarioReport {
        outcome: CorruptionOutcome::Panic,
        detail: format!(
            "{layer}/{shape}/{timing}: PANIC after injection: {}",
            panic_message(&e)
        ),
    }
}

/// Run one Turso scenario. A post-injection driver-thread panic is RECORDED as
/// `CorruptionOutcome::Panic`; a setup failure or setup panic fails the test.
fn turso(shape: TursoCorruption, timing: CorruptionTiming) -> ScenarioReport {
    let rt = runtime();
    let injected = AtomicBool::new(false);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.clone().block_on(async {
            let mut env = TestEnvironment::new(rt.clone()).expect("TestEnvironment::new");
            run_turso_scenario(&mut env, shape, timing, &injected).await
        })
    })) {
        Ok(Ok(report)) => report,
        Ok(Err(setup)) => panic!("turso/{}/{}: {setup}", shape.slug(), timing.slug()),
        Err(e) => verdict_from_panic(e, &injected, "turso", shape.slug(), timing.slug()),
    }
}

fn loro(shape: LoroCorruption, timing: CorruptionTiming) -> ScenarioReport {
    let rt = runtime();
    let injected = AtomicBool::new(false);
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rt.clone().block_on(async {
            let mut env = TestEnvironment::new(rt.clone()).expect("TestEnvironment::new");
            run_loro_scenario(&mut env, shape, timing, &injected).await
        })
    })) {
        Ok(Ok(report)) => report,
        Ok(Err(setup)) => panic!("loro/{}/{}: {setup}", shape.slug(), timing.slug()),
        Err(e) => verdict_from_panic(e, &injected, "loro", shape.slug(), timing.slug()),
    }
}

fn panic_message(e: &(dyn std::any::Any + Send)) -> String {
    e.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| e.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "<non-string panic>".to_string())
}

/// The one forbidden floor: the app must NEVER silently present a clean/empty
/// vault over a damaged store. A raw panic / typed error / disclosed problem
/// are all acceptable-today rungs; `SilentDataLoss` is the guard's real teeth.
fn assert_not_silent(report: &ScenarioReport) {
    eprintln!(
        "[corruption] {} => {}",
        report.outcome.label(),
        report.detail
    );
    if report.outcome == CorruptionOutcome::FaultRejected {
        eprintln!(
            "[corruption] DISCLOSURE: this guard proved nothing — the substrate refused the \
             injection, so no corruption was ever present. Update the ledger, do not read it as \
             recovery evidence."
        );
    }
    assert_ne!(
        report.outcome,
        CorruptionOutcome::SilentDataLoss,
        "SILENT WRONGNESS: corrupted store presented as clean/empty with no error and no \
         disclosure — {}",
        report.detail
    );
}

// ── Per-shape guards (one-flag runnable with --ignored) ────────────────────
// Each asserts the no-silent-wrongness floor and is GREEN today; the
// `#[ignore]` string carries the OBSERVED outcome. They stay `#[ignore]`d
// because each one boots and reboots a real app (seconds, real temp dirs) —
// tighten to the specific DESIRED outcome and un-ignore when the named
// BootLadder increment lands. Full ledger:
// docs/Testing/CorruptionFailureModes-2026-07-18.md

#[test]
#[ignore = "ledger #1 (BootLadder Inc 5 pending): DROP block_raw mid-run — observed TypedError \
            (post-read Err 'no such table: block_raw'). REAL TEETH."]
fn corrupt_turso_drop_block_raw_mid_run() {
    assert_not_silent(&turso(
        TursoCorruption::DropBlockRawTable,
        CorruptionTiming::MidRun,
    ));
}

#[test]
#[ignore = "ledger #2 VACUOUS: Turso hard-refuses 'Cannot drop system table \
            __turso_internal_dbsp_state_v1_*', so observed FaultRejected — the Android \
            stale-epoch class needs a file-level/stale-reopen injector"]
fn corrupt_turso_drop_dbsp_state_mid_run() {
    assert_not_silent(&turso(
        TursoCorruption::DropDbspStateTable,
        CorruptionTiming::MidRun,
    ));
}

#[test]
#[ignore = "ledger #3 (BootLadder rung 1a pending): truncated test.db on reboot — observed \
            TypedError (start_app Err 'short read on page 1'), so Inc 1 is already met"]
fn corrupt_turso_truncate_db_file_pre_restart() {
    assert_not_silent(&turso(
        TursoCorruption::TruncateDbFile,
        CorruptionTiming::PreRestart,
    ));
}

#[test]
#[ignore = "ledger #4 WEAK: invalid-magic holon_tree.loro on reboot — observed Survived; a real \
            ~11KB snapshot IS destroyed, but the oracle counts org-derived rows and prod \
            discloses at WARN, so this guard cannot reach the floor. See the ledger's \
            'Loro blind spot'."]
fn corrupt_loro_corrupt_snapshot_bytes_pre_restart() {
    assert_not_silent(&loro(
        LoroCorruption::CorruptSnapshotBytes,
        CorruptionTiming::PreRestart,
    ));
}

#[test]
#[ignore = "ledger #5 WEAK: truncated holon_tree.loro on reboot — observed Survived; a real \
            snapshot IS destroyed, but (a) the oracle counts block_raw/block, which every boot \
            rebuilds from org, so Loro damage cannot move the number, and (b) prod discloses at \
            WARN while the collector captures ERROR only. This guard cannot reach the floor."]
fn corrupt_loro_truncate_snapshot_pre_restart() {
    assert_not_silent(&loro(
        LoroCorruption::TruncateSnapshot,
        CorruptionTiming::PreRestart,
    ));
}

#[test]
#[ignore = "ledger #6 WEAK: missing holon_tree.loro on reboot — observed Survived; a real \
            snapshot IS deleted, but (a) the oracle counts block_raw/block, which every boot \
            rebuilds from org, so Loro damage cannot move the number, and (b) prod discloses at \
            WARN while the collector captures ERROR only. This guard cannot reach the floor."]
fn corrupt_loro_delete_snapshot_pre_restart() {
    assert_not_silent(&loro(
        LoroCorruption::DeleteSnapshot,
        CorruptionTiming::PreRestart,
    ));
}

#[test]
#[ignore = "ledger #7 WEAK: holon_tree.loro damaged mid-run — observed Survived; latent by \
            design (the live in-memory doc ignores its snapshot, reload leg is guard #4), AND \
            (a) the oracle counts block_raw/block, which every boot rebuilds from org, so Loro \
            damage cannot move the number, and (b) prod discloses at WARN while the collector \
            captures ERROR only. This guard cannot reach the floor."]
fn corrupt_loro_corrupt_snapshot_bytes_mid_run() {
    assert_not_silent(&loro(
        LoroCorruption::CorruptSnapshotBytes,
        CorruptionTiming::MidRun,
    ));
}

// ── Exploratory sweep: capture every shape × timing in one run ──────────────
// Not a guard — prints the full ledger for the docs table. Ignored like the
// rest so it never runs by default.
#[test]
#[ignore = "explorer: prints the full corruption ledger; run with --ignored to refresh the docs \
            table"]
fn corruption_ledger_sweep() {
    let mut lines = Vec::new();
    for shape in TursoCorruption::ALL {
        for timing in [CorruptionTiming::MidRun, CorruptionTiming::PreRestart] {
            let r = turso(shape, timing);
            lines.push(format!("{} | {}", r.outcome.label(), r.detail));
        }
    }
    for shape in LoroCorruption::ALL {
        for timing in [CorruptionTiming::MidRun, CorruptionTiming::PreRestart] {
            let r = loro(shape, timing);
            lines.push(format!("{} | {}", r.outcome.label(), r.detail));
        }
    }
    eprintln!("\n===== CORRUPTION LEDGER =====");
    for l in &lines {
        eprintln!("{l}");
    }
    eprintln!("===== END LEDGER =====\n");
}
