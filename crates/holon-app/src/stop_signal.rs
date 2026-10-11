//! The signals that ask a Holon process to stop. Every frontend installs them
//! before its boot and answers each one through `shutdown_session`.

use anyhow::Context;
use anyhow::Result;
#[cfg(unix)]
use tokio::signal::unix::Signal;
#[cfg(unix)]
use tokio::signal::unix::SignalKind;
#[cfg(unix)]
use tokio::signal::unix::signal;
#[cfg(windows)]
use tokio::signal::windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopSignal {
    /// Ctrl+C in the terminal.
    Interrupt,
    /// `kill`, launchd's stop, a logout.
    Terminate,
    /// The controlling terminal closed.
    Hangup,
}

#[cfg(unix)]
const TERMINATE_NAME: &str = "SIGTERM";
#[cfg(unix)]
const HANGUP_NAME: &str = "SIGHUP";
/// Windows console events: `Terminate` is system shutdown, `Hangup` is the
/// console closing.
#[cfg(windows)]
const TERMINATE_NAME: &str = "console shutdown";
#[cfg(windows)]
const HANGUP_NAME: &str = "console close";

impl std::fmt::Display for StopSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            StopSignal::Interrupt => "Ctrl+C",
            StopSignal::Terminate => TERMINATE_NAME,
            StopSignal::Hangup => HANGUP_NAME,
        })
    }
}

/// Handlers for every [`StopSignal`]. A signal that arrives after
/// [`StopSignals::install`] and before [`StopSignals::recv`] is kept, not
/// answered with the default kill.
pub struct StopSignals {
    #[cfg(unix)]
    interrupt: Signal,
    #[cfg(unix)]
    terminate: Signal,
    #[cfg(unix)]
    hangup: Signal,
    #[cfg(windows)]
    interrupt: windows::CtrlC,
    #[cfg(windows)]
    terminate: windows::CtrlShutdown,
    #[cfg(windows)]
    hangup: windows::CtrlClose,
}

impl StopSignals {
    /// Must run inside a tokio runtime.
    #[cfg(unix)]
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

    /// Must run inside a tokio runtime.
    #[cfg(windows)]
    pub fn install() -> Result<Self> {
        Ok(Self {
            interrupt: windows::ctrl_c().context("installing the Ctrl+C handler")?,
            terminate: windows::ctrl_shutdown().context("installing the shutdown handler")?,
            hangup: windows::ctrl_close().context("installing the console-close handler")?,
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
