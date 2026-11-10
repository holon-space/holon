//! A runner on a tokio runtime of its own, children the test holds and ends,
//! a pid-file write the test holds, and a crash-restart read of the pid file.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::ffi::CString;
use std::fs;
use std::io;
use std::os::unix::fs::FileTypeExt;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;
use std::time::Instant;

use holon_net::exec::AttemptId;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Subject;
use holon_net::file::Outcome;
use holon_net::file::ShellEffect;
use holon_net_shell::effect::Ended;
use holon_net_shell::effect::Finished;
use holon_net_shell::effect::Group;
use holon_net_shell::effect::Kill;
use holon_net_shell::effect::Pgid;
use holon_net_shell::effect::Runner;
use holon_net_shell::effect::ShellError;
use proptest::prelude::*;

/// The longest a caller may wait for `in_flight`, `abandon` or a task on the
/// runner's runtime: the interaction-to-visible SLO (p95 < 200 ms). A call
/// that does not wait for a pid-file write holds the group lock for a map
/// copy, which takes microseconds.
pub const BOUND: Duration = Duration::from_millis(200);

/// How long an eventual check waits for children and writes the test does not
/// hold: far above any child's exit and any write on an idle disk.
pub const SETTLE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    CurrentThread,
    MultiThread(usize),
}

pub fn flavor() -> impl Strategy<Value = Flavor> {
    prop_oneof![
        Just(Flavor::CurrentThread),
        (2usize..=4).prop_map(Flavor::MultiThread),
    ]
}

/// Where a call to the runner comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    /// A thread outside the runtime.
    Thread,
    /// A task on the runtime the runs use.
    RuntimeTask,
}

pub fn caller() -> impl Strategy<Value = Caller> {
    prop_oneof![Just(Caller::Thread), Just(Caller::RuntimeTask)]
}

/// How a child ends once it may.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Code(i32),
    Signal(i32),
}

/// An exit status holds 8 bits; the signals are the ones whose default action
/// ends `/bin/sh`.
pub fn exit() -> impl Strategy<Value = Exit> {
    prop_oneof![
        (0..=255i32).prop_map(Exit::Code),
        prop::sample::select(vec![
            libc::SIGHUP,
            libc::SIGKILL,
            libc::SIGTERM,
            libc::SIGUSR1
        ])
        .prop_map(Exit::Signal),
    ]
}

/// One run's child and the names its group is recorded under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildPlan {
    /// A held child runs until the test tells it to end; the other one ends at
    /// once.
    pub held: bool,
    pub exit: Exit,
    pub transition: String,
    pub subject: String,
}

/// Any characters, control characters and NUL included: the names go into
/// the pid file only, since the log pattern names the attempt alone.
pub fn name() -> impl Strategy<Value = String> {
    prop::collection::vec(any::<char>(), 0..24).prop_map(String::from_iter)
}

pub fn child_plan() -> impl Strategy<Value = ChildPlan> {
    (any::<bool>(), exit(), name(), name()).prop_map(|(held, exit, transition, subject)| {
        ChildPlan {
            held,
            exit,
            transition,
            subject,
        }
    })
}

pub fn held_plan() -> impl Strategy<Value = ChildPlan> {
    child_plan().prop_map(|plan| ChildPlan { held: true, ..plan })
}

/// The end the exit-code map of [`descriptor`] gives a child that ends on its
/// own: `0` is done, `3` failed, every other code unknown.
pub fn ended(exit: Exit) -> Ended {
    match exit {
        Exit::Code(code) => Ended::Exited {
            code,
            outcome: match code {
                0 => Outcome::Done,
                3 => Outcome::Failed,
                _ => Outcome::OutcomeUnknown,
            },
        },
        Exit::Signal(signal) => Ended::Signalled(signal),
    }
}

pub fn abandoned() -> Ended {
    Ended::Killed(Kill::Abandoned)
}

fn descriptor(plan: &ChildPlan, ctl: &Path, working_dir: &Path) -> ShellEffect {
    let end = match plan.exit {
        Exit::Code(code) => format!("exit {code}"),
        Exit::Signal(signal) => format!("kill -{signal} $$"),
    };
    // The marker tells the test that the run has spawned its group, and names
    // the group: the shell leads it. It appears whole, so a reader never sees
    // it empty.
    let mark = "echo $$ > \"$0.started.w\" && mv \"$0.started.w\" \"$0.started\"";
    let script = if plan.held {
        format!("{mark}; while [ ! -e \"$0\" ]; do sleep 0.02; done; {end}")
    } else {
        format!("{mark}; {end}")
    };
    serde_json::from_value(serde_json::json!({
        "command": {
            "argv": ["/bin/sh", "-c", script, ctl.to_str().expect("the temporary path is utf-8")],
            "working_dir": working_dir.to_str().expect("the temporary path is utf-8"),
            "env_allow": ["PATH"],
            "timeout": "600s",
        },
        "log_path": "logs/{attempt}.log",
        "exit_codes": { "done": [0], "failed": [3], "other": "outcome_unknown" },
    }))
    .expect("the descriptor is a valid shell effect")
}

/// What one run's task hands back.
pub type RunResult = Result<Finished, ShellError>;

/// One run the test started.
pub struct Run {
    pub attempt: u64,
    pub plan: ChildPlan,
    ctl: PathBuf,
    result: mpsc::Receiver<RunResult>,
    task: tokio::task::JoinHandle<()>,
    /// When the test first saw the task ended.
    ended_at: OnceLock<Instant>,
}

impl Run {
    /// Lets a held child end with its exit.
    pub fn tell(&self) {
        fs::write(&self.ctl, "").expect("the control file is writable");
    }

    /// Waits until the child runs: the run has spawned its group, and its
    /// register write is under way.
    pub fn wait_started(&self) -> Result<(), String> {
        eventually(|| {
            if self.started() {
                Ok(())
            } else {
                Err(format!("the child of run {} has not started", self.attempt))
            }
        })
    }

    /// Whether the child has started.
    pub fn started(&self) -> bool {
        self.marker().exists()
    }

    /// The group the child leads, once its marker names it.
    pub fn group(&self) -> Option<Pgid> {
        fs::read_to_string(self.marker()).ok()?.trim().parse().ok()
    }

    fn marker(&self) -> PathBuf {
        let mut marker = self.ctl.clone().into_os_string();
        marker.push(".started");
        PathBuf::from(marker)
    }

    /// Drops the run's future wherever it waits.
    pub fn drop_future(&self) {
        self.task.abort();
    }

    /// Waits until the run's task has ended, so its future takes no further
    /// step.
    pub fn wait_task_ended(&self) -> Result<(), String> {
        eventually(|| {
            if self.task.is_finished() {
                self.ended_at.get_or_init(Instant::now);
                Ok(())
            } else {
                Err(format!("the task of run {} has not ended", self.attempt))
            }
        })
    }

    /// The run's result, or `None` when it has none after `wait`.
    pub fn result(&self, wait: Duration) -> Option<RunResult> {
        self.result.recv_timeout(wait).ok()
    }
}

/// A call to the runner that may not have answered yet.
pub struct Pending<T> {
    started: Instant,
    answer: mpsc::Receiver<T>,
}

/// In-time answers that a caller took with their elapsed time, for a judge.
pub static TIMED_IN_TIME: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub enum Answer<T> {
    InTime(T),
    Late(Pending<T>),
}

impl<T> Pending<T> {
    /// The answer, if it comes within `bound` of the call.
    pub fn within(self, bound: Duration) -> Answer<T> {
        let before = TIMED_IN_TIME.load(std::sync::atomic::Ordering::SeqCst);
        let answer = self.timed_within(bound);
        TIMED_IN_TIME.store(before, std::sync::atomic::Ordering::SeqCst);
        match answer {
            Ok((value, _)) => Answer::InTime(value),
            Err(pending) => Answer::Late(pending),
        }
    }

    /// The answer and how long after the call it came, if it comes within
    /// `bound` of the call.
    pub fn timed_within(self, bound: Duration) -> Result<(T, Duration), Self> {
        let answer = match self
            .answer
            .recv_timeout(bound.saturating_sub(self.started.elapsed()))
        {
            Ok(value) => Ok((value, self.started.elapsed())),
            Err(_) => Err(self),
        };
        if answer.is_ok() {
            TIMED_IN_TIME.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        answer
    }

    /// The late answer and how long after the call it came.
    pub fn late(self) -> Option<(T, Duration)> {
        let value = self.answer.recv_timeout(SETTLE).ok()?;
        Some((value, self.started.elapsed()))
    }
}

/// A runner on its own runtime, in a temporary directory that also holds the
/// children's control files and logs.
pub struct Lab {
    pub dir: tempfile::TempDir,
    pub pid_file: PathBuf,
    pub runner: Arc<Runner>,
    handle: tokio::runtime::Handle,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    driver: Option<std::thread::JoinHandle<()>>,
    /// Every group `in_flight` named, so the test can end what it started.
    seen: Mutex<BTreeMap<u64, Pgid>>,
    /// What the pid file holds while its path is a test artifact: the bytes a
    /// held write sent through the FIFO once that is renamed onto the pid
    /// file, or the content a failed rename leaves while the path is the
    /// blocking directory.
    pub stand_in: Mutex<Option<Vec<u8>>>,
    restarts: Mutex<u64>,
    /// Numbers the control files: two runs may share an attempt id.
    starts: Mutex<u64>,
}

impl Lab {
    pub fn new(flavor: Flavor) -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        // The host names every pid file `<net>.pid`.
        let pid_file = dir.path().join("net.pid");
        let (runner, left) = Runner::open(pid_file.clone()).expect("the pid file is writable");
        assert_eq!(left, Vec::new(), "a fresh pid file names no group");
        let runtime = match flavor {
            Flavor::CurrentThread => tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build(),
            Flavor::MultiThread(workers) => tokio::runtime::Builder::new_multi_thread()
                .worker_threads(workers)
                .enable_all()
                .build(),
        }
        .expect("a tokio runtime");
        let handle = runtime.handle().clone();
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let driver = std::thread::spawn(move || {
            let _ = runtime.block_on(stopped);
            runtime.shutdown_timeout(SETTLE);
        });
        Self {
            dir,
            pid_file,
            runner: Arc::new(runner),
            handle,
            stop: Some(stop),
            driver: Some(driver),
            seen: Mutex::new(BTreeMap::new()),
            stand_in: Mutex::new(None),
            restarts: Mutex::new(0),
            starts: Mutex::new(0),
        }
    }

    pub fn start(&self, attempt: u64, plan: ChildPlan) -> Run {
        let n = {
            let mut starts = self.starts.lock().unwrap();
            *starts += 1;
            *starts
        };
        let ctl = self.dir.path().join(format!("ctl-{n}"));
        let effect = descriptor(&plan, &ctl, self.dir.path());
        let request = EffectRequest {
            attempt: AttemptId(attempt),
            occurrence: OccurrenceId::caller("op-1"),
            transition: Arc::from(plan.transition.as_str()),
            subject: Subject::new(&plan.subject),
            input: Content::new(),
            probe: false,
        };
        let (tx, result) = mpsc::channel();
        let runner = Arc::clone(&self.runner);
        let task = self.handle.spawn(async move {
            let _ = tx.send(runner.run(&effect, &request).await);
        });
        Run {
            attempt,
            plan,
            ctl,
            result,
            task,
            ended_at: OnceLock::new(),
        }
    }

    pub fn call<T: Send + 'static>(
        &self,
        caller: Caller,
        f: impl FnOnce(&Runner) -> T + Send + 'static,
    ) -> Pending<T> {
        let (tx, answer) = mpsc::channel();
        let runner = Arc::clone(&self.runner);
        let started = Instant::now();
        match caller {
            Caller::Thread => {
                std::thread::spawn(move || {
                    let _ = tx.send(f(&runner));
                });
            }
            Caller::RuntimeTask => {
                self.handle.spawn(async move {
                    let _ = tx.send(f(&runner));
                });
            }
        }
        Pending { started, answer }
    }

    pub fn in_flight(&self, caller: Caller) -> Pending<Vec<Group>> {
        self.call(caller, |runner| runner.in_flight())
    }

    pub fn abandon(&self, caller: Caller, attempt: u64) -> Pending<bool> {
        self.call(caller, move |runner| runner.abandon(AttemptId(attempt)))
    }

    /// A 1 ms timer on the runtime: it fires late when a worker is blocked.
    pub fn tick(&self) -> Pending<()> {
        let (tx, answer) = mpsc::channel();
        let started = Instant::now();
        self.handle.spawn(async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            let _ = tx.send(());
        });
        Pending { started, answer }
    }

    pub fn note_seen(&self, groups: &[Group]) {
        let mut seen = self.seen.lock().unwrap();
        for g in groups {
            seen.insert(g.attempt.0, g.pgid);
        }
    }

    pub fn pgid(&self, attempt: u64) -> Option<Pgid> {
        self.seen.lock().unwrap().get(&attempt).copied()
    }

    /// What a runner restarted after a crash right now would report: the
    /// current pid file, copied so the restart's own write leaves it alone.
    /// `None` while a rename replaces the file.
    pub fn restart_read(&self) -> Result<Option<Vec<Group>>, String> {
        let bytes = match fs::symlink_metadata(&self.pid_file) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("the pid file cannot be inspected: {e}")),
            Ok(meta) if meta.file_type().is_fifo() || meta.is_dir() => {
                match self.stand_in.lock().unwrap().clone() {
                    Some(bytes) => bytes,
                    None => {
                        return Err(
                            "the pid file path is a test artifact with no stand-in".to_owned()
                        );
                    }
                }
            }
            // A held write may rename the FIFO onto the pid file after the
            // check above: a non-blocking open does not wait for its writer.
            Ok(_) => match fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&self.pid_file)
            {
                Ok(file) if file.metadata().is_ok_and(|m| m.file_type().is_fifo()) => {
                    return self.restart_read();
                }
                Ok(mut file) => {
                    let mut bytes = Vec::new();
                    io::Read::read_to_end(&mut file, &mut bytes)
                        .map_err(|e| format!("the pid file cannot be read: {e}"))?;
                    bytes
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(format!("the pid file cannot be read: {e}")),
            },
        };
        let n = {
            let mut restarts = self.restarts.lock().unwrap();
            *restarts += 1;
            *restarts
        };
        let copy_dir = self.dir.path().join(format!("restart-{n}"));
        fs::create_dir(&copy_dir).expect("the restart directory is writable");
        let copy = copy_dir.join("net.pid");
        fs::write(&copy, &bytes).expect("the restart copy is writable");
        match Runner::open(copy) {
            Ok((_, left)) => Ok(Some(left)),
            Err(e) => Err(format!(
                "a restart refuses the pid file {:?}: {e}",
                String::from_utf8_lossy(&bytes)
            )),
        }
    }

    /// Ends every group this lab saw that is still alive, and the runtime.
    pub fn finish(mut self) -> Arc<Runner> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(driver) = self.driver.take() {
            driver.join().expect("the runtime driver does not panic");
        }
        self.kill_seen();
        Arc::clone(&self.runner)
    }

    pub fn kill_seen(&self) {
        for pgid in self.seen.lock().unwrap().values() {
            if alive(*pgid) {
                // SAFETY: the group is one the runner started for this test.
                unsafe { libc::kill(-*pgid, libc::SIGKILL) };
            }
        }
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.kill_seen();
    }
}

/// Whether the group still has a member, a zombie one included.
pub fn alive(pgid: Pgid) -> bool {
    assert!(pgid > 1, "{pgid} is not one process group");
    // SAFETY: signal 0 checks the group and changes nothing.
    unsafe { libc::kill(-pgid, 0) == 0 }
}

/// How long a dropped run's group may outlive the drop: the kill grace plus
/// scheduling slack.
pub const DROP_KILL: Duration = Duration::from_secs(1);

/// Once the dropped run's task has ended, the group its child leads is dead
/// within [`DROP_KILL`]. A child forked before the drop may write its marker
/// after the task ended; the test waits [`DROP_KILL`] for it.
pub fn dropped_group_dies(run: &Run) -> Result<(), String> {
    run.wait_task_ended()?;
    let marker_by = *run.ended_at.get().expect("the task ended") + DROP_KILL;
    let deadline = Instant::now() + DROP_KILL;
    loop {
        match run.group() {
            None if Instant::now() >= marker_by => return Ok(()),
            Some(pgid) if !alive(pgid) => return Ok(()),
            Some(pgid) if Instant::now() >= deadline => {
                return Err(format!(
                    "the group {pgid} of the dropped run {} lives {DROP_KILL:?} after the drop",
                    run.attempt
                ));
            }
            _ => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// Whether a restart that read `left` read the listed group `g`. A group whose
/// leader is gone has no provable start, so the restart reads it without one.
pub fn read_back(left: &[Group], g: &Group) -> bool {
    left.iter().any(|l| {
        (l.pgid, &l.attempt, &l.transition, &l.subject)
            == (g.pgid, &g.attempt, &g.transition, &g.subject)
            && (l.started == g.started || (l.started.is_none() && !leader_runs(g.pgid)))
    })
}

/// Whether the leader of the group runs: its pid is listed and is no zombie.
fn leader_runs(pgid: Pgid) -> bool {
    let out = std::process::Command::new("/bin/ps")
        .args(["-o", "stat=", "-p", &pgid.to_string()])
        .output()
        .expect("ps runs");
    let stat = String::from_utf8_lossy(&out.stdout);
    let stat = stat.trim();
    !stat.is_empty() && !stat.starts_with('Z')
}

/// Polls `check` until it holds, for at most [`SETTLE`]; the last mismatch
/// otherwise.
pub fn eventually(mut check: impl FnMut() -> Result<(), String>) -> Result<(), String> {
    let deadline = Instant::now() + SETTLE;
    loop {
        match check() {
            Ok(()) => return Ok(()),
            Err(e) if Instant::now() >= deadline => {
                return Err(format!("{e} (still after {SETTLE:?})"));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

/// The records of `groups` that a model compares: attempt, transition and
/// subject.
pub fn records(groups: &[Group]) -> BTreeSet<(u64, String, String)> {
    groups
        .iter()
        .map(|g| (g.attempt.0, g.transition.clone(), g.subject.clone()))
        .collect()
}

pub fn record(attempt: u64, plan: &ChildPlan) -> (u64, String, String) {
    (attempt, plan.transition.clone(), plan.subject.clone())
}

/// A pid-file write the test holds: a FIFO at the temporary path the runner
/// writes before it renames onto the pid file. The write blocks in `open`
/// until the test reads the FIFO.
pub struct Hold {
    fifo: PathBuf,
}

/// How a held write ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    /// The write goes through and the rename puts it in place.
    Write,
    /// The write goes through but its rename fails: the pid file path is a
    /// directory that is not empty.
    FailRename,
}

pub fn release() -> impl Strategy<Value = Release> {
    prop_oneof![Just(Release::Write), Just(Release::FailRename)]
}

/// The pid file before a failing rename, so the test can put it back as the
/// failed write left it.
pub struct Blocked {
    pid_file: PathBuf,
    before: Vec<u8>,
}

impl Blocked {
    pub fn restore(self, lab: &Lab) {
        fs::remove_dir_all(&self.pid_file).expect("the blocking directory is removable");
        fs::write(&self.pid_file, &self.before).expect("the pid file is writable");
        *lab.stand_in.lock().unwrap() = None;
    }
}

impl Hold {
    pub fn new(pid_file: &Path) -> Self {
        let mut fifo = pid_file.as_os_str().to_owned();
        fifo.push(".writing");
        let fifo = PathBuf::from(fifo);
        let path = CString::new(fifo.to_str().expect("the temporary path is utf-8")).unwrap();
        // SAFETY: a valid C path and a mode.
        let made = unsafe { libc::mkfifo(path.as_ptr(), 0o600) };
        assert_eq!(made, 0, "mkfifo {fifo:?}: {}", io::Error::last_os_error());
        Self { fifo }
    }

    /// Makes the pid file path a non-empty directory, so the held write's
    /// rename fails once it is let through.
    pub fn block_rename(&self, lab: &Lab) -> Blocked {
        let pid_file = &lab.pid_file;
        let before = fs::read(pid_file).expect("the pid file was written before the hold");
        *lab.stand_in.lock().unwrap() = Some(before.clone());
        fs::remove_file(pid_file).expect("the pid file is removable");
        fs::create_dir(pid_file).expect("the pid file path takes a directory");
        fs::write(pid_file.join("keep"), "").expect("the directory is writable");
        Blocked {
            pid_file: pid_file.to_owned(),
            before,
        }
    }

    /// Reads the FIFO, which lets the held writer through. With
    /// [`Release::FailRename`] the failed rename leaves the FIFO in place, so
    /// it keeps letting writers through until [`Released::stop`]. `Err` when
    /// no write opened the FIFO within [`SETTLE`]: the hold never held one.
    pub fn release(self, how: Release) -> Result<Drained, String> {
        let fifo = self.fifo.clone();
        let (tx, rx) = mpsc::channel();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let reader = std::thread::spawn(move || {
            loop {
                let read = fs::read(&fifo);
                let failed = read.is_err();
                let _ = tx.send(read);
                if how == Release::Write
                    || failed
                    || stopping.load(std::sync::atomic::Ordering::SeqCst)
                {
                    return;
                }
            }
        });
        let released = Released {
            fifo: self.fifo.clone(),
            stop,
            reader,
        };
        match rx.recv_timeout(SETTLE) {
            Ok(Ok(first)) => {
                if how == Release::Write {
                    // The rename moves the FIFO onto the pid file; it must not
                    // be removed under the writer.
                    released
                        .reader
                        .join()
                        .expect("the FIFO reader does not panic");
                    return Ok(Released::done(first));
                }
                Ok(released.with_first(first))
            }
            Ok(Err(e)) => {
                released.stop();
                Err(format!("the held FIFO cannot be read: {e}"))
            }
            Err(_) => {
                released.stop();
                Err(format!(
                    "no pid-file write opened the held temporary path {:?} within {SETTLE:?}",
                    self.fifo
                ))
            }
        }
    }
}

/// A released hold whose FIFO may still let writers through.
pub struct Released {
    fifo: PathBuf,
    stop: Arc<std::sync::atomic::AtomicBool>,
    reader: std::thread::JoinHandle<()>,
}

/// What the held write sent, and the reader that still drains the FIFO.
pub struct Drained {
    pub first: Vec<u8>,
    rest: Option<Released>,
}

impl Released {
    fn done(first: Vec<u8>) -> Drained {
        Drained { first, rest: None }
    }

    fn with_first(self, first: Vec<u8>) -> Drained {
        Drained {
            first,
            rest: Some(self),
        }
    }

    /// Ends the reader: opening the write end lets a reader that waits in
    /// `open` see an end of file. Then removes the FIFO, if still there.
    fn stop(self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        let path = CString::new(self.fifo.to_str().expect("utf-8")).unwrap();
        while !self.reader.is_finished() {
            // SAFETY: a valid C path; the descriptor is closed at once.
            unsafe {
                let fd = libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_NONBLOCK);
                if fd >= 0 {
                    libc::close(fd);
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        self.reader.join().expect("the FIFO reader does not panic");
        if fs::symlink_metadata(&self.fifo).is_ok_and(|m| m.file_type().is_fifo()) {
            fs::remove_file(&self.fifo).expect("the FIFO is removable");
        }
    }
}

impl Drained {
    /// Stops letting writers through the FIFO.
    pub fn stop(self) {
        if let Some(rest) = self.rest {
            rest.stop();
        }
    }
}
