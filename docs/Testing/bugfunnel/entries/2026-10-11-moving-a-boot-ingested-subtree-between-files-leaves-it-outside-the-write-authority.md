---
id: 2026-10-11-moving-a-boot-ingested-subtree-between-files-leaves-it-outside-the-write-authority
date: 2026-10-11
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  An external cut/paste (`MoveBlockBetweenFiles`) of a nested TODO subtree out of
  a vault file that was present at boot makes the ingest of that file fail with
  "block:inv-fixture-child is not in the write authority", so the file is
  quarantined from write-back.
---
## Bug
Found by the keystone on the places-experiment lane (holon-space/holon-work
#164, workspace places-exp2), not on main. The lane seeds a test-only vault file
`invariant-fixture-page.org` at boot: a profile heading with a
`holon_entity_profile_yaml` source child, then `* TODO inv-fixture-parent` /
`** TODO inv-fixture-child`. A `MoveBlockBetweenFiles` of that subtree on the
`{Loro, Org, Turso}` draw went red on `inv-no-observed-errors`: three ERRORs
from `FileSyncController` / `holon_orgmode::di` — `apply_ingest_batch for
…/invariant-fixture-page.org: block:inv-fixture-child is not in the write
authority (unknown id, deleted, or this vault was never seeded)`, then the same
for `inv-fixture-parent` on the poll backstop. The file is then quarantined from
write-back. Log: places-exp2 `target/places-exp2-run-move-fixture-ingest-error.log`.

## Root cause
Not confirmed. The refusal is `ProjectionRead::NotRead`
(crates/holon-core/src/cell_registry.rs:109): the Loro write authority holds no
node for the block. Two hypotheses, in order of likelihood:

1. The move's cross-document re-home leaves the Loro tree stale. Twelve
   `loro_internal::state: Missing in parent's children` WARNs fire in the same
   millisecond window just before the refusal; the next ingest of the source
   file then writes on blocks Loro already moved away or dropped. This is a
   production defect.
2. The fixture's blocks never entered Loro (the declared "unseeded vault"
   class, bugfunnel
   2026-09-22-unseeded-vault-blocks-are-sql-only-so-a-loro-fed-read-model-cannot-show-them).
   The lane's boot wait (places-exp2 `wide_e2e.rs:~1560`) reads `block_raw`,
   not the authority, so it cannot tell. That is a lane fixture artifact.

To tell them apart: after the boot wait, ask the Loro authority for
`block:inv-fixture-child` before the move. Present → hypothesis 1.

## Missing piece
Main's keystone generates `MoveBlockBetweenFiles`, but no fixture file present
at boot has a nested TODO subtree under a heading that has a source child, so
main's catalog never moves such a subtree. Not reproduced on main: a cheap repro
needs the lane's fixture file.

## Remedy
Open, queued (no fix in this triage). First run the authority check above. If
hypothesis 1 holds, add a nested-subtree boot fixture file to main's keystone so
`MoveBlockBetweenFiles` can draw it, and get a red log before the fix.
