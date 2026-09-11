//! A redirect cannot walk a request off TLS.
//!
//! Guarding the URL a sidecar declares is only half of it. `reqwest`'s default
//! policy follows up to 10 redirects and does not care about the scheme, so an
//! `https://` endpoint that answers `302 Location: http://…` gets the SECOND
//! request — carrying the same auth header — sent in the clear, and the
//! response body that becomes rows in the vault arrives from whoever answered
//! it. The load-time guard sees only the first URL and cannot see this at all.
//!
//! The rule is the same one the load-time guard applies, which is why both read
//! it from one place: https anywhere, or http only to this machine.

use std::net::SocketAddr;

use holon_mcp_client::CredentialRoot;
use holon_mcp_client::McpTransport;
use holon_mcp_client::RestCallSurface;
use holon_mcp_client::integration_config::IntegrationFileConfig;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use rmcp::model::CallToolRequestParam;
use tokio::net::TcpListener;

/// A loopback server that answers every request with `302` to `location`.
///
/// Loopback so the FIRST hop passes the load-time guard; the redirect target
/// is what this test is about.
async fn redirecting_server(location: String) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let location = location.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                use tokio::io::AsyncWriteExt;
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let body = format!(
                    "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\r\n"
                );
                let _ = sock.write_all(body.as_bytes()).await;
                let _ = sock.flush().await;
            });
        }
    });
    addr
}

fn surface_calling(url: &str) -> RestCallSurface {
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
        http_method: GET
        url: "{url}"
holon:
  tools:
    list: {{}}
"#
    );
    let cfg: IntegrationFileConfig = serde_yaml::from_str(&yaml).expect("fixture parses");
    let built = cfg
        .into_mcp_config_with(
            "fixture".to_string(),
            &|_| None,
            &CredentialRoot::new("/tmp/holon-redirect-downgrade-test"),
        )
        .expect("a loopback URL builds");
    match built.transport {
        McpTransport::Rest { manual, .. } => RestCallSurface::new(manual),
        other => panic!("expected a rest transport, got {other:?}"),
    }
}

/// The phrase the redirect policy puts in its refusal. Asserted on, because
/// "the request failed" is NOT evidence the redirect was blocked: an
/// unreachable target fails on its own, which is exactly how a test here can
/// pass while the policy is missing entirely.
const POLICY_MARKER: &str = "refused a redirect";

#[tokio::test]
async fn a_redirect_to_cleartext_is_refused_by_policy_not_by_unreachability() {
    // `.invalid` never resolves (RFC 2606), so WITHOUT a policy this request
    // fails with a DNS error and without one it fails with the refusal below.
    // Distinguishing the two is the whole point.
    let addr = redirecting_server("http://blocked.invalid/stolen".to_string()).await;
    let surface = surface_calling(&format!("http://{addr}/start"));

    let err = surface
        .call_tool(CallToolRequestParam {
            name: "list".into(),
            arguments: Some(serde_json::Map::new()),
        })
        .await
        .expect_err("a redirect off TLS must not be followed");
    let msg = format!("{err:#}");
    assert!(
        msg.contains(POLICY_MARKER),
        "the request must fail because the POLICY refused the hop, not because the target was \
         unreachable; got: {msg}"
    );
    assert!(
        !msg.contains("blocked.invalid"),
        "the refusal must not echo the redirect target, which may carry a credential; got: {msg}"
    );
}

/// The other half: an ordinary redirect that stays on an allowed scheme is
/// still followed, so the policy is not simply "no redirects".
#[tokio::test]
async fn a_redirect_that_stays_on_loopback_is_followed() {
    let final_addr = {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind final");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    use tokio::io::AsyncWriteExt;
                    let mut buf = [0u8; 2048];
                    let _ = sock.read(&mut buf).await;
                    let body = b"{\"ok\":true}";
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                         {}\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(body).await;
                    let _ = sock.flush().await;
                });
            }
        });
        addr
    };
    let addr = redirecting_server(format!("http://{final_addr}/final")).await;
    let surface = surface_calling(&format!("http://{addr}/start"));

    surface
        .call_tool(CallToolRequestParam {
            name: "list".into(),
            arguments: Some(serde_json::Map::new()),
        })
        .await
        .expect("a redirect that stays on loopback must still be followed");
}
