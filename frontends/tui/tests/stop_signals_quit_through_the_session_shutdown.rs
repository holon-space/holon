//! Every stop signal ends the TUI through the session shutdown: the session
//! writes back what it owes, saves, closes and gives up the vault before the
//! process exits. The TUI runs on a pseudo-terminal from `script`, since it
//! needs one to start.
//!
//! @pbt kind harness
//! @pbt covers tui-quit-shutdown — SIGINT, SIGTERM and SIGHUP, during boot or
//! while the TUI runs, end `holon-tui` through `shutdown_session` with exit
//! status 0

use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

const VAULT_ORG: &str = "\
* Stop page
:PROPERTIES:
:ID: stop-page
:END:
";

const BOOTING: &str = "Starting TUI frontend...";
const BOOTED: &str = "Session ready";
const SHUT_DOWN: &str = "session shut down";

struct Tui {
    script: Child,
    log: std::path::PathBuf,
    _vault: tempfile::TempDir,
    _state: tempfile::TempDir,
}

impl Tui {
    /// The TUI always serves MCP, so each instance gets its own `mcp_port`.
    fn spawn(mcp_port: u16) -> Self {
        let vault = tempfile::tempdir().expect("vault dir");
        std::fs::write(vault.path().join("page.org"), VAULT_ORG).expect("write the page");
        let state = tempfile::tempdir().expect("state dir");
        let log = state.path().join("holon-tui.log");

        // Every input is a flag or points into the state dir, so no setting of
        // the developer's own reaches this instance.
        let mut command = Command::new("script");
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("HOLON_") || key == "MCP_SERVER_PORT" {
                command.env_remove(key);
            }
        }
        // The pseudo-terminal starts at 0 x 0, below the size the TUI renders at.
        let script = command
            .arg("-q")
            .arg("/dev/null")
            .arg("sh")
            .arg("-c")
            .arg("stty rows 40 cols 120 && exec \"$0\" \"$@\"")
            .arg(env!("CARGO_BIN_EXE_holon-tui"))
            .arg("--config-dir")
            .arg(state.path().join("config"))
            .arg("--db-path")
            .arg(state.path().join("holon.db"))
            .arg("--vault-root")
            .arg(vault.path())
            .env("HOME", state.path())
            .env("MCP_SERVER_PORT", mcp_port.to_string())
            .env("TERM", "xterm-256color")
            .env("HOLON_LOG", format!("file://{}", log.display()))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn holon-tui under script");
        Self {
            script,
            log,
            _vault: vault,
            _state: state,
        }
    }

    fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn wait_for_log(&mut self, line: &str) {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let text = self.log_text();
            if text.contains(line) {
                return;
            }
            if let Some(status) = self.script.try_wait().expect("poll script") {
                panic!("holon-tui exited before `{line}` with {status}; log:\n{text}");
            }
            assert!(
                Instant::now() < deadline,
                "holon-tui did not log `{line}` within 180 s; log:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn tui_pid(&self) -> String {
        let out = Command::new("pgrep")
            .arg("-P")
            .arg(self.script.id().to_string())
            .output()
            .expect("run pgrep");
        let pids = String::from_utf8(out.stdout).expect("pgrep prints pids");
        let pids: Vec<&str> = pids.lines().collect();
        assert_eq!(
            pids.len(),
            1,
            "script must run exactly one child, holon-tui; got {pids:?}"
        );
        pids[0].to_string()
    }

    fn signal(&self, name: &str) {
        let sent = Command::new("kill")
            .arg(format!("-{name}"))
            .arg(self.tui_pid())
            .status()
            .expect("run kill");
        assert!(
            sent.success(),
            "sending SIG{name} to holon-tui failed: {sent}"
        );
    }

    /// `script` exits with its child's status, and with 128 + the signal
    /// number when a signal killed the child.
    fn wait_for_exit(&mut self, name: &str) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if let Some(status) = self.script.try_wait().expect("poll script") {
                return status;
            }
            if Instant::now() >= deadline {
                self.script.kill().expect("kill the hung script");
                panic!(
                    "holon-tui did not exit within 180 s of SIG{name}; log:\n{}",
                    self.log_text()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn a_signal_quits_through_the_session_shutdown(name: &str, after: &str, mcp_port: u16) {
    let mut tui = Tui::spawn(mcp_port);
    tui.wait_for_log(after);
    tui.signal(name);
    let status = tui.wait_for_exit(name);
    let text = tui.log_text();
    assert!(
        status.success() && text.contains(SHUT_DOWN),
        "SIG{name} after `{after}` must end holon-tui through the session shutdown with status \
         0, got {status}; log:\n{text}"
    );
}

#[test]
fn ctrl_c_ends_the_tui_through_the_session_shutdown() {
    a_signal_quits_through_the_session_shutdown("INT", BOOTED, 18761);
}

#[test]
fn sigterm_ends_the_tui_through_the_session_shutdown() {
    a_signal_quits_through_the_session_shutdown("TERM", BOOTED, 18762);
}

#[test]
fn sighup_ends_the_tui_through_the_session_shutdown() {
    a_signal_quits_through_the_session_shutdown("HUP", BOOTED, 18763);
}

#[test]
fn sigterm_during_boot_ends_the_tui_through_the_session_shutdown() {
    a_signal_quits_through_the_session_shutdown("TERM", BOOTING, 18764);
}
