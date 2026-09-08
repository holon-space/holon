//! Shutting a session down must stop what its boot spawned, BEFORE the storage
//! actor closes.
//!
//! The defect this pins (bugfunnel `2026-09-08-reboot-orphans-watcher-tasks`,
//! found by the keystone's `Reboot` transition): every restart path closed the
//! Turso actor while the previous boot's watchers were still running. Those
//! tasks own their own handles, so dropping the caller's `Arc` did not stop
//! them; they kept reading a dead actor until the org-writeback supervisor
//! spent its restart budget and escalated to permanently degraded —
//! `[supervisor:org-writeback] DIED 4 times within 60s — GIVING UP`,
//! `[UiWatcher] … Actor channel closed`, `[OrgMode] re_render_all_tracked …`,
//! `[holon_rule_watcher] … Actor channel closed`.
//!
//! The test boots the real shared wiring over a vault with content (so the org
//! watchers have work), drives the ONE shutdown seam, closes the actor, and
//! asserts the log that follows is silent. It is a boot-path test, not a
//! keystone transition, because the surface under test is the wiring's task
//! ownership — the keystone reaches it only through a whole reboot.
//!
//! @pbt kind harness
//! @pbt covers session-shutdown-stops-watchers — SessionShutdown joins every
//! session-scoped task before the storage actor closes, so no orphan reads a
//! dead actor
//! @pbt overlaps general_e2e_composed_pbt — the keystone's `Reboot` transition
//! covers the same defect end-to-end; kept because this one names the seam and
//! runs in seconds

#[path = "session_shutdown/harness.rs"]
mod harness;

#[tokio::test(flavor = "multi_thread")]
async fn shutting_the_session_down_stops_its_watchers_before_the_store_closes() {
    let errors = harness::capture_errors();
    let booted = harness::boot_a_working_session().await;

    // Let the boot's watchers arm and drain their initial work, so what follows
    // is a shutdown of a live session rather than of a half-built one.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let _boot_noise = errors.take();

    // Anti-vacuity: `shutdown` reporting no stragglers proves nothing about a
    // watcher that was never registered with it. Each name below is a family
    // that reads the store, so nothing else bounds its lifetime.
    //
    // Two registered families are absent, both unobservable by construction
    // rather than overlooked. `boot-post-ready` spawns only when
    // `wait_for_ready` is false, and this harness boots with the wait ON, so
    // that work is awaited inline. `loro-entity-refresh` belongs to the
    // no-Turso wiring, and this is a Turso session. Asserting either would need
    // a second boot whose only purpose is to satisfy the assertion.
    let registered = booted.shutdown.registered();
    for expected in [
        "supervisor:org-writeback",
        "org-writeback-consumer",
        "file-sync-controller",
        "ui-watchers",
        "loro-outbound-reconcile",
        "action-discovery",
        "holon-rule-discovery",
        "clock-scheduler",
        "advice-reconciler",
        "advice-drainer",
        "integration-reprojector",
    ] {
        assert!(
            registered.iter().any(|n| n == expected),
            "'{expected}' is not registered with the session shutdown, so nothing stops it \
             before the store closes. Registered: {registered:?}"
        );
    }

    // Through the PRODUCTION entry, not the primitive: this is the function
    // every quit path calls, so a no-op version of it must red this test.
    holon_app::shutdown_session(&booted.injector)
        .await
        .expect("the production teardown must stop every task and close the store");

    // The OTHER half of the ordering, and the other half of the anti-vacuity
    // guard: a silent log proves nothing if the store is still open, because
    // then no surviving watcher could have failed. A teardown that skipped the
    // actor close passes every assertion below for free.
    let read_after = booted
        .engine
        .db_handle()
        .query("SELECT 1", std::collections::HashMap::new())
        .await;
    assert!(
        read_after.is_err(),
        "the storage actor is still serving reads after the teardown returned Ok — the second \
         half of the shutdown ordering did not happen"
    );

    // Deterministic provocation: a surviving file-sync controller reads on the
    // next vault change, so the outcome no longer depends on what happened to
    // be in flight when the actor closed.
    booted.touch_vault();

    // An orphan is not instantaneous: it fails on its NEXT read, which a timer
    // or a stream it is still parked on drives.
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let after = errors.take();
    assert!(
        after.is_empty(),
        "tasks outlived the session shutdown and kept working against the closed store:\n{}",
        harness::listed(&after)
    );
}
