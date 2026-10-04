//! `inv-no-database-stuck` — every report of a SQL command that keeps the actor
//! busy past the hang bound is written to stderr, in every session a test
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
//! to file descriptor 2, which libtest's capture does not hold. Under
//! `cargo test` the line appears at once; under nextest it is in the test's
//! captured output, which nextest prints for a test that fails or times out.

use std::collections::HashMap;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionChange;
use holon_api::ConditionKey;
use holon_api::ConditionKind;
use tokio::sync::broadcast::error::RecvError;

const ID: &str = "inv-no-database-stuck";

/// The longest per-test cap in `.config/nextest.toml`, which also exceeds the
/// keystone's wedge bound. A command that has run this long is in a run no
/// harness timeout ends, so the guard ends that run.
const LONGEST_HARNESS_CAP_SECS: u64 = 49 * 60;

/// A guarded bus. A `Weak` keeps the bus's allocation, so no later bus shares
/// the address of one in the list; `alive` falls to false when the guard thread
/// ends.
struct Guarded {
    bus: Weak<ConditionBus>,
    alive: Arc<AtomicBool>,
}

static GUARDED: Mutex<Vec<Guarded>> = Mutex::new(Vec::new());

struct MarkDead(Arc<AtomicBool>);

impl Drop for MarkDead {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Writes every `DatabaseStuck` report `bus` raises to stderr. Ends the process
/// with exit code 101 on `DatabaseWatchFailed`, and when one command has run
/// for [`LONGEST_HARNESS_CAP_SECS`].
pub fn report_database_stuck(bus: &Arc<ConditionBus>) {
    arm(bus, std::io::stderr);
}

fn arm<W: Write + 'static>(bus: &Arc<ConditionBus>, sink: fn() -> W) {
    let alive = Arc::new(AtomicBool::new(true));
    {
        let mut guarded = GUARDED.lock().expect("guarded buses poisoned");
        guarded.retain(|g| g.bus.strong_count() > 0);
        guarded.push(Guarded {
            bus: Arc::downgrade(bus),
            alive: alive.clone(),
        });
    }
    let mut changes = bus.subscribe().changes;
    let bus = Arc::downgrade(bus);
    std::thread::Builder::new()
        .name("database-stuck-guard".into())
        .spawn(move || {
            let _mark_dead = MarkDead(alive);
            let mut ends: HashMap<String, Arc<AtomicBool>> = HashMap::new();
            loop {
                match changes.blocking_recv() {
                    Ok(ConditionChange::Raised(c)) => act_on(&c, &mut ends, sink),
                    Ok(ConditionChange::Cleared(key)) => cancel_end(&key, &mut ends),
                    Err(RecvError::Lagged(_)) => {
                        if let Some(bus) = bus.upgrade() {
                            for c in bus.current() {
                                act_on(&c, &mut ends, sink);
                            }
                        }
                    }
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

/// Panics unless a live guard watches the container's bus.
pub fn assert_guarded_in(injector: &fluxdi::Injector) {
    assert!(
        is_guarded(&bus_of(injector)),
        "[{ID}] no live guard watches the session's ConditionBus: a stuck SQL command would hang \
         the run with no output"
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
        .any(|g| g.bus.ptr_eq(&bus) && g.alive.load(Ordering::SeqCst))
}

/// Reports the condition. A stuck command gets one end-of-run deadline, set
/// from the first report that names it; `ends` holds the live ones by subject.
fn act_on<W: Write + 'static>(
    c: &Condition,
    ends: &mut HashMap<String, Arc<AtomicBool>>,
    sink: fn() -> W,
) {
    let Some(verdict) = verdict(&c.reason) else {
        return;
    };
    writeln!(sink(), "{}", verdict.message)
        .expect("write the database-stuck guard's report to stderr");
    if matches!(c.reason, ConditionKind::DatabaseStuck { .. }) && ends.contains_key(&c.subject) {
        return;
    }
    if verdict.ends_after_secs == 0 {
        end_the_run(&verdict.end_message, sink);
    }
    let still_stuck = Arc::new(AtomicBool::new(true));
    ends.insert(c.subject.clone(), still_stuck.clone());
    let wait = Duration::from_secs(verdict.ends_after_secs);
    let end_message = verdict.end_message;
    std::thread::Builder::new()
        .name("database-stuck-deadline".into())
        .spawn(move || {
            std::thread::sleep(wait);
            if still_stuck.load(Ordering::SeqCst) {
                end_the_run(&end_message, sink);
            }
        })
        .expect("spawn the database-stuck deadline thread");
}

fn cancel_end(key: &ConditionKey, ends: &mut HashMap<String, Arc<AtomicBool>>) {
    if key.kind == ConditionKind::DATABASE_STUCK {
        if let Some(still_stuck) = ends.remove(&key.subject) {
            still_stuck.store(false, Ordering::SeqCst);
        }
    }
}

/// The run ends whether or not the line gets out: a closed stderr must not
/// leave a hung run alive.
fn end_the_run<W: Write>(message: &str, sink: fn() -> W) -> ! {
    let _ = writeln!(sink(), "[{ID}] {message}");
    std::process::exit(101);
}

#[derive(Debug)]
struct Verdict {
    message: String,
    end_message: String,
    /// Seconds from now until the run ends; 0 ends it now.
    ends_after_secs: u64,
}

fn verdict(reason: &ConditionKind) -> Option<Verdict> {
    match reason {
        ConditionKind::DatabaseStuck {
            running_secs,
            report,
            ..
        } => Some(Verdict {
            message: format!("[{ID}] {report}"),
            end_message: format!(
                "No harness timeout ended the run within {LONGEST_HARNESS_CAP_SECS} s, so the \
                 guard ends it."
            ),
            ends_after_secs: LONGEST_HARNESS_CAP_SECS.saturating_sub(*running_secs),
        }),
        ConditionKind::DatabaseWatchFailed { cause } => Some(Verdict {
            message: format!("[{ID}] the SQL actor watch failed: {cause}"),
            end_message: "The watch can no longer report a stuck command, so the guard ends the \
                          run."
                .into(),
            ends_after_secs: 0,
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
    fn a_slow_command_inside_every_harness_cap_is_reported_and_ends_the_run_at_the_cap() {
        for running_secs in [30, 81, LONGEST_HARNESS_CAP_SECS - 1] {
            let verdict = verdict(&stuck_for(running_secs)).expect("a stuck command is reported");
            assert!(
                verdict.ends_after_secs == LONGEST_HARNESS_CAP_SECS - running_secs
                    && verdict.message.contains("a `Transaction` command"),
                "{running_secs} s: {verdict:#?}"
            );
        }
    }

    #[test]
    fn a_command_at_the_harness_cap_ends_the_run_with_its_report() {
        let verdict =
            verdict(&stuck_for(LONGEST_HARNESS_CAP_SECS)).expect("a stuck command is reported");
        assert!(
            verdict.ends_after_secs == 0 && verdict.message.contains("a `Transaction` command"),
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
        let mut reported_at = 1;
        while reported_at < 2 * longest {
            let verdict = verdict(&stuck_for(reported_at)).expect("a stuck command is reported");
            let ends_at = reported_at + verdict.ends_after_secs;
            assert!(
                ends_at >= longest,
                "nextest caps a test at {longest} s, but a report at {reported_at} s ends the run \
                 at {ends_at} s: the guard would end a run nextest still allows"
            );
            reported_at += 1;
        }
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

    #[test]
    fn arming_a_bus_prunes_the_entry_of_a_dropped_bus() {
        let bus = Arc::new(ConditionBus::new());
        report_database_stuck(&bus);
        let dropped = Arc::downgrade(&bus);
        drop(bus);
        report_database_stuck(&Arc::new(ConditionBus::new()));
        assert!(
            !GUARDED
                .lock()
                .expect("guarded buses poisoned")
                .iter()
                .any(|g| g.bus.ptr_eq(&dropped)),
            "the entry of a dropped bus stays in the list"
        );
    }

    struct ClosedStderr;

    impl Write for ClosedStderr {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_guard_whose_report_cannot_be_written_stops_counting_as_guarding() {
        let bus = Arc::new(ConditionBus::new());
        arm(&bus, || ClosedStderr);
        assert!(is_guarded(&bus));
        bus.emit(Condition {
            subject: "database#1".into(),
            reason: stuck_for(30),
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while is_guarded(&bus) {
            assert!(
                std::time::Instant::now() < deadline,
                "a guard thread that died still counts as guarding the bus"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
