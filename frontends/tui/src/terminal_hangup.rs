//! A terminal can die without a SIGHUP reaching the TUI: a launcher that
//! ignores SIGHUP (`nohup`, a shell with `trap '' HUP`) takes the signal and
//! keeps waiting. The terminal's hangup is then visible only on stdin.

use std::time::Duration;

const POLL_PERIOD: Duration = Duration::from_millis(200);

/// Resolves once the terminal on stdin has hung up.
pub async fn hung_up() -> std::io::Result<()> {
    let mut tick = tokio::time::interval(POLL_PERIOD);
    loop {
        tick.tick().await;
        if stdin_hung_up()? {
            return Ok(());
        }
    }
}

/// Reads nothing, so the event loop keeps every key.
fn stdin_hung_up() -> std::io::Result<bool> {
    let mut stdin = libc::pollfd {
        fd: libc::STDIN_FILENO,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one valid `pollfd`, a count of one, and no wait.
    if unsafe { libc::poll(&mut stdin, 1, 0) } < 0 {
        let e = std::io::Error::last_os_error();
        return match e.kind() {
            std::io::ErrorKind::Interrupted => Ok(false),
            _ => Err(e),
        };
    }
    Ok(stdin.revents & libc::POLLHUP != 0)
}
