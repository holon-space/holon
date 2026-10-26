//! One hosted net: its executor behind a [`Shell`], its shell effects and its
//! probes, and the loop that runs them.

use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::time::Duration;

use holon_net::exec::Cause;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::ExecError;
use holon_net::file::EffectSpec;
use holon_net::file::NetFile;
use holon_net::file::Outcome;
use holon_net::load::NetLoadError;
use holon_net::load::net_def;
use holon_net::probe::ProbeOutcome;
use holon_net::probe::Probes;
use holon_net_shell::Shell;
use holon_net_shell::effect::Ended;
use holon_net_shell::effect::Runner;
use holon_net_shell::probe;
use tokio::sync::mpsc;

use crate::NetdError;

/// How often the loop looks at the clock for due probes and timers.
// shortcut: a fixed tick instead of sleeping until the next due time, until a
// net needs timers finer than the tick or many idle nets cost CPU.
const TICK: Duration = Duration::from_millis(100);

pub struct HostedNet {
    pub(crate) file: NetFile,
    pub(crate) shell: Shell,
    runner: Runner,
    probes: Mutex<Probes>,
}

/// A run that ended, for the loop to hand back to the net.
enum RunEnded {
    Effect(EffectRequest, EffectResult),
    Probe(String, ProbeOutcome),
}

impl HostedNet {
    /// Loads the net file at `path`. Its shell effects keep their pid file
    /// as `<state_dir>/<net>.pid`.
    pub fn load(path: &Path, state_dir: &Path) -> Result<Self, NetdError> {
        let shown = path.display().to_string();
        let text = std::fs::read_to_string(path).map_err(|e| NetLoadError::Unreadable {
            path: shown.clone(),
            message: e.to_string(),
        })?;
        let file = NetFile::parse(&text).map_err(|_| {
            net_def(&shown, &text, |t| {
                unreachable!("a refused file has no transition {t}")
            })
            .err()
            .expect("net_def refuses the file NetFile::parse refuses")
        })?;
        let net = net_def(&shown, &text, |t| crate::logic(&file.transitions[t]))?;
        let shell = Shell::new(net).map_err(|source| NetdError::Net {
            path: shown,
            source,
        })?;
        let (runner, leftovers) = Runner::open(state_dir.join(format!("{}.pid", file.name)))
            .map_err(|source| NetdError::Shell {
                net: file.name.clone(),
                source,
            })?;
        for group in leftovers {
            tracing::warn!(
                net = %file.name,
                "a shell effect of a netd that died is still running: process group {} ({} {})",
                group.pgid,
                group.transition,
                group.subject
            );
        }
        let probes = Mutex::new(Probes::new(&file.probes, shell.now()));
        Ok(Self {
            file,
            shell,
            runner,
            probes,
        })
    }

    pub fn name(&self) -> &str {
        &self.file.name
    }

    pub(crate) fn probes(&self) -> MutexGuard<'_, Probes> {
        self.probes
            .lock()
            .expect("the probe state lock is poisoned")
    }

    /// Runs every probe once and commits its facts: the marking a start
    /// serves from.
    pub async fn probe_once(&self) -> Result<(), NetdError> {
        let due = self.probes().due(self.shell.now());
        for (probe, command) in due {
            let outcome = probe::run(&command).await;
            self.settle_probe(&probe, outcome)?;
        }
        Ok(())
    }

    fn settle_probe(&self, probe: &str, outcome: ProbeOutcome) -> Result<(), NetdError> {
        self.shell
            .apply(|exec, now| self.probes().settle(exec, probe, outcome, now))
            .map_err(|e| self.stopped(e))
    }

    fn stopped(&self, e: ExecError) -> NetdError {
        NetdError::Stopped {
            net: self.file.name.clone(),
            message: e.to_string(),
        }
    }

    /// Runs the net's effects and probes as they come due, until the net
    /// stops with an error.
    pub async fn drive(self: Arc<Self>) -> Result<(), NetdError> {
        let (ended, mut ends) = mpsc::unbounded_channel();
        loop {
            for request in self.shell.step().map_err(|e| self.stopped(e))? {
                let (net, ended) = (Arc::clone(&self), ended.clone());
                tokio::spawn(async move {
                    let result = net.run_effect(&request).await;
                    let _ = ended.send(RunEnded::Effect(request, result));
                });
            }
            let due = self.probes().due(self.shell.now());
            for (probe, command) in due {
                let ended = ended.clone();
                tokio::spawn(async move {
                    let outcome = probe::run(&command).await;
                    let _ = ended.send(RunEnded::Probe(probe, outcome));
                });
            }
            tokio::select! {
                end = ends.recv() => match end.expect("the loop holds a sender") {
                    RunEnded::Effect(request, result) => self
                        .shell
                        .complete(&request, result)
                        .map_err(|e| self.stopped(e))?,
                    RunEnded::Probe(probe, outcome) => self.settle_probe(&probe, outcome)?,
                },
                () = self.shell.fired() => {}
                () = tokio::time::sleep(TICK) => {}
            }
        }
    }

    /// The result of one attempt. A run without an exit code, killed by its
    /// command timeout or a signal, may have done part of its work, so its
    /// outcome is unknown and the transition's receiver decides.
    async fn run_effect(&self, request: &EffectRequest) -> EffectResult {
        let spec = &self.file.transitions[&*request.transition];
        let effect = spec
            .effect
            .as_ref()
            .expect("the executor asks for effects of effect transitions only");
        let EffectSpec::Shell(shell) = &self.file.effects[effect];
        let finished = match self.runner.run(shell, request).await {
            Ok(finished) => finished,
            Err(e) => return EffectResult::Failed(Cause::definite(e.to_string())),
        };
        let log = finished.log_path.display();
        match finished.ended {
            Ended::Exited { code, outcome } => match outcome {
                Outcome::Done => EffectResult::Done(crate::outputs(spec)(&request.subject)),
                Outcome::Failed => {
                    EffectResult::Failed(Cause::retryable(format!("exit {code}, log {log}")))
                }
                Outcome::OutcomeUnknown => {
                    EffectResult::Unknown(Cause::definite(format!("exit {code}, log {log}")))
                }
            },
            ended => EffectResult::Unknown(Cause::definite(format!(
                "{ended:?} without an exit code, log {log}"
            ))),
        }
    }
}
