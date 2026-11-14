---
id: 2026-11-14-a-redirect-moves-a-connector-key-header-to-another-host
date: 2026-11-14
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A REST connector that sends its secret in a header it names (X-Api-Key, Private-Token, ...)
  sent that header to whatever host or port a redirect named.
---

## Bug
Found by a code audit of the repository (ponytail audit, finding 14). A first host that
answers `3xx Location: <another host>` received the request, and the redirect target got the
next one with the connector's own key header, a POST body on 307/308, and the OAuth2 token
form (`client_secret`) on 307/308.

## Root cause
`https_only()` (`crates/holon-mcp-client/src/secure_client.rs`) refused a hop that left
https (loopback excepted) and a chain past ten hops, and followed every other hop. reqwest
0.12 removes only `Authorization`, `Cookie`, `Cookie2`, `Proxy-Authorization` and
`WWW-Authenticate` on a host or port change (`remove_sensitive_headers` in its
redirect module). A static REST auth header is set per request under the name the sidecar gives
(`crates/holon-mcp-client/src/rest_transport.rs`, `RestAuth::Static`), so it stayed on the
request.

## Missing piece
- Oracle: no test looked at what a second host receives. The redirect test that went to
  another port on loopback asserted that the hop is followed.
- Coverage: no redirect test sent a key header under a custom name, a POST body, or a
  `localhost` target from a `127.0.0.1` first URL.

## Remedy
`https_only()` refuses a hop whose origin (scheme, host, port) differs from the hop before it,
with an error carrying `REDIRECT_REFUSED` that does not quote the target. Every client built
from it gets the rule: REST calls, the OAuth2 token client, the consent flow and MCP over HTTP.
A connector whose service redirects to another origin is configured with the final URL.

Tests, with a listener on a second loopback origin whose received bytes must be empty:
- `redirect_downgrade::cross_origin_redirect_never_reaches_second_host` (property over status,
  header name, target kind and method);
- `redirect_downgrade::same_origin_redirect_is_followed` (property: a same-origin chain of up
  to nine hops is still followed, with the key header on each hop);
- `http_bounds::mcp_cross_origin_redirect_never_reaches_second_host` (the MCP client).
