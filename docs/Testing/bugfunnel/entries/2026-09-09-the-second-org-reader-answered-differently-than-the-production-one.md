---
id: 2026-09-09-the-second-org-reader-answered-differently-than-the-production-one
date: 2026-09-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `holon_toon::org_reader` is a second reader of the org format and read a
  drawer-authored `:priority: A` as no priority at all, and an invalid `[#Ä]`
  cookie as ordinary title text, where the production parser reads a priority
  and refuses loudly — two readers of one format giving different answers.
---

## Bug

Found by the verifier on the `org-priority` lane (rev 2), reproduced
differentially.

`org_reader` exists to ground the org-vs-TOON token measurement in real vault
files. It is not a production parser, and today it has no in-tree consumer
outside `examples/measure.rs` — that bounds the blast radius but does not make
two disagreeing readers of one format safe, and the measurement it produces is
only meaningful while both read the same files the same way.

Measured divergences, on identical input:

| Input | production parser | `org_reader` (before) |
|---|---|---|
| `:priority: A` in the drawer | `Some(A)` | `None` — dropped into the generic props map |
| `[#Ä]` cookie | refused loudly | silently kept as title text |
| `[#A]` + `:priority: B` | refused loudly | `Some(A)`, drawer ignored |

The first row is rev 1's original defect — a drawer-authored priority being
invisible — surviving in the second reader after it was fixed in the first.

Red (`lane-logs/rev3-RED.log`):

```
FAIL holon-toon::org_reader_agrees_with_org_format
     both_org_readers_agree_on_every_priority_carrier
  [drawer only] the second org reader must read the file to mean the same thing
    left: None   right: Letter('A')
```

## Root cause

`org_reader` handled only the headline cookie and routed every other drawer key
into an untyped `BTreeMap`, so the drawer spelling of the priority never reached
the typed field. The cookie scan indexed byte 1 for the closing bracket, so a
multi-byte letter could not even be seen, let alone judged.

Underneath: the crate keeps its own `Priority` type because it is a
dependency-free codec, and nothing forced the two readers' behaviour to agree.

## Missing piece

**COVERAGE.** Both readers had their own tests and both were green; no test ever
fed the SAME bytes to both and compared. A second implementation of a format is
only as safe as the differential test that ties it to the first, and there was
none.

## Remedy

FIXED in the `org-priority` lane (rev 3). `org_reader` now collects every
priority carrier (the cookie and each drawer spelling, case-insensitively),
refuses any disagreement between them, and refuses a one-character cookie that
is not an uppercase letter — the production parser's rules.

Covering test (red above, green in `lane-logs/rev3-green1.log`):
`holon-toon::org_reader_agrees_with_org_format::both_org_readers_agree_on_every_priority_carrier`
— ten org snippets fed to BOTH readers, asserting each reader's answer against
the production parser's authoritative verdict, so a drift in either shows up by
name rather than as two wrongs agreeing.

Note on scope: the two crates still define two `Priority` types (`holon-toon`
declares zero runtime dependencies by design, and `holon-api` pulls rhai, tokio
and opentelemetry). The differential test is what now holds them to the same
behaviour. Collapsing them into one type needs a small shared crate and is not
done.
