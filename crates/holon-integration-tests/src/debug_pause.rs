//! Env-var-driven pause hooks for live inspection of running PBT tests.
//!
//! When a PBT trips an invariant, the test process panics and the
//! embedded `holon` MCP server tears down with it. By that point the
//! live DB and CDC state that produced the failure are gone. These
//! hooks let the caller hold the process open at chosen moments so an
//! external tool (the `holon` MCP server, a debugger, or a sqlite
//! client) can attach and inspect.
//!
//! Three knobs:
//!
//! - `PBT_PAUSE_SECONDS=<n>` — master switch. When set:
//!   * installs a global panic hook that sleeps `n` seconds before any
//!     panic propagates (so the MCP server stays alive)
//!   * forces the embedded MCP server to start on
//!     [`MCP_PAUSE_PORT`] (8528) regardless of `PBT_MCP_PORT`
//!   When unset, both are no-ops.
//! - `PBT_PAUSE_BEFORE_STEP=N` — sleep before applying transition N
//!   (1-based, matches the `[pbt_step] Step N/M` log line).
//! - `PBT_PAUSE_AFTER_STEP=N` — sleep after the transition's
//!   invariants are checked.
//!
//! Send SIGINT to abort the sleep early.

use std::sync::Once;
use std::time::Duration;

/// Port the embedded MCP server is forced to bind when
/// `PBT_PAUSE_SECONDS` is set. Distinct from the external MCP proxy
/// (which would conflict on its own listening port).
pub const MCP_PAUSE_PORT: u16 = 8528;

static PANIC_HOOK_INSTALLED: Once = Once::new();

/// True iff `PBT_PAUSE_SECONDS` is set in the environment. Treated as
/// the master switch for all panic/MCP pause behavior.
pub fn pause_enabled() -> bool {
    std::env::var_os("PBT_PAUSE_SECONDS").is_some()
}

/// Pause duration. Returns `None` when the master switch is off.
pub fn pause_seconds() -> Option<u64> {
    let raw = std::env::var("PBT_PAUSE_SECONDS").ok()?;
    Some(
        raw.parse::<u64>()
            .expect("PBT_PAUSE_SECONDS must be a non-negative integer (seconds)"),
    )
}

fn sleep_with_banner(header: &str, body: &str) {
    let Some(secs) = pause_seconds() else {
        return;
    };
    let pid = std::process::id();
    eprintln!(
        "\n═══════════════════════════════════════════════════════════════════\n\
         [{header}] {body}\n\
         PID: {pid}    Sleeping: {secs}s    SIGINT aborts.\n\
         Connect via the holon MCP server (port {MCP_PAUSE_PORT}), \
         attach a debugger, or open the test sqlite DB to inspect live state.\n\
         ═══════════════════════════════════════════════════════════════════\n"
    );
    std::thread::sleep(Duration::from_secs(secs));
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

/// Install a global panic hook that sleeps when `PBT_PAUSE_SECONDS` is
/// set, so the embedded MCP server (and any other in-process
/// inspectors) stay alive long enough for an external agent to attach.
/// The previous hook is invoked after the sleep so chrome-trace
/// flushing, default backtraces, etc. still run.
///
/// Idempotent — safe to call from every PBT entrypoint.
pub fn install_panic_pause_hook() {
    if !pause_enabled() {
        return;
    }
    PANIC_HOOK_INSTALLED.call_once(|| {
        let prev_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let reason = info
                .payload()
                .downcast_ref::<&str>()
                .copied()
                .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
                .unwrap_or("<non-string panic payload>");
            let location = info
                .location()
                .map(|l| format!("{}:{}", l.file(), l.line()))
                .unwrap_or_else(|| "<unknown location>".to_string());
            sleep_with_banner(
                "PBT_PAUSE_SECONDS (panic hook)",
                &format!("panic at {location}: {reason}"),
            );
            prev_hook(info);
        }));
    });
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
