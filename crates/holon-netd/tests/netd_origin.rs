//! netd answers only requests a loopback page could not forge: a foreign
//! Host or Origin is refused before a session starts.

mod common;

use std::fs;
use std::io::Read;
use std::io::Write;
use std::net::TcpStream;
use std::path::Path;
use std::path::PathBuf;
use std::time::Duration;

use common::Process;

fn net(dir: &Path) -> PathBuf {
    let path = dir.join("idle.yaml");
    fs::write(
        &path,
        "name: idle\nplaces:\n  A:\n    capacity: single\n  B:\n    capacity: single\ntransitions:\n  Go:\n    trigger: manual\n    priority: 1\n    consumes:\n    - A\n    produces:\n    - B\n",
    )
    .unwrap();
    path
}

struct Head {
    status: u16,
    session: Option<String>,
}

/// Posts `initialize` with the given Host and Origin and reads the response
/// head only, since a granted session answers on a stream that stays open.
fn initialize(url: &str, host: &str, origin: Option<&str>) -> Head {
    let addr = url
        .strip_prefix("http://")
        .and_then(|rest| rest.strip_suffix("/mcp"))
        .expect("netd prints its url as http://<addr>/mcp");
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"origin-test","version":"0"}}}"#;
    let origin = origin.map_or(String::new(), |o| format!("Origin: {o}\r\n"));
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {host}\r\n{origin}Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let n = stream.read(&mut byte).expect("netd answers within 10s");
        assert_eq!(n, 1, "netd closed before the end of the head: {head:?}");
        head.push(byte[0]);
    }
    let head = String::from_utf8(head).unwrap();
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status in {head}"));
    let session = head.lines().find_map(|l| {
        let (name, value) = l.split_once(':')?;
        name.eq_ignore_ascii_case("mcp-session-id")
            .then(|| value.trim().to_owned())
    });
    Head { status, session }
}

fn port(url: &str) -> &str {
    url.rsplit(':')
        .next()
        .and_then(|p| p.strip_suffix("/mcp"))
        .unwrap()
}

#[test]
fn a_foreign_host_is_refused_before_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let netd = Process::start(dir.path(), &[&net(dir.path())]);
    let host = format!("evil.example:{}", port(&netd.url));

    let head = initialize(&netd.url, &host, None);

    assert_eq!(head.status, 403, "Host {host}");
    assert_eq!(head.session, None, "Host {host}");
}

#[test]
fn a_foreign_origin_is_refused_before_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let netd = Process::start(dir.path(), &[&net(dir.path())]);
    let port = port(&netd.url);
    let origin = format!("http://evil.example:{port}");

    let head = initialize(&netd.url, &format!("127.0.0.1:{port}"), Some(&origin));

    assert_eq!(head.status, 403, "Origin {origin}");
    assert_eq!(head.session, None, "Origin {origin}");
}

#[test]
fn a_loopback_host_without_an_origin_starts_a_session() {
    let dir = tempfile::tempdir().unwrap();
    let netd = Process::start(dir.path(), &[&net(dir.path())]);
    let host = format!("127.0.0.1:{}", port(&netd.url));

    let head = initialize(&netd.url, &host, None);

    assert_eq!(head.status, 200, "Host {host}");
    assert!(head.session.is_some(), "Host {host} gets a session id");
}
