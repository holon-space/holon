//! `holon-tui` as a process on a pseudo-terminal, since it needs one to start.
//! Every input is a flag or points into the instance's own state dir, so no
//! setting of the developer's own reaches it. `HOLON_LOG` is unset, as in
//! production: the log is `tui.log` under the instance's `HOME`.
//!
//! Each test target uses part of it.
#![allow(dead_code)]

use std::fs::File;
use std::os::fd::FromRawFd;
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::path::PathBuf;
use std::process::Child;
use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

pub const BOOTING: &str = "Starting TUI frontend...";
pub const BOOTED: &str = "Session ready";
pub const SHUT_DOWN: &str = "session shut down";

pub const PAGE_ORG: &str = "\
* Stop page
:PROPERTIES:
:ID: stop-page
:END:
";

/// How `holon-tui` gets its terminal.
pub enum Launcher {
    /// The shell execs the TUI, so the TUI is the terminal's controlling
    /// process and gets its hangup.
    Exec,
    /// The shell ignores SIGHUP and waits for the TUI, as `nohup` and several
    /// launchers do: when the terminal dies, the TUI gets no signal.
    HupIgnoringShell,
    /// `true | holon-tui`: stdin is a pipe that is already closed.
    ClosedStdin,
    /// Stdin is a live terminal; stdout and stderr are a terminal that has
    /// died, so every draw fails and no hangup shows on stdin.
    DeadOutput,
}

pub struct Launch {
    mcp_port: u16,
    launcher: Launcher,
    vault_root: Option<PathBuf>,
    home: Option<PathBuf>,
    args: Vec<String>,
    envs: Vec<(String, String)>,
}

impl Launch {
    /// Each instance serves MCP on its own `mcp_port` when MCP is on.
    pub fn new(mcp_port: u16) -> Self {
        Self {
            mcp_port,
            launcher: Launcher::Exec,
            vault_root: None,
            home: None,
            args: Vec::new(),
            envs: Vec::new(),
        }
    }

    pub fn launcher(mut self, launcher: Launcher) -> Self {
        self.launcher = launcher;
        self
    }

    /// Boot on this vault root instead of the prepared temp vault.
    pub fn vault_root(mut self, root: PathBuf) -> Self {
        self.vault_root = Some(root);
        self
    }

    /// Boot with this `HOME`, so the instance shares the log and config of
    /// whoever else runs on it.
    pub fn home(mut self, home: PathBuf) -> Self {
        self.home = Some(home);
        self
    }

    pub fn arg(mut self, arg: &str) -> Self {
        self.args.push(arg.to_string());
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.envs.push((key.to_string(), value.to_string()));
        self
    }

    /// Start the TUI on a vault that holds `page.org`; `prepare` adds to the
    /// vault before the TUI starts.
    pub fn spawn(self, prepare: impl FnOnce(&Path)) -> Tui {
        let vault = tempfile::tempdir().expect("vault dir");
        std::fs::write(vault.path().join("page.org"), PAGE_ORG).expect("write the page");
        prepare(vault.path());
        let state = tempfile::tempdir().expect("state dir");
        let home = self.home.unwrap_or_else(|| state.path().to_path_buf());
        let log = home.join(".config/holon/tui.log");
        let terminal = state.path().join("terminal.out");
        let vault_root = self
            .vault_root
            .unwrap_or_else(|| vault.path().to_path_buf());

        // The pseudo-terminal starts at 0 x 0, below the size the TUI renders at.
        let shell = match self.launcher {
            Launcher::Exec => Some("stty rows 40 cols 120 && exec \"$0\" \"$@\""),
            Launcher::HupIgnoringShell => Some("trap '' HUP; stty rows 40 cols 120; \"$0\" \"$@\""),
            Launcher::ClosedStdin => Some("stty rows 40 cols 120 && true | \"$0\" \"$@\""),
            Launcher::DeadOutput => None,
        };
        let tui = env!("CARGO_BIN_EXE_holon-tui");
        let (mut command, input_terminal) = match shell {
            Some(shell) => {
                let mut command = Command::new("script");
                command.args(["-q", "/dev/null", "sh", "-c", shell, tui]);
                command
                    .stdin(Stdio::piped())
                    .stdout(File::create(&terminal).expect("create the terminal capture"))
                    .stderr(Stdio::null());
                (command, None)
            }
            None => {
                let (input, input_side) = pseudo_terminal();
                let (output, output_side) = pseudo_terminal();
                drop(output);
                let stderr = output_side.try_clone().expect("share the dead terminal");
                let mut command = Command::new(tui);
                command.stdin(input_side).stdout(output_side).stderr(stderr);
                // SAFETY: only async-signal-safe calls between fork and exec.
                unsafe {
                    command.pre_exec(|| {
                        if libc::setsid() < 0
                            || libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) < 0
                        {
                            return Err(std::io::Error::last_os_error());
                        }
                        Ok(())
                    });
                }
                (command, Some(input))
            }
        };
        for (key, _) in std::env::vars_os() {
            if key.to_string_lossy().starts_with("HOLON_") || key == "MCP_SERVER_PORT" {
                command.env_remove(key);
            }
        }
        let process = command
            .arg("--config-dir")
            .arg(state.path().join("config"))
            .arg("--db-path")
            .arg(state.path().join("holon.db"))
            .arg("--vault-root")
            .arg(vault_root)
            .args(&self.args)
            .env("HOME", &home)
            .env("MCP_SERVER_PORT", self.mcp_port.to_string())
            .env("TERM", "xterm-256color")
            .envs(self.envs)
            .spawn()
            .expect("spawn holon-tui");
        Tui {
            process,
            input_terminal,
            launcher: self.launcher,
            log,
            terminal,
            vault,
            state,
        }
    }
}

/// A 40 x 120 pseudo-terminal: its controlling side and the side a process
/// runs on.
fn pseudo_terminal() -> (OwnedFd, OwnedFd) {
    let (mut controller, mut process_side) = (-1, -1);
    let mut size = libc::winsize {
        ws_row: 40,
        ws_col: 120,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: valid out-pointers; no name buffer, default termios.
    let opened = unsafe {
        libc::openpty(
            &mut controller,
            &mut process_side,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    assert_eq!(
        opened,
        0,
        "openpty failed: {}",
        std::io::Error::last_os_error()
    );
    // SAFETY: openpty returned two fresh descriptors this function owns.
    unsafe {
        (
            OwnedFd::from_raw_fd(controller),
            OwnedFd::from_raw_fd(process_side),
        )
    }
}

pub struct Tui {
    /// `script`, or `holon-tui` itself under [`Launcher::DeadOutput`].
    process: Child,
    /// Keeps the [`Launcher::DeadOutput`] stdin terminal alive.
    input_terminal: Option<OwnedFd>,
    launcher: Launcher,
    log: PathBuf,
    terminal: PathBuf,
    vault: tempfile::TempDir,
    state: tempfile::TempDir,
}

impl Tui {
    /// The `HOME` this instance runs on.
    pub fn home(&self) -> &Path {
        self.state.path()
    }

    pub fn vault(&self) -> &Path {
        self.vault.path()
    }

    pub fn log_text(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    /// Type `keys` on the terminal.
    pub fn type_keys(&mut self, keys: &[u8]) {
        use std::io::Write;
        let stdin = self
            .process
            .stdin
            .as_mut()
            .expect("script's stdin is piped");
        stdin.write_all(keys).expect("type on the terminal");
        stdin.flush().expect("flush the typed keys");
    }

    /// Everything the TUI wrote to its terminal.
    pub fn terminal_text(&self) -> String {
        String::from_utf8_lossy(&std::fs::read(&self.terminal).unwrap_or_default()).into_owned()
    }

    pub fn wait_for_log(&mut self, line: &str) {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            let text = self.log_text();
            if text.contains(line) {
                return;
            }
            if let Some(status) = self.process.try_wait().expect("poll script") {
                panic!("holon-tui exited before `{line}` with {status}; log:\n{text}");
            }
            assert!(
                Instant::now() < deadline,
                "holon-tui did not log `{line}` within 180 s; log:\n{text}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn tui_pid(&self) -> String {
        let child_of = |parent: &str| {
            let out = Command::new("pgrep")
                .arg("-P")
                .arg(parent)
                .output()
                .expect("run pgrep");
            String::from_utf8(out.stdout)
                .expect("pgrep prints pids")
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        };
        if let Launcher::DeadOutput = self.launcher {
            return self.process.id().to_string();
        }
        let mut pids = child_of(&self.process.id().to_string());
        if let Launcher::HupIgnoringShell = self.launcher {
            assert_eq!(
                pids.len(),
                1,
                "script must run exactly one shell; got {pids:?}"
            );
            pids = child_of(&pids[0]);
        }
        assert_eq!(
            pids.len(),
            1,
            "script must run exactly one holon-tui; got {pids:?}"
        );
        pids.remove(0)
    }

    pub fn signal(&self, name: &str) {
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

    /// Close the terminal the way a closed terminal window does: its master
    /// side dies with `script`.
    pub fn kill_terminal(&mut self) {
        self.process.kill().expect("kill script");
        self.process.wait().expect("reap script");
    }

    /// `script` exits with its child's status, and with 128 + the signal
    /// number when a signal killed the child.
    pub fn wait_for_exit(&mut self, cause: &str) -> std::process::ExitStatus {
        let deadline = Instant::now() + Duration::from_secs(180);
        loop {
            if let Some(status) = self.process.try_wait().expect("poll script") {
                return status;
            }
            if Instant::now() >= deadline {
                self.process.kill().expect("kill the hung script");
                panic!(
                    "holon-tui did not exit within 180 s of {cause}; log:\n{}",
                    self.log_text()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

pub fn is_alive(pid: &str) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid)
        .stderr(Stdio::null())
        .status()
        .expect("run kill -0")
        .success()
}

/// A page whose child the tests edit, in a directory they can make read-only.
pub fn write_locked_page(vault: &Path) {
    let locked = vault.join("locked");
    std::fs::create_dir(&locked).expect("create the page's dir");
    std::fs::write(
        locked.join("locked.org"),
        "* Locked page\n:PROPERTIES:\n:ID: locked-page\n:END:\n\
         ** Locked child\n:PROPERTIES:\n:ID: locked-child\n:END:\n",
    )
    .expect("write the locked page");
}

/// The instance's MCP server, over plain HTTP/1.1.
pub struct Mcp {
    port: u16,
    session: Option<String>,
    next_id: u32,
}

impl Mcp {
    pub fn connect(port: u16) -> Self {
        let mut mcp = Self {
            port,
            session: None,
            next_id: 0,
        };
        let init = mcp.call(
            "initialize",
            r#"{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"tui-test","version":"1"}}"#,
        );
        assert!(
            mcp.session.is_some(),
            "initialize returned no Mcp-Session-Id: {init}"
        );
        mcp.post(r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#);
        mcp
    }

    /// The raw response, headers and body.
    pub fn call(&mut self, method: &str, params: &str) -> String {
        self.next_id += 1;
        let body = format!(
            r#"{{"jsonrpc":"2.0","id":{},"method":"{method}","params":{params}}}"#,
            self.next_id
        );
        self.post(&body)
    }

    /// Set a block's content through `execute_operation`; the raw response.
    pub fn set_content(&mut self, block_id: &str, content: &str) -> String {
        self.call(
            "tools/call",
            &format!(
                r#"{{"name":"execute_operation","arguments":{{"entity_name":"block","operation":"set_field","params":{{"id":"{block_id}","field":"content","value":"{content}"}}}}}}"#
            ),
        )
    }

    fn post(&mut self, body: &str) -> String {
        use std::io::Read;
        use std::io::Write;

        let mut stream = std::net::TcpStream::connect(("127.0.0.1", self.port))
            .unwrap_or_else(|e| panic!("connecting to MCP on port {}: {e}", self.port));
        stream
            .set_read_timeout(Some(Duration::from_secs(60)))
            .expect("set the MCP read timeout");
        let session = self
            .session
            .as_ref()
            .map(|id| format!("Mcp-Session-Id: {id}\r\n"))
            .unwrap_or_default();
        write!(
            stream,
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\nConnection: close\r\n{session}\
             Content-Length: {}\r\n\r\n{body}",
            self.port,
            body.len()
        )
        .expect("send the MCP request");
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .expect("read the MCP response");
        let headers = response.split("\r\n\r\n").next().unwrap_or_default();
        if let Some(id) = headers.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("mcp-session-id")
                .then(|| value.trim().to_string())
        }) {
            self.session = Some(id);
        }
        response
    }
}

/// Whether a `tools/call` response is a successful result.
pub fn tool_succeeded(response: &str) -> bool {
    response.contains(r#""result""#) && !response.contains(r#""isError":true"#)
}
