//! While the TUI runs, stderr belongs in the log: a write to the terminal's
//! stderr corrupts the screen, and once the terminal is dead `eprintln!`
//! panics, which skips the session shutdown.

use std::fs::File;
use std::fs::OpenOptions;
use std::fs::Permissions;
use std::io;
use std::io::Write;
use std::os::fd::AsFd;
use std::os::fd::AsRawFd;
use std::os::fd::RawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::PermissionsExt;
use std::panic::PanicHookInfo;
use std::path::Path;
use std::sync::Arc;

type PanicHook = Box<dyn Fn(&PanicHookInfo<'_>) + Send + Sync>;

/// Stderr appends to an owner-only file until [`StderrToLog::restore`]. A
/// panic until then is also written to the terminal.
pub struct StderrToLog {
    terminal: Arc<File>,
    previous_hook: Arc<PanicHook>,
}

impl StderrToLog {
    pub fn redirect(log: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(log)?;
        file.set_permissions(Permissions::from_mode(0o600))?;
        let terminal = Arc::new(File::from(io::stderr().as_fd().try_clone_to_owned()?));
        point_stderr_at(file.as_raw_fd())?;

        let previous_hook: Arc<PanicHook> = Arc::new(std::panic::take_hook());
        let (to_terminal, then) = (Arc::clone(&terminal), Arc::clone(&previous_hook));
        std::panic::set_hook(Box::new(move |info| {
            then(info);
            // ALLOW(ok): the previous hook has logged the panic; a dead terminal takes
            // nothing.
            let _ = writeln!(&*to_terminal, "holon-tui {info}");
        }));
        Ok(Self {
            terminal,
            previous_hook,
        })
    }

    /// Points stderr back at the descriptor it had before [`Self::redirect`].
    pub fn restore(self) -> io::Result<()> {
        drop(std::panic::take_hook());
        let previous_hook = self.previous_hook;
        std::panic::set_hook(Box::new(move |info| previous_hook(info)));
        point_stderr_at(self.terminal.as_raw_fd())
    }
}

fn point_stderr_at(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is open for the whole call; `dup2` only duplicates it.
    if unsafe { libc::dup2(fd, libc::STDERR_FILENO) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
