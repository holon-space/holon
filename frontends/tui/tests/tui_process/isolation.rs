//! A test that panics leaves no `holon-tui` behind, and a test's MCP client
//! reaches its own instance, never a server that another run left on a port.
//!
//! @pbt kind harness
//! @pbt covers tui-process-isolation — a panic after the boot kills the
//! `script` wrapper and `holon-tui` and frees the MCP port; a listener that
//! another run holds gets no connection from the harness's client
//!
//! Examples, not a PBT: each case is one process lifecycle with no input space.

use std::io::ErrorKind;
use std::net::TcpListener;
use std::net::TcpStream;
use std::panic::AssertUnwindSafe;
use std::time::Duration;
use std::time::Instant;

use super::BOOTED;
use super::Launch;
use super::is_alive;

fn serves(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).is_ok()
}

#[test]
fn a_panicking_test_leaves_no_tui_and_no_listener_behind() {
    let mut seen = None;
    let caught = std::panic::catch_unwind(AssertUnwindSafe(|| {
        let mut tui = Launch::new().spawn(|_| {});
        tui.wait_for_log(BOOTED);
        let deadline = Instant::now() + Duration::from_secs(30);
        while !serves(tui.mcp_port()) {
            assert!(
                Instant::now() < deadline,
                "holon-tui served no MCP within 30 s of the boot; log:\n{}",
                tui.log_text()
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        seen = Some((tui.script_pid(), tui.tui_pid(), tui.mcp_port()));
        panic!("a test failure after the boot");
    }));
    assert!(caught.is_err(), "the test body must panic");
    let (script, tui, port) = seen.expect("the body saw the booted instance");

    let deadline = Instant::now() + Duration::from_secs(10);
    while (is_alive(&tui) || is_alive(&script) || serves(port)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let left = (is_alive(&tui), is_alive(&script), serves(port));
    if left.0 {
        std::process::Command::new("kill")
            .arg("-KILL")
            .arg(&tui)
            .status()
            .expect("kill the orphaned holon-tui");
    }
    assert_eq!(
        left,
        (false, false, false),
        "10 s after the panic: (holon-tui {tui} alive, script {script} alive, port {port} \
         served) must all be false"
    );
}

#[test]
fn the_mcp_client_never_reaches_a_listener_that_another_run_holds() {
    let other_run = TcpListener::bind("127.0.0.1:0").expect("bind the other run's port");
    other_run
        .set_nonblocking(true)
        .expect("make the other run's listener non-blocking");
    let mut tui = Launch::new().spawn(|_| {});
    tui.wait_for_log(BOOTED);

    let response = tui.mcp().call("tools/list", "{}");
    let reached = other_run.accept();
    assert!(
        matches!(&reached, Err(e) if e.kind() == ErrorKind::WouldBlock),
        "the client connected to the other run's listener: {reached:?}"
    );
    assert!(
        response.contains(r#""result""#),
        "the instance's own server did not answer tools/list: {response}"
    );
}
