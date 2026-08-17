---
id: 2026-08-01-link-target-whose-path-starts-colon
date: 2026-08-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A `[[…]]` link target whose path starts with a colon — `[[tag::x]]`,
  `[[person::odd]]` — is REWRITTEN on org write-back to
  `[[block:tag::x][tag::x]]`: the author's link text is replaced by a `block:`
  URI that wraps it, and a display label the author never typed is invented.
  The same call site is worse on `[[tag:a b]]`: it PANICS the ingest outright
  at `crates/holon-api/src/entity_uri.rs:58` (`EntityUri::new` cannot build
  `block:tag:a b`). MECHANISM: `LinkTargetClassifier::classify`'s Resolved arm
  (`crates/holon-api/src/link_parser.rs:178-179`) called `EntityUri::from_raw`
  on the raw authored target. `from_raw` carries a disambiguation heuristic
  that has nothing to do with links — it exists to keep BARE synthetic layout
  ids (`root-layout::src::0`) off the entity path, so it rejects any parse
  whose path starts with `:` unless the scheme is literally `block`, and
  re-mints the whole string as `block:<target>` (`entity_uri.rs:212`). Reached
  from authored link text that heuristic is pure corruption: the shape check
  immediately above it has ALREADY proved the target is a scheme, so there is
  nothing left to disambiguate, and `tag::x` is a perfectly legal RFC 3986 URI
  (scheme `tag`, path `:x`). It also contradicts the discriminator the rest of
  the system uses — `EntityRef::entity_uri()`
  (`crates/holon-api/src/inline_mark.rs:100-119`) calls `EntityUri::parse`, so
  it already answers "yes, `tag::x` is a `tag` entity" while the classifier
  answered "no, it is a block named `tag::x`". Violates the ratified #98-D
  invariant that authored link text round-trips byte-identically and that
  `from_raw` is never reachable from it. Found by a verification probe of the
  #98-D landing, not by any test.
source_line: 1133
---

## Bug

(task #14, verification probe) A `[[…]]` link target whose path starts with
a colon — `[[tag::x]]`, `[[person::odd]]` — is REWRITTEN on org write-back
to `[[block:tag::x][tag::x]]`: the author's link text is replaced by a
`block:` URI that wraps it, and a display label the author never typed is
invented. The same call site is worse on `[[tag:a b]]`: it PANICS the ingest
outright at `crates/holon-api/src/entity_uri.rs:58` (`EntityUri::new` cannot
build `block:tag:a b`). MECHANISM: `LinkTargetClassifier::classify`'s
Resolved arm (`crates/holon-api/src/link_parser.rs:178-179`) called
`EntityUri::from_raw` on the raw authored target. `from_raw` carries a
disambiguation heuristic that has nothing to do with links — it exists to
keep BARE synthetic layout ids (`root-layout::src::0`) off the entity path,
so it rejects any parse whose path starts with `:` unless the scheme is
literally `block`, and re-mints the whole string as `block:<target>`
(`entity_uri.rs:212`). Reached from authored link text that heuristic is
pure corruption: the shape check immediately above it has ALREADY proved the
target is a scheme, so there is nothing left to disambiguate, and `tag::x`
is a perfectly legal RFC 3986 URI (scheme `tag`, path `:x`). It also
contradicts the discriminator the rest of the system uses —
`EntityRef::entity_uri()` (`crates/holon-api/src/inline_mark.rs:100-119`)
calls `EntityUri::parse`, so it already answers "yes, `tag::x` is a `tag`
entity" while the classifier answered "no, it is a block named `tag::x`".
Violates the ratified #98-D invariant that authored link text round-trips
byte-identically and that `from_raw` is never reachable from it. Found by a
verification probe of the #98-D landing, not by any test.

## Missing piece

No generator draws a colon-leading-path target. The keystone's
`typing_text_strategy`
(`crates/holon-integration-tests/src/pbt/generators.rs:236-253`) mints only
bare wiki-name links; the #98-D counterexamples that DO cover colon-bearing
targets
(`org_roundtrip_characterization.rs::colon_in_a_later_path_segment_survives_roundtrip`
/ `whole_scheme_shaped_target_survives_roundtrip`) are hand-written, and
every one of their targets happens to parse into a NON-colon-leading path,
so the family stops exactly one character short of the defect. Not ORACLE:
the byte-equality oracle those tests already use went red on the first try
once the input existed, and the `tag:a b` shape panics loudly. SECOND
coverage finding from the same probe, filed here rather than as its own row
because it is the same escape: `link_mark_strategy` in
`crates/holon-block-roundtrip-testing/src/lib.rs` — the generator whose
comments claim to be the only cover for colon-bearing targets — is DEAD
under `render_marks_fixed_point_pbt`. Its arms build `InlineMark::Link {
label: String::new() }`, and an empty label makes `expected_reparse` demand
empty content, so the renderer degrades every generated link mark to
`AllMarksDropped` and no link TARGET ever reaches the emitted bytes.
Verified by biasing the strategy to emit only the defective target for all
600 cases: still green. Fixing that generator (labels matching their span
text) is a separate follow-up.

## Remedy

FIXED 2026-08-01 — `classify`'s scheme-shaped arm now calls
`EntityUri::parse` instead of `EntityUri::from_raw`, so a target that is a
legal URI with a registered scheme is `Resolved` with the AUTHORED bytes,
and one that is scheme-shaped but unparseable (`tag:a b`) joins the
unregistered-scheme case as `UnknownScheme(raw)` — which persists
identically (`inline_marks.rs:587-588`) and whose `entity_uri()` already
returns `None`, so no consumer changes behaviour and the panic path is gone.
`from_raw` itself is UNTOUCHED: its heuristic is correct for its legitimate
callers (org-parser ids, drawer slugs, fixtures) and only the link call site
was wrong; that call site's `ALLOW(entity_uri_from_raw)` suppression is
deleted. Red-first proof:
`org_roundtrip_characterization.rs::double_colon_scheme_target_survives_roundtrip`
failed with exactly `left: "see [[tag::x]] here"` / `right: "see
[[block:tag::x][tag::x]] here"`, and
`..::unparseable_registered_scheme_target_survives_roundtrip` failed with
the `entity_uri.rs:58` panic; both green after. Acceptance: full `holon-api`
+ `holon-org-format` suites, the three `structural_pbt::teeth` entity-link
tests, `holon-mcp-client`/`holon-markdown`/`holon-frontend`, and
`keystone-smoke` — all green. The generator arm and a fixed-point settle
test were written and then REMOVED once proven unable to go red (see Missing
piece); leaving them would have been false coverage.
