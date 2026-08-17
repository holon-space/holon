---
id: 2026-08-09-tagged-child-headings-quarantined-first-ingest
date: 2026-08-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A `Host Page.org` with `:Page:`-tagged child headings is quarantined on
  FIRST ingest — `INGEST DATA LOSS: … would DELETE 3 block(s) … (source has 4
  block(s), projection has 1)` plus the red "OrgMode initial scan degraded"
  banner — for the most ordinary authoring pattern there is.
source_line: 752
---

## Bug

(task #40 lane, hand-authored vault file) **A `Host Page.org` with
`:Page:`-tagged child headings is quarantined on FIRST ingest — `INGEST DATA
LOSS: … would DELETE 3 block(s) … (source has 4 block(s), projection has 1)`
plus the red "OrgMode initial scan degraded" banner — for the most ordinary
authoring pattern there is.** Nothing was lost: the children became page
doc-roots of their own files in that same ingest (the intended de-inline),
and `render_file_by_doc_id`'s walk legitimately stops at `Page` boundaries.
The ingest guard call
(`crates/holon-filesystem/src/file_sync_controller.rs:3823`) passed
`sibling_renders: &[]` by ADR-0025 design — grounding only against the
file's own projection, which cannot tell a relocated block from a destroyed
one — while the block-driven boundary next to it already asked the authority
which file owns each absent block (`writeback_sibling_grounding:5738`,
`AbsentOwner::AuthorityLost:5691`). The store-side skip
(`foreign_page_ids:2889`) only sees children that are ALREADY pages, so it
covers the second ingest and not the first.

## Root cause

task #40 lane, reported from a hand-authored vault file: **a `Host Page.org`
whose child headings carry `:Page:` is quarantined on FIRST ingest with
`INGEST DATA LOSS: write-back of Host Page.org would DELETE 3 block(s) that
exist on disk but did NOT survive ingest (source has 4 block(s), projection
has 1)`, plus the red "OrgMode initial scan degraded" banner** — the most
ordinary authoring pattern there is (write a page, give it subpages inline).
Nothing was lost: the `:Page:` children became page doc-roots of their own
files during that same ingest, which is the intended de-inline. ROOT CAUSE
is the grounding the ingest boundary was allowed to use.
`render_file_by_doc_id`'s walk stops at `Page` boundaries, so those children
are legitimately absent from the host's re-projection; the ingest guard call
at `crates/holon-filesystem/src/file_sync_controller.rs:3823` passed
`sibling_renders: &[]` on the explicit ADR-0025 reasoning that this boundary
"holds no op, so it grounds ONLY via the file's own projection" — which
cannot distinguish a relocated block from a destroyed one. The controller's
OTHER boundary already had the right evidence (`writeback_sibling_grounding`
:5738 asks the same authority the projection was rendered from which file
owns each absent block, and `AbsentOwner::AuthorityLost` :5691 keeps a
genuinely lost block ungrounded); the ingest boundary just did not call it.
Note the store-side skip (`foreign_page_ids`, :2889) only recognises
children that were ALREADY pages, so it fires on the SECOND ingest and not
the first — which is why the vault self-heals on the next poll and the
lasting damage is the degraded banner plus a failed initial scan. COVERAGE,
not ORACLE and not ENVIRONMENT: the failing path runs in the keystone's own
wiring (the frontend slice runs the real scan over a real temp vault) and
`inv-no-observed-errors` would have fired instantly — but every existing
`:Page:`-inline fixture (`SUBDIR_COMPANION_JOURNALS_ORG`,
`folder_companion_writeback_deinlines_child_page`) seeds a companion whose
child page ALREADY exists in the store, i.e. the second-ingest shape. No
draw ever presented a file whose `:Page:` children are BRAND NEW. Missing
piece = a seed/transition that authors a page file with not-yet-existing
`:Page:` children. FIXED in-lane: the ingest boundary now takes the same
authority-grounded verdict as the block-driven one
(`FileSyncController::writeback_drops`), and the weak entry point is DELETED
rather than left available — `FileFormatAdapter::check_writeback_lossless`
and `writeback_guard::ensure_ingest_lossless`/`IngestLoss` are gone from
holon-core, holon-orgmode and both holon-markdown adapters, so no boundary
can ground against a file's own projection alone. Red-first:
`crates/holon-integration-tests/tests/host_page_with_inline_subpages_ingest.rs`
reproduces the verbatim message and the degraded initial-scan verdict at
base; `crates/holon-orgmode/tests/ingest_data_loss_guard.rs` pins that a
block which never landed still refuses (mutation-proven: disarming the guard
reds exactly that test, its clean-ingest twin stays green). DISCLOSED, out
of scope: a `:Page:` heading nested under a PLAIN heading is still refused,
now as UNRESOLVABLE — `name_chain` fails loud on pages-under-non-pages
(interim ruling 2026-07-13), so that authoring shape stays rejected with a
better message, not accepted.)

## Missing piece

The path runs in the keystone's own wiring and `inv-no-observed-errors`
would have fired, so neither environment nor oracle was the weakness. Every
`:Page:`-inline fixture (`SUBDIR_COMPANION_JOURNALS_ORG`,
`folder_companion_writeback_deinlines_child_page`) seeds a companion whose
child page ALREADY exists in the store — the second-ingest shape. Missing
piece = a seed/transition authoring a page file whose `:Page:` children are
brand new.

## Remedy

**FIXED in-lane 2026-08-09 (task #40).** The ingest boundary takes the same
authority-grounded verdict as the block-driven one
(`FileSyncController::writeback_drops`), and the weak entry point is DELETED
rather than left available: `FileFormatAdapter::check_writeback_lossless`
plus `writeback_guard::ensure_ingest_lossless`/`IngestLoss` are gone from
holon-core, holon-orgmode and both holon-markdown adapters, so grounding
against a file's own projection alone is unrepresentable. Red-first
`crates/holon-integration-tests/tests/host_page_with_inline_subpages_ingest.rs`
(verbatim message + degraded scan verdict at base, green after);
`crates/holon-orgmode/tests/ingest_data_loss_guard.rs` pins that a block
which never landed still refuses, mutation-proven (disarming the guard reds
exactly that test; its clean-ingest twin stays green). Disclosed residual: a
`:Page:` heading nested under a PLAIN heading is still refused, now as
UNRESOLVABLE — `name_chain` fails loud on pages-under-non-pages (interim
ruling 2026-07-13).
