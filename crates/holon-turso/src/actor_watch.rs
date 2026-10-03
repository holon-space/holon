//! Watches the SQL actor for a command that runs past a bound
//! (`HOLON_ACTOR_HANG_MS`, default 30 s) and discloses it: a log record in the
//! span of the code that sent the command, and
//! [`ConditionKind::DatabaseStuck`] on the bus a [`DbHandle`] attaches.
//!
//! The watchdog is its own OS thread, shared by every actor in the process: a
//! stuck command never yields, so nothing on the actor's runtime can run while
//! it holds that runtime's thread. It only reports. A command inside
//! `turso_core` cannot be stopped safely, and the commands queued behind it
//! already wait.
//!
//! wasm32-unknown-unknown has no threads, so there is no watchdog there.
//!
//! [`DbHandle`]: crate::turso::DbHandle

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::Weak;
use std::time::Duration;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::time::Instant;

use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::condition_bus::DATABASE_SUBJECT;
use tokio::sync::mpsc;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use web_time::Instant;

use crate::turso::DbCommand;

const DEFAULT_BOUND: Duration = Duration::from_secs(30);
/// When the first warning is logged, if the bound is later than that.
const FIRST_WARNING: Duration = Duration::from_secs(5);
const RECENT_COMMANDS: usize = 16;
const RECENT_HEAD_BYTES: usize = 120;
const REPORTED_STATEMENTS: usize = 4;
/// A command at least this long is logged when it is the longest so far.
const LONGEST_WORTH_LOGGING: Duration = Duration::from_millis(250);

/// A command on its way to the actor, with what a report needs to say about
/// where it came from.
pub(crate) struct Envelope {
    pub(crate) cmd: DbCommand,
    caller: tracing::Span,
    enqueued: Instant,
}

/// The sending side of the actor's queue. Its methods take and give back a
/// bare [`DbCommand`], so a caller never builds an [`Envelope`].
#[derive(Clone)]
pub(crate) struct ActorSender(mpsc::Sender<Envelope>);

impl ActorSender {
    pub(crate) fn channel(capacity: usize) -> (Self, mpsc::Receiver<Envelope>) {
        let (tx, rx) = mpsc::channel(capacity);
        (Self(tx), rx)
    }

    fn envelope(cmd: DbCommand) -> Envelope {
        Envelope {
            cmd,
            caller: tracing::Span::current(),
            enqueued: Instant::now(),
        }
    }

    pub(crate) async fn send(
        &self,
        cmd: DbCommand,
    ) -> Result<(), mpsc::error::SendError<DbCommand>> {
        self.0
            .send(Self::envelope(cmd))
            .await
            .map_err(|e| mpsc::error::SendError(e.0.cmd))
    }

    pub(crate) fn try_send(
        &self,
        cmd: DbCommand,
    ) -> Result<(), mpsc::error::TrySendError<DbCommand>> {
        self.0.try_send(Self::envelope(cmd)).map_err(|e| match e {
            mpsc::error::TrySendError::Full(env) => mpsc::error::TrySendError::Full(env.cmd),
            mpsc::error::TrySendError::Closed(env) => mpsc::error::TrySendError::Closed(env.cmd),
        })
    }
}

/// What the actor is running now and ran last, read by the watchdog.
pub(crate) struct ActorWatch {
    bound: Duration,
    /// Weak, so the watch never keeps the actor's queue open.
    queue: mpsc::WeakSender<Envelope>,
    commands: Mutex<Commands>,
    bus: OnceLock<Arc<ConditionBus>>,
    disclosure: Mutex<Disclosure>,
}

struct Commands {
    next_seq: u64,
    running: Option<Running>,
    /// The running command's statements, up to [`REPORTED_STATEMENTS`]. The
    /// buffers are reused, so a command start allocates nothing once warm.
    statements: Vec<String>,
    statement_count: usize,
    recent: VecDeque<Finished>,
    longest: Duration,
}

struct Running {
    seq: u64,
    kind: &'static str,
    caller: tracing::Span,
    enqueued: Instant,
    started: Instant,
}

struct Finished {
    seq: u64,
    kind: &'static str,
    took: Duration,
    head: String,
}

/// The watchdog's view of one running command.
#[derive(Default)]
struct Disclosure {
    seq: u64,
    kind: &'static str,
    next_report: Duration,
    past_bound: bool,
    on_bus: bool,
}

impl ActorWatch {
    /// A watch over the actor whose queue `tx` feeds, bounded by
    /// `HOLON_ACTOR_HANG_MS`, already watched by the process's watchdog.
    pub(crate) fn start(tx: &ActorSender) -> Arc<Self> {
        let watch = Arc::new(Self {
            bound: bound_from_env(),
            queue: tx.0.downgrade(),
            commands: Mutex::new(Commands {
                next_seq: 1,
                running: None,
                statements: Vec::new(),
                statement_count: 0,
                recent: VecDeque::with_capacity(RECENT_COMMANDS),
                longest: Duration::ZERO,
            }),
            bus: OnceLock::new(),
            disclosure: Mutex::new(Disclosure::default()),
        });
        watchdog::watch(&watch);
        watch
    }

    /// Raise and clear [`ConditionKind::DatabaseStuck`] on `bus` from now on,
    /// including for a command already past the bound.
    pub(crate) fn disclose_on(&self, bus: Arc<ConditionBus>) {
        if let Err(other) = self.bus.set(bus) {
            assert!(
                Arc::ptr_eq(self.bus.get().expect("set failed, so it is set"), &other),
                "the SQL actor already discloses on another ConditionBus"
            );
        }
    }

    pub(crate) fn begin(&self, envelope: &Envelope) {
        let mut c = self.commands.lock().expect("actor watch poisoned");
        let seq = c.next_seq;
        c.next_seq += 1;
        let Commands {
            statements,
            statement_count,
            ..
        } = &mut *c;
        *statement_count = copy_statements(&envelope.cmd, statements);
        c.running = Some(Running {
            seq,
            kind: crate::turso_actor_stats::cmd_fingerprint(&envelope.cmd).0,
            caller: envelope.caller.clone(),
            enqueued: envelope.enqueued,
            started: Instant::now(),
        });
    }

    pub(crate) fn end(&self) {
        let new_longest = {
            let mut c = self.commands.lock().expect("actor watch poisoned");
            let running = c
                .running
                .take()
                .expect("the actor ends only a command it began");
            let took = running.started.elapsed();
            let mut finished = if c.recent.len() == RECENT_COMMANDS {
                c.recent.pop_front().expect("the ring is full")
            } else {
                Finished {
                    seq: 0,
                    kind: "",
                    took: Duration::ZERO,
                    head: String::with_capacity(RECENT_HEAD_BYTES),
                }
            };
            finished.seq = running.seq;
            finished.kind = running.kind;
            finished.took = took;
            finished.head.clear();
            if c.statement_count > 0 {
                let first = &c.statements[0];
                finished
                    .head
                    .push_str(&first[..floor_char_boundary(first, RECENT_HEAD_BYTES)]);
            }
            c.recent.push_back(finished);
            (took > c.longest).then(|| {
                c.longest = took;
                (running.kind, took)
            })
        };
        if let Some((kind, took)) = new_longest.filter(|(_, t)| *t >= LONGEST_WORTH_LOGGING) {
            tracing::info!(
                target: "holon_actor_watch",
                kind,
                ms = took.as_millis() as u64,
                "longest SQL actor command so far"
            );
        }
    }

    /// One watchdog tick: report the running command at each threshold, and
    /// clear the disclosure of a command that has finished.
    fn check(&self) {
        let now = {
            let c = self.commands.lock().expect("actor watch poisoned");
            c.running
                .as_ref()
                .map(|r| (r.seq, r.kind, r.started.elapsed()))
        };
        let mut d = self
            .disclosure
            .lock()
            .expect("actor watch disclosure poisoned");
        if now.map(|(seq, ..)| seq) != Some(d.seq) {
            self.end_disclosure(&d);
            *d = match now {
                Some((seq, kind, _)) => Disclosure {
                    seq,
                    kind,
                    next_report: FIRST_WARNING.min(self.bound),
                    past_bound: false,
                    on_bus: false,
                },
                None => Disclosure::default(),
            };
        }
        let Some((_, kind, running)) = now else {
            return;
        };
        if running >= d.next_report {
            d.next_report = if d.next_report < self.bound {
                (d.next_report * 2).min(self.bound)
            } else {
                d.next_report * 2
            };
            let (report, caller) = self.report(running);
            let _in_caller = caller.enter();
            if running >= self.bound {
                d.past_bound = true;
                tracing::error!(target: "holon_actor_watch", "{report}");
                if d.on_bus {
                    self.raise(kind, running, report);
                }
            } else {
                tracing::warn!(target: "holon_actor_watch", "{report}");
            }
        }
        if d.past_bound && !d.on_bus && self.bus.get().is_some() {
            let (report, _) = self.report(running);
            self.raise(kind, running, report);
            d.on_bus = true;
        }
    }

    fn raise(&self, kind: &'static str, running: Duration, report: String) {
        self.bus
            .get()
            .expect("raised only once a bus is attached")
            .emit(Condition {
                subject: DATABASE_SUBJECT.to_string(),
                reason: ConditionKind::DatabaseStuck {
                    command: kind.to_string(),
                    running_secs: running.as_secs(),
                    report,
                },
            });
    }

    fn end_disclosure(&self, d: &Disclosure) {
        if !d.past_bound {
            return;
        }
        let took = {
            let c = self.commands.lock().expect("actor watch poisoned");
            c.recent.iter().find(|f| f.seq == d.seq).map(|f| f.took)
        };
        tracing::warn!(
            target: "holon_actor_watch",
            kind = d.kind,
            "the stuck SQL actor command finished after {}",
            took.map_or_else(
                || format!("more than {:?} (it left the recent ring)", self.bound),
                |t| format!("{t:?}"),
            )
        );
        if d.on_bus {
            let bus = self.bus.get().expect("on_bus implies an attached bus");
            bus.clear(
                &Condition {
                    subject: DATABASE_SUBJECT.to_string(),
                    reason: ConditionKind::DatabaseStuck {
                        command: String::new(),
                        running_secs: 0,
                        report: String::new(),
                    },
                }
                .condition_key(),
            );
        }
    }

    /// The report on the running command, with every SQL literal and comment
    /// blanked, and the span of the code that sent it.
    fn report(&self, running: Duration) -> (String, tracing::Span) {
        let queued = self
            .queue
            .upgrade()
            .map(|tx| tx.max_capacity() - tx.capacity());
        let c = self.commands.lock().expect("actor watch poisoned");
        let r = c
            .running
            .as_ref()
            .expect("a report is made only on a running command");
        let mut out = String::new();
        let _ = writeln!(
            out,
            "SQL actor stuck: a `{}` command has run for {running:?} (bound {:?}). It waited {:?} \
             in the queue. Queued behind it: {}.",
            r.kind,
            self.bound,
            r.started.duration_since(r.enqueued),
            queued.map_or_else(
                || "none, every sender is gone".to_string(),
                |n| n.to_string()
            ),
        );
        let _ = writeln!(out, "Statements: {}", c.statement_count);
        for (i, sql) in c.statements.iter().take(c.statement_count).enumerate() {
            let _ = writeln!(out, "  [{i}] ({} bytes) {}", sql.len(), redact(sql));
        }
        let _ = writeln!(out, "Last {} commands, oldest first:", c.recent.len());
        for f in &c.recent {
            let _ = writeln!(out, "  {} {:?}: {}", f.kind, f.took, redact(&f.head));
        }
        (out, r.caller.clone())
    }
}

fn redact(sql: &str) -> String {
    crate::turso::sql_fingerprint(&crate::turso::blank_comments_and_string_literals(sql))
}

/// Copies the command's SQL into `into`, up to [`REPORTED_STATEMENTS`]
/// statements, and returns how many statements it has.
fn copy_statements(cmd: &DbCommand, into: &mut Vec<String>) -> usize {
    let mut count = 0;
    let mut copy = |sql: &str| {
        if count < REPORTED_STATEMENTS {
            if into.len() == count {
                into.push(String::new());
            }
            into[count].clear();
            into[count].push_str(sql);
        }
        count += 1;
    };
    match cmd {
        DbCommand::Transaction { statements, .. } => {
            for (sql, _) in statements {
                copy(sql);
            }
        }
        other => {
            if let (_, Some(sql)) = crate::turso_actor_stats::cmd_fingerprint(other) {
                copy(sql);
            }
        }
    }
    count
}

fn floor_char_boundary(s: &str, max: usize) -> usize {
    let mut i = max.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn bound_from_env() -> Duration {
    match std::env::var("HOLON_ACTOR_HANG_MS") {
        Ok(ms) => Duration::from_millis(
            ms.parse()
                .expect("HOLON_ACTOR_HANG_MS must be a u64 millisecond count"),
        ),
        Err(_) => DEFAULT_BOUND,
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod watchdog {
    use super::*;

    static WATCHED: Mutex<Vec<Weak<ActorWatch>>> = Mutex::new(Vec::new());
    static STARTED: OnceLock<()> = OnceLock::new();

    pub(super) fn watch(watch: &Arc<ActorWatch>) {
        WATCHED
            .lock()
            .expect("watched actors poisoned")
            .push(Arc::downgrade(watch));
        STARTED.get_or_init(|| {
            if let Err(e) = std::thread::Builder::new()
                .name("sql-actor-watchdog".into())
                .spawn(run)
            {
                tracing::error!(
                    target: "holon_actor_watch",
                    "the SQL actor watchdog thread did not start ({e}); a stuck SQL command will \
                     not be disclosed"
                );
            }
        });
    }

    fn run() {
        loop {
            let live: Vec<Arc<ActorWatch>> = {
                let mut watched = WATCHED.lock().expect("watched actors poisoned");
                watched.retain(|w| w.strong_count() > 0);
                watched.iter().filter_map(Weak::upgrade).collect()
            };
            for watch in &live {
                watch.check();
            }
            let tick = live
                .iter()
                .map(|w| w.bound / 5)
                .min()
                .unwrap_or(Duration::from_secs(1))
                .clamp(Duration::from_millis(20), Duration::from_secs(1));
            drop(live);
            std::thread::sleep(tick);
        }
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
mod watchdog {
    use super::*;

    static DISCLOSED: OnceLock<()> = OnceLock::new();

    pub(super) fn watch(_: &Arc<ActorWatch>) {
        DISCLOSED.get_or_init(|| {
            tracing::warn!(
                target: "holon_actor_watch",
                "no SQL actor watchdog on wasm32-unknown-unknown: it has no threads, and a stuck \
                 command holds the only one, so a stuck SQL command is not disclosed"
            );
        });
    }
}
