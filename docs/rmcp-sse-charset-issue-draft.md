# DRAFT — upstream issue for `modelcontextprotocol/rust-sdk` (rmcp)

> **Status: DRAFT. NOT filed.** This is a ready-to-file issue report kept in-repo
> so we can file it upstream when we decide to. Holon currently carries a local
> workaround (see "Our workaround" below); this draft is the path to removing it.
>
> Target repo: <https://github.com/modelcontextprotocol/rust-sdk>
> Affected version: `rmcp` v0.12.0 (tag `rmcp-v0.12.0`, commit `0d65822`)

---

## Title

Streamable-HTTP server hardcodes `Content-Type: text/event-stream` (and
`application/json`) with **no `charset` parameter** → non-UTF-8 default decoding
(mojibake) for spec-compliant clients

## Summary

The streamable-HTTP server transport sets the SSE response `Content-Type` to the
bare `text/event-stream`, with no `charset` parameter. Per the HTTP and WHATWG
specs, a `text/*` response **with no `charset`** does not default to UTF-8 for
all clients; a conformant client may decode the body as ISO-8859-1 (Latin-1).
Any multibyte UTF-8 in tool output (non-ASCII text, emoji, CJK, accented
characters) is then mis-decoded into mojibake. MCP payloads are JSON, which is
UTF-8 by the JSON spec (RFC 8259 §8.1), so the transport's charset-less framing
contradicts the payload it carries.

## Where (source)

`rmcp` v0.12.0 (`0d65822`):

- The MIME constants are bare, no charset:
  `crates/rmcp/src/transport/common/http_header.rs:3-4`
  ```rust
  pub const EVENT_STREAM_MIME_TYPE: &str = "text/event-stream";
  pub const JSON_MIME_TYPE: &str = "application/json";
  ```
- The SSE response builder emits the bare constant:
  `crates/rmcp/src/transport/common/server_side_http.rs:89-91`
  ```rust
  Response::builder()
      .status(http::StatusCode::OK)
      .header(http::header::CONTENT_TYPE, EVENT_STREAM_MIME_TYPE)   // <- no ; charset=utf-8
      .header(http::header::CACHE_CONTROL, "no-cache")
      .body(stream)
  ```
  The single-shot JSON responses in
  `crates/rmcp/src/transport/streamable_http_server/tower.rs` emit
  `JSON_MIME_TYPE` the same way.

## Spec citations

- **RFC 8259 (JSON) §8.1** — "JSON text exchanged between systems … SHALL be
  encoded using UTF-8." MCP bodies are JSON, so they are UTF-8.
- **WHATWG Fetch / MIME Sniffing** — a `text/*` resource with no `charset`
  parameter has no guaranteed UTF-8 default; the charset is unspecified and the
  consumer may fall back to a legacy encoding (Latin-1). The fix is to *state*
  the charset in the `Content-Type`.
- **RFC 6838 §4.2.1** — the `charset` parameter is the standard, in-band way to
  declare a text media type's encoding; omitting it delegates the decision to
  the client's default, which is not portable.

## Reproduction

1. Start any `rmcp` streamable-HTTP MCP server (v0.12.0).
2. Have a tool return a string with non-ASCII UTF-8, e.g. `"café — 日本語 — 🚀"`.
3. Inspect the SSE response headers:
   ```
   Content-Type: text/event-stream
   ```
   Note: **no `; charset=utf-8`**.
4. Decode the body with a spec-default (Latin-1) text decoder — the bytes
   `0xC3 0xA9` (`é`) render as `Ã©`, etc. A client that assumes UTF-8 happens to
   work, but the response does not *tell* it to, so correctness depends on client
   leniency rather than on the wire format.

## Suggested patch

Declare the charset in the emitted headers. Minimal, spec-safe:

Option A — bake the charset into the constants (touches only `http_header.rs`,
fixes both SSE and JSON at once). Note this changes the constants used in
`Accept`-matching, so those comparisons must switch to a prefix/`starts_with`
check (some already do), or keep the bare constants for matching and add
separate `*_CONTENT_TYPE` constants for emission:

```rust
// http_header.rs — emission values
pub const EVENT_STREAM_CONTENT_TYPE: &str = "text/event-stream; charset=utf-8";
pub const JSON_CONTENT_TYPE: &str        = "application/json; charset=utf-8";
// keep EVENT_STREAM_MIME_TYPE / JSON_MIME_TYPE (bare) for Accept-header matching
```

Option B — append the charset at each response builder site
(`server_side_http.rs` SSE builder + the JSON builders in `tower.rs`):

```rust
.header(http::header::CONTENT_TYPE, "text/event-stream; charset=utf-8")
```

Option A is preferred: it centralizes the encoding declaration and keeps SSE and
JSON consistent. Happy to open a PR.

## Our workaround (for context)

Because the header is set inside the dependency, we can't patch the literal in
place without forking. We wrap the whole Axum router in a response-rewriting
middleware that appends `; charset=utf-8` to bare `text/event-stream` and
`application/json` responses on the way out
(`frontends/mcp/src/di.rs`, `add_utf8_charset_to_content_type`). Landing the fix
upstream lets us delete that middleware.
