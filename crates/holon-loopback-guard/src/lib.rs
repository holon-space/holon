//! @c4 component
//! @c4 layer Adapters
//! Pattern: Adapter
//!
//! Host and Origin check for Holon's local HTTP servers.
//!
//! A web page that rebinds its DNS name to 127.0.0.1 reaches a loopback server
//! as a same-origin page, so the browser's CORS rules do not stop it. The page
//! still sends its own name in `Host` and `Origin`, and that is what this
//! check refuses:
//!
//! ```rust,ignore
//! router.layer(axum::middleware::from_fn_with_state(
//!     AllowedAuthorities::for_bind(listener.local_addr()?),
//!     refuse_foreign_host_or_origin,
//! ))
//! ```

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::Request;
use axum::extract::State;
use axum::http::StatusCode;
use axum::http::header;
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::response::Response;

/// The `host:port` authorities a request may name in `Host` and `Origin`.
#[derive(Clone, Debug)]
pub struct AllowedAuthorities(Arc<[String]>);

impl AllowedAuthorities {
    /// The loopback names on the bound port, plus the bound address itself
    /// unless it is a wildcard: an IP literal is a name no rebinding page can
    /// send, so a deliberate bind to one address keeps serving its clients.
    pub fn for_bind(bound: SocketAddr) -> Self {
        let port = bound.port();
        let mut authorities = vec![
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
        ];
        if bound.ip().is_unspecified() {
            tracing::warn!(
                "{bound} is a wildcard bind, but only requests naming {authorities:?} in Host \
                 are served; bind a specific address to serve clients that reach it by that address"
            );
        } else if !authorities.contains(&bound.to_string()) {
            authorities.push(bound.to_string());
        }
        Self(authorities.into())
    }

    fn allows(&self, authority: &[u8]) -> bool {
        self.0
            .iter()
            .any(|allowed| allowed.as_bytes().eq_ignore_ascii_case(authority))
    }
}

/// Answers 403 unless every `Host` (and the request URI's authority, if any)
/// is an allowed authority and every `Origin` is `http://` plus one.
pub async fn refuse_foreign_host_or_origin(
    State(allowed): State<AllowedAuthorities>,
    request: Request,
    next: Next,
) -> Response {
    match refusal(&allowed, &request) {
        None => next.run(request).await,
        Some(reason) => {
            tracing::warn!("refused {} {}: {reason}", request.method(), request.uri());
            (StatusCode::FORBIDDEN, reason).into_response()
        }
    }
}

fn refusal(allowed: &AllowedAuthorities, request: &Request) -> Option<String> {
    let headers = request.headers();
    let mut hosts: Vec<&[u8]> = headers
        .get_all(header::HOST)
        .iter()
        .map(|value| value.as_bytes())
        .collect();
    if let Some(authority) = request.uri().authority() {
        hosts.push(authority.as_str().as_bytes());
    }
    if hosts.is_empty() {
        return Some("the request names no host".to_string());
    }
    if let Some(host) = hosts.into_iter().find(|host| !allowed.allows(host)) {
        return Some(format!(
            "host {:?} is not one of {:?}",
            String::from_utf8_lossy(host),
            allowed.0
        ));
    }
    headers
        .get_all(header::ORIGIN)
        .iter()
        .find(|origin| {
            !origin
                .as_bytes()
                .strip_prefix(b"http://")
                .is_some_and(|authority| allowed.allows(authority))
        })
        .map(|origin| {
            format!(
                "origin {:?} is not http:// plus one of {:?}",
                String::from_utf8_lossy(origin.as_bytes()),
                allowed.0
            )
        })
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt;

    use super::*;

    async fn status(bound: &str, headers: &[(&str, &str)]) -> StatusCode {
        let app = Router::new().route("/", get(|| async { "served" })).layer(
            axum::middleware::from_fn_with_state(
                AllowedAuthorities::for_bind(bound.parse().unwrap()),
                refuse_foreign_host_or_origin,
            ),
        );
        let mut request = Request::builder().uri("/");
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        app.oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn loopback_names_are_served_in_any_case() {
        for host in ["127.0.0.1:8520", "LocalHost:8520", "[::1]:8520"] {
            assert_eq!(
                status("127.0.0.1:8520", &[("host", host)]).await,
                StatusCode::OK,
                "{host}"
            );
        }
    }

    #[tokio::test]
    async fn a_missing_or_foreign_host_is_refused() {
        let cases: [&[(&str, &str)]; 5] = [
            &[],
            &[("host", "localhost")],
            &[("host", "localhost:9999")],
            &[("host", "evil.example:8520")],
            &[("host", "localhost:8520"), ("host", "evil.example:8520")],
        ];
        for headers in cases {
            assert_eq!(
                status("127.0.0.1:8520", headers).await,
                StatusCode::FORBIDDEN,
                "{headers:?}"
            );
        }
    }

    #[tokio::test]
    async fn only_a_plain_http_loopback_origin_is_served() {
        let served = |origin| async move {
            status(
                "127.0.0.1:8520",
                &[("host", "localhost:8520"), ("origin", origin)],
            )
            .await
        };
        assert_eq!(served("http://localhost:8520").await, StatusCode::OK);
        for origin in [
            "null",
            "http://evil.example:8520",
            "http://localhost:3000",
            "https://localhost:8520",
            "http://localhost:8520.evil.example",
        ] {
            assert_eq!(served(origin).await, StatusCode::FORBIDDEN, "{origin}");
        }
    }

    #[tokio::test]
    async fn a_specific_bind_also_serves_its_own_address_and_a_wildcard_does_not() {
        let lan = [("host", "192.168.1.5:8000")];
        assert_eq!(status("192.168.1.5:8000", &lan).await, StatusCode::OK);
        assert_eq!(status("0.0.0.0:8000", &lan).await, StatusCode::FORBIDDEN);
        assert_eq!(
            status("0.0.0.0:8000", &[("host", "localhost:8000")]).await,
            StatusCode::OK
        );
    }
}
