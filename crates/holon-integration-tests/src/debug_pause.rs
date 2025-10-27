//! Env-var-driven pause hooks for live inspection of running PBT tests.
//!
//! When a PBT (especially `gpui_ui_pbt`) trips an invariant, the test
//! process panics and the GPUI window — and with it the embedded
//! `holon` MCP server — tears down. By that point the live DB and CDC
//! state that produced the failure are gone. These hooks let the
//! caller hold the process open at chosen moments so an external tool
//! (the `holon` MCP server, a debugger, or a sqlite client) can attach
//! and inspect.
//!
//! Three knobs, each independent:
//!
//! - `PBT_PAUSE_ON_FAIL=1` — sleep right before a failing assertion
//!   panics. Wire-in points are the test code paths that own the
//!   panic; today: [`crate::test_environment::TestEnvironment::
//!   assert_cdc_quiescent`].
//! - `PBT_PAUSE_BEFORE_STEP=N` — sleep immediately before applying
//!   transition N (1-based, matches the `[pbt_step] Step N/M` log
//!   line). Useful with a debugger to set breakpoints just before
//!   the operation fires.
//! - `PBT_PAUSE_AFTER_STEP=N` — sleep immediately after the
//!   transition's invariants are checked.
//!
//! `PBT_PAUSE_SECONDS=<n>` (default 900s = 15 min) controls the
//! pause duration for all three. Send SIGINT to abort early.
//!
//! All three are no-ops when their env var is unset, so it's safe to
//! leave the hook calls in place permanently.

use std::time::Duration;

const DEFAULT_PAUSE_SECONDS: u64 = 900;

fn pause_seconds() -> u64 {
    std::env::var("PBT_PAUSE_SECONDS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_PAUSE_SECONDS)
}

fn sleep_with_banner(header: &str, body: &str) {
    let secs = pause_seconds();
    let pid = std::process::id();
    eprintln!(
        "\n═══════════════════════════════════════════════════════════════════\n\
         [{header}] {body}\n\
         PID: {pid}    Sleeping: {secs}s    SIGINT aborts.\n\
         Connect via the holon MCP server, attach a debugger, or open the\n\
         test sqlite DB to inspect live state.\n\
         Set PBT_PAUSE_SECONDS=<n> to override the pause duration.\n\
         ═══════════════════════════════════════════════════════════════════\n"
    );
    std::thread::sleep(Duration::from_secs(secs));
}

/// Pause if `PBT_PAUSE_ON_FAIL` is set. Call right before a failing
/// assertion's panic so the live process state remains inspectable.
///
/// `reason` is a short human description of which assertion is about
/// to fail — it goes into the banner so a passer-by sees what they're
/// inspecting.
pub fn pause_on_fail(reason: &str) {
    if std::env::var_os("PBT_PAUSE_ON_FAIL").is_none() {
        return;
    }
    sleep_with_banner("PBT_PAUSE_ON_FAIL", reason);
}

/// Pause before step `step_index` (1-based) if `PBT_PAUSE_BEFORE_STEP`
/// matches. `transition_name` is included in the banner.
pub fn pause_before_step(step_index: u32, transition_name: &str) {
    pause_at_step(
        "PBT_PAUSE_BEFORE_STEP",
        step_index,
        "before",
        transition_name,
    );
}

/// Pause after step `step_index` (1-based) if `PBT_PAUSE_AFTER_STEP`
/// matches.
pub fn pause_after_step(step_index: u32, transition_name: &str) {
    pause_at_step("PBT_PAUSE_AFTER_STEP", step_index, "after", transition_name);
}

fn pause_at_step(var: &str, step_index: u32, when: &str, transition_name: &str) {
    let Ok(target) = std::env::var(var) else {
        return;
    };
    let Ok(target_n) = target.parse::<u32>() else {
        return;
    };
    if target_n != step_index {
        return;
    }
    sleep_with_banner(
        &format!("{var}={target_n}"),
        &format!("paused {when} step {step_index}: {transition_name}"),
    );
}
