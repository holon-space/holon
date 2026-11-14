//! The MCP HTTP server has no login, so `holon-mcp --http` on an address the
//! LAN reaches must exit non-zero before it boots or opens any socket.
//! An example test: the observable is a process exit, which a property over
//! addresses cannot see; the address space itself is covered in `di.rs`.
//!
//! @pbt kind example
//! @pbt covers mcp-http-loopback-only — a non-loopback `--http` address ends
//! `holon-mcp` with a non-zero exit and no listener

use std::process::Command;
use std::process::Stdio;
use std::time::Duration;
use std::time::Instant;

#[test]
fn http_on_a_non_loopback_address_exits_without_listening() {
    let state = tempfile::tempdir().expect("state dir");
    let vault = state.path().join("vault");
    std::fs::create_dir(&vault).expect("vault dir");
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("probe a free port")
        .local_addr()
        .expect("probe address")
        .port();

    // HOME points into the state dir, so the config dir the binary resolves is
    // a throwaway one and never the developer's own.
    let mut child = Command::new(env!("CARGO_BIN_EXE_holon-mcp"))
        .arg("--http")
        .arg(format!("0.0.0.0:{port}"))
        .arg("--orgmode-root")
        .arg(&vault)
        .arg(state.path().join("holon.db"))
        .env("HOME", state.path())
        .env("HOLON_MCP_LOG_FILE", state.path().join("holon-mcp.log"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn holon-mcp");

    let local = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        assert!(
            std::net::TcpStream::connect_timeout(&local, Duration::from_millis(50)).is_err(),
            "holon-mcp accepted a connection on port {port} while asked for 0.0.0.0"
        );
        if let Some(status) = child.try_wait().expect("poll holon-mcp") {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().expect("kill holon-mcp");
            panic!("holon-mcp --http 0.0.0.0:{port} still runs after 60 s");
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let output = child.wait_with_output().expect("collect stderr");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !status.success(),
        "holon-mcp --http 0.0.0.0:{port} exited with {status}; stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("non-loopback"),
        "the exit names no loopback refusal; stderr:\n{stderr}"
    );
    assert!(
        std::net::TcpStream::connect_timeout(&local, Duration::from_millis(200)).is_err(),
        "port {port} accepts connections after holon-mcp exited"
    );
}
