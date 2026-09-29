//! launchd stops `holon-mcp` with SIGTERM. The server must answer it through
//! the session shutdown — write back what it owes, save, close, give up the
//! vault — and exit cleanly, not die on the default signal action with its
//! write-back backlog unwritten.
//!
//! @pbt kind harness
//! @pbt covers mcp-sigterm-shutdown — SIGTERM ends `holon-mcp` through
//! `shutdown_session` with exit status 0

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

#[test]
fn sigterm_ends_the_server_through_the_session_shutdown() {
    let vault = tempfile::tempdir().expect("vault dir");
    std::fs::write(vault.path().join("page.org"), VAULT_ORG).expect("write the page");
    let state = tempfile::tempdir().expect("state dir");
    let log = state.path().join("holon-mcp.log");

    // HOME points into the state dir, so the config dir the binary resolves is
    // a throwaway one and never the developer's own.
    let mut child = Command::new(env!("CARGO_BIN_EXE_holon-mcp"))
        .arg("--orgmode-root")
        .arg(vault.path())
        .arg(state.path().join("holon.db"))
        .env("HOME", state.path())
        .env("HOLON_MCP_LOG_FILE", &log)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn holon-mcp");

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        if text.contains(BOOTED) {
            break;
        }
        if let Some(status) = child.try_wait().expect("poll holon-mcp") {
            let mut stderr = String::new();
            child
                .stderr
                .take()
                .expect("stderr piped")
                .read_to_string(&mut stderr)
                .expect("read stderr");
            panic!("holon-mcp exited during boot with {status}:\n{stderr}\nlog:\n{text}");
        }
        assert!(
            Instant::now() < deadline,
            "holon-mcp did not boot within 120 s; log:\n{text}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // The child is not reaped yet, so its pid cannot name another process.
    let sent = Command::new("kill")
        .arg("-TERM")
        .arg(child.id().to_string())
        .status()
        .expect("run kill");
    assert!(
        sent.success(),
        "sending SIGTERM to holon-mcp failed: {sent}"
    );

    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll holon-mcp") {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().expect("kill the hung holon-mcp");
            let text = std::fs::read_to_string(&log).unwrap_or_default();
            let tail: Vec<&str> = text.lines().rev().take(40).collect();
            panic!(
                "holon-mcp did not exit within 60 s of SIGTERM; last log lines:\n{}",
                tail.into_iter().rev().collect::<Vec<_>>().join("\n")
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
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
