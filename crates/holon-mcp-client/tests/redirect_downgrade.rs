//! A redirect cannot walk a request off TLS or to another origin.
//!
//! The load-time guard sees only the URL a sidecar declares; every hop after it
//! is chosen by the peer, and the request it gets carries the connection's
//! credentials, including a header the connector names itself.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;

use holon_mcp_client::CredentialRoot;
use holon_mcp_client::McpTransport;
use holon_mcp_client::RestCallSurface;
use holon_mcp_client::integration_config::IntegrationFileConfig;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use proptest::prelude::*;
use proptest::test_runner::Config;
use proptest::test_runner::TestCaseError;
use proptest::test_runner::TestRunner;
use rmcp::model::CallToolRequestParam;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::net::TcpStream;

/// The phrase the redirect policy puts in its refusal. "The request failed"
/// is no evidence the redirect was blocked: an unreachable target fails on its
/// own.
const POLICY_MARKER: &str = "refused a redirect";

const SECRET_VAR: &str = "REDIRECT_TEST_KEY";
const SECRET: &str = "s3cret-key-value";
const BODY_SECRET: &str = "body-s3cret";

/// Every redirect target below carries this in its path or userinfo, so a
/// refusal that quotes the target is visible.
const TARGET_TOKEN: &str = "stolen";

const REDIRECT_STATUSES: [u16; 5] = [301, 302, 303, 307, 308];

const KEY_HEADERS: [&str; 6] = [
    "X-Api-Key",
    "Private-Token",
    "Api-Key",
    "X-Custom-Auth",
    "Authorization",
    "Cookie",
];

/// One HTTP/1.1 request as sent: its head and the body its
/// `Content-Length` announces.
async fn read_request(sock: &mut TcpStream) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(head_end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&request[..head_end]).to_ascii_lowercase();
            let body_len = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map(|len| {
                    len.trim()
                        .parse::<usize>()
                        .expect("a numeric content-length")
                })
                .unwrap_or(0);
            if request.len() >= head_end + 4 + body_len {
                return request;
            }
        }
        let n = sock.read(&mut chunk).await.expect("read request");
        assert!(n > 0, "the client closed mid-request after {request:?}");
        request.extend_from_slice(&chunk[..n]);
    }
}

pub(crate) async fn bind_loopback() -> TcpListener {
    TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback")
}

/// Answers its n-th request with `plan[n]` as `(status, Location)`, every
/// request past the plan with `200 {"ok":true}`, and records each request.
pub(crate) fn serve_chain(
    listener: TcpListener,
    plan: Vec<(u16, String)>,
) -> Arc<Mutex<Vec<String>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = seen.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let request = String::from_utf8_lossy(&read_request(&mut sock).await).into_owned();
            let n = {
                let mut seen = record.lock().unwrap();
                seen.push(request);
                seen.len() - 1
            };
            let response = match plan.get(n) {
                Some((status, location)) => format!(
                    "HTTP/1.1 {status} Redirect\r\nLocation: {location}\r\nContent-Length: \
                     0\r\nConnection: close\r\n\r\n"
                ),
                None => "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                         11\r\nConnection: close\r\n\r\n{\"ok\":true}"
                    .to_string(),
            };
            sock.write_all(response.as_bytes())
                .await
                .expect("write response");
        }
    });
    seen
}

/// Records every byte a connection sends it, then closes the connection
/// without an answer.
pub(crate) fn serve_victim(listener: TcpListener) -> Arc<Mutex<Vec<u8>>> {
    let received = Arc::new(Mutex::new(Vec::new()));
    let record = received.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.expect("accept");
            let request = read_request(&mut sock).await;
            record.lock().unwrap().extend_from_slice(&request);
        }
    });
    received
}

/// A manual with one tool, `list`, calling `url` with GET, or with POST and a
/// JSON body, sending `auth_header: <SECRET>` when one is named.
fn surface_calling(url: &str, auth_header: Option<&str>, post: bool) -> RestCallSurface {
    let (method, tool) = if post {
        (
            "POST",
            format!("\n      body:\n        payload: {BODY_SECRET}"),
        )
    } else {
        ("GET", " {}".to_string())
    };
    let auth = match auth_header {
        Some(header) => {
            format!("\n  auth:\n    header: {header}\n    value: \"${{{SECRET_VAR}}}\"")
        }
        None => String::new(),
    };
    let yaml = format!(
        r#"
schema_version: 2
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:
    - name: list
      tool_call_template:
        call_template_type: http
        http_method: {method}
        url: "{url}"
holon:{auth}
  tools:
    list:{tool}
"#
    );
    let cfg = IntegrationFileConfig::from_yaml(&yaml).expect("fixture parses");
    let lookup = |name: &str| (name == SECRET_VAR).then(|| SECRET.to_string());
    let built = cfg
        .into_mcp_config_with(
            "fixture".to_string(),
            &lookup,
            &CredentialRoot::new("/tmp/holon-redirect-downgrade-test"),
        )
        .expect("a loopback URL builds");
    match built.transport {
        McpTransport::Rest { manual, .. } => RestCallSurface::new(manual),
        other => panic!("expected a rest transport, got {other:?}"),
    }
}

async fn call_list(
    surface: &RestCallSurface,
) -> Result<rmcp::model::CallToolResult, rmcp::service::ServiceError> {
    surface
        .call_tool(CallToolRequestParam {
            name: "list".into(),
            arguments: Some(serde_json::Map::new()),
        })
        .await
}

fn run_on_fresh_runtime<F: std::future::Future<Output = Result<(), TestCaseError>>>(
    case: F,
) -> Result<(), TestCaseError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(case)
}

#[tokio::test]
async fn a_redirect_to_cleartext_is_refused_by_policy_not_by_unreachability() {
    // `.invalid` never resolves (RFC 2606): without the policy this request
    // fails with a DNS error, with it it fails with the refusal.
    let first = bind_loopback().await;
    let addr = first.local_addr().expect("addr");
    serve_chain(
        first,
        vec![(302, format!("http://blocked.invalid/{TARGET_TOKEN}"))],
    );
    let surface = surface_calling(&format!("http://{addr}/start"), None, false);

    let err = call_list(&surface)
        .await
        .expect_err("a redirect off TLS must not be followed");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(POLICY_MARKER),
        "the request must fail because the POLICY refused the hop, not because the target was \
         unreachable; got: {msg}"
    );
    assert!(
        !msg.contains("blocked.invalid") && !msg.contains(TARGET_TOKEN),
        "the refusal must not echo the redirect target, which may carry a credential; got: {msg}"
    );
}

/// Where a cross-origin redirect points, relative to the first host
/// `127.0.0.1:<first port>`.
#[derive(Debug, Clone)]
enum Target {
    OtherPort,
    /// `localhost` names the same machine but is another host in the origin.
    LocalhostName,
    OtherPortWithPathAndQuery(String),
    OtherPortWithUserinfo,
    /// `[::1]` on the first host's port: only the host differs.
    OtherHostSamePort,
}

#[derive(Debug, Clone)]
struct CrossOrigin {
    status: u16,
    header: &'static str,
    target: Target,
    post: bool,
}

fn cross_origin_case() -> impl Strategy<Value = CrossOrigin> {
    let target = prop_oneof![
        Just(Target::OtherPort),
        Just(Target::LocalhostName),
        "[a-z0-9]{1,8}/[a-z0-9]{1,8}\\?[a-z]{1,4}=[a-z0-9]{1,8}"
            .prop_map(Target::OtherPortWithPathAndQuery),
        Just(Target::OtherPortWithUserinfo),
        Just(Target::OtherHostSamePort),
    ];
    (
        prop::sample::select(REDIRECT_STATUSES.to_vec()),
        prop::sample::select(KEY_HEADERS.to_vec()),
        target,
        any::<bool>(),
    )
        .prop_map(|(status, header, target, post)| CrossOrigin {
            status,
            header,
            target,
            post,
        })
}

async fn second_host_receives_nothing(case: CrossOrigin) -> Result<(), TestCaseError> {
    let first = bind_loopback().await;
    let first_addr = first.local_addr().expect("addr");
    let victim = match case.target {
        Target::OtherHostSamePort => TcpListener::bind(SocketAddr::from((
            std::net::Ipv6Addr::LOCALHOST,
            first_addr.port(),
        )))
        .await
        .expect("bind [::1] on the first host's port"),
        _ => bind_loopback().await,
    };
    let victim_port = victim.local_addr().expect("addr").port();
    let location = match &case.target {
        Target::OtherPort => format!("http://127.0.0.1:{victim_port}/{TARGET_TOKEN}"),
        Target::LocalhostName => format!("http://localhost:{victim_port}/{TARGET_TOKEN}"),
        Target::OtherPortWithPathAndQuery(path) => {
            format!("http://127.0.0.1:{victim_port}/{TARGET_TOKEN}/{path}")
        }
        Target::OtherPortWithUserinfo => {
            format!("http://user:{TARGET_TOKEN}@127.0.0.1:{victim_port}/{TARGET_TOKEN}")
        }
        Target::OtherHostSamePort => format!("http://[::1]:{victim_port}/{TARGET_TOKEN}"),
    };
    let first_seen = serve_chain(first, vec![(case.status, location)]);
    let received = serve_victim(victim);
    let surface = surface_calling(
        &format!("http://{first_addr}/start"),
        Some(case.header),
        case.post,
    );

    let result = call_list(&surface).await;

    let received = received.lock().unwrap().clone();
    prop_assert!(
        received.is_empty(),
        "the second host received:\n{}",
        String::from_utf8_lossy(&received)
    );
    let sent_header = format!("{}: {SECRET}", case.header.to_ascii_lowercase());
    prop_assert!(
        first_seen.lock().unwrap()[0]
            .to_ascii_lowercase()
            .contains(&sent_header),
        "the first host did not get the key header, so the case proves nothing"
    );
    let msg = match result {
        Ok(out) => return Err(TestCaseError::fail(format!("the call succeeded: {out:?}"))),
        Err(e) => format!("{e:#}"),
    };
    prop_assert!(
        msg.contains(POLICY_MARKER),
        "not refused by the policy: {msg}"
    );
    prop_assert!(
        !msg.contains(TARGET_TOKEN) && !msg.contains(SECRET),
        "the refusal quotes the target or the key: {msg}"
    );
    Ok(())
}

#[test]
fn cross_origin_redirect_never_reaches_second_host() {
    let examples = [
        CrossOrigin {
            status: 302,
            header: "X-Api-Key",
            target: Target::OtherPort,
            post: false,
        },
        CrossOrigin {
            status: 307,
            header: "Private-Token",
            target: Target::OtherPort,
            post: true,
        },
        CrossOrigin {
            status: 302,
            header: "X-Api-Key",
            target: Target::LocalhostName,
            post: false,
        },
    ];
    for case in examples {
        run_on_fresh_runtime(second_host_receives_nothing(case.clone()))
            .unwrap_or_else(|e| panic!("{case:?}: {e}"));
    }
    TestRunner::new(Config {
        cases: 128,
        failure_persistence: None,
        ..Config::default()
    })
    .run(&cross_origin_case(), |case| {
        run_on_fresh_runtime(second_host_receives_nothing(case))
    })
    .unwrap_or_else(|e| panic!("{e}"));
}

/// Redirects on one listener, each a status and a path relative to it; the
/// last one lands on a `200`.
fn same_origin_chain() -> impl Strategy<Value = (&'static str, Vec<(u16, String)>)> {
    let hop = (
        prop::sample::select(REDIRECT_STATUSES.to_vec()),
        "(/[a-z0-9]{1,8}){1,3}(\\?[a-z]{1,4}=[a-z0-9]{1,8})?",
    );
    (
        prop::sample::select(KEY_HEADERS.to_vec()),
        prop::collection::vec(hop, 1..=9),
    )
}

async fn every_hop_is_followed_with_the_key(
    header: &'static str,
    plan: Vec<(u16, String)>,
) -> Result<(), TestCaseError> {
    let listener = bind_loopback().await;
    let addr = listener.local_addr().expect("addr");
    let seen = serve_chain(listener, plan.clone());
    let surface = surface_calling(&format!("http://{addr}/start"), Some(header), false);

    let result = call_list(&surface).await;
    prop_assert!(
        result.is_ok(),
        "a same-origin chain was not followed: {result:?}"
    );

    let seen = seen.lock().unwrap().clone();
    let paths = std::iter::once("/start").chain(plan.iter().map(|(_, path)| path.as_str()));
    prop_assert_eq!(seen.len(), plan.len() + 1);
    let sent_header = format!("{}: {SECRET}", header.to_ascii_lowercase());
    for (request, path) in seen.iter().zip(paths) {
        let request_line = format!("GET {path} HTTP/1.1\r\n");
        prop_assert!(
            request.starts_with(&request_line),
            "expected {request_line:?}, got:\n{request}"
        );
        prop_assert_eq!(
            request.to_ascii_lowercase().matches(&sent_header).count(),
            1,
            "the key header once per hop, got:\n{}",
            request
        );
    }
    Ok(())
}

#[test]
fn same_origin_redirect_is_followed() {
    let examples = [("X-Api-Key", vec![(302, "/final".to_string())])];
    for (header, plan) in examples {
        run_on_fresh_runtime(every_hop_is_followed_with_the_key(header, plan.clone()))
            .unwrap_or_else(|e| panic!("{header} {plan:?}: {e}"));
    }
    TestRunner::new(Config {
        cases: 64,
        failure_persistence: None,
        ..Config::default()
    })
    .run(&same_origin_chain(), |(header, plan)| {
        run_on_fresh_runtime(every_hop_is_followed_with_the_key(header, plan))
    })
    .unwrap_or_else(|e| panic!("{e}"));
}
