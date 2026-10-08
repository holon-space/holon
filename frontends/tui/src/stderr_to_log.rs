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
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;

/// Stderr appends to an owner-only file until [`StderrToLog::restore`]. A
/// panic until then is also written to the terminal.
pub struct StderrToLog {
    terminal: Arc<File>,
    redirected: Arc<AtomicBool>,
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

        let redirected = Arc::new(AtomicBool::new(true));
        let previous_hook = std::panic::take_hook();
        let (to_terminal, still_redirected) = (Arc::clone(&terminal), Arc::clone(&redirected));
        // Stays installed after `restore`: hooks installed on top of it chain it.
        std::panic::set_hook(Box::new(move |info| {
            previous_hook(info);
            if still_redirected.load(Ordering::SeqCst) {
                // ALLOW(ok): the previous hook has logged the panic; a dead terminal takes
                // nothing.
                let _ = writeln!(&*to_terminal, "holon-tui {info}");
            }
        }));
        Ok(Self {
            terminal,
            redirected,
        })
    }

    /// Points stderr back at the descriptor it had before [`Self::redirect`].
    pub fn restore(self) -> io::Result<()> {
        point_stderr_at(self.terminal.as_raw_fd())?;
        self.redirected.store(false, Ordering::SeqCst);
        Ok(())
    }
}

fn point_stderr_at(fd: RawFd) -> io::Result<()> {
    // SAFETY: `fd` is open for the whole call; `dup2` only duplicates it.
    if unsafe { libc::dup2(fd, libc::STDERR_FILENO) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
