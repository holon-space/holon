---
id: 2026-07-10-gpui-live-text-path-mcp-describe
date: 2026-07-10
gap: PERCEPTION
secondary: null
status: UNCLASSIFIED
summary: >-
  GPUI live text path (or MCP describe_ui capture) truncates multibyte chars
  to their first UTF-8 byte (em-dash U+2014 → `â`); any non-ASCII in a
  rendered text node regresses the same way (found while fixing the rule-card
  mojibake, via byte-level probe — not by a test)
source_line: 843
---

## Bug

GPUI live text path (or MCP describe_ui capture) truncates multibyte chars
to their first UTF-8 byte (em-dash U+2014 → `â`); any non-ASCII in a
rendered text node regresses the same way (found while fixing the rule-card
mojibake, via byte-level probe — not by a test)

## Missing piece

no encoding assertion on the live render/capture channel; headless snapshot
never sees the shaped text

## Remedy

RE-TRIAGED 2026-07-10 (dogfood #2): the truncation was the CAPTURE channel,
not GPUI — wire bytes proven correct UTF-8 (raw urllib probe of the MCP SSE
stream) and GPUI pixels render 深い木/🐍/café/naïve/你好世界 perfectly; the MCP SSE
response omits `charset=utf-8` so spec-default Latin-1 decoders (incl. the
dogfood CLI's `r.text`) mojibake every multibyte char. See the 2026-07-10
SSE-charset row. Residual real defect: seeded rule-card description strings
carry mojibake IN the DB seed (`â¦` survives a correct-decoding channel)
