---
id: 2026-10-01-external-rewrite-drops-a-pages-body
date: 2026-10-01
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  After an External org rewrite, a page whose content is title + body (e.g.
  made by BlockToPage on a multi-line block) reads back as its first line only;
  the rest of its text is gone from Loro, block_raw and the view model.
---

## Bug
Found by the verifier of the editor-leg-axis lane (`lane-logs/adm0b-verify.md`, O1): a keystone run
pinned to `HOLON_PBT_PIN_WIRING='Loro,Turso;;ActionEngine;Dispatch'` (shrink off, no seed recorded)
went red with a signature no registry row matched
(verifier scratch `v0b/pin3.log:1503`, classifier `v0b/pin3-classify.log`):
`inv-displayed-text/viewmodel` shown `"fwv RBY"`, expected `"fwv RBY\nsi4 \ne z Ph1239E \n,#+gh: x"`,
co-fired by `inv-blocks-match-ref/loro`, `/block_raw`, `/matview` and `inv-block-content/{block_raw,sql}`;
first-divergent-layer store/CRDT. The red came after an `ApplyMutation::Create` from the External
source; the truncated block was a page under `block:journals` that an earlier `BlockToPage` made.
The verifier's 4 further runs did not reproduce it, because a random run rarely makes that sequence.

Reproduced deterministically by the hand-authored row `external-rewrite-keeps-a-promoted-pages-body`
(`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`): BulkExternalAdd of a
two-line block, BlockToPage on it, then any External ApplyMutation. Probes
(`lane-logs/adm0b3-o1b-probe.log`, `lane-logs/adm0b3-o1c-probe.log`):

| Wiring | With External step | Without it |
|---|---|---|
| {Loro,Turso}+ActionEngine, Dispatch | red | green |
| {Loro,Turso}+ActionEngine, Cell | red | |
| {Turso} | red | |
| {Loro,Org,Turso}+ActionEngine (Org write-back on) | red | green |
| {Org,Turso} | | green |

The editor leg does not matter. Known red `external-rewrite-drops-page-body`
(docs/Testing/KeystoneKnownReds.md).

## Root cause
Not confirmed by a fix. By code reading: the External seam
(`HeadlessFrontendComponent::apply_mutation`,
`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs`) rewrites every doc file with
`serialize_blocks_to_org_with_doc` (`crates/holon-integration-tests/src/org_utils.rs`). It writes
`render_document_header` (the `#+TITLE:` line, the first line of the content) and the headlines, but
not the page's own body. Production's `OrgRenderer::render_document`
(`crates/holon-org-format/src/org_renderer.rs`, `render_pass`) writes that body as the preamble
between the header and the first headline. The re-ingest then reads a page with a title and no body.
With Org write-back on and no External step, the file production writes reads back whole, so the
evidence points at the harness's rewrite and not at production's write-back or parser. If that is
confirmed, this is a FALSE-ALARM (the test's external editor writes bytes production never writes).
Until then it stays OPEN as a possible data loss, classed ENVIRONMENT: the suspected cause is that
the test's external writer differs from production's writer.

## Missing piece
The External seam does not write a page file the way production does: the page body is not written.

## Remedy
Open. Confirm by writing the page body (or by rendering the page file with
`OrgRenderer::render_document`) in the External seam and replaying the row; if it turns green with no
product change, re-classify as FALSE-ALARM and retire the known red.
