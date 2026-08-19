//! The Loro→SQL projector run loop must not reconcile while the org initial
//! scan owns the write path.
//!
//! The scan already batches its own sink writes: one `DownstreamProjection`
//! flush per file. A run loop running CONCURRENTLY with the scan defeats that
//! — it wakes once per Loro commit, so the vault is projected one block at a
//! time, each pass paying a full sibling-scope snapshot and its own SQL
//! transaction. Measured on Martin's vault (dogfood 2026-07-28, BugFunnel ENV
//! row): 16,333 projection passes for 25,139 ops, 87.2 % of them single-op,
//! only 37 of them inside an `org.ingest_file` span — a 1,472 s readiness gap.
//!
//! The boot sequence is already designed to prevent this: `holon-app`'s
//! `wiring.rs` resolves `LoroSyncControllerHandle` inside `post_ready`, behind
//! the org ready signal. What defeats it is that resolving the handle from
//! ANYWHERE starts the loop, and the GPUI frontend resolves it eagerly at boot
//! for its MCP debug-handles cell. So the gate belongs on the controller, not
//! on each caller's discipline: the handle resolves immediately (nothing
//! blocks) and its loop waits on `SyncGate`, which `post_ready` opens on every
//! scan-completion path.
//!
//! @pbt kind harness
//! @pbt covers boot-projector-gating — the projector run loop starts only
//! after the org initial scan (BugFunnel ENV 2026-07-29)
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone boots two org
//! files, far too few for the scan to still be running when the handle is
//! resolved, so it structurally cannot observe boot ordering

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon_core::SyncGate;
use holon_core::SyncGateState;
use holon_integration_tests::TestEnvironmentBuilder;

/// Enough files that the scan is still running when we resolve the handle, and
/// no more — this runs in the default (debug) workspace profile, whose 2-minute
/// slow-timeout kills anything larger. The non-vacuity guard fails the test
/// rather than letting it pass if the scan ever outruns the probe.
const FILES: usize = 60;
const BLOCKS_PER_FILE: usize = 8;

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime"),
    )
}

fn org_file(i: usize, blocks: usize) -> String {
    let mut s = format!("#+ID: file-{i}\n#+TITLE: Page {i}\n\n");
    for j in 0..blocks {
        s.push_str(&format!(
            "* Block {i}-{j} with a [[link]] and *emphasis*\n:PROPERTIES:\n:ID: \
             p{i}_{j}\n:END:\nBody {i}-{j}.\n\n"
        ));
    }
    s
}

#[test]
fn projector_run_loop_waits_for_org_initial_scan() {
    let rt = runtime();
    let rt2 = rt.clone();
    rt.block_on(async move {
        let mut builder = TestEnvironmentBuilder::new()
            // Production's boot shape: the session factory returns while the
            // initial scan is still running (GPUI passes `wait_for_ready=false`).
            // With the default `true` the scan is already over by the time
            // anything can resolve the handle, which is exactly why no existing
            // fixture could see this ordering.
            .wait_for_file_watcher(false);
        for i in 0..FILES {
            builder = builder.with_org_file(format!("page-{i}.org"), org_file(i, BLOCKS_PER_FILE));
        }
        let env = builder.build(rt2).await.expect("boot test environment");
        let injector = env.injector().expect("injector on TestEnvironment");

        let gate = injector.resolve::<SyncGate>();
        // Resolve the handle the way the GPUI frontend does at boot — eagerly,
        // without waiting for readiness.
        let handle = injector
            .try_resolve_async::<holon_loro::LoroSyncControllerHandle>()
            .await
            .expect("resolve LoroSyncControllerHandle");

        // While the scan still owns the write path, the loop must stay parked.
        let mut probed = 0usize;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && gate.state() == SyncGateState::DeferredUntilScan {
            assert!(
                !handle.run_loop_started(),
                "projector run loop started while the org initial scan was still \
                 in progress (SyncGate deferred) — it will reconcile once per \
                 Loro commit and serialize the whole scan"
            );
            probed += 1;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        // Margin, not a bare `> 0`: the corpus is sized for the debug workspace
        // profile, and a test that only ever caught the window once would rot
        // into a vacuous pass the next time someone trims it.
        eprintln!("[gated-window] probed {probed} time(s) while the scan held the gate");
        assert!(
            probed >= 3,
            "vacuous: only {probed} probe(s) landed while the org initial scan \
             held the gate — the window is too small to catch a regression; \
             raise FILES"
        );

        // …and it must actually start once the scan is done. A projector that
        // never starts is the inverse bug.
        gate.wait_open().await.expect("sync gate opened");
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline && !handle.run_loop_started() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            handle.run_loop_started(),
            "projector run loop never started after the org initial scan \
             completed — Loro changes would never reach the SQL sink"
        );
    });
}
