//! Every stop signal quits the GUI the way closing its window does: the session
//! writes back what it owes, saves, closes and gives up the vault before the
//! process exits. A signal during boot lets the boot finish, then takes the
//! same shutdown, and no window opens.
//!
//! @pbt kind harness
//! @pbt covers gui-quit-shutdown — SIGINT, SIGTERM and SIGHUP, during boot or
//! while the window is open, end `holon-gpui` through `shutdown_session` with
//! exit status 0

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

const BOOTING: &str = "Starting GPUI frontend...";
const BOOTED: &str = "Session ready";
const SHUT_DOWN: &str = "session shut down";

struct Gui {
    child: Child,
    log: std::path::PathBuf,
    _vault: tempfile::TempDir,
    _state: tempfile::TempDir,
}

impl Gui {
    fn spawn() -> Self {
        let vault = tempfile::tempdir().expect("vault dir");
        std::fs::write(vault.path().join("page.org"), VAULT_ORG).expect("write the page");
        let state = tempfile::tempdir().expect("state dir");
        let log = state.path().join("holon-gpui.log");

        // Every input is a flag or points into the state dir, so no setting of
        // the developer's own reaches this instance.
        let mut command = Command::new(env!("CARGO_BIN_EXE_holon-gpui"));
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("HOLON_") || key == "MCP_SERVER_PORT" {
                command.env_remove(key);
            }
        }
        let child = command
            .arg("--config-dir")
            .arg(state.path().join("config"))
            .arg("--db-path")
            .arg(state.path().join("holon.db"))
            .arg("--vault-root")
            .arg(vault.path())
            .arg("--mcp-enabled")
            .arg("false")
            .env("HOME", state.path())
            .env("HOLON_LOG", format!("file://{}", log.display()))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn holon-gpui");
        Self {
            child,
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
            if let Some(status) = self.child.try_wait().expect("poll holon-gpui") {
                panic!("holon-gpui exited before `{line}` with {status}; log:\n{text}");
            }
            assert!(
                Instant::now() < deadline,
                "holon-gpui did not log `{line}` within 180 s; log:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn signal(&self, name: &str) {
        // The child is not reaped yet, so its pid cannot name another process.
        let sent = Command::new("kill")
            .arg(format!("-{name}"))
            .arg(self.child.id().to_string())
            .status()
            .expect("run kill");
        assert!(
            sent.success(),
            "sending SIG{name} to holon-gpui failed: {sent}"
        );
    }

    fn wait_for_exit(&mut self, name: &str) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if let Some(status) = self.child.try_wait().expect("poll holon-gpui") {
                return status;
            }
            if Instant::now() >= deadline {
                self.child.kill().expect("kill the hung holon-gpui");
                panic!(
                    "holon-gpui did not exit within 180 s of SIG{name}; log:\n{}",
                    self.log_text()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

fn a_signal_to_the_open_window_quits_through_the_session_shutdown(name: &str) {
    let mut gui = Gui::spawn();
    gui.wait_for_log(BOOTED);
    // The window opens after the boot; a signal before the app runs would
    // test the boot, not the quit.
    std::thread::sleep(Duration::from_secs(3));
    gui.signal(name);
    let status = gui.wait_for_exit(name);
    let text = gui.log_text();
    assert!(
        status.success() && text.contains(SHUT_DOWN),
        "SIG{name} must end holon-gpui through the session shutdown with status 0, got \
         {status}; log:\n{text}"
    );
}

fn a_signal_during_boot_quits_through_the_session_shutdown(name: &str) {
    let mut gui = Gui::spawn();
    gui.wait_for_log(BOOTING);
    gui.signal(name);
    let status = gui.wait_for_exit(name);
    let text = gui.log_text();
    assert!(
        status.success() && text.contains(SHUT_DOWN),
        "SIG{name} during boot must end holon-gpui through the session shutdown with status \
         0, got {status}; log:\n{text}"
    );
    assert!(
        text.contains("during boot, shutting the session down"),
        "a GUI stopped during boot must not open its window; log:\n{text}"
    );
}

#[test]
fn ctrl_c_ends_the_gui_through_the_session_shutdown() {
    a_signal_to_the_open_window_quits_through_the_session_shutdown("INT");
}

#[test]
fn sigterm_ends_the_gui_through_the_session_shutdown() {
    a_signal_to_the_open_window_quits_through_the_session_shutdown("TERM");
}

#[test]
fn sighup_ends_the_gui_through_the_session_shutdown() {
    a_signal_to_the_open_window_quits_through_the_session_shutdown("HUP");
}

#[test]
fn ctrl_c_during_boot_ends_the_gui_through_the_session_shutdown() {
    a_signal_during_boot_quits_through_the_session_shutdown("INT");
}

#[test]
fn sigterm_during_boot_ends_the_gui_through_the_session_shutdown() {
    a_signal_during_boot_quits_through_the_session_shutdown("TERM");
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
