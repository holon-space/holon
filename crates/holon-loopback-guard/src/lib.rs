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
    use axum::http::Method;
    use axum::routing::get;
    use proptest::prelude::*;
    use proptest::test_runner::Config;
    use proptest::test_runner::TestCaseError;
    use proptest::test_runner::TestRunner;
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

    /// A `host:port` a request names, and whether the guard may serve it.
    #[derive(Clone, Debug)]
    struct Named {
        authority: String,
        allowed: bool,
    }

    const LOOPBACK_NAMES: [&str; 3] = ["127.0.0.1", "localhost", "[::1]"];

    /// A loopback name on `port` in a drawn letter case, a foreign name on
    /// `port`, or a loopback name on another port.
    fn named(port: u16) -> impl Strategy<Value = Named> {
        let loopback = || prop::sample::select(LOOPBACK_NAMES.to_vec());
        prop_oneof![
            (loopback(), prop::collection::vec(any::<bool>(), 9)).prop_map(move |(name, upper)| {
                let name: String = name
                    .chars()
                    .zip(upper)
                    .map(|(c, up)| if up { c.to_ascii_uppercase() } else { c })
                    .collect();
                Named {
                    authority: format!("{name}:{port}"),
                    allowed: true,
                }
            }),
            "[a-z]{1,8}\\.(example|invalid)".prop_map(move |name| Named {
                authority: format!("{name}:{port}"),
                allowed: false,
            }),
            (loopback(), any::<u16>())
                .prop_filter("another port", move |(_, other)| *other != port)
                .prop_map(|(name, other)| Named {
                    authority: format!("{name}:{other}"),
                    allowed: false,
                }),
        ]
    }

    #[derive(Clone, Debug)]
    struct GuardCase {
        bound: SocketAddr,
        method: Method,
        /// The authority of an absolute-form request target.
        target: Option<Named>,
        host: Option<Named>,
        /// The scheme and the authority of an `Origin` header.
        origin: Option<(&'static str, Named)>,
    }

    impl GuardCase {
        /// Served iff some host-like value names the server, and every
        /// present host, target authority and origin is allowed.
        fn served(&self) -> bool {
            let present_allowed = |n: &Option<Named>| n.as_ref().is_none_or(|n| n.allowed);
            (self.target.is_some() || self.host.is_some())
                && present_allowed(&self.target)
                && present_allowed(&self.host)
                && self
                    .origin
                    .as_ref()
                    .is_none_or(|(scheme, n)| *scheme == "http://" && n.allowed)
        }
    }

    fn guard_case() -> impl Strategy<Value = GuardCase> {
        (
            prop::sample::select(vec![
                std::net::IpAddr::from([127, 0, 0, 1]),
                std::net::IpAddr::from(std::net::Ipv6Addr::LOCALHOST),
            ]),
            1u16..,
        )
            .prop_flat_map(|(ip, port)| {
                (
                    Just(SocketAddr::new(ip, port)),
                    prop::sample::select(vec![Method::GET, Method::POST]),
                    prop::option::of(named(port)),
                    prop::option::of(named(port)),
                    prop::option::of((
                        prop::sample::select(vec!["http://", "https://"]),
                        named(port),
                    )),
                )
            })
            .prop_map(|(bound, method, target, host, origin)| GuardCase {
                bound,
                method,
                target,
                host,
                origin,
            })
    }

    async fn judge(case: &GuardCase) -> StatusCode {
        let app = Router::new()
            .route("/mcp", axum::routing::any(|| async { "served" }))
            .layer(axum::middleware::from_fn_with_state(
                AllowedAuthorities::for_bind(case.bound),
                refuse_foreign_host_or_origin,
            ));
        let uri = match &case.target {
            Some(target) => format!("http://{}/mcp", target.authority),
            None => "/mcp".to_string(),
        };
        let mut request = Request::builder().method(case.method.clone()).uri(uri);
        if let Some(host) = &case.host {
            request = request.header("host", &host.authority);
        }
        if let Some((scheme, origin)) = &case.origin {
            request = request.header("origin", format!("{scheme}{}", origin.authority));
        }
        app.oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    fn check(case: &GuardCase) -> Result<(), TestCaseError> {
        let status = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime")
            .block_on(judge(case));
        let expected = if case.served() {
            StatusCode::OK
        } else {
            StatusCode::FORBIDDEN
        };
        prop_assert_eq!(status, expected, "{:?}", case);
        Ok(())
    }

    #[test]
    fn every_host_like_value_and_origin_must_name_the_server() {
        let bound: SocketAddr = "127.0.0.1:8520".parse().unwrap();
        let named = |authority: &str, allowed| Named {
            authority: authority.to_string(),
            allowed,
        };
        let examples = [
            GuardCase {
                bound,
                method: Method::POST,
                target: Some(named("evil.example:8520", false)),
                host: Some(named("127.0.0.1:8520", true)),
                origin: None,
            },
            GuardCase {
                bound,
                method: Method::POST,
                target: Some(named("localhost:8520", true)),
                host: Some(named("evil.example:8520", false)),
                origin: None,
            },
            GuardCase {
                bound,
                method: Method::GET,
                target: Some(named("LOCALHOST:8520", true)),
                host: None,
                origin: None,
            },
        ];
        for case in &examples {
            check(case).unwrap_or_else(|e| panic!("{e}"));
        }
        TestRunner::new(Config {
            cases: 512,
            failure_persistence: None,
            ..Config::default()
        })
        .run(&guard_case(), |case| check(&case))
        .unwrap_or_else(|e| panic!("{e}"));
    }
}
