---
id: 2026-08-08-wiki-link-typed-page-exists-makes
date: 2026-08-08
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A wiki link typed to a page that EXISTS makes the org file oscillate forever
source_line: 760
---

## Bug

(task #12 lane, found while probing the typed-link path — outside any
committed draw) **A wiki link typed to a page that EXISTS makes the org file
oscillate forever**: adoption and resolution both succeed (`block_links`
resolves to `block:journals`), but the writeback emits the RESOLVED
`[[block:journals][Journals]]` while SQL keeps the Name-form mark, so the
render from SQL is `[[Journals]]` and the two never agree —
`inv-org-render-fixed-point` reports "not a transient projection lag but a
real echo-loop / oscillation: the next `re_render_all_tracked` would keep
rewriting the file", PERSISTED over 5s. A DANGLING link settles in one
cycle, so it is link RESOLUTION, not adoption, that has no fixed point.

## Root cause

task #12 lane: **a wiki link typed to a page that EXISTS makes the org file
oscillate forever — `render != disk` PERSISTED for 5s, the harness's own
echo-loop signature.** Found while probing the typed-link path, OUTSIDE any
committed draw. Typing `see [[Journals]] now` into an existing block adopts
correctly and the junction resolves to `block:journals`, but the writeback
then emits the RESOLVED form `* see [[block:journals][Journals]]` while SQL
still holds the Name-form mark (`Link{target: Name{"Journals"}}`), so
re-rendering from SQL produces `* see [[Journals]] now` and the two never
agree: `inv-org-render-fixed-point` reds with "not a transient projection
lag but a real echo-loop / oscillation: the next re_render_all_tracked would
keep rewriting the file". A DANGLING link settles in one cycle — it is
resolution, not adoption, that fails to reach a fixed point. COVERAGE: the
keystone's link draws never type a link whose target page exists, so no draw
ever reached the resolve-then-render step; the oracle needed no change and
fired on the first such case, which is what makes this a coverage gap rather
than an oracle one. Independent of the task #12 create-boundary fix (it
reproduces on the `set_field` path, which has adopted since
links-increment-3). NOT FIXED here — the committed hand-authored row
deliberately uses a dangling target so it asserts adoption without asserting
the unmodeled resolution; task #21 tracks the fix. Evidence:
`docs/Testing/fixture-logs-2026-08-08/typed-wiki-links-create-boundary-fix.txt`
§G. **FIXED 2026-08-08 (task #21), and the ECHO-LOOP READING IS REFUTED**:
prod's disk IS a fixed point — every production writeback renders through
`WritebackRenderer::with_resolved_links`, so `re_render_all_tracked`
rewrites identical bytes. The permanent divergence was the harness's own:
`SutOrgRender` rendered through `OrgRenderer` directly and skipped that
upgrade, comparing a render production never performs against bytes
production wrote. See the Ledger row for the root cause, the fix, and the
store↔disk asymmetry that stays OPEN)

## Missing piece

No keystone draw types a link whose target page exists, so no draw reached
the resolve-then-render step — which is also why nothing had ever exercised
the harness's own org render against a resolved link. Missing piece: a
generator arm that mints a link to a page already in the vault. (Verifier
note 2026-08-08, task #26: before adding that arm, make the ref lens
monotone — prod `page_reresolve_statements` fills only `resolved_id IS NULL`
and never un-resolves on rename/delete, while
`ReferenceState::resolve_page_name` resolves live; a rename-after-resolve
therefore draws a FALSE red on inv-blocks-match-ref/org. Also: RenamePage of
a SutOrgRender-tracked page is currently a hard ABORT, components.rs:2086.)
Secondary ORACLE (task #21): once a draw reached it, BOTH disk-view organs
were wrong — the SUT render and the reference's `org_blocks` lens.

## Remedy

**FIXED 2026-08-08 (task #21) — and the oscillation claim in the description
is REFUTED.** Root cause of the PERSISTED red:
`HeadlessFrontendComponent::snapshot_org_render_pairs`
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:2036`)
rendered via `OrgRenderer::render_document` DIRECTLY, skipping the
`block_links` link-mark upgrade
(`CacheBlockReader::resolve_link_marks_impl`,
`crates/holon-app/src/turso_seams.rs:190`) that
`WritebackRenderer::with_resolved_links`
(`crates/holon-filesystem/src/writeback_render.rs:85`) applies before EVERY
production write — so the invariant compared a render production never
performs against bytes production wrote, and the divergence could never
converge. Prod's disk IS a fixed point: `re_render_all_tracked` re-renders
through the resolving path and writes identical bytes, so there is no echo
loop and no repeated FSEvent. Second, independent red on the same case: the
reference's `RefBackend::org_blocks`
(`crates/holon-integration-tests/src/pbt/ref_caps/blocks.rs:432`) did not
model the resolved on-disk form (`sut=Link{Scheme "block:journals"}` vs
`ref=Link{Name "Journals"}`). Fix: the SUT render applies
`resolve_link_marks`; the ref's org lens applies
`apply_org_resolved_link_lens` (a mirror of
`SqlOperationProvider::resolve_page_name`). Regression: hand-authored
`wiki-link-to-an-existing-page-reaches-an-org-fixed-point` — the twin of the
dangling-target row. Red, A/B (each fix reverted in isolation reds exactly
its own invariant) and green preserved verbatim in
`docs/Testing/fixture-logs-2026-08-08/typed-wiki-link-to-existing-page-fixed-point.txt`.
**STILL OPEN, ESCALATED, not fixed here**: the STORES keep the authored
`Name` mark while disk carries `Scheme`, so the two authorities disagree
about the same link until some later file re-ingest (e.g. the next boot)
flips the store — which also flips its `block_links` row from `kind='page',
target='Journals'` to `kind='block', target='block:journals'`. The design
comment at `turso_seams.rs:186` expects re-ingest to "upgrade every store",
but the writeback's own write is echo-suppressed, so the upgrade fires only
by accident. Closing it is an architecture fork (resolve at every write
boundary including provider-direct org ingest, vs stop emitting the resolved
form and keep the authored bytes — `docs/Explanation/DESIGN_LINKS.md`
rewrites on NAVIGATE, not on writeback), which is Martin's call, not this
lane's. Evidence
`docs/Testing/fixture-logs-2026-08-08/typed-wiki-links-create-boundary-fix.txt`
§G. **ROOT CLOSED 2026-08-08 (task #32) — Martin ratified ruling (B): org
write-back emits the user's AUTHORED link bytes, never the resolved form.**
`[[Journals]]` now stays `[[Journals]]` on disk even when the target page
exists; resolution lives solely in `block_links`, and the id-rewrite belongs
to NAVIGATE (`docs/Explanation/DESIGN_LINKS.md` Phase 2-3), which resolves
the name live at click time (`SqlOperationProvider::create_page_from_link` →
`resolve_page_name`) and therefore never needed the id in the file. Shipped:
the render-time upgrade is DELETED, not disabled —
`BlockReader::resolve_link_marks` (the required trait method),
`CacheBlockReader::resolve_link_marks_impl` and
`WritebackRenderer::with_resolved_links` are gone, so no write path can
re-acquire it. The task #21 harness additions are REVERTED with them
(`SutOrgRender` renders marks verbatim; `apply_org_resolved_link_lens`
deleted), because under (B) prod performs no upgrade for the oracle to
model. The store↔disk asymmetry that was escalated is therefore GONE, and
with it the re-ingest `kind='page'`→`kind='block'` junction flip: it is now
unreachable, and PROVEN unreachable end to end by
`crates/holon/tests/live_edit_link_marks.rs::a_resolved_name_link_re_ingests_to_the_same_mark_and_junction`
(the fixed point across re-ingest that #21's verifier noted had never been
executed). Red-first log:
`crates/holon-orgmode/tests/writeback_emits_authored_link_bytes.rs::a_name_form_link_writes_back_as_authored`
failed against the pre-fix tree emitting `*
[[block:550e8400-e29b-41d4-a716-446655440000][Linked Page]]`; verbatim in
`lane-logs/task32-red-seam.log`. Input side untouched and pinned: a file
already carrying `[[block:<id>][Label]]` parses, resolves `kind='block'` and
is re-emitted verbatim
(`a_resolved_form_link_on_disk_still_ingests_as_a_block_link`,
`an_authored_id_link_writes_back_as_authored`). The keystone row
`wiki-link-to-an-existing-page-reaches-an-org-fixed-point` stays, with its
meaning flipped: the AUTHORED bytes are the fixed point. **OPEN FOLLOW-UP
(named, not fixed here): name-form links do not follow a page RENAME.**
After renaming `Journals`, clicking `[[Journals]]` resolves the old name
live and MINTS A NEW PAGE. Honest comparison with the status quo: the click
path has always read the block's MARK, and the stores always kept the `Name`
mark — the disk's resolved form only ever reached the mark if an accidental
re-ingest flipped the store, so pre-(B) rename behaviour was the same
duplicate-minting outcome, non-deterministically avoided. (B) therefore
makes renames EQUAL in the reachable worst case and strictly more
predictable, not worse; `page_reresolve_statements` fills only `resolved_id
IS NULL` and never un-resolves, so the junction row (and hence backlinks)
keeps pointing at the renamed page under both regimes — only the click path
is name-driven. Rename-time link maintenance is the real fix and is
deliberately out of this lane's scope.
