---
id: 2026-10-12-an-abandoned-id-stamp-leaves-an-id-less-org-file-whose-blocks-lose-identity
date: 2026-10-12
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A stalled `:ID:` write-back is never retried, so the file stays id-less and
  every later parse of it mints fresh ids: the blocks lose their identity.
---

## Bug

A headline exists on disk without an `:ID:` drawer while its row already exists
in the store — the window between committing the row and stamping the id back
into the file. If that stamp is never written, the window never closes: the
file is id-less forever, and every parse of it invents new ids for its
headlines. The rows the vault named are replaced by strangers.

Found by reading prod, in the honeymoon 4 PN experiment
(holon-space/holon-work#175): its `crash_before_flush::commit_first` variant
panics with "a duplicate after the next ingest" (`["Old", "New", "New"]`), and
its `database_down_lazy_ids` variant with "the new block's id is not on disk".
The riskiest assumption of that model ("the boot ingest already is the merge")
holds only if a fresh id is in the file before any row has it.

Lane `hm4-repro` (#175) measured the prod shape 2026-10-12, and it is NOT the
model's: no restart is involved and nothing is duplicated. It is enough that
the owed `:ID:` write-back is ABANDONED while the app keeps running — the ids
are then REPLACED on the next parse (2 rows out, 2 fresh uuids in, 0 field
deltas).

## Root cause

- `crates/holon-org-format/src/parser.rs:1566-1573 (extract_or_generate_id)` —
  an id-less headline gets `Uuid::new_v4()`, a **new id per parse**. Identity
  for such a headline comes from reconciling against the store's current
  children (`tiered_match`), not from the file. That reconciliation holds only
  as long as the positional match holds; the file itself carries no identity.
- The owed write-back that would stamp the ids back is not retried when the
  stall clears — the registered known red
  `wb-d8-stalled-edit-never-reaches-disk`
  (`docs/Testing/KeystoneKnownReds.md`, bugfunnel
  `2026-10-06-pre-ingest-stall-loses-a-store-edit`). So the id-less state, which
  prod treats as transient, becomes permanent.

The gap exists wherever the id stamp can lag the row: a stalled or churning
write-back, a crash between commit and flush, and (per ruling
DB-none-sqlonly.d) the file-fallback writer, whose `:ID:` flush must therefore
be synchronous.

## Missing piece

Narrowed alphabet: `ArmWriteChurn` is `HandAuthoredOnly` without
`HOLON_PBT_WRITEBACK_FAULTS=1`
(`crates/holon-integration-tests/src/pbt/transitions/arm_write_churn.rs`), so
the generator never arms the stall that abandons the stamp. The oracle is not
the gap — `inv-blocks-match-ref/org` and `inv-org-render-fixed-point` both
judge the state as soon as the churn ends.

Measured 2026-10-12, the restart framing of the original entry was wrong in
both directions:

- With a RESTART the ids survive: `hm4-a-crash-over-an-id-less-file-re-stamps-its-ids-at-boot`
  is green (the boot re-ingests the id-less file and re-stamps it), as is
  `hm4-reboot-after-an-id-less-rewrite-that-was-re-stamped`.
- WITHOUT any restart the ids are lost: the churn simply ends under a live app.

Harness note: the only id-less-on-disk state the harness can author is
`StaleExternalRewrite`, which strips every `:ID:` of a document. A single NEW
id-less headline added by an external editor (the narrowest form of this bug)
has no transition — `ExternalFileEdit` offers only `EditLine`, `DeleteLine` and
`StripDocId` (`crates/holon-pbt-core/src/types.rs`,
`crates/holon-integration-tests/src/pbt/transitions/external_line_edit.rs`).
An `AddIdlessLine` variant would narrow the repro; this lane did not build it.

## Remedy

- Red-for-the-right-reason keystone case
  `hm4-an-abandoned-id-stamp-leaves-an-id-less-file-whose-blocks-lose-identity`
  in
  `crates/holon-integration-tests/hand-authored-regressions/known-red-hm4-restart.jsonl`
  (kept out of `keystone.jsonl` so the landing gate stays green; registered in
  `docs/Testing/KeystoneKnownReds.md` as
  `hm4-abandoned-id-stamp-loses-block-identity`). The red:
  `inv-blocks-match-ref/org` reports the two rows replaced by two fresh uuids
  with 0 field deltas, `inv-org-render-fixed-point` stays divergent for 5s
  (disk 38 bytes, rendered 100), and the `writeback-degraded` condition on
  hm4b.org is stale. Log
  `target/hm4-hm4-an-abandoned-id-stamp-leaves-an-id-less-file-whose-blocks-lose-identity.log`
  (hm4-repro lane).
- GREEN controls `hm4-a-crash-over-an-id-less-file-re-stamps-its-ids-at-boot`
  and `hm4-reboot-after-an-id-less-rewrite-that-was-re-stamped` pin that a boot
  re-stamps the ids, so the loss is on the live-app path.
- Fix (ruling HM4-crash.a, holon-space/holon-work#175): a file-born `:ID:`
  reaches the file before any row names it, so no id-less-on-disk window
  outlives the write that opened it. The retry half of that is
  `wb-d8-stalled-edit-never-reaches-disk`, owned by the write-back state
  machine.
