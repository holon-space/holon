//! A test that panics leaves no `holon-tui` behind, and a test's MCP client
//! reaches its own instance, never a server that another run left on a port.
//!
//! @pbt kind harness
//! @pbt covers tui-process-isolation — a panic after the boot kills the
//! `script` wrapper and `holon-tui` and frees the MCP port; a listener that
//! another run holds gets no connection from the harness's client; a client
//! made before its server binds waits for the bind
//!
//! Examples, not a PBT: each case is one process lifecycle with no input space.

use std::io::ErrorKind;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::net::TcpStream;
use std::panic::AssertUnwindSafe;
use std::time::Duration;
use std::time::Instant;

use super::BOOTED;
use super::Launch;
use super::Mcp;
use super::free_port;
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
        drop(tui.mcp());
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

#[test]
fn concurrent_instances_serve_mcp_on_distinct_ports() {
    let first = Launch::new().spawn(|_| {});
    let second = Launch::new().spawn(|_| {});
    assert_ne!(first.mcp_port(), second.mcp_port());
}

/// Answers one HTTP request with an MCP session id; returns at once on a bare
/// connect.
fn answer(mut stream: TcpStream) {
    let mut request = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let read = stream.read(&mut chunk).expect("read the request");
        if read == 0 {
            return;
        }
        request.extend_from_slice(&chunk[..read]);
        let text = String::from_utf8_lossy(&request);
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length: usize = head
                .lines()
                .find_map(|line| line.strip_prefix("Content-Length: "))
                .expect("the client sends a Content-Length")
                .parse()
                .expect("a numeric Content-Length");
            if body.len() >= length {
                break;
            }
        }
    }
    let body = r#"{"jsonrpc":"2.0","id":1,"result":{}}"#;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nMcp-Session-Id: late\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .expect("send the response");
}

#[test]
fn a_client_made_before_its_server_binds_waits_for_the_bind() {
    let port = free_port();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        let listener = TcpListener::bind(("127.0.0.1", port)).expect("bind the late listener");
        for stream in listener.incoming() {
            answer(stream.expect("accept a client"));
        }
    });

    let response =
        Mcp::connect(port, || "no log: a test listener".to_string()).call("tools/list", "{}");
    assert!(
        response.contains(r#""result""#),
        "the late listener did not answer: {response}"
    );
}
