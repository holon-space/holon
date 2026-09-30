//! Every stop signal, and a terminal that dies without one, ends the TUI
//! through the session shutdown: the session writes back what it owes, saves,
//! closes and gives up the vault before the process exits, and a shutdown
//! that fails says so in the log.
//!
//! @pbt kind harness
//! @pbt covers tui-quit-shutdown — SIGINT, SIGTERM and SIGHUP, during boot or
//! while the TUI runs, and a dead terminal end `holon-tui` through
//! `shutdown_session`; a clean shutdown exits with status 0, a failed one
//! logs its error and exits non-zero; while the TUI runs, a stderr write
//! lands in the log, never on the terminal, and a draw to a dead terminal
//! cannot panic; a panic shows on a live terminal

mod tui_process;

use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use std::time::Instant;

use tui_process::BOOTED;
use tui_process::BOOTING;
use tui_process::Launch;
use tui_process::Launcher;
use tui_process::SHUT_DOWN;

fn a_signal_quits_through_the_session_shutdown(name: &str, after: &str, mcp_port: u16) {
    let mut tui = Launch::new(mcp_port).spawn(|_| {});
    tui.wait_for_log(after);
    tui.signal(name);
    let status = tui.wait_for_exit(&format!("SIG{name}"));
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

#[test]
fn a_refused_write_back_is_in_the_log_after_the_terminal_hung_up() {
    let port = 18765;
    let mut tui = Launch::new(port).spawn(tui_process::write_locked_page);
    tui.wait_for_log(BOOTED);
    let locked = tui.vault().join("locked");
    let page = locked.join("locked.org");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !std::fs::read_to_string(&page)
        .expect("read the page")
        .contains("#+ID:")
    {
        assert!(
            Instant::now() < deadline,
            "the boot did not write the page's #+ID: line within 60 s; log:\n{}",
            tui.log_text()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // The atomic write cannot create its temp file beside the page.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555))
        .expect("make the page's dir read-only");
    let ack = tui_process::Mcp::connect(port).set_content("block:locked-child", "EDITED-READ-ONLY");
    tui.signal("HUP");
    let status = tui.wait_for_exit("SIGHUP");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755))
        .expect("make the page's dir writable again");

    let text = tui.log_text();
    assert!(
        tui_process::tool_succeeded(&ack),
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
        "the log, the only record once the terminal is gone, must name the document the \
         shutdown did not write; log:\n{text}"
    );
}

#[test]
fn a_dead_terminal_ends_the_tui_through_the_session_shutdown() {
    let mut tui = Launch::new(18766)
        .launcher(Launcher::HupIgnoringShell)
        .spawn(|_| {});
    tui.wait_for_log(BOOTED);
    let pid = tui.tui_pid();
    tui.kill_terminal();

    let deadline = Instant::now() + Duration::from_secs(60);
    while tui_process::is_alive(&pid) {
        if Instant::now() >= deadline {
            std::process::Command::new("kill")
                .arg("-KILL")
                .arg(&pid)
                .status()
                .expect("kill the orphaned holon-tui");
            panic!(
                "holon-tui still ran 60 s after its terminal died, holding the vault; log:\n{}",
                tui.log_text()
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let text = tui.log_text();
    assert!(
        text.contains(SHUT_DOWN),
        "a dead terminal must end holon-tui through the session shutdown; log:\n{text}"
    );
}

#[test]
fn a_stderr_write_while_the_tui_draws_lands_in_the_log() {
    let mut tui = Launch::new(18769)
        .env("HOLON_DEBUG_CHORD", "1")
        .spawn(|_| {});
    tui.wait_for_log(BOOTED);
    // Alt+i: indent, which reports on stderr under HOLON_DEBUG_CHORD.
    tui.type_keys(b"\x1bi");

    let written = "[chord] indent:";
    let deadline = Instant::now() + Duration::from_secs(30);
    while !tui.log_text().contains(written) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    tui.signal("TERM");
    let status = tui.wait_for_exit("SIGTERM");
    let text = tui.log_text();
    assert!(
        text.contains(written) && text.contains(SHUT_DOWN),
        "`{written}` is not in the TUI log after the shutdown: it never arrived, or a later \
         log write overwrote it; on the terminal: {}; log:\n{text}",
        tui.terminal_text().contains(written)
    );
    assert!(
        !tui.terminal_text().contains(written),
        "a stderr write while the TUI draws must not reach the terminal"
    );
    assert!(
        status.success(),
        "SIGTERM must end holon-tui with status 0, got {status}; log:\n{text}"
    );
}

/// A release build (`panic = "abort"`) ends on a panic without the session
/// shutdown, so a failed draw must not panic.
#[test]
fn a_draw_to_a_dead_terminal_ends_the_tui_through_the_session_shutdown() {
    let mut tui = Launch::new(18770)
        .launcher(Launcher::DeadOutput)
        .spawn(|_| {});
    tui.wait_for_log(BOOTED);

    let failed_draw = "Failed to write ANSI bytes";
    let deadline = Instant::now() + Duration::from_secs(30);
    while !tui.log_text().contains(failed_draw) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let pid = tui.tui_pid();
    assert!(
        tui_process::is_alive(&pid),
        "a draw to a dead terminal ended holon-tui; log:\n{}",
        tui.log_text()
    );
    assert!(
        tui.log_text().contains(failed_draw),
        "no draw failed within 30 s, so this test proves nothing; log:\n{}",
        tui.log_text()
    );
    tui.signal("TERM");
    let status = tui.wait_for_exit("SIGTERM");
    let text = tui.log_text();
    assert!(
        status.success() && text.contains(SHUT_DOWN),
        "SIGTERM must end holon-tui through the session shutdown with status 0, got {status}; \
         log:\n{text}"
    );
}

/// `true | holon-tui` panics in crossterm's event reader.
#[test]
fn a_panic_reaches_the_terminal() {
    let mut tui = Launch::new(18771)
        .launcher(Launcher::ClosedStdin)
        .spawn(|_| {});
    let status = tui.wait_for_exit("a boot on a closed stdin");
    let terminal = tui.terminal_text();
    assert!(
        !status.success() && terminal.contains("reader source not set"),
        "the panic must show on the terminal, got {status}; terminal:\n{terminal}\nlog:\n{}",
        tui.log_text()
    );
}
