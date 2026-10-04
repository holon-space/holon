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
//! A watch that fails raises [`ConditionKind::DatabaseWatchFailed`]. The
//! watchdog thread is not restarted when it stops.
//!
//! wasm32-unknown-unknown has no threads, so there is no watchdog there.
//!
//! [`DbHandle`]: crate::turso::DbHandle

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::io::Write as _;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
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
const COPIED_SQL_BYTES: usize = 4096;
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
    /// The condition subject, one per actor in the process.
    subject: String,
    bound: Duration,
    /// Weak, so the watch never keeps the actor's queue open.
    queue: mpsc::WeakSender<Envelope>,
    commands: Mutex<Commands>,
    bus: OnceLock<Arc<ConditionBus>>,
    /// Held only to copy it out and to store it back, so no panic poisons it.
    disclosure: Mutex<Disclosure>,
    /// Why the watch failed. Set at most once.
    failed: Mutex<Option<String>>,
    #[cfg(test)]
    after_read: Mutex<Option<Box<dyn FnOnce(&ActorWatch) + Send>>>,
}

struct Commands {
    next_seq: u64,
    running: Option<Running>,
    /// The running command's statements, up to [`REPORTED_STATEMENTS`]. The
    /// buffers are reused, so a command start allocates nothing once warm.
    statements: Vec<CopiedSql>,
    statement_count: usize,
    recent: VecDeque<Finished>,
    longest: Duration,
}

#[derive(Clone)]
struct CopiedSql {
    bytes: usize,
    /// At most [`COPIED_SQL_BYTES`]. Only a prefix: redaction lexes from the
    /// start, and a tail can begin inside a string literal.
    head: String,
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
#[derive(Default, Clone, Copy)]
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
        let watch = Self::new(bound_from_env(), tx);
        watchdog::watch(&watch);
        watch
    }

    fn new(bound: Duration, tx: &ActorSender) -> Arc<Self> {
        static NEXT_ACTOR: AtomicU64 = AtomicU64::new(1);
        Arc::new(Self {
            subject: format!(
                "{DATABASE_SUBJECT}#{}",
                NEXT_ACTOR.fetch_add(1, Ordering::Relaxed)
            ),
            bound,
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
            failed: Mutex::new(None),
            #[cfg(test)]
            after_read: Mutex::new(None),
        })
    }

    /// Raise and clear this actor's conditions on `bus` from now on,
    /// including for a command already past the bound and a watch that
    /// already failed.
    pub(crate) fn disclose_on(&self, bus: Arc<ConditionBus>) {
        if let Err(other) = self.bus.set(bus) {
            assert!(
                Arc::ptr_eq(self.bus.get().expect("set failed, so it is set"), &other),
                "the SQL actor already discloses on another ConditionBus"
            );
        }
        // `fail` records its cause before it reads the bus, so one of the two
        // sees the other.
        let failed = self
            .failed
            .lock()
            .expect("actor watch failure poisoned")
            .clone();
        if let Some(cause) = failed {
            self.bus
                .get()
                .expect("set above")
                .emit(self.watch_failed(cause));
        }
    }

    /// Disclose that this watch failed: log it once, and raise
    /// [`ConditionKind::DatabaseWatchFailed`] now or when a bus is attached.
    /// Never panics: the watchdog fails watches when its own code panicked,
    /// so a panicking subscriber or bus must not stop the other disclosure.
    fn fail(&self, cause: String) {
        let bus = {
            let mut failed = self.failed.lock().expect("actor watch failure poisoned");
            if failed.is_some() {
                return;
            }
            *failed = Some(cause.clone());
            self.bus.get().cloned()
        };
        let logged = std::panic::catch_unwind(AssertUnwindSafe(|| {
            tracing::error!(
                target: "holon_actor_watch",
                subject = self.subject,
                "the SQL actor watch failed: {cause}"
            )
        }));
        let raised = bus.map(|bus| {
            std::panic::catch_unwind(AssertUnwindSafe(|| {
                bus.emit(self.watch_failed(cause.clone()))
            }))
        });
        if logged.is_err() || matches!(raised, Some(Err(_))) {
            let _ = writeln!(
                std::io::stderr(),
                "the SQL actor watch {} failed: {cause} (disclosing it panicked)",
                self.subject
            );
        }
    }

    fn watch_failed(&self, cause: String) -> Condition {
        Condition {
            subject: self.subject.clone(),
            reason: ConditionKind::DatabaseWatchFailed { cause },
        }
    }

    fn commands(&self) -> MutexGuard<'_, Commands> {
        self.commands.lock().unwrap_or_else(|_| {
            self.fail("its command record is poisoned, so every SQL command on it fails".into());
            panic!("actor watch poisoned");
        })
    }

    /// Watches the command of `envelope` until the returned guard drops.
    pub(crate) fn watching(&self, envelope: &Envelope) -> Watching<'_> {
        self.begin(envelope);
        Watching(self)
    }

    fn begin(&self, envelope: &Envelope) {
        let mut c = self.commands();
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

    fn end(&self) {
        let new_longest = {
            let mut c = self.commands();
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
                let first = &c.statements[0].head;
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
            let c = self.commands();
            c.running
                .as_ref()
                .map(|r| (r.seq, r.kind, r.started.elapsed()))
        };
        #[cfg(test)]
        if let Some(hook) = self.after_read.lock().expect("hook poisoned").take() {
            hook(self);
        }
        let mut d = *self
            .disclosure
            .lock()
            .expect("actor watch disclosure poisoned");
        let ended = (now.map(|(seq, ..)| seq) != Some(d.seq)).then(|| {
            std::mem::replace(
                &mut d,
                match now {
                    Some((seq, kind, _)) => Disclosure {
                        seq,
                        kind,
                        next_report: FIRST_WARNING.min(self.bound),
                        past_bound: false,
                        on_bus: false,
                    },
                    None => Disclosure::default(),
                },
            )
        });
        let due = now.and_then(|(seq, kind, running)| self.due(&mut d, seq, kind, running));
        *self
            .disclosure
            .lock()
            .expect("actor watch disclosure poisoned") = d;

        if let Some(ended) = ended {
            self.end_disclosure(&ended);
        }
        let Some(due) = due else {
            return;
        };
        if due.raise {
            self.raise(due.kind, due.running, due.report.clone());
        }
        let _in_caller = due.caller.enter();
        match due.log {
            Some(Level::Error) => tracing::error!(target: "holon_actor_watch", "{}", due.report),
            Some(Level::Warn) => tracing::warn!(target: "holon_actor_watch", "{}", due.report),
            None => {}
        }
    }

    /// What this tick owes command `seq`, with `d` moved past it. `None` when
    /// nothing is due, or `seq` no longer runs: a command that finished since
    /// the read has its disclosure ended on the next tick.
    fn due(
        &self,
        d: &mut Disclosure,
        seq: u64,
        kind: &'static str,
        running: Duration,
    ) -> Option<Due> {
        let log_due = running >= d.next_report;
        let past_bound = d.past_bound || (log_due && running >= self.bound);
        let raise = past_bound && self.bus.get().is_some() && (log_due || !d.on_bus);
        if !log_due && !raise {
            return None;
        }
        let (report, caller) = self.report(seq, running)?;
        if log_due {
            d.next_report = if d.next_report < self.bound {
                (d.next_report * 2).min(self.bound)
            } else {
                d.next_report * 2
            };
        }
        d.past_bound = past_bound;
        d.on_bus |= raise;
        Some(Due {
            kind,
            running,
            report,
            caller,
            log: log_due.then_some(if running >= self.bound {
                Level::Error
            } else {
                Level::Warn
            }),
            raise,
        })
    }

    fn raise(&self, kind: &'static str, running: Duration, report: String) {
        self.bus
            .get()
            .expect("raised only once a bus is attached")
            .emit(Condition {
                subject: self.subject.clone(),
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
            let c = self.commands();
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
            self.clear();
        }
    }

    fn clear(&self) {
        self.bus
            .get()
            .expect("on_bus implies an attached bus")
            .clear(
                &Condition {
                    subject: self.subject.clone(),
                    reason: ConditionKind::DatabaseStuck {
                        command: String::new(),
                        running_secs: 0,
                        report: String::new(),
                    },
                }
                .condition_key(),
            );
    }

    /// The report on command `seq`, with every SQL literal and comment
    /// blanked, and the span of the code that sent it. `None` once `seq` no
    /// longer runs.
    fn report(&self, seq: u64, running: Duration) -> Option<(String, tracing::Span)> {
        let queued = self
            .queue
            .upgrade()
            .map(|tx| tx.max_capacity() - tx.capacity());
        let (kind, waited, caller, statement_count, statements, recent) = {
            let c = self.commands();
            let r = c.running.as_ref().filter(|r| r.seq == seq)?;
            (
                r.kind,
                r.started.duration_since(r.enqueued),
                r.caller.clone(),
                c.statement_count,
                c.statements[..c.statement_count.min(REPORTED_STATEMENTS)].to_vec(),
                c.recent
                    .iter()
                    .map(|f| (f.kind, f.took, f.head.clone()))
                    .collect::<Vec<_>>(),
            )
        };
        let mut out = String::new();
        let _ = writeln!(
            out,
            "SQL actor stuck: a `{kind}` command has run for {running:?} (bound {:?}). It waited \
             {waited:?} in the queue. Queued behind it: {}.",
            self.bound,
            queued.map_or_else(
                || "none, every sender is gone".to_string(),
                |n| n.to_string()
            ),
        );
        let _ = writeln!(out, "Statements: {statement_count}");
        for (i, sql) in statements.iter().enumerate() {
            let copied = if sql.head.len() < sql.bytes {
                format!(", first {} copied", sql.head.len())
            } else {
                String::new()
            };
            let _ = writeln!(
                out,
                "  [{i}] ({} bytes{copied}) {}",
                sql.bytes,
                redact(&sql.head)
            );
        }
        let _ = writeln!(out, "Last {} commands, oldest first:", recent.len());
        for (kind, took, head) in &recent {
            let _ = writeln!(out, "  {kind} {took:?}: {}", redact(head));
        }
        Some((out, caller))
    }
}

struct Due {
    kind: &'static str,
    running: Duration,
    report: String,
    caller: tracing::Span,
    log: Option<Level>,
    raise: bool,
}

enum Level {
    Warn,
    Error,
}

/// Ends its command's watch when dropped, also when the command panics.
pub(crate) struct Watching<'a>(&'a ActorWatch);

impl Drop for Watching<'_> {
    fn drop(&mut self) {
        self.0.end();
    }
}

impl Drop for ActorWatch {
    fn drop(&mut self) {
        let d = std::mem::take(
            self.disclosure
                .get_mut()
                .expect("actor watch disclosure poisoned"),
        );
        if !d.past_bound {
            return;
        }
        tracing::warn!(
            target: "holon_actor_watch",
            kind = d.kind,
            "the SQL actor stopped while its stuck command still ran"
        );
        if d.on_bus {
            self.clear();
        }
    }
}

fn redact(sql: &str) -> String {
    crate::turso::sql_fingerprint(&crate::turso::blank_comments_and_string_literals(sql))
}

/// Copies the command's SQL into `into`, up to [`REPORTED_STATEMENTS`]
/// statements, and returns how many statements it has.
fn copy_statements(cmd: &DbCommand, into: &mut Vec<CopiedSql>) -> usize {
    let mut count = 0;
    let mut copy = |sql: &str| {
        if count < REPORTED_STATEMENTS {
            if into.len() == count {
                into.push(CopiedSql {
                    bytes: 0,
                    head: String::with_capacity(COPIED_SQL_BYTES),
                });
            }
            let copied = &mut into[count];
            copied.bytes = sql.len();
            copied.head.clear();
            copied
                .head
                .push_str(&sql[..floor_char_boundary(sql, COPIED_SQL_BYTES)]);
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
    use std::convert::Infallible;
    use std::sync::PoisonError;
    use std::sync::Weak;

    use super::*;

    /// The actors the watchdog thread watches.
    pub(super) struct Registry {
        watched: Mutex<Vec<Weak<ActorWatch>>>,
        /// Why the watchdog thread does not run.
        stopped: OnceLock<String>,
    }

    impl Registry {
        pub(super) const fn new() -> Self {
            Self {
                watched: Mutex::new(Vec::new()),
                stopped: OnceLock::new(),
            }
        }

        /// A panic under the `watched` lock stops the registry, which still
        /// fails the watches it holds and those added later.
        pub(super) fn add(&self, watch: &Arc<ActorWatch>) {
            self.watched
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Arc::downgrade(watch));
        }

        /// Fails `watch` if the registry has stopped. `stop` sets the cause
        /// before it reads the watches, so a watch added after that read sees
        /// it.
        pub(super) fn fail_if_stopped(&self, watch: &ActorWatch) {
            if let Some(cause) = self.stopped.get() {
                watch.fail(not_disclosed(cause));
            }
        }

        /// Fails every watch, those added later included.
        pub(super) fn stop(&self, cause: String) {
            let cause = self.stopped.get_or_init(|| cause);
            let watched: Vec<Arc<ActorWatch>> = self
                .watched
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .filter_map(Weak::upgrade)
                .collect();
            for watch in watched {
                watch.fail(not_disclosed(cause));
            }
        }
    }

    static REGISTRY: Registry = Registry::new();
    /// The watchdog thread, `None` when it did not start.
    static STARTED: OnceLock<Option<std::thread::Thread>> = OnceLock::new();

    pub(super) fn watch(watch: &Arc<ActorWatch>) {
        REGISTRY.add(watch);
        let started = STARTED.get_or_init(|| {
            match std::thread::Builder::new()
                .name("sql-actor-watchdog".into())
                .spawn(run)
            {
                Ok(thread) => Some(thread.thread().clone()),
                Err(e) => {
                    REGISTRY.stop(format!("the watchdog thread did not start ({e})"));
                    None
                }
            }
        });
        // A new watch can have a shorter tick than the one the thread sleeps.
        if let Some(thread) = started {
            thread.unpark();
        }
        REGISTRY.fail_if_stopped(watch);
    }

    fn not_disclosed(cause: &str) -> String {
        format!("{cause}, so a stuck SQL command is not disclosed")
    }

    #[cfg(test)]
    pub(super) fn poison_registry() {
        std::thread::scope(|s| {
            let poisoner = s.spawn(|| {
                let _held = REGISTRY.watched.lock().expect("not yet poisoned");
                panic!("poisoning the watchdog registry");
            });
            assert!(poisoner.join().is_err(), "the poisoning thread panics");
        });
    }

    /// Stops the registry after the unwind ends, so no failure is disclosed
    /// while the thread unwinds.
    fn run() {
        let Err(panic) = std::panic::catch_unwind(watch_until_a_panic);
        REGISTRY.stop(format!(
            "the watchdog thread stopped on a panic ({})",
            crate::turso::panic_message(&*panic)
        ));
    }

    fn watch_until_a_panic() -> Infallible {
        loop {
            let live: Vec<Arc<ActorWatch>> = {
                let mut watched = REGISTRY.watched.lock().expect("watched actors poisoned");
                watched.retain(|w| w.strong_count() > 0);
                watched.iter().filter_map(Weak::upgrade).collect()
            };
            for watch in &live {
                if let Err(panic) = std::panic::catch_unwind(AssertUnwindSafe(|| watch.check())) {
                    watch.fail(not_disclosed(&format!(
                        "its watchdog check panicked ({})",
                        crate::turso::panic_message(&*panic)
                    )));
                    REGISTRY
                        .watched
                        .lock()
                        .expect("watched actors poisoned")
                        .retain(|w| !std::ptr::eq(w.as_ptr(), Arc::as_ptr(watch)));
                }
            }
            let tick = live
                .iter()
                .map(|w| w.bound / 5)
                .min()
                .unwrap_or(Duration::from_secs(1))
                .clamp(Duration::from_millis(20), Duration::from_secs(1));
            drop(live);
            std::thread::park_timeout(tick);
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

#[cfg(test)]
impl ActorWatch {
    pub(crate) fn poison(&self) {
        std::thread::scope(|s| {
            let poisoner = s.spawn(|| {
                let _held = self.commands.lock().expect("not yet poisoned");
                panic!("poisoning the actor watch");
            });
            assert!(poisoner.join().is_err(), "the poisoning thread panics");
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUND: Duration = Duration::from_millis(1);

    fn watched() -> (Arc<ActorWatch>, Arc<ConditionBus>, ActorSender) {
        let bus = Arc::new(ConditionBus::new());
        let (watch, tx) = watched_on(&bus);
        (watch, bus, tx)
    }

    fn watched_on(bus: &Arc<ConditionBus>) -> (Arc<ActorWatch>, ActorSender) {
        let (tx, _rx) = ActorSender::channel(4);
        let watch = ActorWatch::new(BOUND, &tx);
        watch.disclose_on(bus.clone());
        (watch, tx)
    }

    fn envelope(sql: &str) -> Envelope {
        Envelope {
            cmd: DbCommand::Execute {
                sql: sql.to_string(),
                params: Vec::new(),
                response: tokio::sync::oneshot::channel().0,
            },
            caller: tracing::Span::none(),
            enqueued: Instant::now(),
        }
    }

    fn run_past_bound() {
        std::thread::sleep(BOUND * 5);
    }

    fn after_read(watch: &ActorWatch, hook: impl FnOnce(&ActorWatch) + Send + 'static) {
        *watch.after_read.lock().expect("hook poisoned") = Some(Box::new(hook));
    }

    /// (command, report) of each `DatabaseStuck` in effect.
    fn stuck(bus: &ConditionBus) -> Vec<(String, String)> {
        bus.current()
            .into_iter()
            .filter_map(|c| match c.reason {
                ConditionKind::DatabaseStuck {
                    command, report, ..
                } => Some((command, report)),
                _ => None,
            })
            .collect()
    }

    fn watch_failures(bus: &ConditionBus) -> Vec<String> {
        bus.current()
            .into_iter()
            .filter_map(|c| match c.reason {
                ConditionKind::DatabaseWatchFailed { cause } => Some(cause),
                _ => None,
            })
            .collect()
    }

    fn wait_until(what: &str, bus: &ConditionBus, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(
                Instant::now() < deadline,
                "{what}; conditions in effect: {:#?}",
                bus.current()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_command_that_finishes_after_the_watchdog_read_it_is_not_reported() {
        let (watch, bus, _tx) = watched();
        watch.begin(&envelope("SELECT 1 AS finishing_command"));
        run_past_bound();
        after_read(&watch, ActorWatch::end);

        watch.check();

        assert_eq!(stuck(&bus), Vec::new());
        watch.begin(&envelope("SELECT 2 AS next_command"));
        watch.end();
    }

    #[test]
    fn a_busy_actor_never_reports_the_next_command_under_the_previous_ones_time() {
        let (watch, bus, _tx) = watched();
        watch.begin(&envelope("SELECT 1 AS first_command"));
        run_past_bound();
        after_read(&watch, |w| {
            w.end();
            w.begin(&envelope("SELECT 2 AS second_command"));
        });

        watch.check();

        let reports = stuck(&bus);
        assert!(
            reports.iter().all(|(_, r)| !r.contains("second_command")),
            "the report names the command that started after the one whose time it gives: \
             {reports:#?}"
        );
    }

    #[test]
    fn a_long_statement_is_copied_up_to_the_cap_and_reported_with_its_full_length() {
        let sql = format!("SELECT '{}' AS long_command", "x".repeat(64 * 1024));
        let (watch, bus, _tx) = watched();
        watch.begin(&envelope(&sql));
        let copied = watch.commands.lock().expect("not poisoned").statements[0]
            .head
            .len();
        assert!(
            copied <= COPIED_SQL_BYTES,
            "begin copied {copied} bytes of a {} byte statement",
            sql.len()
        );
        run_past_bound();

        watch.check();

        let reports = stuck(&bus);
        assert_eq!(reports.len(), 1, "{reports:#?}");
        assert!(
            reports[0].1.contains(&format!("({} bytes", sql.len())),
            "{}",
            reports[0].1
        );
    }

    #[test]
    fn a_watch_dropped_while_its_command_is_stuck_clears_the_condition() {
        let (watch, bus, _tx) = watched();
        watch.begin(&envelope("SELECT 1 AS stuck_command"));
        run_past_bound();
        watch.check();
        assert_eq!(stuck(&bus).len(), 1, "raised before the drop");

        drop(watch);

        assert_eq!(stuck(&bus), Vec::new());
    }

    #[test]
    fn a_poisoned_watch_raises_a_watch_failure_on_its_bus() {
        let (watch, bus, _tx) = watched();
        watch.poison();

        let began = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            watch.begin(&envelope("SELECT 1 AS any_command"))
        }));

        assert!(began.is_err(), "a poisoned watch fails the command");
        assert_eq!(watch_failures(&bus).len(), 1, "{:#?}", bus.current());
    }

    #[test]
    fn a_watch_that_failed_before_its_bus_was_attached_raises_on_attach() {
        let (tx, _rx) = ActorSender::channel(4);
        let watch = ActorWatch::new(BOUND, &tx);
        watch.poison();
        let began = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            watch.begin(&envelope("SELECT 1 AS any_command"))
        }));
        assert!(began.is_err(), "a poisoned watch fails the command");

        let bus = Arc::new(ConditionBus::new());
        watch.disclose_on(bus.clone());

        assert_eq!(watch_failures(&bus).len(), 1, "{:#?}", bus.current());
    }

    #[test]
    fn the_watchdog_outlives_a_watch_whose_check_panics() {
        let bus = Arc::new(ConditionBus::new());
        let (broken, _broken_tx) = watched_on(&bus);
        let (healthy, _healthy_tx) = watched_on(&bus);
        after_read(&broken, |_| panic!("a watchdog check panics"));
        watchdog::watch(&broken);
        watchdog::watch(&healthy);

        healthy.begin(&envelope("SELECT 1 AS stuck_command"));

        wait_until(
            "the healthy actor's stuck command is disclosed, and the broken watch's failure",
            &bus,
            || {
                stuck(&bus)
                    .iter()
                    .any(|(_, report)| report.contains("stuck_command"))
                    && watch_failures(&bus).len() == 1
            },
        );
        healthy.end();
    }

    #[test]
    fn a_stopped_registry_fails_the_watches_it_holds_and_those_added_later() {
        let registry = watchdog::Registry::new();
        let bus = Arc::new(ConditionBus::new());
        let (held, _held_tx) = watched_on(&bus);
        registry.add(&held);

        registry.stop("the watchdog thread stopped on a panic".into());
        let (later, _later_tx) = watched_on(&bus);
        registry.add(&later);
        registry.fail_if_stopped(&later);

        let failures = watch_failures(&bus);
        assert_eq!(failures.len(), 2, "{:#?}", bus.current());
        assert!(
            failures
                .iter()
                .all(|cause| cause.contains("stopped on a panic")
                    && cause.contains("a stuck SQL command is not disclosed")),
            "{failures:#?}"
        );
    }

    struct PanicOnWatchdogEvent;

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for PanicOnWatchdogEvent {
        fn on_event(&self, _: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
            if std::thread::current().name() == Some("sql-actor-watchdog") {
                panic!("the subscriber panics on a watchdog event");
            }
        }
    }

    #[test]
    fn a_watchdog_whose_logging_panics_still_raises_every_condition() {
        use tracing_subscriber::layer::SubscriberExt;
        tracing::subscriber::set_global_default(
            tracing_subscriber::registry().with(PanicOnWatchdogEvent),
        )
        .expect("nextest runs this test in a process of its own");
        let bus = Arc::new(ConditionBus::new());
        let (broken, _broken_tx) = watched_on(&bus);
        let (healthy, _healthy_tx) = watched_on(&bus);
        after_read(&broken, |_| panic!("a watchdog check panics"));
        watchdog::watch(&broken);
        watchdog::watch(&healthy);

        healthy.begin(&envelope("SELECT 1 AS stuck_command"));

        wait_until(
            "the healthy actor's stuck command is disclosed, and both watches' failures",
            &bus,
            || {
                stuck(&bus)
                    .iter()
                    .any(|(_, report)| report.contains("stuck_command"))
                    && watch_failures(&bus).len() == 2
            },
        );
        healthy.end();
    }

    #[test]
    fn a_watchdog_thread_that_panics_fails_every_watch_and_those_added_later() {
        let bus = Arc::new(ConditionBus::new());
        let (held, _held_tx) = watched_on(&bus);
        watchdog::watch(&held);

        watchdog::poison_registry();

        wait_until("the held watch fails", &bus, || {
            watch_failures(&bus).len() == 1
        });
        let (later, _later_tx) = watched_on(&bus);
        watchdog::watch(&later);
        let failures = watch_failures(&bus);
        assert_eq!(failures.len(), 2, "{:#?}", bus.current());
        assert!(
            failures
                .iter()
                .all(|cause| cause.contains("the watchdog thread stopped on a panic")),
            "{failures:#?}"
        );
    }

    struct PanicOnEnter;

    impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for PanicOnEnter {
        fn on_enter(&self, _: &tracing::span::Id, _: tracing_subscriber::layer::Context<'_, S>) {
            panic!("entering the sender's span panics");
        }
    }

    #[test]
    fn a_report_whose_logging_panics_leaves_the_watch_droppable() {
        use tracing_subscriber::layer::SubscriberExt;
        let (watch, _bus, _tx) = watched();
        let subscriber = tracing_subscriber::registry().with(PanicOnEnter);
        tracing::subscriber::with_default(subscriber, || {
            let mut stuck_command = envelope("SELECT 1 AS stuck_command");
            stuck_command.caller = tracing::info_span!("sender");
            watch.begin(&stuck_command);
            run_past_bound();
            let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| watch.check()));
            assert!(checked.is_err(), "the report enters the sender's span");
        });

        drop(watch);
    }

    #[test]
    fn two_stuck_actors_on_one_bus_are_disclosed_and_cleared_apart() {
        let bus = Arc::new(ConditionBus::new());
        let (first, _first_tx) = watched_on(&bus);
        let (second, _second_tx) = watched_on(&bus);
        first.begin(&envelope("SELECT 1 AS stuck_on_first"));
        second.begin(&envelope("SELECT 1 AS stuck_on_second"));
        run_past_bound();
        first.check();
        second.check();
        assert_eq!(stuck(&bus).len(), 2, "{:#?}", bus.current());

        drop(first);

        let left = stuck(&bus);
        assert!(
            left.len() == 1 && left[0].1.contains("stuck_on_second"),
            "{left:#?}"
        );
    }
}
