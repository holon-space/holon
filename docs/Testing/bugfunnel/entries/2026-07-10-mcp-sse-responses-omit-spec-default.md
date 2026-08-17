---
id: 2026-07-10-mcp-sse-responses-omit-spec-default
date: 2026-07-10
gap: PERCEPTION
secondary: ENVIRONMENT
status: FIXED
summary: >-
  MCP SSE responses omit `charset=utf-8` → spec-default Latin-1 clients (incl.
  the dogfood CLI's `r.text`) mojibake every multibyte char; this explains the
  prior "GPUI truncates multibyte" PERCEPTION row (wire bytes proven correct
  UTF-8; GPUI pixels render CJK/emoji/diacritics perfectly). Separately real:
  seeded rule-card/`indent`-op description strings contain mojibake IN the DB
  seed (`â¦` survives a correct-decoding channel — ingested mangled at
  authoring/seed time)
source_line: 885
---

## Bug

MCP SSE responses omit `charset=utf-8` → spec-default Latin-1 clients (incl.
the dogfood CLI's `r.text`) mojibake every multibyte char; this explains the
prior "GPUI truncates multibyte" PERCEPTION row (wire bytes proven correct
UTF-8; GPUI pixels render CJK/emoji/diacritics perfectly). Separately real:
seeded rule-card/`indent`-op description strings contain mojibake IN the DB
seed (`â¦` survives a correct-decoding channel — ingested mangled at
authoring/seed time)

## Missing piece

no UTF-8 round-trip assertion on the MCP/SSE channel; evidence tooling
decoded wrong

## Remedy

FIXED (stream 2026-07-10): bare header lives in the rmcp DEPENDENCY
(`server_side_http.rs:91`, rmcp-v0.12.0) — fixed via axum middleware
`add_utf8_charset_to_content_type` on both routers (SSE + JSON gain `;
charset=utf-8`; parameterized values pass through; 3 unit tests) +
`r.encoding="utf-8"` in holon_mcp_cli.py. Durable fix = upstream to
modelcontextprotocol/rust-sdk. Seed-mojibake sub-claim CORRECTED: exhaustive
≥0x80 byte sweep found NO mojibake in any checked-in source — the observed
`â¦` is live-DB data ingested/decoded through the charset-less channel;
needs a live-DB cleanup pass, not source edits
