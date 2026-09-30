---
id: 2026-09-30-authored-marker-line-parse-time-unbounded
date: 2026-09-30
gap: COVERAGE
secondary: ORACLE
status: PARTIAL
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
Three mechanisms, measured as release CPU time of one parse and render of a
headline body line (`lane-logs/r14-scan-single.log`, `r14-scan-multi.log`,
`r14-cpu-slash.log` in the org-drawer-faithful lane):

- `[`: the orgize fork's `link_node` accepts `[` in a link path, so every
  `[[` reads to the end of the line. Quadratic (10k 1.17 s, 20k 4.62 s).
  Org's `org-link-bracket-re` excludes `[` and `]` from the path.
- `/`, `+`, `_`: linear, but about 60 us a character. `inline_marks.rs`
  parsed each emphasis node's contents again with orgize, 16 levels deep, and
  a render runs about 15 extractions.
- `(`: `scan_text_for_block_refs` searched for `))` to the end of the line for
  every `((`. Quadratic.

The same scan found more quadratic shapes, all in orgize object parsers
(release CPU time, 10k / 20k characters of the unit repeated, before any fix;
`lane-logs/r14-scan-multi.log`):

| unit | 10k | 20k | orgize parser |
|---|---|---|---|
| `*a ` `/a ` `+a ` `_a ` `=a ` `~a ` | 0.63 s | 2.54 s | emphasis |
| `_{` | 1.55 s | 6.18 s | subscript |
| `^{` | 0.51 s | 1.89 s | superscript |
| `a_{` / `a^{` | 0.23 / 0.19 s | 0.86 / 0.85 s | sub/superscript |
| `x_` | 0.19 s | 0.71 s | subscript |
| `[fn::` | 0.37 s | 1.36 s | footnote reference |
| `[fn:a:` | 0.26 s | 0.96 s | footnote reference |
| `\(` / `\[` | 0.27 / 0.28 s | 1.03 / 1.06 s | LaTeX fragment |
| `<%%(` | 0.26 s | 1.02 s | diary timestamp |
| `src_` | 0.06 s | 0.22 s | inline source |
| `call_` | 0.04 s | 0.15 s | inline call |
| `{{{a(` | 0.03 s | 0.13 s | macro |

## Missing piece
The 100k pin used only `*`, the generator drew no `[`, `{` or `(`, and no
assertion bounded the time.

## Remedy
- `inline_marks.rs` walks the emphasis contents in the tree orgize already
  built (`extract_contents_within`), and the block-ref scan stops at the
  first `((` with no `))` after it. 100k `/`: 0.35 s CPU release.
- The orgize fork excludes `[` from a link path (holon-space/orgize branch
  `holon` at `086b536c`, with an orgize test; Cargo.lock pins it). 100k `[`:
  0.14 s CPU release, byte-equal, 0 losses
  (`lane-logs/r14-release-100k-summary.log`).
- `a_marker_line_costs_time_linear_in_its_length` fails when 4x the length
  costs more than 8x the time (red `lane-logs/r14-f1-red.log`: `[` 1k 141 ms,
  4k 1.88 s; green on the pin `lane-logs/r14b-pin-tests.log`). The
  100k pin and the generator draw `[`, `{`, `(`, `~` and `=` too.
- The `[` exclusion also dropped links with an escaped bracket in the path:
  entry [2026-10-01-escaped-bracket-link-path-read-as-text](2026-10-01-escaped-bracket-link-path-read-as-text.md)
  (fork patch pending re-pin).
- Closed emphasis was still quadratic in the parse (whole-source copy per
  emphasis level) and in the render (mark-by-mark comparisons): entry
  [2026-10-01-closed-emphasis-line-quadratic-parse-and-render](2026-10-01-closed-emphasis-line-quadratic-parse-and-render.md),
  fixed, with an allocation-count test and a render time test.
- Emphasis (`*a ` and every other marker, any opening border), `_{`, `^{`,
  `a_{`, `a^{` and `x_` parse in linear time with the fork patch
  `lane-logs/r19-orgize.patch` (instruction count per doubling 1.96-2.00):
  entry [2026-10-02-unclosed-emphasis-opener-quadratic-parse](2026-10-02-unclosed-emphasis-opener-quadratic-parse.md).
- OPEN: still quadratic at the pinned fork `de82ea9` (verified in `lane-logs/groupB-r19v-verify.md`; instructions per doubling, debug,
  500/1000/2000 repeats): `[fn::` 3.8, `[fn:a:` 3.8, `\(` 3.8, `\[` 3.7,
  `<%%(` 3.9, `{{{a(` 3.1, ` src_x{` 3.9, ` call_f(` 3.9.
