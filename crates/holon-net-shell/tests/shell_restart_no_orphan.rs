//! A host that dies at any step, SIGKILLed so it cleans up nothing, leaves no
//! group behind that a restart does not find, and a restart never signals a
//! group that no dead host started:
//! - the next `open` reports every live group a dead host started whose run had
//!   not ended, also one spawned just before the host died in its pid-file
//!   write, and also after earlier restarts that kept their leftovers;
//! - `open` signals no group, and `kill_group` signals no foreign group:
//!   neither one named by a line without the group's identity, nor the opening
//!   host's own group, nor a group that now holds the pgid of a dead group;
//! - `kill_group`, and a run that kills its group, return only once the group
//!   has no living member;
//! - a run whose group the pid file cannot record returns an error with its
//!   group dead, also when the shell forks at that moment;
//! - a run whose leader exits, or whose wait fails, returns only once no member
//!   of its group runs, and `in_flight` no longer lists it;
//! - a dropped run, also one a runtime shutdown drops, leaves no member of its
//!   group running a moment after the drop, and `in_flight` and `abandon` no
//!   longer know it;
//! - a second runner on the pid file a live runner holds, in this process or
//!   another and in any spelling of its path, is refused, names the pid file
//!   and the holder and changes nothing; pid files with other names in the same
//!   directory stay independent.

mod restart_support;

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use holon_net_shell::effect::Ended;
use holon_net_shell::effect::Runner;
use proptest::prelude::*;
use proptest::sample::Index;
use proptest::test_runner::Config;
use restart_support::ChildPlan;
use restart_support::Foreign;
use restart_support::HOST_DIR;
use restart_support::HOST_FLAVOR;
use restart_support::Host;
use restart_support::Leader;
use restart_support::bare_line;
use restart_support::child_plan;
use restart_support::forking_plan;
use restart_support::group_lives;
use restart_support::kill_token_groups;
use restart_support::leader_is_zombie;
use restart_support::line_names_start;
use restart_support::line_pgid;
use restart_support::living_members;
use restart_support::make_writable;
use restart_support::mkfifo;
use restart_support::open_fifo_reader;
use restart_support::pass_held_write;
use restart_support::release_fifo;
use restart_support::script;
use restart_support::wait_until;
use restart_support::with_pgid;
use serde_json::Value;
use serde_json::json;

const HOST_TEST: &str = "restart_host";
/// Attempt ids one runner gives are distinct; a restarted runner draws them
/// from the same small range, so a leftover and a new run often share one.
const ATTEMPTS: u64 = 6;
const ANSWER: Duration = Duration::from_secs(10);
/// A kill waits up to the grace period for a TERM-ignoring group.
const ENDED: Duration = Duration::from_secs(15);
/// How long the groups of a dead host get to settle before the restart reads
/// them.
const SETTLE: Duration = Duration::from_millis(150);
/// A drop SIGKILLs its run's group before it returns, without the kill
/// grace; the members get this long to die.
const DROPPED: Duration = Duration::from_secs(1);

/// The host process; a no-op unless a test started it.
#[test]
fn restart_host() {
    let Some(dir) = std::env::var_os(HOST_DIR) else {
        return;
    };
    let multi = std::env::var(HOST_FLAVOR).expect("the test names the flavor") == "multi";
    restart_support::host_main(PathBuf::from(dir), multi);
}

#[derive(Debug, Clone)]
enum KillPoint {
    /// Between two operations.
    Now,
    /// While a run that just started goes on for this many milliseconds.
    InRun(ChildPlan, u64),
    /// Once the run has spawned its group and its pid-file write is held.
    InRecord(ChildPlan),
    /// This many milliseconds after the test released a running run's leader,
    /// so its group may keep members whose leader exited before the host died.
    Released(Index, u64),
}

#[derive(Debug, Clone)]
enum Forge {
    /// A line without identity naming a foreign group; `leaderless`: one whose
    /// leader exited.
    Foreign { leaderless: bool },
    /// A line without identity naming the opening host's own group.
    Opener,
    /// The line of a dead host's group, after that group died and a foreign
    /// group took its pgid; `leaderless`: one whose leader exited.
    Reuse { index: Index, leaderless: bool },
    /// A line naming the group of a process of another user, with an
    /// identity or without one.
    Alien(Option<u64>),
    /// A dead host's spawn record, renamed to a host pid a live process with
    /// another start holds.
    SpawnHostReused(Index),
    /// A spawn record whose name the runner never writes.
    SpawnMalformed,
}

const MALFORMED_SPAWN: &str = "groups.pid.spawn-x";

#[derive(Debug, Clone)]
enum Fault {
    /// The pid directory is read-only.
    ReadOnly,
    /// The write is held while the shell forks; then its rename fails, as a
    /// directory took the pid file's path.
    Held(u64),
}

/// The moment a run's future is dropped.
#[derive(Debug, Clone)]
enum DropAt {
    /// While its child runs.
    Running(Index),
    /// A new run, this many milliseconds after it started.
    Starting(ChildPlan, u64),
    /// A new run whose pid-file write is held, passed this many milliseconds
    /// after the drop was sent.
    Recording(ChildPlan, u64),
    /// This many milliseconds after an abandon of it, while the kill may wait
    /// for a TERM-ignoring group.
    Ending(Index, u64),
    /// Just after the test released its child.
    Exiting(Index),
}

#[derive(Debug, Clone)]
enum Op {
    Run(Index, ChildPlan),
    Release(Index),
    Abandon(Index),
    AbandonUnknown(Index),
    Drop(DropAt),
    /// The host reaps the run's leader itself, so the runner's wait fails.
    Reap(Index),
    /// A second runner opens the pid file, in another process or this one.
    Second {
        process: bool,
    },
    InFlight,
    RecordFails(Index, ChildPlan, Fault),
    Restart {
        kill: KillPoint,
        multi: bool,
        /// The child of one of the dead host's runs ends before the open.
        release: Option<Index>,
        /// The child of one reported leftover ends after the open, before the
        /// kills.
        release_reported: Option<Index>,
        forge: Vec<Forge>,
        /// Per reported leftover, in order: kill it with `kill_group`, or keep
        /// it.
        kill_left: Vec<bool>,
    },
    /// A kept leftover's child ends on its own.
    ReleaseLeftover(Index),
    /// The host's runtime shuts down, `ms` after a run started, and a new
    /// runtime in the same process opens.
    Reboot {
        multi: bool,
        run: Option<(ChildPlan, u64)>,
    },
}

fn kill_point() -> BoxedStrategy<KillPoint> {
    prop_oneof![
        1 => Just(KillPoint::Now),
        2 => (child_plan(), 0u64..40).prop_map(|(p, ms)| KillPoint::InRun(p, ms)),
        2 => child_plan().prop_map(KillPoint::InRecord),
        2 => (any::<Index>(), 0u64..40).prop_map(|(i, ms)| KillPoint::Released(i, ms)),
    ]
    .boxed()
}

fn forge() -> impl Strategy<Value = Forge> {
    prop_oneof![
        2 => any::<bool>().prop_map(|leaderless| Forge::Foreign { leaderless }),
        2 => Just(Forge::Opener),
        4 => (any::<Index>(), any::<bool>())
            .prop_map(|(index, leaderless)| Forge::Reuse { index, leaderless }),
        2 => proptest::option::of(any::<u64>()).prop_map(Forge::Alien),
        3 => any::<Index>().prop_map(Forge::SpawnHostReused),
        1 => Just(Forge::SpawnMalformed),
    ]
}

fn drop_at() -> BoxedStrategy<DropAt> {
    let plan = || prop_oneof![child_plan(), stubborn_plan()];
    prop_oneof![
        any::<Index>().prop_map(DropAt::Running),
        (plan(), 0u64..40).prop_map(|(p, ms)| DropAt::Starting(p, ms)),
        (plan(), 0u64..100).prop_map(|(p, ms)| DropAt::Recording(p, ms)),
        (any::<Index>(), 0u64..300).prop_map(|(i, ms)| DropAt::Ending(i, ms)),
        any::<Index>().prop_map(DropAt::Exiting),
    ]
    .boxed()
}

fn op() -> BoxedStrategy<Op> {
    prop_oneof![
        4 => (any::<Index>(), child_plan()).prop_map(|(a, p)| Op::Run(a, p)),
        2 => any::<Index>().prop_map(Op::Release),
        2 => any::<Index>().prop_map(Op::Abandon),
        1 => any::<Index>().prop_map(Op::AbandonUnknown),
        2 => drop_at().prop_map(Op::Drop),
        1 => any::<Index>().prop_map(Op::Reap),
        1 => any::<bool>().prop_map(|process| Op::Second { process }),
        1 => Just(Op::InFlight),
        2 => (
            any::<Index>(),
            prop_oneof![child_plan(), forking_plan()],
            prop_oneof![Just(Fault::ReadOnly), (0u64..80).prop_map(Fault::Held)],
        )
            .prop_map(|(a, p, f)| Op::RecordFails(a, p, f)),
        3 => restart_with(
            kill_point(),
            proptest::option::of(any::<Index>()),
            prop::collection::vec(forge(), 0..=2),
            prop::collection::vec(any::<bool>(), 0..=4),
        ),
        1 => any::<Index>().prop_map(Op::ReleaseLeftover),
        2 => (any::<bool>(), proptest::option::of((child_plan(), 0u64..40)))
            .prop_map(|(multi, run)| Op::Reboot { multi, run }),
    ]
    .boxed()
}

#[derive(Debug, Clone)]
struct Scenario {
    multi: bool,
    ops: Vec<Op>,
}

/// Every operation, with the ones `emphasis` draws more often.
fn scenario(emphasis: BoxedStrategy<Op>) -> impl Strategy<Value = Scenario> {
    (
        any::<bool>(),
        prop::collection::vec(prop_oneof![2 => op(), 3 => emphasis], 1..=6),
    )
        .prop_map(|(multi, ops)| Scenario { multi, ops })
}

fn restart_with(
    kill: impl Strategy<Value = KillPoint>,
    release: impl Strategy<Value = Option<Index>>,
    forge: impl Strategy<Value = Vec<Forge>>,
    kill_left: impl Strategy<Value = Vec<bool>>,
) -> impl Strategy<Value = Op> {
    (
        kill,
        any::<bool>(),
        release,
        proptest::option::of(any::<Index>()),
        forge,
        kill_left,
    )
        .prop_map(
            |(kill, multi, release, release_reported, forge, kill_left)| Op::Restart {
                kill,
                multi,
                release,
                release_reported,
                forge,
                kill_left,
            },
        )
}

fn kill_all() -> impl Strategy<Value = Vec<bool>> {
    Just(vec![true; 16])
}

/// A host killed while a run spawns, records or runs.
fn dying_host() -> BoxedStrategy<Op> {
    restart_with(
        prop_oneof![
            (child_plan(), 0u64..40).prop_map(|(p, ms)| KillPoint::InRun(p, ms)),
            child_plan().prop_map(KillPoint::InRecord),
        ],
        proptest::option::of(any::<Index>()),
        Just(Vec::new()),
        prop::collection::vec(any::<bool>(), 0..=4),
    )
    .boxed()
}

/// A restart whose pid file names groups no dead host started.
fn forged_restart() -> BoxedStrategy<Op> {
    restart_with(
        kill_point(),
        Just(None),
        prop::collection::vec(forge(), 1..=3),
        kill_all(),
    )
    .boxed()
}

/// A child whose group a SIGTERM does not end at once.
fn stubborn_plan() -> BoxedStrategy<ChildPlan> {
    (
        child_plan(),
        prop_oneof![
            Just(restart_support::Member::TermIgnoring),
            any::<bool>().prop_map(|leaves| restart_support::Member::Unreaped { leaves }),
            (1u16..=40).prop_map(restart_support::Member::Forks),
        ],
        any::<bool>(),
    )
        .prop_map(|(plan, member, trap_term)| ChildPlan {
            member,
            trap_term,
            ..plan
        })
        .boxed()
}

/// Runs of stubborn groups, ended by an abandon, a timeout or `kill_group`
/// after a restart.
fn stubborn_kill() -> BoxedStrategy<Op> {
    prop_oneof![
        (any::<Index>(), stubborn_plan()).prop_map(|(a, p)| Op::Run(a, p)),
        any::<Index>().prop_map(Op::Abandon),
        restart_with(
            prop_oneof![
                Just(KillPoint::Now),
                (stubborn_plan(), 0u64..40).prop_map(|(p, ms)| KillPoint::InRun(p, ms)),
            ],
            Just(None),
            Just(Vec::new()),
            kill_all(),
        ),
    ]
    .boxed()
}

/// A child the test releases, with any members in its group.
fn releasable_plan() -> BoxedStrategy<ChildPlan> {
    prop_oneof![child_plan(), stubborn_plan()]
        .prop_map(|p| ChildPlan {
            leader: Leader::Reads,
            timeout_ms: None,
            ..p
        })
        .boxed()
}

/// Leaders that exit on their own while members of their group may run.
fn leader_exits() -> BoxedStrategy<Op> {
    prop_oneof![
        (any::<Index>(), releasable_plan()).prop_map(|(a, p)| Op::Run(a, p)),
        any::<Index>().prop_map(Op::Release),
    ]
    .boxed()
}

/// Groups whose leader is gone while members run: the leader exits before
/// its host dies or after, or a foreign group's leader exited.
fn leaderless_group() -> BoxedStrategy<Op> {
    prop_oneof![
        (any::<Index>(), releasable_plan()).prop_map(|(a, p)| Op::Run(a, p)),
        restart_with(
            prop_oneof![
                Just(KillPoint::Now),
                (any::<Index>(), 0u64..40).prop_map(|(i, ms)| KillPoint::Released(i, ms)),
            ],
            proptest::option::of(any::<Index>()),
            prop::collection::vec(
                prop_oneof![
                    any::<Index>().prop_map(|index| Forge::Reuse {
                        index,
                        leaderless: true
                    }),
                    Just(Forge::Foreign { leaderless: true }),
                ],
                0..=2,
            ),
            prop::collection::vec(any::<bool>(), 0..=4),
        ),
        any::<Index>().prop_map(Op::ReleaseLeftover),
    ]
    .boxed()
}

/// Runs whose future is dropped at any step, also by a runtime shutdown.
fn dropped_run() -> BoxedStrategy<Op> {
    prop_oneof![
        (any::<Index>(), prop_oneof![child_plan(), stubborn_plan()])
            .prop_map(|(a, p)| Op::Run(a, p)),
        drop_at().prop_map(Op::Drop),
        (
            any::<bool>(),
            proptest::option::of((stubborn_plan(), 0u64..40))
        )
            .prop_map(|(multi, run)| Op::Reboot { multi, run }),
    ]
    .boxed()
}

/// Runs whose leader the host reaps before the runner can.
fn reaped_run() -> BoxedStrategy<Op> {
    prop_oneof![
        (any::<Index>(), releasable_plan()).prop_map(|(a, p)| Op::Run(a, p)),
        any::<Index>().prop_map(Op::Reap),
    ]
    .boxed()
}

/// Second runners on a held pid file, and restarts once its holder died.
fn second_runner() -> BoxedStrategy<Op> {
    prop_oneof![
        (any::<Index>(), child_plan()).prop_map(|(a, p)| Op::Run(a, p)),
        any::<bool>().prop_map(|process| Op::Second { process }),
        restart_with(
            kill_point(),
            Just(None),
            Just(Vec::new()),
            prop::collection::vec(any::<bool>(), 0..=4),
        ),
    ]
    .boxed()
}

fn failing_record() -> BoxedStrategy<Op> {
    (
        any::<Index>(),
        prop_oneof![child_plan(), forking_plan(), stubborn_plan()],
        prop_oneof![Just(Fault::ReadOnly), (0u64..80).prop_map(Fault::Held)],
    )
        .prop_map(|(a, p, f)| Op::RecordFails(a, p, f))
        .boxed()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Its host lives and its run has not returned.
    Running,
    /// Its run returned.
    Ended,
    /// Its host died before its run returned.
    Orphaned,
    /// A restart, a kill of its run or its drop ended it, or the test ended
    /// its child.
    Gone,
}

#[derive(Debug, Clone)]
struct RunModel {
    id: u64,
    generation: u32,
    attempt: u64,
    plan: ChildPlan,
    state: State,
    pgid: Option<i32>,
}

/// A line the test wrote into the pid file whose group has no provable
/// identity: it names no start, or its group's leader is gone.
struct Bare {
    pgid: i32,
    attempt: u64,
    transition: String,
    subject: String,
}

impl Bare {
    fn run(pgid: i32, subject: &str) -> Self {
        Self {
            pgid,
            attempt: 1,
            transition: "Run".to_owned(),
            subject: subject.to_owned(),
        }
    }
}

struct Case {
    dir: tempfile::TempDir,
    token: String,
    host: Option<Host>,
    generation: u32,
    next_id: u64,
    runs: Vec<RunModel>,
    foreign: Vec<Foreign>,
    bare: Vec<Bare>,
    /// An open refused a malformed spawn record; the case ends.
    refused: bool,
}

impl Drop for Case {
    fn drop(&mut self) {
        make_writable(&self.dir.path().join("pids"));
        if let Some(mut host) = self.host.take() {
            host.kill();
        }
        let _ = kill_token_groups(&self.token, Duration::from_secs(5));
    }
}

impl Case {
    fn new(case: u64) -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        for sub in ["pids", "ctl", "work"] {
            std::fs::create_dir(dir.path().join(sub)).expect("a case directory");
        }
        Self {
            dir,
            token: format!("{}{case:05}", std::process::id()),
            host: None,
            generation: 0,
            next_id: 0,
            runs: Vec::new(),
            foreign: Vec::new(),
            bare: Vec::new(),
            refused: false,
        }
    }

    fn path(&self, sub: &str) -> PathBuf {
        self.dir.path().join(sub)
    }

    fn pid_file(&self) -> PathBuf {
        self.path("pids/groups.pid")
    }

    fn ctl(&self, id: u64) -> PathBuf {
        self.path("ctl").join(id.to_string())
    }

    fn host(&mut self) -> &mut Host {
        self.host.as_mut().expect("a host runs")
    }

    fn marker_pgid(&self, id: u64) -> Option<i32> {
        std::fs::read_to_string(self.ctl(id).with_extension("pgid"))
            .ok()
            .and_then(|t| t.trim().parse().ok())
    }

    /// Starts a host and opens; `forge` edits the pid file in between.
    /// `None`: the open refused a malformed spawn record and named it.
    fn start_host(&mut self, multi: bool, forge: &[Forge]) -> Result<Option<Vec<Value>>, String> {
        self.generation += 1;
        let host = Host::start(HOST_TEST, self.dir.path(), multi, self.generation);
        let opener = host.pid;
        self.host = Some(host);
        for f in forge {
            self.forge(f, opener)?;
        }
        let malformed = forge.iter().any(|f| matches!(f, Forge::SpawnMalformed));
        let opened = match self.host().call(json!({"op": "open"}), "opened", ANSWER) {
            Ok(v) if malformed => {
                return Err(format!(
                    "open accepted the spawn record {MALFORMED_SPAWN}, a name the runner never writes: {v}"
                ));
            }
            Ok(v) => v,
            Err(e) => {
                let events = self.host().drain();
                let named = events.iter().any(|v| {
                    v["ev"] == "open_failed"
                        && v["error"]
                            .as_str()
                            .is_some_and(|t| t.contains(MALFORMED_SPAWN))
                });
                if named && malformed {
                    return Ok(None);
                }
                return Err(format!("{e}; events {events:?}"));
            }
        };
        let spawn_records: Vec<String> = std::fs::read_dir(self.path("pids"))
            .expect("the pid directory lists")
            .map(|e| {
                e.expect("a pid directory entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .filter(|n| n.starts_with("groups.pid.spawn-") && n != MALFORMED_SPAWN)
            .collect();
        if !spawn_records.is_empty() {
            return Err(format!(
                "spawn records of dead hosts outlive the open: {spawn_records:?}"
            ));
        }
        Ok(Some(opened["groups"].as_array().expect("groups").clone()))
    }

    fn forge(&mut self, forge: &Forge, opener: i32) -> Result<(), String> {
        let pids = self.path("pids");
        match forge {
            Forge::SpawnHostReused(index) => {
                let mut records: Vec<String> = std::fs::read_dir(&pids)
                    .expect("the pid directory lists")
                    .map(|e| {
                        e.expect("a pid directory entry")
                            .file_name()
                            .to_string_lossy()
                            .into_owned()
                    })
                    .filter(|n| n.starts_with("groups.pid.spawn-") && n != MALFORMED_SPAWN)
                    .collect();
                records.sort();
                if records.is_empty() {
                    return Ok(());
                }
                let name = &records[index.index(records.len())];
                let rest = name
                    .strip_prefix("groups.pid.spawn-")
                    .expect("a spawn record");
                let (_host_pid, start_and_n) = rest
                    .split_once('-')
                    .ok_or_else(|| format!("the spawn record {name} names no host pid"))?;
                let f = Foreign::start(&self.token);
                let renamed = format!("groups.pid.spawn-{}-{start_and_n}", f.pgid);
                self.foreign.push(f);
                std::fs::rename(pids.join(name), pids.join(renamed))
                    .expect("the test renames a spawn record");
                return Ok(());
            }
            Forge::SpawnMalformed => {
                std::fs::write(pids.join(MALFORMED_SPAWN), "x\n")
                    .expect("the test writes a spawn record");
                return Ok(());
            }
            _ => {}
        }
        let pid_file = self.pid_file();
        let text = std::fs::read_to_string(&pid_file).unwrap_or_default();
        let mut lines: Vec<String> = text.lines().map(str::to_owned).collect();
        match forge {
            Forge::Foreign { leaderless } => {
                let f = if *leaderless {
                    Foreign::leaderless(&self.token)
                } else {
                    Foreign::start(&self.token)
                };
                lines.push(bare_line(f.pgid, 1, "Run", "foreign"));
                self.bare.push(Bare::run(f.pgid, "foreign"));
                self.foreign.push(f);
            }
            Forge::Opener => {
                lines.push(bare_line(opener, 1, "Run", "opener"));
                self.bare.push(Bare::run(opener, "opener"));
            }
            Forge::Reuse { index, leaderless } => {
                let orphans: Vec<usize> = (0..self.runs.len())
                    .filter(|&i| {
                        self.runs[i].state == State::Orphaned && self.runs[i].pgid.is_some()
                    })
                    .collect();
                if orphans.is_empty() {
                    return Ok(());
                }
                let i = orphans[index.index(orphans.len())];
                let pgid = self.runs[i].pgid.expect("an orphan with a pgid");
                let Some(at) = lines.iter().position(|l| line_pgid(l) == Some(pgid)) else {
                    // Its record is missing: the open finds that.
                    return Ok(());
                };
                // A member forking while the signal walks the group escapes
                // one SIGKILL.
                let killed = wait_until(Duration::from_secs(5), || {
                    // SAFETY: the group is one this case started.
                    unsafe { libc::kill(-pgid, libc::SIGKILL) };
                    !group_lives(pgid)
                });
                if !killed {
                    return Err(format!("the group {pgid} outlived its SIGKILL"));
                }
                self.runs[i].state = State::Gone;
                let f = if *leaderless {
                    Foreign::leaderless(&self.token)
                } else {
                    Foreign::start(&self.token)
                };
                // No leader to compare with the line's start, or no start in
                // the line: the open cannot tell the two groups apart.
                if *leaderless || !line_names_start(&lines[at]) {
                    let r = &self.runs[i];
                    self.bare.push(Bare {
                        pgid: f.pgid,
                        attempt: r.attempt,
                        transition: r.plan.transition.clone(),
                        subject: r.plan.subject.clone(),
                    });
                }
                lines[at] = with_pgid(&lines[at], f.pgid);
                self.foreign.push(f);
            }
            Forge::Alien(started) => {
                let Some(pgid) = restart_support::alien_leader() else {
                    return Ok(());
                };
                match started {
                    Some(s) => lines.push(json!([pgid, s, 1, "Run", "alien"]).to_string()),
                    None => {
                        lines.push(bare_line(pgid, 1, "Run", "alien"));
                        self.bare.push(Bare::run(pgid, "alien"));
                    }
                }
            }
            Forge::SpawnHostReused(_) | Forge::SpawnMalformed => unreachable!("handled above"),
        }
        let mut out = lines.join("\n");
        out.push('\n');
        std::fs::write(&pid_file, out).expect("the test rewrites the pid file");
        Ok(())
    }

    fn check_foreign(&mut self) -> Result<(), String> {
        for f in &mut self.foreign {
            if !f.alive() {
                return Err(format!("the foreign group {} was killed", f.pgid));
            }
        }
        Ok(())
    }

    fn current(&self, index: Index, keep: impl Fn(&RunModel) -> bool) -> Option<usize> {
        let picks: Vec<usize> = (0..self.runs.len())
            .filter(|&i| self.runs[i].generation == self.generation && keep(&self.runs[i]))
            .collect();
        (!picks.is_empty()).then(|| picks[index.index(picks.len())])
    }

    /// The attempts no run of this host has.
    fn free_attempts(&self) -> Vec<u64> {
        let used: BTreeSet<u64> = self
            .runs
            .iter()
            .filter(|r| r.generation == self.generation)
            .map(|r| r.attempt)
            .collect();
        (0..ATTEMPTS).filter(|a| !used.contains(a)).collect()
    }

    fn free_attempt(&self, index: Index) -> Option<u64> {
        let free = self.free_attempts();
        (!free.is_empty()).then(|| free[index.index(free.len())])
    }

    fn start_run(&mut self, attempt: u64, plan: &ChildPlan) -> Result<usize, String> {
        let id = self.next_id;
        self.next_id += 1;
        mkfifo(&self.ctl(id).with_extension("fifo"));
        let script = script(plan, &self.token);
        self.host().send(json!({
            "op": "run", "id": id, "attempt": attempt, "script": script,
            "transition": plan.transition, "subject": plan.subject,
            "timeout_ms": plan.timeout_ms,
        }))?;
        self.runs.push(RunModel {
            id,
            generation: self.generation,
            attempt,
            plan: plan.clone(),
            state: State::Running,
            pgid: None,
        });
        Ok(self.runs.len() - 1)
    }

    fn wait_spawned(&mut self, i: usize, limit: Duration) -> Result<i32, String> {
        let id = self.runs[i].id;
        if !wait_until(limit, || self.marker_pgid(id).is_some()) {
            return Err(format!("run {id} never spawned its child"));
        }
        let pgid = self.marker_pgid(id).expect("the marker exists");
        self.runs[i].pgid = Some(pgid);
        Ok(pgid)
    }

    fn ended(&mut self, i: usize) -> Result<Value, String> {
        let id = self.runs[i].id;
        let v = self
            .host()
            .expect(ENDED, "ended", |e| e["ev"] == "ended" && e["id"] == id)?;
        self.runs[i].state = State::Ended;
        Ok(v)
    }

    /// Waits until `in_flight` lists the run, so an abandon finds it filed.
    fn wait_listed(&mut self, i: usize) -> Result<(), String> {
        let pgid = self.wait_spawned(i, ANSWER)?;
        for _ in 0..200 {
            let v = self
                .host()
                .call(json!({"op": "in_flight"}), "in_flight", ANSWER)?;
            if v["groups"]
                .as_array()
                .expect("groups")
                .iter()
                .any(|g| g["pgid"] == pgid)
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Err(format!("in_flight never lists the running group {pgid}"))
    }

    fn apply(&mut self, op: &Op) -> Result<(), String> {
        match op {
            Op::Run(a, plan) => {
                let Some(attempt) = self.free_attempt(*a) else {
                    return Ok(());
                };
                let i = self.start_run(attempt, plan)?;
                // A timeout may kill the child before its shell writes the marker; the end then
                // says so.
                if plan.timeout_ms.is_none() {
                    self.wait_spawned(i, ANSWER)?;
                }
            }
            Op::Release(index) => {
                let Some(i) =
                    self.current(*index, |r| r.state == State::Running && released(&r.plan))
                else {
                    return Ok(());
                };
                self.wait_spawned(i, ANSWER)?;
                let id = self.runs[i].id;
                if !release_fifo(&self.ctl(id).with_extension("fifo"), ANSWER) {
                    return Err(format!("the child of run {id} never blocked on its FIFO"));
                }
                let v = self.ended(i)?;
                let want = json!({"kind": "exited", "code": self.runs[i].plan.code});
                if v["result"] != want || v["living"] != json!([]) {
                    return Err(format!(
                        "run {id}, released, must end {want} with no living member of its group: {v}"
                    ));
                }
                self.not_in_flight(i)?;
            }
            Op::Abandon(index) => {
                // An abandon racing a timeout is no question of this test.
                let Some(i) = self.current(*index, |r| {
                    r.state == State::Running && r.plan.timeout_ms.is_none()
                }) else {
                    return Ok(());
                };
                self.wait_listed(i)?;
                let attempt = self.runs[i].attempt;
                let v = self.host().call(
                    json!({"op": "abandon", "attempt": attempt}),
                    "abandon",
                    ANSWER,
                )?;
                if v["ok"] != true {
                    return Err(format!(
                        "abandon of the listed attempt {attempt} returned {v}"
                    ));
                }
                let v = self.ended(i)?;
                if v["result"] != json!({"kind": "abandoned"}) || v["living"] != json!([]) {
                    return Err(format!(
                        "the abandoned run {} ended with {v}",
                        self.runs[i].id
                    ));
                }
            }
            Op::AbandonUnknown(index) => {
                let mine: BTreeSet<u64> = self
                    .runs
                    .iter()
                    .filter(|r| r.generation == self.generation)
                    .map(|r| r.attempt)
                    .collect();
                let others: Vec<u64> = (0..ATTEMPTS + 2).filter(|a| !mine.contains(a)).collect();
                let attempt = others[index.index(others.len())];
                let v = self.host().call(
                    json!({"op": "abandon", "attempt": attempt}),
                    "abandon",
                    ANSWER,
                )?;
                if v["ok"] != false {
                    return Err(format!(
                        "abandon of the attempt {attempt}, which this runner does not run, returned {v}"
                    ));
                }
            }
            Op::Drop(at) => self.drop_run(at)?,
            Op::Reap(index) => self.reap(*index)?,
            Op::Second { process } => self.second_runner(*process)?,
            Op::InFlight => {
                let v = self
                    .host()
                    .call(json!({"op": "in_flight"}), "in_flight", ANSWER)?;
                // A timed run is not waited for at its start, so its pgid may
                // be known only from its marker, or not yet at all.
                let mine: Vec<(u64, Option<i32>)> = self
                    .runs
                    .iter()
                    .filter(|r| r.generation == self.generation && r.state == State::Running)
                    .map(|r| (r.attempt, r.pgid.or_else(|| self.marker_pgid(r.id))))
                    .collect();
                for g in v["groups"].as_array().expect("groups") {
                    let pgid = g["pgid"].as_i64().expect("a pgid") as i32;
                    let attempt = g["attempt"].as_u64().expect("an attempt");
                    let owned = mine
                        .iter()
                        .any(|(a, p)| *a == attempt && p.is_none_or(|p| p == pgid));
                    if !owned {
                        return Err(format!(
                            "in_flight lists {g}, which no running run of this runner owns ({mine:?})"
                        ));
                    }
                }
            }
            Op::RecordFails(a, plan, fault) => {
                let Some(attempt) = self.free_attempt(*a) else {
                    return Ok(());
                };
                // A timeout in the fault's window fails the write that forgets
                // its group, which is no question of this test.
                if self.timed_run_runs() {
                    return Ok(());
                }
                self.record_fails(attempt, plan, fault)?;
            }
            Op::Restart {
                kill,
                multi,
                release,
                release_reported,
                forge,
                kill_left,
            } => self.restart(kill, *multi, *release, *release_reported, forge, kill_left)?,
            Op::ReleaseLeftover(index) => self.release_orphan(*index, |_| true),
            Op::Reboot { multi, run } => self.reboot(*multi, run)?,
        }
        self.unexpected_ends(false)
    }

    /// The child of an orphaned run exits; its members, if any, run on.
    fn release_orphan(&mut self, index: Index, keep: impl Fn(&RunModel) -> bool) {
        let picks: Vec<usize> = (0..self.runs.len())
            .filter(|&i| {
                self.runs[i].state == State::Orphaned
                    && self.runs[i].pgid.is_some()
                    && released(&self.runs[i].plan)
                    && keep(&self.runs[i])
            })
            .collect();
        if picks.is_empty() {
            return;
        }
        let i = picks[index.index(picks.len())];
        let id = self.runs[i].id;
        let pgid = self.runs[i].pgid.expect("an orphan with a pgid");
        // A leader a kill already hit no longer reads its FIFO.
        if release_fifo(
            &self.ctl(id).with_extension("fifo"),
            Duration::from_millis(500),
        ) {
            wait_until(Duration::from_secs(2), || {
                !living_members(pgid).contains(&pgid)
            });
        }
        if !group_lives(pgid) {
            self.runs[i].state = State::Gone;
        }
    }

    fn record_fails(
        &mut self,
        attempt: u64,
        plan: &ChildPlan,
        fault: &Fault,
    ) -> Result<(), String> {
        let pids = self.path("pids");
        let pid_file = self.pid_file();
        let fifo = beside(&pid_file, ".writing");
        let kept = pid_file.with_extension("kept");
        match fault {
            Fault::ReadOnly => {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&pids, std::fs::Permissions::from_mode(0o500))
                    .expect("the test makes its pid directory read-only");
            }
            Fault::Held(_) => {
                mkfifo(&fifo);
                if pid_file.exists() {
                    std::fs::rename(&pid_file, &kept).expect("the test moves its pid file aside");
                }
                std::fs::create_dir(&pid_file).expect("the test puts a directory in its place");
                std::fs::write(pid_file.join("keep"), "").expect("the directory is not empty");
            }
        }
        let i = self.start_run(attempt, plan)?;
        // A runner that records before it spawns fails before the child starts.
        let _ = self.wait_spawned(i, Duration::from_secs(2));
        // Open until the run ended: the write goes into the pipe and never blocks.
        let reader = match fault {
            Fault::Held(ms) => {
                std::thread::sleep(Duration::from_millis(*ms));
                Some(open_fifo_reader(&fifo))
            }
            Fault::ReadOnly => None,
        };
        let ended = self.ended(i);
        drop(reader);
        make_writable(&pids);
        if let Fault::Held(_) = fault {
            std::fs::remove_dir_all(&pid_file).expect("the test removes its directory");
            if kept.exists() {
                std::fs::rename(&kept, &pid_file).expect("the test restores its pid file");
            }
            let _ = std::fs::remove_file(&fifo);
        }
        let v = ended?;
        let kind = &v["result"]["kind"];
        let refused = kind == "pid_file" || (matches!(fault, Fault::ReadOnly) && kind == "spawn");
        if !refused || v["living"] != json!([]) {
            return Err(format!(
                "a run whose group the pid file cannot record ({fault:?}) ended with {v}"
            ));
        }
        if self.runs[i].pgid.is_none() {
            self.runs[i].pgid = self.marker_pgid(self.runs[i].id);
        }
        if let Some(pgid) = self.runs[i].pgid {
            // Members the kill missed would run on with no record anywhere.
            std::thread::sleep(Duration::from_millis(100));
            let survivors = living_members(pgid);
            if !survivors.is_empty() {
                return Err(format!(
                    "members {survivors:?} of the unrecorded group {pgid} survived"
                ));
            }
        }
        Ok(())
    }

    /// A held child ends only when the test releases or kills it, or when its
    /// timeout kills it.
    /// `shutdown`: the host's runtime shut down, so a run whose file work it
    /// cancelled may return ShutDown, with its group dead.
    fn unexpected_ends(&mut self, shutdown: bool) -> Result<(), String> {
        let Some(host) = self.host.as_mut() else {
            return Ok(());
        };
        let ends: Vec<Value> = host
            .drain()
            .into_iter()
            .filter(|v| v["ev"] == "ended")
            .collect();
        for v in ends {
            let current = |r: &RunModel| {
                v["id"] == r.id && r.generation == self.generation && r.state == State::Running
            };
            if shutdown && v["result"]["kind"] == "shut_down" {
                if !self.runs.iter().any(current) {
                    return Err(format!("a run no shutdown cut short ended with {v}"));
                }
                if v["living"] != json!([]) {
                    return Err(format!(
                        "a run the shutdown cut short returned with its group living: {v}"
                    ));
                }
                // The shutdown's kill of its group is checked with the dropped runs.
                continue;
            }
            let timed = self.runs.iter().position(|r| {
                v["id"] == r.id
                    && r.generation == self.generation
                    && r.state == State::Running
                    && r.plan.timeout_ms.is_some()
            });
            let Some(i) = timed else {
                return Err(format!("a run ended that nothing ended: {v}"));
            };
            if v["result"] != json!({"kind": "timeout"}) || v["living"] != json!([]) {
                return Err(format!("a run whose timeout passed ended with {v}"));
            }
            self.runs[i].state = State::Ended;
        }
        Ok(())
    }

    fn restart(
        &mut self,
        kill: &KillPoint,
        multi: bool,
        release: Option<Index>,
        release_reported: Option<Index>,
        forge: &[Forge],
        kill_left: &[bool],
    ) -> Result<(), String> {
        let fifo = beside(&self.pid_file(), ".writing");
        let first_free = self.free_attempts().first().copied();
        let mut exiting = None;
        match (kill, first_free) {
            (KillPoint::Released(index, ms), _) => {
                if let Some(i) = self.current(*index, |r| {
                    r.state == State::Running && r.pgid.is_some() && released(&r.plan)
                }) {
                    let id = self.runs[i].id;
                    if !release_fifo(&self.ctl(id).with_extension("fifo"), ANSWER) {
                        return Err(format!("the child of run {id} never blocked on its FIFO"));
                    }
                    std::thread::sleep(Duration::from_millis(*ms));
                    exiting = self.runs[i].pgid;
                }
            }
            (KillPoint::Now, _) | (_, None) => {}
            (KillPoint::InRun(plan, ms), Some(attempt)) => {
                self.start_run(attempt, plan)?;
                std::thread::sleep(Duration::from_millis(*ms));
            }
            (KillPoint::InRecord(plan), Some(attempt)) => {
                mkfifo(&fifo);
                let i = self.start_run(attempt, plan)?;
                // A runner that records before it spawns blocks before the
                // child starts; then nothing was spawned.
                let _ = self.wait_spawned(i, Duration::from_secs(2));
            }
        }
        let mut host = self.host.take().expect("a host runs");
        host.kill();
        drop(host);
        let _ = std::fs::remove_file(&fifo);
        std::thread::sleep(SETTLE);
        let dead = self.generation;
        for i in 0..self.runs.len() {
            if self.runs[i].generation != dead {
                continue;
            }
            if self.runs[i].pgid.is_none() {
                self.runs[i].pgid = self.marker_pgid(self.runs[i].id);
            }
            if self.runs[i].state == State::Running {
                self.runs[i].state = State::Orphaned;
            }
        }
        if let Some(index) = release {
            self.release_orphan(index, |r| r.generation == dead);
        }
        // A released leader exits; once no leader is a zombie, each leader
        // either runs or is gone for the whole open.
        if let Some(pgid) = exiting {
            wait_until(ANSWER, || !living_members(pgid).contains(&pgid));
        }
        let groups: Vec<i32> = self.runs.iter().filter_map(|r| r.pgid).collect();
        if !wait_until(ANSWER, || groups.iter().all(|g| !leader_is_zombie(*g))) {
            return Err(format!("a leader of the groups {groups:?} stays a zombie"));
        }
        let dead_before = self.dead_groups();
        let Some(reported) = self.start_host(multi, forge)? else {
            self.refused = true;
            return Ok(());
        };
        self.check_reported(&reported, &dead_before)?;
        if let Some(index) = release_reported {
            self.release_orphan(index, |r| r.generation == dead);
        }
        for (index, group) in reported.iter().enumerate() {
            if !kill_left.get(index).copied().unwrap_or(false) {
                continue;
            }
            self.kill_leftover(index, group)?;
        }
        self.check_foreign()?;
        self.host()
            .call(json!({"op": "ping"}), "pong", ANSWER)
            .map(|_| ())
    }

    fn reboot(&mut self, multi: bool, run: &Option<(ChildPlan, u64)>) -> Result<(), String> {
        if let (Some((plan, ms)), Some(attempt)) = (run, self.free_attempts().first().copied()) {
            self.start_run(attempt, plan)?;
            std::thread::sleep(Duration::from_millis(*ms));
        }
        self.host()
            .call(json!({"op": "reboot", "multi": multi}), "rebooted", ENDED)?;
        self.unexpected_ends(true)?;
        let old = self.generation;
        self.generation += 1;
        for i in 0..self.runs.len() {
            if self.runs[i].generation != old {
                continue;
            }
            let id = self.runs[i].id;
            if self.runs[i].pgid.is_none() {
                // A child forked before the shutdown writes its marker soon
                // after; a run dropped before its spawn has none.
                wait_until(Duration::from_secs(2), || self.marker_pgid(id).is_some());
                self.runs[i].pgid = self.marker_pgid(id);
            }
            if self.runs[i].state != State::Running {
                continue;
            }
            self.runs[i].state = State::Gone;
            if let Some(pgid) = self.runs[i].pgid
                && !wait_until(DROPPED, || living_members(pgid).is_empty())
            {
                return Err(format!(
                    "members {:?} of the group {pgid} of run {id} run {DROPPED:?} after a runtime shutdown dropped the run",
                    living_members(pgid)
                ));
            }
        }
        let dead_before = self.dead_groups();
        let opened = self.host().call(json!({"op": "open"}), "opened", ANSWER)?;
        self.check_reported(opened["groups"].as_array().expect("groups"), &dead_before)?;
        self.check_foreign()
    }

    /// A run of this host whose timeout may write the pid file at any moment.
    fn timed_run_runs(&self) -> bool {
        self.runs.iter().any(|r| {
            r.generation == self.generation
                && r.state == State::Running
                && r.plan.timeout_ms.is_some()
        })
    }

    fn not_in_flight(&mut self, i: usize) -> Result<(), String> {
        let Some(pgid) = self.runs[i].pgid else {
            return Ok(());
        };
        let v = self
            .host()
            .call(json!({"op": "in_flight"}), "in_flight", ANSWER)?;
        if v["groups"]
            .as_array()
            .expect("groups")
            .iter()
            .any(|g| g["pgid"] == pgid)
        {
            return Err(format!(
                "in_flight still lists the group {pgid} of run {}, which returned or was dropped: {v}",
                self.runs[i].id
            ));
        }
        Ok(())
    }

    fn drop_run(&mut self, at: &DropAt) -> Result<(), String> {
        let temp = beside(&self.pid_file(), ".writing");
        let mut passer = None;
        let i = match at {
            DropAt::Running(index) => {
                let Some(i) = self.current(*index, |r| r.state == State::Running) else {
                    return Ok(());
                };
                i
            }
            DropAt::Starting(plan, ms) => {
                let Some(attempt) = self.free_attempts().first().copied() else {
                    return Ok(());
                };
                let i = self.start_run(attempt, plan)?;
                std::thread::sleep(Duration::from_millis(*ms));
                i
            }
            DropAt::Recording(plan, ms) => {
                let free = self.free_attempts().first().copied();
                // A timeout would write through the held FIFO too.
                let Some(attempt) = free.filter(|_| !self.timed_run_runs()) else {
                    return Ok(());
                };
                mkfifo(&temp);
                let i = self.start_run(attempt, plan)?;
                // A runner that records before it spawns blocks before the
                // child starts.
                let _ = self.wait_spawned(i, Duration::from_secs(2));
                passer = Some(pass_held_write(
                    temp,
                    self.pid_file(),
                    Duration::from_millis(*ms),
                ));
                i
            }
            DropAt::Ending(index, ms) => {
                let Some(i) = self.current(*index, |r| {
                    r.state == State::Running && r.plan.timeout_ms.is_none()
                }) else {
                    return Ok(());
                };
                self.wait_listed(i)?;
                let attempt = self.runs[i].attempt;
                let v = self.host().call(
                    json!({"op": "abandon", "attempt": attempt}),
                    "abandon",
                    ANSWER,
                )?;
                if v["ok"] != true {
                    return Err(format!(
                        "abandon of the listed attempt {attempt} returned {v}"
                    ));
                }
                std::thread::sleep(Duration::from_millis(*ms));
                i
            }
            DropAt::Exiting(index) => {
                let Some(i) =
                    self.current(*index, |r| r.state == State::Running && released(&r.plan))
                else {
                    return Ok(());
                };
                self.wait_spawned(i, ANSWER)?;
                let id = self.runs[i].id;
                if !release_fifo(&self.ctl(id).with_extension("fifo"), ANSWER) {
                    return Err(format!("the child of run {id} never blocked on its FIFO"));
                }
                i
            }
        };
        let id = self.runs[i].id;
        let dropped = self
            .host()
            .call(json!({"op": "drop", "id": id}), "dropped", ANSWER);
        if let Some(passer) = passer {
            passer.join().expect("the test passes the held write");
        }
        dropped?;
        self.runs[i].state = State::Gone;
        // The host emits a run's end before its drop completes.
        if let Ok(v) = self.host().expect(Duration::ZERO, "ended", |e| {
            e["ev"] == "ended" && e["id"] == id
        }) {
            let plan = &self.runs[i].plan;
            let want = match at {
                DropAt::Exiting(_) => json!({"kind": "exited", "code": plan.code}),
                DropAt::Ending(..) => json!({"kind": "abandoned"}),
                _ if plan.timeout_ms.is_some() => json!({"kind": "timeout"}),
                _ => {
                    return Err(format!(
                        "run {id} ended before its drop, though nothing ended it: {v}"
                    ));
                }
            };
            if v["result"] != want || v["living"] != json!([]) {
                return Err(format!(
                    "run {id}, ended before its drop, must end {want} with no living member: {v}"
                ));
            }
        }
        if self.runs[i].pgid.is_none() {
            // A child spawned just before the drop writes its marker soon after.
            wait_until(Duration::from_secs(1), || self.marker_pgid(id).is_some());
            self.runs[i].pgid = self.marker_pgid(id);
        }
        if let Some(pgid) = self.runs[i].pgid
            && !wait_until(DROPPED, || living_members(pgid).is_empty())
        {
            return Err(format!(
                "members {:?} of the group {pgid} of run {id}, dropped {at:?}, run {DROPPED:?} after the drop",
                living_members(pgid)
            ));
        }
        self.not_in_flight(i)?;
        let attempt = self.runs[i].attempt;
        let v = self.host().call(
            json!({"op": "abandon", "attempt": attempt}),
            "abandon",
            ANSWER,
        )?;
        if v["ok"] != false {
            return Err(format!(
                "abandon of the attempt {attempt} of the dropped run {id} returned {v}"
            ));
        }
        Ok(())
    }

    fn reap(&mut self, index: Index) -> Result<(), String> {
        let Some(i) = self.current(index, |r| r.state == State::Running && released(&r.plan))
        else {
            return Ok(());
        };
        self.wait_listed(i)?;
        let id = self.runs[i].id;
        self.host().send(json!({"op": "reap", "id": id}))?;
        self.host()
            .expect(ANSWER, "reaping", |e| e["ev"] == "reaping" && e["id"] == id)?;
        if !release_fifo(&self.ctl(id).with_extension("fifo"), ANSWER) {
            return Err(format!("the child of run {id} never blocked on its FIFO"));
        }
        let reaped = self
            .host()
            .expect(ENDED, "reaped", |e| e["ev"] == "reaped" && e["id"] == id)?;
        let v = self.ended(i)?;
        // Who reaped the leader first decides: the host, so the runner's wait
        // fails, or the runner, before the host's wait.
        let ok = if reaped["reaped"] == true {
            v["result"]["kind"] == "wait"
        } else {
            v["result"] == json!({"kind": "exited", "code": self.runs[i].plan.code})
        };
        if !ok || v["living"] != json!([]) {
            return Err(format!(
                "run {id}, whose leader the host reaped first ({reaped}), must end with a wait error and no living member: {v}"
            ));
        }
        self.not_in_flight(i)
    }

    fn second_runner(&mut self, process: bool) -> Result<(), String> {
        // A timeout's write would change the pid file meanwhile.
        if self.timed_run_runs() {
            return Ok(());
        }
        let pid_file = self.pid_file();
        let running: Vec<i32> = self
            .runs
            .iter()
            .filter(|r| r.generation == self.generation && r.state == State::Running)
            .filter_map(|r| r.pgid)
            .collect();
        // The record writes of the runs this host started have landed.
        let recorded = || {
            let text = std::fs::read_to_string(&pid_file).unwrap_or_default();
            running
                .iter()
                .all(|&p| text.lines().any(|l| line_pgid(l) == Some(p)))
        };
        if !wait_until(ANSWER, recorded) {
            return Err(format!(
                "the pid file never names the running groups {running:?}"
            ));
        }
        let before = self.pid_dir();
        let answer = if process {
            let mut second =
                Host::start(HOST_TEST, self.dir.path(), false, 1_000 + self.generation);
            second.send(json!({"op": "open"}))?;
            let v = second.expect(ANSWER, "open", |e| {
                e["ev"] == "opened" || e["ev"] == "open_failed"
            });
            second.kill();
            v?
        } else {
            self.host()
                .call(json!({"op": "open_second"}), "second", ANSWER)?
        };
        let refused = answer["ev"] == "open_failed" || answer["ok"] == false;
        let path = pid_file.display().to_string();
        let holder = self.host().pid.to_string();
        let names_both = |e: &str| e.contains(&path) && e.replace(&path, "").contains(&holder);
        if !refused || !answer["error"].as_str().is_some_and(names_both) {
            return Err(format!(
                "a second runner (in another process: {process}) on the pid file a live runner holds must be refused naming {path} and the holder {holder}: {answer}"
            ));
        }
        let after = self.pid_dir();
        if before != after {
            return Err(format!(
                "a refused second runner changed the pid directory from {before:?} to {after:?}"
            ));
        }
        Ok(())
    }

    /// Each file of the pid directory, the pid file and spawn records among
    /// them, with its text.
    fn pid_dir(&self) -> BTreeMap<String, String> {
        std::fs::read_dir(self.path("pids"))
            .expect("the pid directory lists")
            .map(|e| {
                let e = e.expect("a pid directory entry");
                let text = std::fs::read(e.path()).expect("the test reads a pid directory file");
                (
                    e.file_name().to_string_lossy().into_owned(),
                    String::from_utf8_lossy(&text).into_owned(),
                )
            })
            .collect()
    }

    fn kill_leftover(&mut self, index: usize, group: &Value) -> Result<(), String> {
        let pgid = group["pgid"].as_i64().expect("a pgid") as i32;
        // A leader that exited after the open leaves the group without
        // identity too; init reaps it at once.
        if !wait_until(ANSWER, || !leader_is_zombie(pgid)) {
            return Err(format!("the leader of {group} stays a zombie"));
        }
        let living = living_members(pgid);
        let unverified =
            group["verified"] == false || (!living.is_empty() && !living.contains(&pgid));
        let v = self
            .host()
            .call(json!({"op": "kill", "index": index}), "killed", ENDED)?;
        if unverified {
            // A group without identity cannot be told from a foreign one.
            let named = v["error"].as_str().is_some_and(|e| {
                e.contains(&format!("process group {pgid} ")) && e.contains("is not signalled")
            });
            if !named || !group_lives(pgid) {
                return Err(format!(
                    "kill_group of {group}, which has no identity, returned {v}"
                ));
            }
            return Ok(());
        }
        if !v["error"].is_null() || v["living"] != json!([]) {
            return Err(format!("kill_group of {group} returned {v}"));
        }
        for r in self.runs.iter_mut().filter(|r| r.pgid == Some(pgid)) {
            r.state = State::Gone;
        }
        Ok(())
    }

    /// Every live group a dead host left and every live line without identity
    /// is reported, with its names, and nothing else is.
    /// The groups of every run and bare child that no member runs in now: an
    /// open that starts after this may not report them. A group that ends
    /// during or after the open may be reported.
    fn dead_groups(&self) -> BTreeSet<i32> {
        self.runs
            .iter()
            .filter_map(|r| r.pgid)
            .chain(self.bare.iter().map(|b| b.pgid))
            .filter(|pgid| !group_lives(*pgid))
            .collect()
    }

    fn check_reported(
        &mut self,
        reported: &[Value],
        dead_before: &BTreeSet<i32>,
    ) -> Result<(), String> {
        // A group is verified while its leader runs: no leader is a zombie
        // now, and a dead host's leader neither starts nor ends during the open.
        let run_json = |r: &RunModel, pgid: i32| {
            json!({"pgid": pgid, "attempt": r.attempt, "transition": r.plan.transition,
                "subject": r.plan.subject, "verified": living_members(pgid).contains(&pgid)})
        };
        let mut known: BTreeMap<i32, Value> = self
            .runs
            .iter()
            .filter(|r| r.generation < self.generation)
            .filter_map(|r| Some((r.pgid?, run_json(r, r.pgid?))))
            .collect();
        for b in &self.bare {
            known.insert(
                b.pgid,
                json!({"pgid": b.pgid, "attempt": b.attempt, "transition": b.transition,
                    "subject": b.subject, "verified": false}),
            );
        }
        let mut got = BTreeSet::new();
        for g in reported {
            let pgid = g["pgid"].as_i64().expect("a pgid") as i32;
            if let std::collections::btree_map::Entry::Vacant(slot) = known.entry(pgid) {
                // A child of a dead host writes its marker only after its
                // spawn record; under load that is after the marker read.
                let late = |c: &Self| {
                    c.runs.iter().position(|r| {
                        r.generation < c.generation
                            && r.pgid.is_none()
                            && c.marker_pgid(r.id) == Some(pgid)
                    })
                };
                if wait_until(ANSWER, || late(self).is_some()) {
                    let i = late(self).expect("the marker stays");
                    self.runs[i].pgid = Some(pgid);
                    slot.insert(run_json(&self.runs[i], pgid));
                }
            }
            let Some(want) = known.get(&pgid) else {
                return Err(format!(
                    "open reports {g}, which no dead host started (foreign {:?})",
                    self.foreign.iter().map(|f| f.pgid).collect::<Vec<_>>()
                ));
            };
            if g != want {
                return Err(format!("open reports {g} for the group of {want}"));
            }
            if dead_before.contains(&pgid) {
                let model: Vec<String> = self
                    .runs
                    .iter()
                    .filter(|r| r.pgid == Some(pgid))
                    .map(|r| format!("run {} of host {} {:?}", r.id, r.generation, r.state))
                    .collect();
                return Err(format!(
                    "open reports the group {g}, dead before the open (model: {model:?})"
                ));
            }
            got.insert(pgid);
        }
        let must = self
            .runs
            .iter()
            .filter(|r| r.state == State::Orphaned)
            .filter_map(|r| r.pgid)
            .chain(self.bare.iter().map(|b| b.pgid));
        for pgid in must {
            if group_lives(pgid) && !got.contains(&pgid) {
                return Err(format!(
                    "the group {pgid} ({}) runs, but open reports {reported:?}",
                    known[&pgid]
                ));
            }
        }
        Ok(())
    }
}

/// The test releases the child: its leader reads the FIFO, and no timeout
/// ends it first.
fn released(plan: &ChildPlan) -> bool {
    plan.leader == Leader::Reads && plan.timeout_ms.is_none()
}

fn run_case(case: u64, scenario: &Scenario) -> Result<(), String> {
    let mut c = Case::new(case);
    let groups = c.start_host(scenario.multi, &[])?;
    if groups.as_ref().is_none_or(|g| !g.is_empty()) {
        return Err(format!("a fresh pid file reports {groups:?}"));
    }
    let mut ops = scenario.ops.clone();
    ops.push(Op::Restart {
        kill: KillPoint::Now,
        multi: scenario.multi,
        release: None,
        release_reported: None,
        forge: Vec::new(),
        kill_left: vec![true; 64],
    });
    for op in &ops {
        c.apply(op).map_err(|e| format!("{e}\n  at {op:?}"))?;
        if c.refused {
            break;
        }
    }
    c.check_foreign()?;
    let token = c.token.clone();
    let dir: PathBuf = c.dir.path().to_owned();
    drop(c);
    no_orphans(&token, &dir)
}

/// The case's cleanup reached every process it started.
fn no_orphans(token: &str, dir: &Path) -> Result<(), String> {
    let left = restart_support::token_processes(token);
    if left.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "the case in {} left processes behind: {left:?}",
            dir.display()
        ))
    }
}

/// Any index: a fixed scenario picks from collections of one.
fn first() -> Index {
    use proptest::strategy::ValueTree;
    any::<Index>()
        .new_tree(&mut proptest::test_runner::TestRunner::deterministic())
        .expect("an index draws")
        .current()
}

fn plan(member: restart_support::Member, leader: Leader) -> ChildPlan {
    ChildPlan {
        trap_term: false,
        member,
        leader,
        timeout_ms: None,
        code: 0,
        transition: "t".to_owned(),
        subject: "s".to_owned(),
    }
}

fn restart(kill: KillPoint, forge: Vec<Forge>, kill_left: bool) -> Op {
    Op::Restart {
        kill,
        multi: false,
        release: None,
        release_reported: None,
        forge,
        kill_left: vec![kill_left; 16],
    }
}

/// The scenarios the properties run first: one per way a run or a restart
/// was seen to orphan, to kill a foreign group or to share a held pid file.
fn fixed_scenarios() -> Vec<(&'static str, Scenario)> {
    use restart_support::Member;
    let one = |ops: Vec<Op>| Scenario { multi: false, ops };
    let sleeper = plan(Member::Sleeper, Leader::Reads);
    vec![
        (
            "a host killed while it records a spawned group",
            one(vec![restart(
                KillPoint::InRecord(plan(Member::None, Leader::Reads)),
                vec![],
                false,
            )]),
        ),
        (
            "a leftover the first open kept is reported by the next open",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                restart(KillPoint::Now, vec![], false),
                restart(KillPoint::Now, vec![], false),
            ]),
        ),
        (
            "a runtime shut down while its run records, then a host restart",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                Op::Reboot {
                    multi: true,
                    run: Some((sleeper.clone(), 0)),
                },
                Op::Run(first(), sleeper.clone()),
                restart(KillPoint::Now, vec![], false),
            ]),
        ),
        (
            "a dead host's runs whose names need escaping in the pid file",
            one(["a\tb", "a\nb", "", "h\u{e9}llo"]
                .into_iter()
                .map(|name| {
                    Op::Run(
                        first(),
                        ChildPlan {
                            transition: name.to_owned(),
                            subject: name.to_owned(),
                            ..sleeper.clone()
                        },
                    )
                })
                .chain([restart(KillPoint::Now, vec![], true)])
                .collect()),
        ),
        (
            "a dead group's pgid that a foreign group took",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                restart(
                    KillPoint::Now,
                    vec![Forge::Reuse {
                        index: first(),
                        leaderless: false,
                    }],
                    true,
                ),
            ]),
        ),
        (
            "a dead group's pgid that a foreign group whose leader exited took",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                restart(
                    KillPoint::Now,
                    vec![Forge::Reuse {
                        index: first(),
                        leaderless: true,
                    }],
                    true,
                ),
            ]),
        ),
        (
            "a line without identity naming a foreign group",
            one(vec![restart(
                KillPoint::Now,
                vec![Forge::Foreign { leaderless: false }],
                true,
            )]),
        ),
        (
            "a line without identity naming a foreign group whose leader exited",
            one(vec![restart(
                KillPoint::Now,
                vec![Forge::Foreign { leaderless: true }],
                true,
            )]),
        ),
        (
            "a dead host's group whose leader exited before the host died",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                restart(KillPoint::Released(first(), 0), vec![], true),
            ]),
        ),
        (
            "a dead host's group whose leader exited after the host died",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                Op::Restart {
                    kill: KillPoint::Now,
                    multi: false,
                    release: Some(first()),
                    release_reported: None,
                    forge: vec![],
                    kill_left: vec![true; 16],
                },
                restart(KillPoint::Now, vec![], true),
            ]),
        ),
        (
            "a reported leftover whose leader exits before kill_group",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                Op::Restart {
                    kill: KillPoint::Now,
                    multi: false,
                    release: None,
                    release_reported: Some(first()),
                    forge: vec![],
                    kill_left: vec![true; 16],
                },
            ]),
        ),
        (
            "a line without a start whose pgid a foreign group took",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                Op::Restart {
                    kill: KillPoint::Now,
                    multi: false,
                    release: None,
                    release_reported: Some(first()),
                    forge: vec![],
                    kill_left: vec![false; 16],
                },
                restart(KillPoint::Now, vec![], false),
                restart(
                    KillPoint::Now,
                    vec![Forge::Reuse {
                        index: first(),
                        leaderless: false,
                    }],
                    true,
                ),
            ]),
        ),
        (
            "a leftover whose member ignores SIGTERM",
            one(vec![
                Op::Run(first(), plan(Member::TermIgnoring, Leader::Reads)),
                restart(KillPoint::Now, vec![], true),
            ]),
        ),
        (
            "a group the pid file cannot record while its shell forks",
            one(vec![Op::RecordFails(
                first(),
                ChildPlan {
                    trap_term: true,
                    ..plan(Member::Forks(250), Leader::Reads)
                },
                Fault::Held(10),
            )]),
        ),
        (
            "a line without identity naming another user's group",
            one(vec![restart(
                KillPoint::Now,
                vec![Forge::Alien(None)],
                true,
            )]),
        ),
        (
            "a group the read-only pid directory cannot record",
            one(vec![Op::RecordFails(
                first(),
                sleeper.clone(),
                Fault::ReadOnly,
            )]),
        ),
        (
            "a spawn record whose name the runner never writes",
            one(vec![restart(
                KillPoint::Now,
                vec![Forge::SpawnMalformed],
                true,
            )]),
        ),
        (
            "a leftover whose leader runs a setuid program",
            one(vec![
                Op::Run(first(), plan(Member::None, Leader::Setuid)),
                restart(KillPoint::Now, vec![], true),
            ]),
        ),
        (
            "an abandoned run whose group then holds only a zombie",
            one(vec![
                Op::Run(
                    first(),
                    plan(Member::Unreaped { leaves: true }, Leader::Reads),
                ),
                Op::Abandon(first()),
            ]),
        ),
        (
            "a leader that exits while a member of its group sleeps",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                Op::Release(first()),
            ]),
        ),
        (
            "a dropped run whose child runs",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                Op::Drop(DropAt::Running(first())),
            ]),
        ),
        (
            "a dropped run whose leader runs a setuid program",
            one(vec![
                Op::Run(first(), plan(Member::None, Leader::Setuid)),
                Op::Drop(DropAt::Running(first())),
            ]),
        ),
        (
            "a runtime shut down while its run runs",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                Op::Reboot {
                    multi: false,
                    run: None,
                },
            ]),
        ),
        (
            "a run whose leader the host reaps first",
            one(vec![
                Op::Run(first(), plan(Member::None, Leader::Reads)),
                Op::Reap(first()),
            ]),
        ),
        (
            "a second runner in another process",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                Op::Second { process: true },
            ]),
        ),
        (
            "a second runner in the same process",
            one(vec![
                Op::Run(first(), sleeper.clone()),
                Op::Second { process: false },
            ]),
        ),
    ]
}

#[test]
fn every_fixed_scenario_holds_every_property() {
    let failures: Vec<String> = fixed_scenarios()
        .iter()
        .enumerate()
        .filter_map(|(i, (name, scenario))| {
            run_case(1_000 + i as u64, scenario)
                .err()
                .map(|e| format!("{name}: {e}"))
        })
        .collect();
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

fn check(scenario: &Scenario) -> Result<(), TestCaseError> {
    static CASE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let case = CASE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    run_case(case, scenario).map_err(TestCaseError::fail)
}

proptest! {
    // Each case starts host processes and waits for real groups; the budgets
    // keep a test, shrinking included, within the harness's time limit.
    #![proptest_config(Config {
        cases: 6,
        max_shrink_iters: 12,
        max_shrink_time: 40_000,
        failure_persistence: None,
        ..Config::default()
    })]

    #[test]
    fn a_restart_reports_every_live_group_a_dead_host_left(scenario in scenario(dying_host())) {
        check(&scenario)?;
    }

    #[test]
    fn neither_open_nor_kill_group_signals_a_group_no_dead_host_started(
        scenario in scenario(forged_restart()),
    ) {
        check(&scenario)?;
    }

    #[test]
    fn kill_group_and_a_killed_run_return_once_no_member_of_the_group_runs(
        scenario in scenario(stubborn_kill()),
    ) {
        check(&scenario)?;
    }

    #[test]
    fn a_run_the_pid_file_cannot_record_leaves_no_member_running(
        scenario in scenario(failing_record()),
    ) {
        check(&scenario)?;
    }

    #[test]
    fn a_run_whose_leader_exits_returns_once_no_member_of_its_group_runs(
        scenario in scenario(leader_exits()),
    ) {
        check(&scenario)?;
    }

    #[test]
    fn neither_open_nor_kill_group_signals_a_group_whose_leader_is_gone(
        scenario in scenario(leaderless_group()),
    ) {
        check(&scenario)?;
    }

    #[test]
    fn a_dropped_run_leaves_no_member_of_its_group_running_and_no_trace_in_the_runner(
        scenario in scenario(dropped_run()),
    ) {
        check(&scenario)?;
    }

    #[test]
    fn a_run_whose_wait_fails_returns_the_error_with_its_group_dead(
        scenario in scenario(reaped_run()),
    ) {
        check(&scenario)?;
    }

    #[test]
    fn a_second_runner_on_a_held_pid_file_is_refused_and_changes_nothing(
        scenario in scenario(second_runner()),
    ) {
        check(&scenario)?;
    }
}

/// How a caller spells the path of a pid file in the shared directory.
#[derive(Debug, Clone, Copy)]
enum Spelling {
    Plain,
    DotSegment,
    ParentSegment,
    LinkedDir,
}

impl Spelling {
    fn path(self, dir: &Path, name: &str) -> PathBuf {
        match self {
            Self::Plain => dir.join(name),
            Self::DotSegment => dir.join(".").join(name),
            Self::ParentSegment => dir.join("sub").join("..").join(name),
            Self::LinkedDir => dir
                .parent()
                .expect("the shared dir has a parent")
                .join("link")
                .join(name),
        }
    }
}

#[derive(Debug, Clone)]
enum NameOp {
    Open(String, Spelling),
    /// A run that exits at once, on a live runner.
    Run(Index),
    Drop(Index),
    /// A directory with this name in the shared directory.
    Occupy(String),
}

/// The file next to a pid file that its runner locks.
fn lock_name(name: &str) -> String {
    format!("{name}.lock")
}

/// The path of a file the runner keeps next to `pid_file`: its name plus
/// `suffix`.
fn beside(pid_file: &Path, suffix: &str) -> PathBuf {
    let mut name = pid_file.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// The file next to a pid file that its runner writes and renames onto it.
fn temp_name(name: &str) -> String {
    format!("{name}.writing")
}

/// A pid file's name must end in `.pid`.
fn accepted(name: &str) -> bool {
    name.ends_with(".pid")
}

/// Names a caller can pass, accepted or not, whose derived neighbours
/// collide: a name that is another's lock, temporary file or spawn record
/// with `.pid` appended, the same stem with another extension, another case.
fn pid_file_name() -> impl Strategy<Value = String> {
    prop_oneof![
        3 => prop::sample::select(vec![
            "a.pid", "A.pid", ".pid", "a.lock.pid", "a.pid.lock.pid", "a.pid.writing.pid",
            "a.pid.spawn-1-2-3.pid", "a.pid.spawn-x.pid", "a.pid.pid",
            "a", "a.x", "a.lock", "a.pid.lock", "a.pid.writing", "a.pid.spawn-1-2-3", "a.pid.spawn-x", "a.pid.spawn-1-2-3.lock", "a.pid.spawn-1-2-3x.pidq", "a.PID", "pid",
        ])
        .prop_map(str::to_owned),
        2 => "[a-cA-C]{1,2}(\\.(pid|lock|writing|spawn-[0-9]-[0-9]-[0-9]|[a-z]{1,4})){0,3}",
        2 => "[a-cA-C]{1,2}(\\.(pid|lock|writing|spawn-[0-9]-[0-9]-[0-9]|[a-z]{1,4})){0,2}"
            .prop_map(|n| n + ".pid"),
    ]
}

fn spelling() -> impl Strategy<Value = Spelling> {
    prop_oneof![
        3 => Just(Spelling::Plain),
        1 => Just(Spelling::DotSegment),
        1 => Just(Spelling::ParentSegment),
        1 => Just(Spelling::LinkedDir),
    ]
}

fn name_op() -> impl Strategy<Value = NameOp> {
    prop_oneof![
        3 => (pid_file_name(), spelling()).prop_map(|(n, s)| NameOp::Open(n, s)),
        2 => any::<Index>().prop_map(NameOp::Run),
        1 => any::<Index>().prop_map(NameOp::Drop),
        1 => prop_oneof![
            pid_file_name(),
            pid_file_name().prop_map(|n| lock_name(&n)),
            pid_file_name().prop_map(|n| temp_name(&n)),
            pid_file_name().prop_map(|n| n + ".spawn-1-2-3"),
            (pid_file_name(), "[0-9x-]{0,3}\\.pid[a-z.]{0,3}")
                .prop_map(|(n, tail)| format!("{n}.spawn-{tail}")),
        ]
        .prop_map(NameOp::Occupy),
    ]
}

fn entries(dir: &Path) -> BTreeSet<std::ffi::OsString> {
    std::fs::read_dir(dir)
        .expect("the pid dir is readable")
        .map(|e| e.expect("a dir entry").file_name())
        .collect()
}

/// Pid files with distinct names in one directory are independent: each opens
/// next to the others and keeps its runs, while an open of the file a live
/// runner holds, in any spelling, is refused naming the file and this process.
/// An open whose lock or pid file is a directory fails naming that path. A
/// name that does not end in `.pid` is refused naming it, and the open leaves
/// the directory as it was.
fn check_names(ops: &[NameOp]) -> Result<(), TestCaseError> {
    let root = tempfile::tempdir().expect("a temporary dir");
    let dir = root.path().join("pids");
    std::fs::create_dir_all(dir.join("sub")).expect("the pid dir is created");
    std::os::unix::fs::symlink(&dir, root.path().join("link")).expect("the dir link is created");
    std::fs::write(dir.join("case"), "").expect("the case probe is created");
    let fold = dir.join("CASE").exists();
    std::fs::remove_file(dir.join("case")).expect("the case probe is removed");
    // Names the file system holds as one file are one pid file.
    let same = |a: &str, b: &str| {
        if fold {
            a.eq_ignore_ascii_case(b)
        } else {
            a == b
        }
    };
    let work = root.path().join("work");
    std::fs::create_dir(&work).expect("the work dir is created");
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("a runtime");
    let mut live: Vec<(String, Runner)> = Vec::new();
    let mut occupied: Vec<String> = Vec::new();
    let mut attempt = 0u64;
    for op in ops {
        match op {
            NameOp::Open(name, spelling) => {
                let path = spelling.path(&dir, name);
                let before = entries(&dir);
                let opened = Runner::open(path.clone());
                if !accepted(name) {
                    let text = match opened {
                        Ok(_) => {
                            return Err(TestCaseError::fail(format!(
                                "an open of {} succeeded, though its name does not end in .pid",
                                path.display()
                            )));
                        }
                        Err(e) => e.to_string(),
                    };
                    prop_assert!(
                        text.contains(&path.display().to_string()),
                        "the refusal of {} must name it: {text}",
                        path.display()
                    );
                    let after = entries(&dir);
                    prop_assert_eq!(
                        &before,
                        &after,
                        "the refused open of {} changed the directory",
                        path.display()
                    );
                    continue;
                }
                let held = live.iter().any(|(n, _)| same(n, name));
                let lock = lock_name(name);
                let temp = temp_name(name);
                // An entry `<name>.spawn-<tail>` is this pid file's own unless its tail names
                // another pid file.
                let records = occupied.iter().filter(|o| {
                    o.strip_prefix(&format!("{name}.spawn-"))
                        .is_some_and(|tail| !tail.contains(".pid"))
                });
                let blocked = [&lock, name]
                    .into_iter()
                    .chain(records)
                    .chain([&temp])
                    .find(|n| occupied.iter().any(|o| same(o, n)));
                if let Some(blocked) = blocked.filter(|n| *n == &lock || !held) {
                    let text = match opened {
                        Ok(_) => {
                            return Err(TestCaseError::fail(format!(
                                "an open of {} succeeded while {blocked} is a directory",
                                path.display()
                            )));
                        }
                        Err(e) => e.to_string(),
                    };
                    let roots = [
                        root.path().to_path_buf(),
                        root.path().canonicalize().expect("the root resolves"),
                    ];
                    let outside_paths = roots
                        .iter()
                        .fold(text.clone(), |t, r| t.replace(&r.display().to_string(), ""));
                    prop_assert!(
                        text.to_lowercase().contains(&blocked.to_lowercase())
                            && !outside_paths.contains(&std::process::id().to_string()),
                        "an open of {} must fail naming the directory {blocked}, not a holder: {text}",
                        path.display()
                    );
                    continue;
                }
                match (opened, held) {
                    (Ok((runner, left)), false) => {
                        prop_assert!(
                            left.is_empty(),
                            "an open of {} reported groups no run left: {left:?}",
                            path.display()
                        );
                        live.push((name.clone(), runner));
                    }
                    (Ok(_), true) => {
                        return Err(TestCaseError::fail(format!(
                            "an open of {} succeeded while a live runner holds it",
                            path.display()
                        )));
                    }
                    (Err(e), true) => {
                        let text = e.to_string();
                        prop_assert!(
                            text.contains(name.as_str())
                                && text.contains(&std::process::id().to_string()),
                            "the refusal of an open of the held {} must name it and this process: {text}",
                            path.display()
                        );
                    }
                    (Err(e), false) => {
                        return Err(TestCaseError::fail(format!(
                            "an open of {} next to the live runners on {:?} failed: {e}",
                            path.display(),
                            live.iter().map(|(n, _)| n).collect::<Vec<_>>()
                        )));
                    }
                }
            }
            NameOp::Run(index) if !live.is_empty() => {
                let (name, runner) = index.get(&live);
                attempt += 1;
                let effect = restart_support::descriptor("exit 0", &work, &work, None);
                let ended = rt
                    .block_on(runner.run(&effect, &restart_support::request(attempt, "t", "s")))
                    .map(|f| f.ended);
                let temp = temp_name(name);
                if occupied.iter().any(|o| same(o, &temp)) {
                    prop_assert!(
                        ended.as_ref().is_err_and(|e| e.to_string().contains(&temp)),
                        "a run on the runner of {name} must fail naming the directory {temp}: {ended:?}"
                    );
                } else {
                    prop_assert!(
                        matches!(ended, Ok(Ended::Exited { code: 0, .. })),
                        "a run on the runner of {name} must exit 0: {ended:?}"
                    );
                }
                prop_assert!(
                    runner.in_flight().is_empty(),
                    "the runner of {name} lists an ended run"
                );
            }
            NameOp::Drop(index) if !live.is_empty() => {
                live.remove(index.index(live.len()));
            }
            NameOp::Run(_) | NameOp::Drop(_) => {}
            NameOp::Occupy(entry) => {
                let in_use = live
                    .iter()
                    .any(|(n, _)| same(n, entry) || same(&lock_name(n), entry));
                if !in_use && std::fs::symlink_metadata(dir.join(entry)).is_err() {
                    std::fs::create_dir(dir.join(entry)).expect("the directory is created");
                    occupied.push(entry.clone());
                }
            }
        }
    }
    Ok(())
}

#[test]
fn pid_files_whose_derived_names_collide_hold_every_property() {
    let open = |name: &str| NameOp::Open(name.to_owned(), Spelling::Plain);
    let run = || NameOp::Run(first());
    let occupy = |name: &str| NameOp::Occupy(name.to_owned());
    let fixed: Vec<(&str, Vec<NameOp>)> = vec![
        (
            "the same stem with another extension",
            vec![
                open("a.pid"),
                open("a.x"),
                open("a.pid.pid"),
                run(),
                open("a.pid"),
            ],
        ),
        (
            "a pid file named as another's lock",
            vec![open("a.pid"), open("a.pid.lock.pid"), run(), open("a.pid")],
        ),
        (
            "a pid file named as another's temporary file",
            vec![
                open("a.pid"),
                open("a.pid.writing.pid"),
                run(),
                open("a.pid.writing.pid"),
            ],
        ),
        (
            "a pid file named as another's spawn record",
            vec![
                open("a.pid.spawn-1-2-3.pid"),
                open("a.pid"),
                run(),
                open("a.pid"),
            ],
        ),
        (
            "a name that ends in .lock",
            vec![open("a.pid"), open("a.pid.lock"), run(), open("a.pid")],
        ),
        (
            "a name whose lock is the next name",
            vec![open("a.lock"), open("a.lock.lock")],
        ),
        (
            "a second spelling of a held file",
            vec![
                open("a.pid"),
                NameOp::Open("a.pid".to_owned(), Spelling::LinkedDir),
            ],
        ),
        (
            "a lock path that is a directory",
            vec![occupy("a.pid.lock"), open("a.pid")],
        ),
        (
            "a pid file path that is a directory",
            vec![occupy("a.pid"), open("a.pid")],
        ),
        (
            "a temporary path that is a directory",
            vec![
                occupy("a.pid.writing"),
                open("a.pid"),
                open("b.pid"),
                occupy("b.pid.writing"),
                run(),
            ],
        ),
        (
            "a spawn record path that is a directory",
            vec![occupy("a.pid.spawn-1-2-3"), open("a.pid")],
        ),
        (
            "a spawn-record-like entry of no record form",
            vec![
                occupy("a.pid.spawn-x"),
                open("a.pid"),
                occupy("b.pid.spawn-1-2-3.lock"),
                open("b.pid"),
            ],
        ),
        (
            "a spawn-record-like entry with .pid inside its tail",
            vec![occupy("a.pid.spawn-1-2-3x.pidq"), open("a.pid"), run()],
        ),
        (
            "a name that is only the extension",
            vec![open(".pid"), open("pid"), open("a.PID")],
        ),
    ];
    let failed: Vec<String> = fixed
        .iter()
        .filter_map(|(what, ops)| check_names(ops).err().map(|e| format!("{what}: {e}")))
        .collect();
    assert!(failed.is_empty(), "{}", failed.join("\n\n"));
}

proptest! {
    #![proptest_config(Config { cases: 32, failure_persistence: None, ..Config::default() })]

    #[test]
    fn pid_files_with_distinct_names_in_one_directory_are_independent(
        ops in prop::collection::vec(name_op(), 1..=10),
    ) {
        check_names(&ops)?;
    }
}
