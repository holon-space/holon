//! launchd stops `holon-mcp` with SIGTERM. The server must answer it through
//! the session shutdown — write back what it owes, save, close, give up the
//! vault — and exit cleanly, not die on the default signal action with its
//! write-back backlog unwritten. A SIGTERM during boot lets the boot finish,
//! then takes the same shutdown, and nothing is served.
//!
//! @pbt kind harness
//! @pbt covers mcp-sigterm-shutdown — SIGTERM, during boot or while serving,
//! and SIGHUP end `holon-mcp` through `shutdown_session` with exit status 0

use std::io::Read;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

const VAULT_ORG: &str = "\
* Sigterm page
:PROPERTIES:
:ID: sigterm-page
:END:
";

const BOOTED: &str = "holon-mcp: session booted, serving stdio";
const BOOTING: &str = "holon-mcp: booting the session";
const SHUT_DOWN: &str = "session shut down";

fn spawn_on(vault: &std::path::Path, state: &std::path::Path) -> std::process::Child {
    // HOME points into the state dir, so the config dir the binary resolves is
    // a throwaway one and never the developer's own.
    Command::new(env!("CARGO_BIN_EXE_holon-mcp"))
        .arg("--orgmode-root")
        .arg(vault)
        .arg(state.join("holon.db"))
        .env("HOME", state)
        .env("HOLON_MCP_LOG_FILE", state.join("holon-mcp.log"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn holon-mcp")
}

fn wait_for_log(child: &mut std::process::Child, log: &std::path::Path, line: &str) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let text = std::fs::read_to_string(log).unwrap_or_default();
        if text.contains(line) {
            return;
        }
        if let Some(status) = child.try_wait().expect("poll holon-mcp") {
            let mut stderr = String::new();
            child
                .stderr
                .take()
                .expect("stderr piped")
                .read_to_string(&mut stderr)
                .expect("read stderr");
            panic!("holon-mcp exited before `{line}` with {status}:\n{stderr}\nlog:\n{text}");
        }
        assert!(
            Instant::now() < deadline,
            "holon-mcp did not log `{line}` within 120 s; log:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn signal(child: &std::process::Child, name: &str) {
    // The child is not reaped yet, so its pid cannot name another process.
    let sent = Command::new("kill")
        .arg(format!("-{name}"))
        .arg(child.id().to_string())
        .status()
        .expect("run kill");
    assert!(
        sent.success(),
        "sending SIG{name} to holon-mcp failed: {sent}"
    );
}

fn wait_for_exit(
    child: &mut std::process::Child,
    log: &std::path::Path,
) -> std::process::ExitStatus {
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        if let Some(status) = child.try_wait().expect("poll holon-mcp") {
            return status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill the hung holon-mcp");
            let text = std::fs::read_to_string(log).unwrap_or_default();
            let tail: Vec<&str> = text.lines().rev().take(40).collect();
            panic!(
                "holon-mcp did not exit within 180 s of SIGTERM; last log lines:\n{}",
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn sigterm_during_boot_ends_the_boot_through_the_session_shutdown() {
    let vault = tempfile::tempdir().expect("vault dir");
    std::fs::write(vault.path().join("page.org"), VAULT_ORG).expect("write the page");
    let state = tempfile::tempdir().expect("state dir");
    let log = state.path().join("holon-mcp.log");

    let mut child = spawn_on(vault.path(), state.path());
    wait_for_log(&mut child, &log, BOOTING);
    signal(&child, "TERM");
    let status = wait_for_exit(&mut child, &log);

    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        status.success(),
        "SIGTERM during boot must end holon-mcp through the session shutdown with status 0, \
         got {status}; log:\n{text}"
    );
    assert!(
        text.contains("Received SIGTERM") && text.contains(SHUT_DOWN),
        "the log does not show the boot ending through the session shutdown; log:\n{text}"
    );
    assert!(
        !text.contains(BOOTED),
        "a server stopped during boot must not start serving; log:\n{text}"
    );
}

#[test]
fn sigterm_ends_the_server_through_the_session_shutdown() {
    let vault = tempfile::tempdir().expect("vault dir");
    std::fs::write(vault.path().join("page.org"), VAULT_ORG).expect("write the page");
    let state = tempfile::tempdir().expect("state dir");
    let log = state.path().join("holon-mcp.log");

    let mut child = spawn_on(vault.path(), state.path());
    wait_for_log(&mut child, &log, BOOTED);
    signal(&child, "TERM");
    let status = wait_for_exit(&mut child, &log);

    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        status.success(),
        "SIGTERM must end holon-mcp through the session shutdown with status 0, got {status}; \
         log:\n{text}"
    );
    assert!(
        text.contains("Received SIGTERM"),
        "the log does not show the SIGTERM handler; log:\n{text}"
    );
}

#[test]
fn sighup_ends_the_server_through_the_session_shutdown() {
    let vault = tempfile::tempdir().expect("vault dir");
    std::fs::write(vault.path().join("page.org"), VAULT_ORG).expect("write the page");
    let state = tempfile::tempdir().expect("state dir");
    let log = state.path().join("holon-mcp.log");

    let mut child = spawn_on(vault.path(), state.path());
    wait_for_log(&mut child, &log, BOOTED);
    signal(&child, "HUP");
    let status = wait_for_exit(&mut child, &log);

    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        status.success() && text.contains("Received SIGHUP") && text.contains(SHUT_DOWN),
        "SIGHUP must end holon-mcp through the session shutdown with status 0, got {status}; \
         log:\n{text}"
    );
}
