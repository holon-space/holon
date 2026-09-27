---
id: 2026-09-27-id-fragment-or-query-dropped-from-org-id-line
date: 2026-09-27
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block or page id with a URI fragment or query (`block:a#b`, `block:a?b`)
  was written to its org id line as `a` and read back as `block:a`: a silent
  id rewrite on heading, source block and page `#+ID:`.
---

## Bug
Found by the adversarial verifier of org-faithful group A, round 5
(`lane-logs/groupA-r5-verify.md`, defect 1). The live route is an explicit id
from an unchecked Markdown carrier
(`2026-09-27-markdown-declared-id-bypasses-the-id-rule`).

## Root cause
`own_drawer_id` (`crates/holon-org-format/src/models.rs`) checked
`EntityUri::id()`, the URI path. The fragment and query are outside the path,
so they were dropped before the check and the rest passed.

## Missing piece
The id property drew ids from closed alphabets without `#` or `?`, so it
could not generate this shape.

## Remedy
The check reads the whole URI text after `block:`. The property
`any_text_is_accepted_exactly_when_every_carrier_keeps_it` draws arbitrary
text (printable ASCII, unicode, whitespace, control) and asserts that the
accept set equals the set that round-trips on heading, source block and page;
plus `an_id_with_a_fragment_or_query_is_refused_on_every_carrier`
(`crates/holon-org-format/tests/org_id_contract.rs`, red
`lane-logs/groupA-r6-red.log`, minimal input `"+?"`).
