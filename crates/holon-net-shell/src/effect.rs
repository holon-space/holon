//! The shell effect of a net file (`docs/Architecture/NetFileFormat.md`): one
//! attempt is one child process group, its output is one log file, and its
//! exit code is one outcome.
//!
//! The runner owns the group, not just the child it spawned, so a kill reaches
//! what the child started as well. A killed child has no outcome: the net's
//! timers settle that run.
//!
//! The groups the runner started are in its pid file, so a runner that opens
//! the pid file of a runner that died reports the groups still running under
//! it. A group is named by its pgid and the start of its leader, so a pgid
//! that another group took since is never reported nor signalled.

mod process;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::Weak;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::time::Duration;

use holon_net::exec::AttemptId;
use holon_net::exec::EffectRequest;
use holon_net::file::ExitCodeMap;
use holon_net::file::Outcome;
use holon_net::file::ShellEffect;
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::Notify;

use self::process::Members;
use self::process::Process;

/// How long a group has after SIGTERM before it gets SIGKILL.
pub const KILL_GRACE: Duration = Duration::from_secs(2);

const POLL: Duration = Duration::from_millis(20);
/// How long `open` waits for the pid-file lock of a runner that is being
/// dropped, such as the late open of a dropped netd start.
const OWN_WAIT: Duration = Duration::from_secs(1);

/// A process group id. Negated, it addresses the whole group.
pub type Pgid = i32;

/// When the leader of a group started: with the pgid, it names one group,
/// since a pgid is free for reuse only once its group is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Started(u64);

/// One process group the runner started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub pgid: Pgid,
    /// `None` only for a pid-file line that names no start: such a group is
    /// reported, never signalled.
    pub started: Option<Started>,
    pub attempt: AttemptId,
    pub transition: String,
    pub subject: String,
}

/// What one attempt left behind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finished {
    pub ended: Ended,
    pub log_path: PathBuf,
}

/// How one attempt ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The child exited on its own, with the outcome its code maps to.
    Exited { code: i32, outcome: Outcome },
    /// The runner killed the group on one of its timers, so the exit-code map
    /// decides nothing here.
    Killed(Kill),
    /// A signal the runner did not send ended the child, which therefore has
    /// no exit code.
    Signalled(i32),
}

/// Which timer ended a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kill {
    Timeout,
    Abandoned,
}

#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("the shell effect's pid file `{path}` is unusable: {source}")]
    PidFile { path: PathBuf, source: io::Error },
    #[error("the shell effect's pid file `{path}` holds the line `{line}`, which is not a group")]
    PidFileLine { path: PathBuf, line: String },
    #[error("the log `{path}` of a shell effect cannot be written: {source}")]
    Log { path: PathBuf, source: io::Error },
    #[error(
        "the log pattern `{pattern}` is refused: it must be relative, name `{{attempt}}`, use no brace outside `{{transition}}` `{{subject}}` `{{attempt}}` `{{occurrence}}`, have no `..` segment and end in a file name (working dir `{working_dir}`, subject `{subject}`)"
    )]
    LogPath {
        pattern: String,
        working_dir: String,
        subject: String,
    },
    #[error("the shell effect `{argv}` does not start: {source}")]
    Spawn { argv: String, source: io::Error },
    #[error("the child of the shell effect `{argv}` cannot be waited for: {source}")]
    Wait { argv: String, source: io::Error },
    #[error("the process table cannot be read for the process group {pgid}: {source}")]
    Group { pgid: Pgid, source: io::Error },
    #[error(
        "the process group {pgid} is not signalled: its identity cannot be proved (its pid-file line names no start, or its leader is gone), so it may be another group by now"
    )]
    Unverified { pgid: Pgid },
    #[error("the runtime shut down before the shell effect's file work `{work}` ran")]
    ShutDown { work: &'static str },
    #[error(
        "the shell effect's pid file `{path}` is held by a runner that still runs, in process {}",
        holder.map_or_else(|| "unknown".to_owned(), |pid| pid.to_string())
    )]
    InUse { path: PathBuf, holder: Option<u32> },
    #[error(
        "the shell effect's pid file `{path}` must have a name that ends in `.pid`, so that no file the runner keeps next to it is another pid file"
    )]
    PidFileName { path: PathBuf },
}

/// Runs the shell effects of one net and owns their process groups.
pub struct Runner {
    groups: Arc<Groups>,
    /// The exclusive lock on the pid file: a second runner on it would
    /// overwrite the lines of this one's runs.
    _owner: fs::File,
}

struct Groups {
    pid_file: PathBuf,
    live: Mutex<BTreeMap<AttemptId, Live>>,
    /// The leftovers `open` reported: each write keeps those still running in
    /// the file, for the next runner to report.
    left: Mutex<Vec<Group>>,
    /// Held from the read of `live` until the rename's groups are marked filed,
    /// so concurrent writes never share the temporary file and land in order.
    /// Every runner of this process on `pid_file` holds the same lock.
    write: Arc<Mutex<()>>,
}

struct Live {
    group: Group,
    abandon: Arc<Notify>,
    /// The pid file holds the group: only then do `in_flight` and `abandon`
    /// see it, so a restart reads every group they named.
    filed: bool,
}

/// Why a run is over.
enum Stop {
    Exited(ExitStatus),
    Kill(Kill),
}

impl Runner {
    /// Opens the pid file at `pid_file`. The groups it and the spawn records
    /// of dead hosts name that are still running are the leftovers of a
    /// runner that died without reaping them; they are reported, and the file
    /// then holds them and this runner's runs.
    pub fn open(pid_file: PathBuf) -> Result<(Self, Vec<Group>), ShellError> {
        if !pid_file
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().ends_with(b".pid"))
        {
            return Err(ShellError::PidFileName { path: pid_file });
        }
        let owner = own(&pid_file)?;
        // Under the write lock, so a write of an earlier runner of this process
        // on this file is read, and never lands after this runner's own write.
        let write = write_lock(&pid_file);
        let held = lock_write(&write);
        let mut recorded = match fs::read_to_string(&pid_file) {
            Ok(text) => parse_groups(&text, &pid_file)?,
            Err(source) if source.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(source) => {
                return Err(ShellError::PidFile {
                    path: pid_file,
                    source,
                });
            }
        };
        let (records, spawned) = orphan_records(&pid_file)?;
        for group in spawned {
            if !recorded
                .iter()
                .any(|g| g.pgid == group.pgid && g.started == group.started)
            {
                recorded.push(group);
            }
        }
        let left = still_left(recorded)?;
        let runner = Self {
            _owner: owner,
            groups: Arc::new(Groups {
                write: Arc::clone(&write),
                pid_file,
                live: Mutex::new(BTreeMap::new()),
                left: Mutex::new(left.clone()),
            }),
        };
        runner.groups.write_held()?;
        for record in records {
            fs::remove_file(&record).map_err(|source| ShellError::PidFile {
                path: record,
                source,
            })?;
        }
        drop(held);
        Ok((runner, left))
    }

    /// Runs one attempt of `effect` as its own process group, with both its
    /// streams in a new log file that [`Finished::log_path`] names: `log_path`
    /// rendered for this request, with a `.N` suffix when that file exists.
    pub async fn run(
        &self,
        effect: &ShellEffect,
        request: &EffectRequest,
    ) -> Result<Finished, ShellError> {
        let argv = effect.command.argv.join(" ");
        let log_path = log_path(effect, request)?;
        let (log, log_path) = unblock(
            tokio::task::spawn_blocking(move || create_log(log_path)),
            "create the log",
        )
        .await??;
        let (attempt, transition, subject) = (
            request.attempt,
            request.transition.to_string(),
            request.subject.to_string(),
        );
        let (record, record_file) = self
            .blocking(move |groups| groups.spawn_record(attempt, &transition, &subject))
            .await??;
        let spawned = spawn(effect, log, &log_path, &record_file);
        drop(record_file);
        let (mut child, pgid, started) = match spawned {
            Ok(spawned) => spawned,
            Err(error) => {
                self.blocking(move |_| remove_record(&record)).await??;
                return Err(error);
            }
        };
        let abandon = Arc::new(Notify::new());
        // After `child`, so a dropped run kills its group while the child is
        // unreaped and no other group can take its pgid.
        let mut unended = Unended {
            groups: &self.groups,
            attempt: request.attempt,
            abandon: Arc::clone(&abandon),
            pgid,
            started,
            record: record.clone(),
            ended: false,
        };
        self.register(request, pgid, started, record, &abandon)
            .await?;
        let stop = tokio::select! {
            status = child.wait() => Stop::Exited(status.map_err(|source| ShellError::Wait {
                argv: argv.clone(),
                source,
            })?),
            () = tokio::time::sleep(effect.command.timeout.duration()) => Stop::Kill(Kill::Timeout),
            () = abandon.notified() => Stop::Kill(Kill::Abandoned),
        };
        let ended = match stop {
            Stop::Exited(status) => {
                // Members the leader left behind end with the run.
                end_group(pgid, started, None).await?;
                exited(status, &effect.exit_codes)
            }
            Stop::Kill(kill) => {
                end_group(pgid, started, Some((&mut child, &argv))).await?;
                Ended::Killed(kill)
            }
        };
        unended.ended = true;
        self.forget(request.attempt).await?;
        Ok(Finished { ended, log_path })
    }

    /// The groups running right now that the pid file holds.
    pub fn in_flight(&self) -> Vec<Group> {
        self.groups
            .live()
            .values()
            .filter(|live| live.filed)
            .map(|live| live.group.clone())
            .collect()
    }

    /// Kills the group of `attempt`, whose [`Runner::run`] then reports
    /// [`Kill::Abandoned`] or the error that ended it. `false` when the attempt
    /// is no longer running or the pid file does not hold its group yet.
    pub fn abandon(&self, attempt: AttemptId) -> bool {
        let Some(abandon) = self
            .groups
            .live()
            .get(&attempt)
            .filter(|l| l.filed)
            .map(|l| Arc::clone(&l.abandon))
        else {
            return false;
        };
        abandon.notify_one();
        true
    }

    /// Takes the group into the pid file, which is what a restart reads, in
    /// place of its spawn record. A run whose group the file cannot hold
    /// returns the error, and its [`Unended`] kills the group.
    async fn register(
        &self,
        request: &EffectRequest,
        pgid: Pgid,
        started: Started,
        record: PathBuf,
        abandon: &Arc<Notify>,
    ) -> Result<(), ShellError> {
        self.groups.live().insert(
            request.attempt,
            Live {
                group: Group {
                    pgid,
                    started: Some(started),
                    attempt: request.attempt,
                    transition: request.transition.to_string(),
                    subject: request.subject.to_string(),
                },
                abandon: Arc::clone(abandon),
                filed: false,
            },
        );
        self.blocking(move |groups| {
            groups.write_pid_file()?;
            remove_record(&record)
        })
        .await?
    }

    async fn forget(&self, attempt: AttemptId) -> Result<(), ShellError> {
        self.groups.live().remove(&attempt);
        self.blocking(Groups::write_pid_file).await?
    }

    /// On the blocking pool: a slow pid file stalls neither the async worker
    /// nor `in_flight` and `abandon`.
    async fn blocking<T: Send + 'static>(
        &self,
        write: impl FnOnce(&Groups) -> T + Send + 'static,
    ) -> Result<T, ShellError> {
        let groups = Arc::clone(&self.groups);
        unblock(
            tokio::task::spawn_blocking(move || write(&groups)),
            "write the pid file",
        )
        .await
    }
}

/// A panic of the work panics here too.
async fn unblock<T>(work: tokio::task::JoinHandle<T>, name: &'static str) -> Result<T, ShellError> {
    match work.await {
        Ok(done) => Ok(done),
        Err(error) if error.is_cancelled() => Err(ShellError::ShutDown { work: name }),
        Err(error) => std::panic::resume_unwind(error.into_panic()),
    }
}

/// A run that has not ended its group yet. Dropped so, it kills the group
/// and takes it out of `in_flight` and `abandon`; a group it cannot kill
/// becomes a leftover, which the pid file keeps naming.
struct Unended<'a> {
    groups: &'a Groups,
    attempt: AttemptId,
    abandon: Arc<Notify>,
    pgid: Pgid,
    started: Started,
    record: PathBuf,
    ended: bool,
}

impl Drop for Unended<'_> {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        // A drop has no caller to return an error to.
        let killed = kill_now(self.pgid, self.started);
        let mut live = self.groups.live();
        let entry = live
            .get(&self.attempt)
            .is_some_and(|entry| Arc::ptr_eq(&entry.abandon, &self.abandon))
            .then(|| live.remove(&self.attempt))
            .flatten();
        match killed {
            Ok(()) => {
                drop(live);
                match fs::remove_file(&self.record) {
                    Err(source) if source.kind() != io::ErrorKind::NotFound => eprintln!(
                        "the spawn record `{}` of a dropped shell effect run stays: {source}",
                        self.record.display()
                    ),
                    _ => {}
                }
            }
            Err(error) => {
                eprintln!("a dropped shell effect run could not end its process group: {error}");
                self.groups.left().extend(entry.map(|entry| entry.group));
            }
        }
    }
}

impl Groups {
    fn live(&self) -> MutexGuard<'_, BTreeMap<AttemptId, Live>> {
        self.live
            .lock()
            .expect("the shell effect's group lock is poisoned")
    }

    fn left(&self) -> MutexGuard<'_, Vec<Group>> {
        self.left
            .lock()
            .expect("the shell effect's leftover lock is poisoned")
    }

    /// A new file next to the pid file that names one run until the pid file
    /// does: its first line is the run, and the child adds its pid and start
    /// before it execs, so a host killed before the pid-file write leaves
    /// the group named.
    fn spawn_record(
        &self,
        attempt: AttemptId,
        transition: &str,
        subject: &str,
    ) -> Result<(PathBuf, fs::File), ShellError> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let host = std::process::id() as Pgid;
        let started = match process::start(host)
            .map_err(|source| ShellError::Group { pgid: host, source })?
        {
            Process::Started(started) => started,
            other => unreachable!("this process {host} is {other:?} to itself"),
        };
        let mut name = record_prefix(&self.pid_file);
        name.push(format!(
            "{host}-{started}-{}",
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let path = self.pid_file.with_file_name(name);
        let failed = |source| ShellError::PidFile {
            path: path.clone(),
            source,
        };
        let mut file = fs::OpenOptions::new()
            .append(true)
            .create_new(true)
            .open(&path)
            .map_err(failed)?;
        let run = serde_json::to_string(&(attempt.0, transition, subject))
            .expect("a run is serializable");
        io::Write::write_all(&mut file, (run + "\n").as_bytes()).map_err(failed)?;
        Ok((path, file))
    }

    fn write_pid_file(&self) -> Result<(), ShellError> {
        let _held = lock_write(&self.write);
        self.write_held()
    }

    /// Blocks on the file system; the caller holds `write`. The group lock is
    /// held only to read the groups and to mark them filed after the rename,
    /// so filing does not depend on the run that waits for it.
    fn write_held(&self) -> Result<(), ShellError> {
        let looked_at = self.left().clone();
        let running = still_left(looked_at.clone())?;
        // `live`, then `left`, as a dropped run moves its group between them:
        // the file names it in one or the other.
        let (text, written) = {
            let live = self.live();
            let mut left = self.left();
            // Groups a dropped run moved here since `looked_at` stay as they are.
            let moved: Vec<_> = left.drain(..).filter(|g| !looked_at.contains(g)).collect();
            *left = running;
            left.extend(moved);
            let (runs, written): (String, Vec<_>) = live
                .iter()
                .map(|(attempt, live)| (line(&live.group), (*attempt, Arc::clone(&live.abandon))))
                .unzip();
            (
                left.iter().map(line).collect::<String>() + runs.as_str(),
                written,
            )
        };
        let temp = beside(&self.pid_file, ".writing");
        let failed = |path: &Path, source: io::Error| ShellError::PidFile {
            path: path.to_owned(),
            source,
        };
        fs::write(&temp, text).map_err(|source| failed(&temp, source))?;
        fs::rename(&temp, &self.pid_file).map_err(|source| failed(&self.pid_file, source))?;
        let mut live = self.live();
        for (attempt, abandon) in written {
            // A run with the same attempt id may have replaced the entry since.
            if let Some(entry) = live
                .get_mut(&attempt)
                .filter(|entry| Arc::ptr_eq(&entry.abandon, &abandon))
            {
                entry.filed = true;
            }
        }
        Ok(())
    }
}

/// Locks `<pid file>.lock` for as long as the file lives and names
/// this process in it; the kernel lets go of the lock when the host dies.
/// The lock is per open file, so it also refuses a second runner of this
/// process.
fn own(pid_file: &Path) -> Result<fs::File, ShellError> {
    let path = beside(pid_file, ".lock");
    let failed = |source| ShellError::PidFile {
        path: path.clone(),
        source,
    };
    let mut file = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .map_err(failed)?;
    let deadline = std::time::Instant::now() + OWN_WAIT;
    // SAFETY: `file` is an open descriptor.
    while unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(failed(error));
        }
        if std::time::Instant::now() >= deadline {
            let holder = fs::read_to_string(&path).map_err(failed)?;
            let holder = match holder.trim() {
                // Empty while the holder has locked but not named itself yet.
                "" => None,
                pid => Some(pid.parse().map_err(|_| {
                    failed(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("the lock names no holder pid: {pid:?}"),
                    ))
                })?),
            };
            return Err(ShellError::InUse {
                path: pid_file.to_owned(),
                holder,
            });
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    file.set_len(0).map_err(failed)?;
    io::Write::write_all(&mut file, format!("{}\n", std::process::id()).as_bytes())
        .map_err(failed)?;
    Ok(file)
}

fn lock_write(write: &Mutex<()>) -> MutexGuard<'_, ()> {
    write
        .lock()
        .expect("the shell effect's pid-file lock is poisoned")
}

fn write_lock(pid_file: &Path) -> Arc<Mutex<()>> {
    static LOCKS: Mutex<BTreeMap<PathBuf, Weak<Mutex<()>>>> = Mutex::new(BTreeMap::new());
    let mut locks = LOCKS.lock().expect("the pid-file lock table is poisoned");
    locks.retain(|_, lock| lock.strong_count() > 0);
    let lock = locks
        .get(pid_file)
        .and_then(Weak::upgrade)
        .unwrap_or_default();
    locks.insert(pid_file.to_owned(), Arc::downgrade(&lock));
    lock
}

/// Ends a group this runner did not spawn: there is no child to reap. It
/// returns once no member of the group runs; a group that is gone, or whose
/// pgid another group holds by now, gets no signal. A group whose identity
/// cannot be proved is [`ShellError::Unverified`] while a member of it runs.
pub async fn kill_group(group: &Group) -> Result<(), ShellError> {
    let pgid = group.pgid;
    let started = group.started.ok_or(ShellError::Unverified { pgid })?;
    match identity(pgid, started)? {
        Identity::Other => Ok(()),
        Identity::Leaderless if running_member(pgid)? => Err(ShellError::Unverified { pgid }),
        Identity::Leaderless => Ok(()),
        // Once the leader proved the group, its pgid stays the group's while
        // a member runs, so end_group may outlive the leader.
        Identity::Same => end_group(pgid, started, None).await,
    }
}

/// SIGTERM to the whole group, then SIGKILL to it while a member is left
/// after the grace period, until none runs. The grace period waits on the
/// group, not on the child: a member that ignores SIGTERM outlives the child.
pub(crate) async fn end_group(
    pgid: Pgid,
    started: Started,
    mut child: Option<(&mut Child, &str)>,
) -> Result<(), ShellError> {
    let waited = |argv: &str, source| ShellError::Wait {
        argv: argv.to_owned(),
        source,
    };
    if running(pgid, started)? {
        signal_running(pgid, libc::SIGTERM)?;
    }
    let deadline = tokio::time::Instant::now() + KILL_GRACE;
    loop {
        if let Some((child, argv)) = child.as_mut() {
            child.try_wait().map_err(|source| waited(argv, source))?;
        }
        if !running(pgid, started)? {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            signal_running(pgid, libc::SIGKILL)?;
        }
        tokio::time::sleep(POLL).await;
    }
    if let Some((child, argv)) = child {
        child.wait().await.map_err(|source| waited(argv, source))?;
    }
    Ok(())
}

/// SIGKILL to the group until no member runs, for a group whose leader is an
/// unreaped child, so no other group can hold its pgid meanwhile.
fn kill_until_gone(pgid: Pgid) -> Result<(), ShellError> {
    while running_member(pgid)? {
        signal_running(pgid, libc::SIGKILL)?;
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

/// SIGKILL to the recorded group until no member of it runs, without a
/// grace period: for a run dropped before it ended its group.
fn kill_now(pgid: Pgid, started: Started) -> Result<(), ShellError> {
    while running(pgid, started)? {
        signal_running(pgid, libc::SIGKILL)?;
        std::thread::sleep(Duration::from_millis(1));
    }
    Ok(())
}

/// Sends `signal` to the group. A refusal fails only while a member runs
/// that is not exiting: macOS refuses a signal to zombies and exiting members.
fn signal_running(pgid: Pgid, signal: i32) -> Result<(), ShellError> {
    match signal_group(pgid, signal) {
        Ok(()) => Ok(()),
        Err(_) if members(pgid)? != Members::Running => Ok(()),
        Err(source) => Err(ShellError::Group { pgid, source }),
    }
}

/// Whether a member of the group is not a zombie yet.
fn running_member(pgid: Pgid) -> Result<bool, ShellError> {
    Ok(members(pgid)? != Members::None)
}

fn members(pgid: Pgid) -> Result<Members, ShellError> {
    assert!(
        pgid > 1,
        "a signal to the process group {pgid} is not a signal to one group"
    );
    process::members(pgid).map_err(|source| ShellError::Group { pgid, source })
}

/// Whether the group is the recorded one and a member of it runs.
fn running(pgid: Pgid, started: Started) -> Result<bool, ShellError> {
    Ok(holds(pgid, started)? && running_member(pgid)?)
}

/// What the leader of a pgid says about the group recorded with `started`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Identity {
    Same,
    Other,
    /// No leader to compare: the recorded group or a foreign one.
    Leaderless,
}

fn identity(pgid: Pgid, started: Started) -> Result<Identity, ShellError> {
    match process::start(pgid).map_err(|source| ShellError::Group { pgid, source })? {
        Process::Started(at) if at == started.0 => Ok(Identity::Same),
        Process::Started(_) => Ok(Identity::Other),
        Process::Gone => Ok(Identity::Leaderless),
        #[cfg(target_os = "linux")]
        Process::Foreign => Ok(Identity::Other),
    }
}

/// Whether the pgid still names the group whose leader started at
/// `started`, for a group this host started and still ends: a group whose
/// leader exited is that group while a member runs.
fn holds(pgid: Pgid, started: Started) -> Result<bool, ShellError> {
    Ok(identity(pgid, started)? != Identity::Other)
}

/// The recorded groups that `open` reports: a member runs and, when the
/// record names a start, still the recorded group. One whose leader is gone
/// is reported without its start, as unverified.
fn still_left(groups: Vec<Group>) -> Result<Vec<Group>, ShellError> {
    let mut left = Vec::new();
    for group in groups {
        let started = match group.started {
            Some(started) => match identity(group.pgid, started)? {
                Identity::Same => Some(started),
                Identity::Other => continue,
                Identity::Leaderless => None,
            },
            None => None,
        };
        if running_member(group.pgid)? {
            left.push(Group { started, ..group });
        }
    }
    Ok(left)
}

fn spawn(
    effect: &ShellEffect,
    log: fs::File,
    log_path: &Path,
    record: &fs::File,
) -> Result<(Child, Pgid, Started), ShellError> {
    let stderr = log.try_clone().map_err(|source| ShellError::Log {
        path: log_path.to_owned(),
        source,
    })?;
    let command = &effect.command;
    let mut spawn = Command::new(&command.argv[0]);
    spawn
        .args(&command.argv[1..])
        .current_dir(&command.working_dir)
        .env_clear()
        .stdin(Stdio::null())
        .stdout(log)
        .stderr(stderr)
        .process_group(0);
    for name in &command.env_allow {
        if let Some(value) = std::env::var_os(name) {
            spawn.env(name, value);
        }
    }
    let record = record.as_raw_fd();
    // SAFETY: the hook allocates nothing and makes only async-signal-safe
    // calls; `record` stays open in the parent until `spawn` returns.
    unsafe {
        spawn.pre_exec(move || {
            let pid = libc::getpid();
            let Process::Started(started) = process::start(pid)? else {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            };
            let mut line = [0u8; 48];
            let mut at = process::put_decimal(&mut line, 0, pid as u64);
            at = process::put(&mut line, at, b" ");
            at = process::put_decimal(&mut line, at, started);
            at = process::put(&mut line, at, b"\n");
            match libc::write(record, line.as_ptr().cast(), at) {
                n if n < 0 => Err(io::Error::last_os_error()),
                n if n as usize != at => Err(io::Error::from(io::ErrorKind::WriteZero)),
                _ => Ok(()),
            }
        });
    }
    // macOS std opens the pipe on which spawn waits for the exec without
    // O_CLOEXEC; a child forked meanwhile holds it open until that child exits.
    static SPAWN: Mutex<()> = Mutex::new(());
    let one_spawn = SPAWN
        .lock()
        .expect("the shell effect's spawn lock is poisoned");
    let spawned = spawn.spawn();
    drop(one_spawn);
    let child = spawned.map_err(|source| ShellError::Spawn {
        argv: command.argv.join(" "),
        source,
    })?;
    let pgid = child
        .id()
        .expect("a child that has not been waited for has a pid") as Pgid;
    Ok((child, pgid, child_started(pgid)?))
}

/// The start of an unreaped child that leads its own group; a child whose
/// start cannot be read is killed, as no kill of it could check it.
pub(crate) fn child_started(pgid: Pgid) -> Result<Started, ShellError> {
    let source = match process::start(pgid) {
        Ok(Process::Started(started)) => return Ok(Started(started)),
        // A process table that hides other users' processes hides a child
        // that exec'd a setuid program.
        Ok(other) => io::Error::other(format!("the unreaped child reads as {other:?}")),
        Err(source) => source,
    };
    kill_until_gone(pgid)?;
    Err(ShellError::Group { pgid, source })
}

fn exited(status: ExitStatus, codes: &ExitCodeMap) -> Ended {
    match (status.code(), status.signal()) {
        (Some(code), _) => Ended::Exited {
            code,
            outcome: outcome_of(code, codes),
        },
        (None, Some(signal)) => Ended::Signalled(signal),
        (None, None) => unreachable!("an ended child has a code or a signal: {status:?}"),
    }
}

fn outcome_of(code: i32, codes: &ExitCodeMap) -> Outcome {
    if codes.done.contains(&code) {
        Outcome::Done
    } else if codes.failed.contains(&code) {
        Outcome::Failed
    } else {
        codes.other
    }
}

/// Renders the pattern one `/`-separated segment at a time, so no value can
/// add a separator or a `..`, and no value is read as a placeholder. A
/// pattern the net-file loader refuses, an absolute one, one with a `..`
/// segment and one that ends in no file name are refused here too.
fn log_path(effect: &ShellEffect, request: &EffectRequest) -> Result<PathBuf, ShellError> {
    let pattern = &effect.log_path;
    let refused = || ShellError::LogPath {
        pattern: pattern.clone(),
        working_dir: effect.command.working_dir.clone(),
        subject: request.subject.as_str().to_owned(),
    };
    if pattern.starts_with('/') || !pattern.contains("{attempt}") {
        return Err(refused());
    }
    let values = [
        ("{transition}", request.transition.to_string()),
        ("{subject}", request.subject.as_str().to_owned()),
        ("{attempt}", request.attempt.0.to_string()),
        ("{occurrence}", request.occurrence.to_string()),
    ]
    .map(|(name, value)| (name, component(&value)));
    let segments = pattern
        .split('/')
        .map(|segment| render(segment, &values).ok_or_else(refused))
        .collect::<Result<Vec<_>, _>>()?;
    let last = segments.last().expect("a split yields a segment");
    if matches!(last.as_str(), "" | ".") || segments.iter().any(|s| s == "..") {
        return Err(refused());
    }
    let mut path = PathBuf::from(&effect.command.working_dir);
    path.extend(segments.iter().filter(|s| !matches!(s.as_str(), "" | ".")));
    Ok(path)
}

/// `None` when a brace is not part of one of the four placeholders.
fn render(segment: &str, values: &[(&str, String)]) -> Option<String> {
    let mut rendered = String::new();
    let mut rest = segment;
    while let Some(brace) = rest.find(['{', '}']) {
        rendered.push_str(&rest[..brace]);
        rest = &rest[brace..];
        let (name, value) = values.iter().find(|(name, _)| rest.starts_with(name))?;
        rendered.push_str(value);
        rest = &rest[name.len()..];
    }
    Some(rendered + rest)
}

/// One path component that names `value` and no other value: never empty,
/// `.` or `..`, and printable ASCII only, so no file system refuses it for
/// its characters. Every other byte is percent-encoded.
fn component(value: &str) -> String {
    match value {
        "" => "%".to_owned(),
        "." | ".." => value.replace('.', "%2E"),
        _ => value
            .bytes()
            .map(|b| match b {
                b' '..=b'~' if b != b'/' && b != b'%' => char::from(b).to_string(),
                _ => format!("%{b:02X}"),
            })
            .collect(),
    }
}

/// Never opens an existing file, so a restarted runner that reuses an attempt
/// id keeps the earlier log and writes `<path>.1`, `<path>.2`, ... instead.
fn create_log(path: PathBuf) -> Result<(fs::File, PathBuf), ShellError> {
    let failed = |path: &Path, source| ShellError::Log {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|source| failed(&path, source))?;
    }
    let name = path
        .file_name()
        .expect("a log path ends in a file name")
        .to_owned();
    let mut candidate = path.clone();
    for n in 1u64.. {
        match fs::File::create_new(&candidate) {
            Ok(log) => return Ok((log, candidate)),
            Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                let mut next = name.clone();
                next.push(format!(".{n}"));
                candidate = path.with_file_name(next);
            }
            Err(source) => return Err(failed(&candidate, source)),
        }
    }
    unreachable!("a directory holds fewer than u64::MAX logs")
}

fn line(g: &Group) -> String {
    serde_json::to_string(&(
        g.pgid,
        g.started.map(|s| s.0),
        g.attempt.0,
        &g.transition,
        &g.subject,
    ))
    .expect("a group is serializable")
        + "\n"
}

/// Also reads the line without a start that earlier runners wrote.
fn parse_groups(text: &str, path: &Path) -> Result<Vec<Group>, ShellError> {
    text.lines()
        .map(|line| {
            let refused = || ShellError::PidFileLine {
                path: path.to_owned(),
                line: line.to_owned(),
            };
            type Line = (Pgid, Option<u64>, u64, String, String);
            type Unstarted = (Pgid, u64, String, String);
            let (pgid, started, attempt, transition, subject) =
                match serde_json::from_str::<Line>(line) {
                    Ok(parsed) => parsed,
                    Err(_) => {
                        let (pgid, attempt, transition, subject): Unstarted =
                            serde_json::from_str(line).map_err(|_| refused())?;
                        (pgid, None, attempt, transition, subject)
                    }
                };
            Ok(Group {
                pgid,
                started: started.map(Started),
                attempt: AttemptId(attempt),
                transition,
                subject,
            })
        })
        .collect()
}

fn record_prefix(pid_file: &Path) -> std::ffi::OsString {
    side_name(pid_file, ".spawn-")
}

/// The file `<pid file><suffix>` next to the pid file.
fn beside(pid_file: &Path, suffix: &str) -> PathBuf {
    pid_file.with_file_name(side_name(pid_file, suffix))
}

fn side_name(pid_file: &Path, suffix: &str) -> std::ffi::OsString {
    let mut name = pid_file
        .file_name()
        .expect("a pid file path ends in a file name")
        .to_owned();
    name.push(suffix);
    name
}

fn remove_record(record: &Path) -> Result<(), ShellError> {
    fs::remove_file(record).map_err(|source| ShellError::PidFile {
        path: record.to_owned(),
        source,
    })
}

/// The spawn records next to `pid_file` of hosts that no longer run, and the
/// groups they name: a record of a host that runs belongs to its run.
fn orphan_records(pid_file: &Path) -> Result<(Vec<PathBuf>, Vec<Group>), ShellError> {
    let dir = match pid_file.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    let failed = |path: &Path, source| ShellError::PidFile {
        path: path.to_owned(),
        source,
    };
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        // The pid-file write then names the missing directory.
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            return Ok((Vec::new(), Vec::new()));
        }
        Err(source) => return Err(failed(dir, source)),
    };
    let prefix = record_prefix(pid_file);
    let (mut records, mut groups) = (Vec::new(), Vec::new());
    for entry in entries {
        let entry = entry.map_err(|source| failed(dir, source))?;
        let name = entry.file_name();
        let Some(host) = name
            .as_encoded_bytes()
            .strip_prefix(prefix.as_encoded_bytes())
        else {
            continue;
        };
        let path = entry.path();
        let (host, host_started) = match record_host(host) {
            Some(host) => host,
            // The pid file `<pid file>.spawn-<x>.pid` or one of its own files.
            None if host.windows(4).any(|part| part == b".pid") => continue,
            None => {
                return Err(ShellError::PidFileLine {
                    path,
                    line: name.to_string_lossy().into_owned(),
                });
            }
        };
        let host_state =
            process::start(host).map_err(|source| ShellError::Group { pgid: host, source })?;
        if host_state == Process::Started(host_started) {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|source| failed(&path, source))?;
        groups.extend(parse_record(&text, &path)?);
        records.push(path);
    }
    Ok((records, groups))
}

/// The host pid and start in the `<pid>-<start>-<n>` tail of a record
/// name; `None` when the tail has another form.
fn record_host(tail: &[u8]) -> Option<(i32, u64)> {
    let fields: Vec<&[u8]> = tail.split(|&byte| byte == b'-').collect();
    let [host, started, n] = fields[..] else {
        return None;
    };
    let number = |field: &[u8]| {
        (!field.is_empty() && field.iter().all(u8::is_ascii_digit))
            .then(|| {
                let digits = std::str::from_utf8(field).expect("ASCII digits are UTF-8");
                // ALLOW(ok): a number beyond u64 makes the tail one of another form.
                digits.parse::<u64>().ok()
            })
            .flatten()
    };
    number(n)?;
    // ALLOW(ok): a host pid beyond i32 makes the tail one of another form.
    Some((number(host)?.try_into().ok()?, number(started)?))
}

/// `None` when the record names no group: its host died before the run's
/// line was whole, or before the child wrote its pid.
fn parse_record(text: &str, path: &Path) -> Result<Option<Group>, ShellError> {
    let refused = |line: &str| ShellError::PidFileLine {
        path: path.to_owned(),
        line: line.to_owned(),
    };
    let mut lines = text.split_inclusive('\n');
    let Some(run) = lines.next().filter(|run| run.ends_with('\n')) else {
        return Ok(None);
    };
    let (attempt, transition, subject): (u64, String, String) =
        serde_json::from_str(run).map_err(|_| refused(run))?;
    let Some(child) = lines.next() else {
        return Ok(None);
    };
    if let Some(extra) = lines.next() {
        return Err(refused(extra));
    }
    let (pid, started) = child
        .strip_suffix('\n')
        .and_then(|child| child.split_once(' '))
        .ok_or_else(|| refused(child))?;
    let pgid: Pgid = pid.parse().map_err(|_| refused(child))?;
    let started: u64 = started.parse().map_err(|_| refused(child))?;
    Ok(Some(Group {
        pgid,
        started: Some(Started(started)),
        attempt: AttemptId(attempt),
        transition,
        subject,
    }))
}

/// A group with no member left is no error; `Err` with EPERM when the group
/// has only members this user may not signal.
fn signal_group(pgid: Pgid, signal: i32) -> io::Result<()> {
    assert!(
        pgid > 1,
        "a signal to the process group {pgid} is not a signal to one group"
    );
    // SAFETY: a negated pgid addresses that process group and nothing else.
    if unsafe { libc::kill(-pgid, signal) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() {
        Some(libc::ESRCH) => Ok(()),
        _ => Err(error),
    }
}
