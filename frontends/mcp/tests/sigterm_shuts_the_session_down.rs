//! launchd stops `holon-mcp` with SIGTERM. The server must answer it through
//! the session shutdown — write back what it owes, save, close, give up the
//! vault — and exit cleanly, not die on the default signal action with its
//! write-back backlog unwritten. A SIGTERM during boot lets the boot finish,
//! then takes the same shutdown, and nothing is served.
//!
//! @pbt kind harness
//! @pbt covers mcp-sigterm-shutdown — SIGTERM, during boot or while serving,
//! and SIGHUP end `holon-mcp` through `shutdown_session` with exit status 0;
//! a failed shutdown logs its error, and the exit error, and exits non-zero

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

/// Initialize a stdio session and set a block's content; the raw response.
fn set_content_over_stdio(
    child: &mut std::process::Child,
    block_id: &str,
    content: &str,
) -> String {
    use std::io::BufRead;
    use std::io::Write;

    let mut stdin = child.stdin.take().expect("stdin piped");
    let mut stdout = std::io::BufReader::new(child.stdout.take().expect("stdout piped"));
    let mut response_to = |id: u32| {
        let mut line = String::new();
        loop {
            line.clear();
            let read = stdout.read_line(&mut line).expect("read holon-mcp stdout");
            assert!(
                read > 0,
                "holon-mcp closed stdout before answering request {id}"
            );
            if line.contains(&format!(r#""id":{id}"#)) {
                return line.clone();
            }
        }
    };
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"sigterm-test","version":"1"}}}}}}"#
    )
    .expect("send initialize");
    response_to(1);
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized","params":{{}}}}"#
    )
    .expect("send initialized");
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"execute_operation","arguments":{{"entity_name":"block","operation":"set_field","params":{{"id":"{block_id}","field":"content","value":"{content}"}}}}}}}}"#
    )
    .expect("send the edit");
    let ack = response_to(2);
    // A closed stdin ends the stdio server.
    child.stdin = Some(stdin);
    ack
}

#[test]
fn a_refused_write_back_is_in_the_log() {
    use std::os::unix::fs::PermissionsExt;

    let vault = tempfile::tempdir().expect("vault dir");
    std::fs::write(vault.path().join("page.org"), VAULT_ORG).expect("write the page");
    let locked = vault.path().join("locked");
    std::fs::create_dir(&locked).expect("create the page's dir");
    let page = locked.join("locked.org");
    std::fs::write(
        &page,
        "* Locked page\n:PROPERTIES:\n:ID: locked-page\n:END:\n\
         ** Locked child\n:PROPERTIES:\n:ID: locked-child\n:END:\n",
    )
    .expect("write the locked page");
    let state = tempfile::tempdir().expect("state dir");
    let log = state.path().join("holon-mcp.log");

    let mut child = spawn_on(vault.path(), state.path());
    wait_for_log(&mut child, &log, BOOTED);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !std::fs::read_to_string(&page)
        .expect("read the page")
        .contains("#+ID:")
    {
        assert!(
            Instant::now() < deadline,
            "the boot did not write the page's #+ID: line within 60 s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // The atomic write cannot create its temp file beside the page.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555))
        .expect("make the page's dir read-only");
    let ack = set_content_over_stdio(&mut child, "block:locked-child", "EDITED-READ-ONLY");
    signal(&child, "TERM");
    let status = wait_for_exit(&mut child, &log);
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
        .expect("make the page's dir writable again");

    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        ack.contains(r#""result""#) && !ack.contains(r#""isError":true"#),
        "the edit was not acknowledged, so this test proves nothing: {ack}\nlog:\n{text}"
    );
    let on_disk = std::fs::read_to_string(&page).expect("read the page");
    assert!(
        !on_disk.contains("EDITED-READ-ONLY"),
        "the write-back was not refused, so this test proves nothing:\n{on_disk}"
    );
    assert!(
        !status.success(),
        "a shutdown that left an edit out of its org file must exit non-zero; log:\n{text}"
    );
    assert!(
        text.contains("refused the org write-back") && text.contains("locked.org"),
        "the log must name the document the shutdown did not write; log:\n{text}"
    );
    assert!(
        text.contains("holon-mcp exits with an error"),
        "the log must record the error the process exits with; log:\n{text}"
    );
}
