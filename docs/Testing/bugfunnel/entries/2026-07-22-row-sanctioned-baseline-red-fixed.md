---
id: 2026-07-22-row-sanctioned-baseline-red-fixed
date: 2026-07-22
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  ROW 28 (the sanctioned baseline RED) — FIXED.
source_line: 799
---

## Bug

**ROW 28 (the sanctioned baseline RED) — FIXED.** The keystone's ONE
whitelisted red since ~2026-07-18: `inv-blocks-match-ref/org`
`only_in_ref=[block:journals::action::0]` (the `daily_journal` `holon_rule`
action). NOT a store bug — `/block_raw`, `/matview`, `/loro` were always
green; the block IS in the store and ON DISK. It was a REAL org ROUND-TRIP
data loss: when `convert_block_to_page(journals::auto-create)` fires, the
`holon_rule` child moves into the new page's file (`Journals/Journal
Auto-Create.org`) as a TOP-LEVEL `#+BEGIN_SRC` directly under the page
`#+ID:` header (no enclosing `* headline`), and
`holon_org_format::parser::parse_org_file` only walked `doc.headlines()` —
silently dropping EVERY pre-first-headline source block on read-back. Masked
for months because the only top-level sources are normally the seed display
blocks (`journals::src::0`/`render::0`), which the ref excludes; the
non-seed `action::0` only surfaces after a BlockToPage. RULING = PROD-SIDE
(the ref correctly expects the block; prod loses it on write->read). The
2026-07-16 oracle-asymmetry precedent is RULED OUT — that was a non-frontend
draw modelling frontend-only seeds; this is a genuine frontend round-trip
loss.

## Missing piece

The convert-to-page page-file top-level-source round-trip was never a GREEN
assertion — the red was accepted as a baseline and whitelisted by every gate
(`grep -q inv-blocks-match-ref` allowlists) rather than root-caused, so no
rung proved a page whose direct child is a source block survives parse.

## Remedy

FIXED 2026-07-22 at TWO layers. (1) PARSER
(`crates/holon-org-format/src/parser.rs`): `parse_org_file` now extracts the
document's top-level section (`doc.section()`) sources/images as direct doc
children via the shared `emit_section_children` helper (also used by
`process_headlines`), so a top-level `#+BEGIN_SRC` round-trips identically
to one under a headline. (2) ORG CORRESPONDENCE
(`crates/holon-integration-tests/src/pbt/composed/correspondences.rs`):
`extract_org_snapshot` now filters the SUT org blocks by
`RefBackend::seed_block_ids` — the symmetric twin of turso-testing's
`extract_block_raw` — because the parser fix newly surfaces the seed display
sources the ref already excludes (without this they read as spurious
`only_in_actual`). Red-first proven by
`crates/holon-org-format/tests/top_level_source_roundtrip.rs` (0 blocks
parsed pre-fix -> action::0 survives post-fix). Keystone `test result: ok`
(4 passed), `inv-blocks-match-ref/org` N/N green across every engagement
summary; re-run twice at different seeds. Harness sanctioning comment
(`composed/harness.rs`) updated: NO sanctioned red remains, any red is a
regression. SIBLING FIX (hand-authored sidecar
`block-to-page-slash-content-empty-segment`, dogfood bug a1): its RED was a
DIFFERENT root — the keystone oracle's BlockToPage did not mirror the landed
backend trailing-`/` sanitize (866977e85e), so the born-equal page
id/content disagreed (`harness.rs:603` reconcile panic). FIXED by
`transitions/block_to_page.rs::sanitize_page_leaf` (mirrors the backend;
used for the page id in `plan_new_page` and the page content in
`reference_state::apply_block_to_page`; origin block + `[[P]]` link label
keep the RAW content, matching the backend). RESIDUAL (sidecar RETIRED in
`keystone.jsonl`, not deleted): replayed standalone it further surfaces a
hand-authored-replay-only org/loro divergence on the boot-fired journal
day-page (`2026-01-15` under `block:journals`) + a `__document_root__`
sentinel the direct `test_sequential` path does not read (the GENERATED
keystone reads it fine) — a replay-harness gap, NOT the `/`-content bug;
re-enable the case once the replay path materialises/reads the boot-journal
file.
