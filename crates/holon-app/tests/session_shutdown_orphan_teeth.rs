//! Teeth for `session_shutdown_stops_watchers`: closing the storage actor
//! WITHOUT stopping the session's tasks first must be loud.
//!
//! Its sibling asserts that an orderly shutdown leaves the log silent. That
//! assertion is only worth something if the same boot, shut down in the WRONG
//! order, is noisy — otherwise a session with no live watchers would pass it
//! vacuously. This test pins the wrong order and stays green forever: nothing
//! here calls `SessionShutdown::shutdown`, so the orphans are produced whatever
//! the production wiring does.
//!
//! @pbt kind harness
//! @pbt covers session-shutdown-teeth — closing the actor with the session's
//! tasks still running produces orphan reads, so the orderly-shutdown assertion
//! is not vacuous

#[path = "session_shutdown/harness.rs"]
mod harness;

#[tokio::test(flavor = "multi_thread")]
async fn closing_the_store_under_a_live_session_orphans_its_watchers() {
    let errors = harness::capture_errors();
    let booted = harness::boot_a_working_session().await;

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let _boot_noise = errors.take();

    // The wrong order, deliberately: the actor goes away under a session whose
    // watchers are still reading it.
    booted
        .engine
        .db_handle()
        .shutdown()
        .await
        .expect("the Turso actor must accept a shutdown command");

    // Deterministic provocation: a surviving file-sync controller reads on the
    // next vault change, so the outcome no longer depends on what happened to
    // be in flight when the actor closed.
    booted.touch_vault();

    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

    let after = errors.take();
    assert!(
        !after.is_empty(),
        "closing the store under a live session produced no errors at all — this boot has no \
         live watchers, so its sibling's silence would prove nothing"
    );
    eprintln!(
        "[teeth] the orphans this pins:\n{}",
        harness::listed(&after)
    );
}
