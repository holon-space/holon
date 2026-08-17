---
id: 2026-08-05-inc-differential-shadow-structurally-blind-block
date: 2026-08-05
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  The Inc-1 differential shadow was structurally blind to block-value
  provenance: it compares per-document member ids only (§9.1, deliberate), so
  the render-time Name→Scheme link upgrade living inside CacheBlockReader
  (get_blocks/get_block_authoritative ran substitute_resolved_links; the block
  matview projects marks verbatim) was invisible to the whole shadow phase —
  cutting over to feed-sourced values would have reverted every resolved page
  link ([[block:<id>][Label]] → [[Label]]; 18 in the real vault proper) with
  no existing end-to-end test on the upgrade.
source_line: 775
---

## Bug

(Option-C Inc-2 cutover lane — stopped itself pre-code) **The Inc-1
differential shadow was structurally blind to block-value provenance: it
compares per-document member ids only (§9.1, deliberate), so the render-time
Name→Scheme link upgrade living inside CacheBlockReader
(get_blocks/get_block_authoritative ran substitute_resolved_links; the block
matview projects marks verbatim) was invisible to the whole shadow phase —
cutting over to feed-sourced values would have reverted every resolved page
link ([[block:<id>][Label]] → [[Label]]; 18 in the real vault proper) with
no existing end-to-end test on the upgrade.** Caught by provenance-checking
during the cutover lane's read phase, before any cutover code.

## Root cause

Option-C Inc-2 cutover lane, caught by READING before any cutover code — Inc
1's differential shadow compares per-document member ids ONLY (design §9.1,
deliberate: values may legitimately differ under lag), leaving it
structurally blind to block VALUES' provenance;
CacheBlockReader::get_blocks/get_block_authoritative applied
substitute_resolved_links (render-time Name→Scheme link upgrade) while the
block matview projects marks verbatim, so cutover to feed-sourced values
would have reverted [[block:<id>][Label]] → [[Label]] (18 resolved links in
the real vault proper) with no end-to-end test covering the upgrade through
org write-back. Fixed pre-cutover: substitution moved to the render seam —
required BlockReader::resolve_link_marks applied in WritebackRenderer, raw
renderers private, MCP render_org routed through the resolving entries
(keystone-smoke caught a missed external caller the crate suites never would
— build red 3/3). Pinned by feed_value_drops_resolved_link_form.rs
(provenance is byte-visible) + render_seam_resolves_link_marks.rs (negative
case: unresolved links stay bare))

## Missing piece

The shadow's oracle scoped values out by design and no test rendered
feed-shaped values through org write-back; missing piece =
provenance-independence as a pinned property.

## Remedy

FIXED 2026-08-05 (Increment C, pre-cutover): substitution moved to the
render seam — required BlockReader::resolve_link_marks (no default: a
defaulted no-op is how this class recurs), applied in WritebackRenderer; raw
renderers private; materialize_page_identity_file renderer-bypass closed;
MCP render_org routed through resolving entries (verifier-confirmed; also
erases a prior render_org Loro-vs-Sql divergence — Loro-sourced slices now
resolve via the junction too). Verifier corrections recorded: the suspected
ingest-diff-base change at file_sync_controller.rs:2209 does NOT exist
(content_differs is mark-blind, proven empirically); pinned by
holon-org-format/tests/feed_value_drops_resolved_link_form.rs +
holon-orgmode/tests/render_seam_resolves_link_marks.rs.
