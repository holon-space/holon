---
id: 2026-09-30-authored-marker-line-parse-time-unbounded
date: 2026-09-30
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  One authored body line of 100,000 `[` takes 65.5 s to parse and render in
  release; the parse neither aborts nor loses text.
---

## Bug
Found by the org-faithful group B r13 verifier (`lane-logs/groupB-r13-verify.md`,
F-1). One `parse_org_file` plus `render_document` of a single body line of
100,000 of one marker character, output byte-equal with 0 losses. Measured in
`lane-logs/B13v2-gate4.log`:

| line | release | debug/test |
|---|---|---|
| 100k `*` | 93 ms | 3.28 s |
| 100k `/` | 3.66 s | 108.1 s |
| 100k `[` | 65.5 s | not reached (nextest 120 s timeout) |
| 100k `+` | 3.65 s | not measured |
| 100k `_` | 3.75 s | not measured |
| 100k `{` / `~` / `=` | 48 ms / 12.5 ms / 12.0 ms | not measured |

The SLO is p95 < 200 ms. The entry
[2026-09-30-deeply-nested-authored-text-aborts-the-parse](2026-09-30-deeply-nested-authored-text-aborts-the-parse.md)
claims bounded time.

## Root cause
Not yet analysed. The cost depends on the marker character.

## Missing piece
The 100k pin in `crates/holon-org-format/tests/deep_authored_text_never_aborts.rs`
(line 122) uses only `*`, the fastest marker. The property draws `*`, `/`, `+`,
`_` at 2,000 to 8,000 characters and never draws `[` or `{`. No assertion
bounds the time.

## Remedy
Open. Find the super-linear scan for `[`, `/`, `+` and `_`. Add the missing
markers to the pin and the generator, and add a time bound.
