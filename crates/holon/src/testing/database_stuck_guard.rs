//! `inv-no-database-stuck` — every report of a SQL command that keeps the actor
//! busy past the hang bound reaches the test output, in every session a test
//! harness boots, and the watch that reports it keeps running.
//!
//! @pbt oracle internal-consistency — the session's `ConditionBus` never
//!   carries `DatabaseWatchFailed`, and no `DatabaseStuck` outlasts every
//!   harness timeout (no ref)
//! @pbt covers stuck SQL actor — a command that spins or blocks inside the
//!   engine, which freezes every later read and write of the session, and a
//!   failed watch that can no longer report one
//! @pbt slips-if-removed a transition whose write spins the IVM commit hangs
//!   the run until a harness timeout, with no line that names the command
//!
//! A stuck command never returns, so the step that sent it never reaches an
//! invariant check. The guard is a thread of its own that writes each report
//! to stderr, past the test harness's output capture.

use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;

use holon_api::ConditionBus;
use holon_api::ConditionChange;
use holon_api::ConditionKind;
use tokio::sync::broadcast::error::RecvError;

const ID: &str = "inv-no-database-stuck";

/// The longest per-test cap in `.config/nextest.toml`, which also exceeds the
/// keystone's wedge bound. A command that runs past it is in a run no harness
/// timeout ends, so the guard ends that run.
const LONGEST_HARNESS_CAP_SECS: u64 = 49 * 60;

/// A `Weak` keeps its bus's allocation, so no later bus shares the address of
/// one in the list.
static GUARDED: Mutex<Vec<Weak<ConditionBus>>> = Mutex::new(Vec::new());

/// Writes every `DatabaseStuck` report `bus` raises to stderr. Ends the process
/// with exit code 101 on `DatabaseWatchFailed`, and on a stuck command that has
/// run past every harness cap.
pub fn report_database_stuck(bus: &Arc<ConditionBus>) {
    {
        let mut guarded = GUARDED.lock().expect("guarded buses poisoned");
        guarded.retain(|g| g.strong_count() > 0);
        guarded.push(Arc::downgrade(bus));
    }
    let mut changes = bus.subscribe().changes;
    let bus = Arc::downgrade(bus);
    std::thread::Builder::new()
        .name("database-stuck-guard".into())
        .spawn(move || {
            loop {
                match changes.blocking_recv() {
                    Ok(ConditionChange::Raised(c)) => act_on(&c.reason),
                    Ok(ConditionChange::Cleared(_)) => {}
                    Err(RecvError::Lagged(_)) => check_current(&bus),
                    Err(RecvError::Closed) => return,
                }
            }
        })
        .expect("spawn the database-stuck guard thread");
}

/// Guards the container's bus. Called from the first DI closure, so the boot's
/// own commands are guarded too.
pub fn report_database_stuck_in(injector: &fluxdi::Injector) {
    report_database_stuck(&bus_of(injector));
}

/// Panics unless a guard watches the container's bus.
pub fn assert_guarded_in(injector: &fluxdi::Injector) {
    assert!(
        is_guarded(&bus_of(injector)),
        "[{ID}] no guard watches the session's ConditionBus: a stuck SQL command would hang the \
         run with no output"
    );
}

fn bus_of(injector: &fluxdi::Injector) -> Arc<ConditionBus> {
    (*injector
        .try_resolve::<Arc<ConditionBus>>()
        .expect("every container provides a ConditionBus"))
    .clone()
}

fn is_guarded(bus: &Arc<ConditionBus>) -> bool {
    let bus = Arc::downgrade(bus);
    GUARDED
        .lock()
        .expect("guarded buses poisoned")
        .iter()
        .any(|g| g.ptr_eq(&bus))
}

fn check_current(bus: &Weak<ConditionBus>) {
    if let Some(bus) = bus.upgrade() {
        for c in bus.current() {
            act_on(&c.reason);
        }
    }
}

fn act_on(reason: &ConditionKind) {
    let Some(verdict) = verdict(reason) else {
        return;
    };
    writeln!(std::io::stderr().lock(), "{}", verdict.message)
        .expect("write the database-stuck guard's report to stderr");
    if verdict.end_the_run {
        std::process::exit(101);
    }
}

#[derive(Debug)]
struct Verdict {
    message: String,
    end_the_run: bool,
}

fn verdict(reason: &ConditionKind) -> Option<Verdict> {
    match reason {
        ConditionKind::DatabaseStuck {
            running_secs,
            report,
            ..
        } => {
            let end_the_run = *running_secs > LONGEST_HARNESS_CAP_SECS;
            let mut message = format!("[{ID}] {report}");
            if end_the_run {
                message.push_str(&format!(
                    "No harness timeout ended the run within {LONGEST_HARNESS_CAP_SECS} s, so the \
                     guard ends it."
                ));
            }
            Some(Verdict {
                message,
                end_the_run,
            })
        }
        ConditionKind::DatabaseWatchFailed { cause } => Some(Verdict {
            message: format!("[{ID}] the SQL actor watch failed: {cause}"),
            end_the_run: true,
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stuck_for(running_secs: u64) -> ConditionKind {
        ConditionKind::DatabaseStuck {
            command: "Transaction".into(),
            running_secs,
            report: "SQL actor stuck: a `Transaction` command has run for a while.\n".into(),
        }
    }

    #[test]
    fn a_slow_command_inside_every_harness_cap_is_reported_and_the_run_goes_on() {
        for running_secs in [30, 81, LONGEST_HARNESS_CAP_SECS] {
            let verdict = verdict(&stuck_for(running_secs)).expect("a stuck command is reported");
            assert!(
                !verdict.end_the_run && verdict.message.contains("a `Transaction` command"),
                "{running_secs} s: {verdict:#?}"
            );
        }
    }

    #[test]
    fn a_command_past_every_harness_cap_ends_the_run_with_its_report() {
        let verdict =
            verdict(&stuck_for(LONGEST_HARNESS_CAP_SECS + 1)).expect("a stuck command is reported");
        assert!(
            verdict.end_the_run && verdict.message.contains("a `Transaction` command"),
            "{verdict:#?}"
        );
    }

    #[test]
    fn the_guard_outlasts_every_nextest_cap() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../.config/nextest.toml");
        let config = std::fs::read_to_string(path).expect("read .config/nextest.toml");
        let caps: Vec<u64> = config
            .lines()
            .filter(|l| l.trim_start().starts_with("slow-timeout"))
            .map(|l| {
                let number_after = |key: &str| -> u64 {
                    let start = l.find(key).unwrap_or_else(|| panic!("no `{key}` in {l}"));
                    let digits: String = l[start + key.len()..]
                        .chars()
                        .skip_while(|c| !c.is_ascii_digit())
                        .take_while(char::is_ascii_digit)
                        .collect();
                    digits
                        .parse()
                        .unwrap_or_else(|e| panic!("no number after `{key}` in {l}: {e}"))
                };
                assert!(
                    l.contains("s\""),
                    "a slow-timeout period not in seconds: {l}"
                );
                number_after("period") * number_after("terminate-after")
            })
            .collect();
        assert!(!caps.is_empty(), "no slow-timeout in {path}");
        let longest = *caps.iter().max().expect("checked non-empty");
        assert!(
            LONGEST_HARNESS_CAP_SECS >= longest,
            "nextest caps a test at {longest} s, past the guard's {LONGEST_HARNESS_CAP_SECS} s: the \
             guard would end a run nextest still allows"
        );
    }

    #[test]
    fn a_dropped_bus_never_lends_its_guard_to_a_new_bus() {
        for _ in 0..64 {
            let guarded = Arc::new(ConditionBus::new());
            report_database_stuck(&guarded);
            drop(guarded);
            let fresh: Vec<Arc<ConditionBus>> =
                (0..16).map(|_| Arc::new(ConditionBus::new())).collect();
            assert!(
                fresh.iter().all(|bus| !is_guarded(bus)),
                "a new bus counts as guarded by the guard of a dropped one"
            );
        }
    }
}
