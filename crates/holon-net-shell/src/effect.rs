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
//! it.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;

use holon_net::exec::AttemptId;
use holon_net::exec::EffectRequest;
use holon_net::file::ExitCodeMap;
use holon_net::file::Outcome;
use holon_net::file::ShellEffect;
use tokio::process::Child;
use tokio::process::Command;
use tokio::sync::Notify;

/// How long a group has after SIGTERM before it gets SIGKILL.
pub const KILL_GRACE: Duration = Duration::from_secs(2);

const POLL: Duration = Duration::from_millis(20);

/// A process group id. Negated, it addresses the whole group.
pub type Pgid = i32;

/// One process group the runner started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub pgid: Pgid,
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
    #[error("the shell effect `{argv}` does not start: {source}")]
    Spawn { argv: String, source: io::Error },
    #[error("the child of the shell effect `{argv}` cannot be waited for: {source}")]
    Wait { argv: String, source: io::Error },
}

/// Runs the shell effects of one net and owns their process groups.
pub struct Runner {
    pid_file: PathBuf,
    live: Mutex<BTreeMap<AttemptId, Live>>,
}

struct Live {
    group: Group,
    abandon: Arc<Notify>,
}

/// Why a run is over.
enum Stop {
    Exited(ExitStatus),
    Kill(Kill),
}

impl Runner {
    /// Opens the pid file at `pid_file`. The groups it already names that are
    /// still running are the leftovers of a runner that died without reaping
    /// them; they are reported, and the file then holds this runner's runs.
    pub fn open(pid_file: PathBuf) -> Result<(Self, Vec<Group>), ShellError> {
        let left = match fs::read_to_string(&pid_file) {
            Ok(text) => parse_groups(&text, &pid_file)?,
            Err(source) if source.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(source) => {
                return Err(ShellError::PidFile {
                    path: pid_file,
                    source,
                });
            }
        };
        let runner = Self {
            pid_file,
            live: Mutex::new(BTreeMap::new()),
        };
        runner.write_pid_file()?;
        Ok((runner, left.into_iter().filter(|g| alive(g.pgid)).collect()))
    }

    /// Runs one attempt of `effect` as its own process group, with both its
    /// streams in the log its `log_path` names for this request.
    pub async fn run(
        &self,
        effect: &ShellEffect,
        request: &EffectRequest,
    ) -> Result<Finished, ShellError> {
        let argv = effect.command.argv.join(" ");
        let log_path = log_path(effect, request);
        let (mut child, pgid) = spawn(effect, &log_path)?;
        let abandon = self.register(request, pgid)?;
        let stop = tokio::select! {
            status = child.wait() => Stop::Exited(status.map_err(|source| ShellError::Wait {
                argv: argv.clone(),
                source,
            })?),
            () = tokio::time::sleep(effect.command.timeout.duration()) => Stop::Kill(Kill::Timeout),
            () = abandon.notified() => Stop::Kill(Kill::Abandoned),
        };
        let ended = match stop {
            Stop::Exited(status) => exited(status, &effect.exit_codes),
            Stop::Kill(kill) => {
                end_group(pgid, Some(&mut child))
                    .await
                    .map_err(|source| ShellError::Wait {
                        argv: argv.clone(),
                        source,
                    })?;
                Ended::Killed(kill)
            }
        };
        self.forget(request.attempt)?;
        Ok(Finished { ended, log_path })
    }

    /// The groups running right now.
    pub fn in_flight(&self) -> Vec<Group> {
        self.live()
            .values()
            .map(|live| live.group.clone())
            .collect()
    }

    /// Kills the group of `attempt`, whose [`Runner::run`] then reports
    /// [`Kill::Abandoned`]. `false` when the attempt is no longer running.
    pub fn abandon(&self, attempt: AttemptId) -> bool {
        let Some(abandon) = self.live().get(&attempt).map(|l| Arc::clone(&l.abandon)) else {
            return false;
        };
        abandon.notify_one();
        true
    }

    /// Takes the group into the pid file, which is what a restart reads: a
    /// run whose group the file cannot hold is killed.
    fn register(&self, request: &EffectRequest, pgid: Pgid) -> Result<Arc<Notify>, ShellError> {
        let abandon = Arc::new(Notify::new());
        self.live().insert(
            request.attempt,
            Live {
                group: Group {
                    pgid,
                    attempt: request.attempt,
                    transition: request.transition.to_string(),
                    subject: request.subject.to_string(),
                },
                abandon: Arc::clone(&abandon),
            },
        );
        match self.write_pid_file() {
            Ok(()) => Ok(abandon),
            Err(e) => {
                signal_group(pgid, libc::SIGKILL);
                self.live().remove(&request.attempt);
                Err(e)
            }
        }
    }

    fn forget(&self, attempt: AttemptId) -> Result<(), ShellError> {
        self.live().remove(&attempt);
        self.write_pid_file()
    }

    fn live(&self) -> MutexGuard<'_, BTreeMap<AttemptId, Live>> {
        self.live
            .lock()
            .expect("the shell effect's group lock is poisoned")
    }

    fn write_pid_file(&self) -> Result<(), ShellError> {
        let text: String = self
            .live()
            .values()
            .map(|live| {
                let g = &live.group;
                let line = serde_json::to_string(&(g.pgid, g.attempt.0, &g.transition, &g.subject))
                    .expect("a group is serializable");
                line + "\n"
            })
            .collect();
        let temp = self.pid_file.with_extension("writing");
        let failed = |path: &Path, source: io::Error| ShellError::PidFile {
            path: path.to_owned(),
            source,
        };
        fs::write(&temp, text).map_err(|source| failed(&temp, source))?;
        fs::rename(&temp, &self.pid_file).map_err(|source| failed(&self.pid_file, source))
    }
}

/// Ends a group this runner did not spawn: there is no child to reap.
pub async fn kill_group(pgid: Pgid) {
    end_group(pgid, None)
        .await
        .expect("a group without a child has nothing to wait for");
}

/// SIGTERM to the whole group, then SIGKILL to it when a member is left at
/// the end of the grace period. The grace period waits on the group, not on
/// the child: a member that ignores SIGTERM outlives the child.
async fn end_group(pgid: Pgid, mut child: Option<&mut Child>) -> io::Result<()> {
    signal_group(pgid, libc::SIGTERM);
    let deadline = tokio::time::Instant::now() + KILL_GRACE;
    loop {
        if let Some(child) = child.as_deref_mut() {
            child.try_wait()?;
        }
        if !alive(pgid) || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    if alive(pgid) {
        signal_group(pgid, libc::SIGKILL);
    }
    if let Some(child) = child {
        child.wait().await?;
    }
    Ok(())
}

fn spawn(effect: &ShellEffect, log_path: &Path) -> Result<(Child, Pgid), ShellError> {
    let log = create_log(log_path)?;
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
    let child = spawn.spawn().map_err(|source| ShellError::Spawn {
        argv: command.argv.join(" "),
        source,
    })?;
    let pgid = child
        .id()
        .expect("a child that has not been waited for has a pid") as Pgid;
    Ok((child, pgid))
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

fn log_path(effect: &ShellEffect, request: &EffectRequest) -> PathBuf {
    let expanded = effect
        .log_path
        .replace("{transition}", &request.transition)
        .replace("{subject}", request.subject.as_str())
        .replace("{attempt}", &request.attempt.0.to_string())
        .replace("{occurrence}", &request.occurrence.to_string());
    Path::new(&effect.command.working_dir).join(expanded)
}

fn create_log(path: &Path) -> Result<fs::File, ShellError> {
    let failed = |source| ShellError::Log {
        path: path.to_owned(),
        source,
    };
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(failed)?;
    }
    fs::File::create(path).map_err(failed)
}

fn parse_groups(text: &str, path: &Path) -> Result<Vec<Group>, ShellError> {
    text.lines()
        .map(|line| {
            let refused = || ShellError::PidFileLine {
                path: path.to_owned(),
                line: line.to_owned(),
            };
            let (pgid, attempt, transition, subject): (Pgid, u64, String, String) =
                serde_json::from_str(line).map_err(|_| refused())?;
            Ok(Group {
                pgid,
                attempt: AttemptId(attempt),
                transition,
                subject,
            })
        })
        .collect()
}

/// Whether the group still has a member, a zombie one included.
fn alive(pgid: Pgid) -> bool {
    signal_group(pgid, 0)
}

/// `true` while the group exists.
fn signal_group(pgid: Pgid, signal: i32) -> bool {
    assert!(
        pgid > 1,
        "a signal to the process group {pgid} is not a signal to one group"
    );
    // SAFETY: a negated pgid addresses that process group and nothing else.
    unsafe { libc::kill(-pgid, signal) == 0 }
}
