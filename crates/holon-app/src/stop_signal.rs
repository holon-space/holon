//! The signals that ask a Holon process to stop. Every frontend installs them
//! before its boot and answers each one through `shutdown_session`.

use anyhow::Context;
use anyhow::Result;
use tokio::signal::unix::Signal;
use tokio::signal::unix::SignalKind;
use tokio::signal::unix::signal;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopSignal {
    /// Ctrl+C in the terminal.
    Interrupt,
    /// `kill`, launchd's stop, a logout.
    Terminate,
    /// The controlling terminal closed.
    Hangup,
}

impl std::fmt::Display for StopSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StopSignal::Interrupt => "Ctrl+C",
            StopSignal::Terminate => "SIGTERM",
            StopSignal::Hangup => "SIGHUP",
        })
    }
}

/// Handlers for every [`StopSignal`]. A signal that arrives after
/// [`StopSignals::install`] and before [`StopSignals::recv`] is kept, not
/// answered with the default kill.
pub struct StopSignals {
    interrupt: Signal,
    terminate: Signal,
    hangup: Signal,
}

impl StopSignals {
    /// Must run inside a tokio runtime.
    pub fn install() -> Result<Self> {
        let install = |kind: SignalKind, name: &str| {
            signal(kind).with_context(|| format!("installing the {name} handler"))
        };
        Ok(Self {
            interrupt: install(SignalKind::interrupt(), "SIGINT")?,
            terminate: install(SignalKind::terminate(), "SIGTERM")?,
            hangup: install(SignalKind::hangup(), "SIGHUP")?,
        })
    }

    pub async fn recv(&mut self) -> StopSignal {
        tokio::select! {
            _ = self.interrupt.recv() => StopSignal::Interrupt,
            _ = self.terminate.recv() => StopSignal::Terminate,
            _ = self.hangup.recv() => StopSignal::Hangup,
        }
    }

    /// The signal that has already arrived, if any.
    pub fn arrived(&mut self) -> Option<StopSignal> {
        futures::FutureExt::now_or_never(self.recv())
    }
}
