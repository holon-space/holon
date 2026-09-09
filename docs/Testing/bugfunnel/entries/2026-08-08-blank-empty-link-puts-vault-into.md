---
id: 2026-08-08-blank-empty-link-puts-vault-into
date: 2026-08-08
gap: COVERAGE
secondary: PERCEPTION
status: FIXED
summary: >-
  A blank `[[ ]]` or empty `[[]]` link puts the vault into a permanently
  ERROR-logging state
source_line: 764
---

## Bug

(dogfood-explorer gate pass) **A blank `[[ ]]` or empty `[[]]` link puts the
vault into a permanently ERROR-logging state**: both survive ingest
byte-for-byte and do NOT vanish, and no write-back loop was observed, but
the renderer classifies them `rung="Unrepresentable"` and logs `org render
is DEGRADED … NO emission of this block settles; write-back may loop on it`
at ERROR on every render, including a clean cold boot with no user action.

## Root cause

dogfood-explorer gate pass — **a blank `[[ ]]` or empty `[[]]` link puts the
vault into a permanently ERROR-logging state**. Both survive ingest
byte-for-byte and do NOT vanish (the behaviour the padded-link work asked
for), and no write-back loop was observed over 10 sha samples plus a forced
re-ingest — but the org renderer classifies them `rung="Unrepresentable"`
and says so at ERROR on every render, including a clean cold boot with no
user action: `org render is DEGRADED for this block: NO emission of this
block settles; write-back may loop on it block="block:ingest-blank-link"`.
Honest disclosure, wrong consequence: ordinary user-typable content reds
`inv-no-observed-errors` forever and buries real errors. COVERAGE — no
keystone draw generates a link with an empty or whitespace-only target, and
`LinkTarget` has no honest bucket for one (ORG_SYNTAX.md §"whitespace-only
segment" names the same hole from the classifier side). Evidence:
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§1)

## Missing piece

No keystone draw generates a link with an empty or whitespace-only target,
and `LinkTarget` has no honest bucket for one — ORG_SYNTAX.md names the same
hole from the classifier side.

## Remedy

**FIXED.** The COVERAGE half closed on its own: `extended_content_arm`
(`crates/holon-pbt-core/src/content_generators.rs:49-51`) draws
`[[{tail}]]` with `tail` matching `[A-Za-z0-9 ]{0,12}`, so an empty and a
whitespace-only target are both live draws. That is what turned this into a
stochastic keystone red — `inv-blocks-match-ref` / `inv-block-content`
reporting `content sut="[[]]" ref=""` on a bulk block — rather than a
dogfood-only sighting.

The ERROR was a symptom of the two halves of the store↔disk cycle
disagreeing. The render half deliberately kept the raw bytes and took the
loud `Unrepresentable` rung, while the extract half ERASED them:
`emit_mark`'s empty-label arm returned without emitting content
(`crates/holon-org-format/src/inline_marks.rs:811`), so a store holding
`[[]]` rendered `[[]]` to disk but re-parsed to `""`. The emission therefore
never settled, which is exactly what the ladder reports as
`rung="Unrepresentable"`, and a reference model that derives content by
running the cycle to convergence landed on `""` against the store's `[[]]`.

The fix keeps the literal's bytes at the parse boundary too: the empty-label
arm now emits the raw literal as plain text and still mints no mark, so the
zero-width Link mark that caused the original `]][[` corruption is still
never created. The cycle is now a fixed point at the authored bytes, and the
render is honestly `Exact` with no disclosure.

Covering tests (`crates/holon-org-format`):
- `render_marks_fixed_point_pbt::a_link_that_adopts_to_nothing_survives_re_ingest`
  — the re-ingest half, red before the fix with `left: "" right: "[[   ]]"`.
- `render_marks_fixed_point_pbt::a_link_that_adopts_to_nothing_keeps_its_bytes_and_settles`
  — the render half, now `Exact`.
- `degraded_render_severity::a_link_that_adopts_to_nothing_is_not_a_degradation_at_all`
  — pins that no ERROR is emitted, which is this entry's symptom.
- `inline_marks::tests::empty_link_mints_no_zero_width_mark_and_keeps_its_bytes`
  — pins both halves of the contract.

Original evidence
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§1.
