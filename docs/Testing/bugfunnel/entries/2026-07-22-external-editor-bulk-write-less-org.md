---
id: 2026-07-22-external-editor-bulk-write-less-org
date: 2026-07-22
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  External-editor bulk-write of ID-less org headlines duplicates blocks under
  a running app. Observed live on the vault ("Prepare personal usage.org"): an
  external editor wrote a ~60-block restructure with ID-less headlines while
  the app was running; the app ingested it, minted UUIDs, and wrote the file
  back with `:ID:` props. A SECOND external write derived from the pre-mint
  text (the classic editor/agent write-then-follow-up-edit workflow)
  re-ingested the still-ID-less headlines, and the reconciler — matching
  UPDATE-vs-CREATE by block-id ONLY — treated every ID-less headline as NEW,
  minting a *fresh* UUID while the previously-minted block survived: every
  such block ended up duplicated under two different IDs (~60 duplicated at
  scale). Blocks that already carried `:ID:` were never duplicated. Likely the
  mechanism behind at least part of Martin's recurring "duplicate content"
  reports. Workaround: pre-mint UUIDs in the external editor + a single atomic
  write. ROOT CAUSE (verified): the external-file-change re-ingest handler
  `FileSyncController::on_file_changed`
  (`crates/holon-filesystem/src/file_sync_controller.rs:828`) → `ingest_file`
  reconciles the freshly-parsed blocks against `old_blocks: HashMap<EntityUri,
  Block>` keyed SOLELY by `block.id` (`file_sync_controller.rs:1226`–1231,
  `.map( | s | (s.block.id.clone(), s.block.clone()))`). The decision is
  by-id-only: CREATE when the id is absent (`file_sync_controller.rs:1602` `if
  !old_blocks.contains_key(&block.id)` → `create_in_tree` at 1636), UPDATE
  when present (`file_sync_controller.rs:1694` `old_blocks.get(id)`). And
  every ID-less headline is minted a *brand-new* `Uuid::new_v4()` on EVERY
  parse at the boundary (`crates/holon-org-format/src/parser.rs:726`–741,
  `extract_or_generate_id` returns a fresh UUID at line 741 when no `:ID:` is
  present). So a stale re-write of the same ID-less text parses to a different
  id than the first ingest → misses `old_blocks` → created anew → duplicate.
  There is NO content/position fallback in the match key.
source_line: 796
---

## Bug

External-editor bulk-write of ID-less org headlines duplicates blocks under
a running app. Observed live on the vault ("Prepare personal usage.org"): an
external editor wrote a ~60-block restructure with ID-less headlines while
the app was running; the app ingested it, minted UUIDs, and wrote the file
back with `:ID:` props. A SECOND external write derived from the pre-mint
text (the classic editor/agent write-then-follow-up-edit workflow)
re-ingested the still-ID-less headlines, and the reconciler — matching
UPDATE-vs-CREATE by block-id ONLY — treated every ID-less headline as NEW,
minting a *fresh* UUID while the previously-minted block survived: every
such block ended up duplicated under two different IDs (~60 duplicated at
scale). Blocks that already carried `:ID:` were never duplicated. Likely the
mechanism behind at least part of Martin's recurring "duplicate content"
reports. Workaround: pre-mint UUIDs in the external editor + a single atomic
write. ROOT CAUSE (verified): the external-file-change re-ingest handler
`FileSyncController::on_file_changed`
(`crates/holon-filesystem/src/file_sync_controller.rs:828`) → `ingest_file`
reconciles the freshly-parsed blocks against `old_blocks: HashMap<EntityUri,
Block>` keyed SOLELY by `block.id` (`file_sync_controller.rs:1226`–1231,
`.map( | s | (s.block.id.clone(), s.block.clone()))`). The decision is
by-id-only: CREATE when the id is absent (`file_sync_controller.rs:1602` `if
!old_blocks.contains_key(&block.id)` → `create_in_tree` at 1636), UPDATE
when present (`file_sync_controller.rs:1694` `old_blocks.get(id)`). And
every ID-less headline is minted a *brand-new* `Uuid::new_v4()` on EVERY
parse at the boundary (`crates/holon-org-format/src/parser.rs:726`–741,
`extract_or_generate_id` returns a fresh UUID at line 741 when no `:ID:` is
present). So a stale re-write of the same ID-less text parses to a different
id than the first ingest → misses `old_blocks` → created anew → duplicate.
There is NO content/position fallback in the match key.

## Missing piece

Litmus: "is there a transition sequence in the catalog+wiring that reaches
this state?" — NO. The keystone drives org round-trip as
SUT-writes-then-reads; there is no transition that models an EXTERNAL editor
bulk-writing an org file, and none that re-writes a *stale* pre-mint version
after the app's writeback. The triggering interaction (external write → app
writeback → stale external re-write → re-ingest) is ungeneratable → COVERAGE
primary. Secondary ENVIRONMENT (litmus: "does the failing code path even run
in the keystone's wiring?"): the OS-filewatch external-write re-ingest path
(`FileSyncController`/`file_watcher.rs`/`OrgModeSyncProvider`) plus the
app-running-concurrently-with-an-external-editor timing is prod-only wiring
the keystone never constructs — the same wiring-absence class as the prior
Directory/OrgModeSyncProvider ENVIRONMENT rows. Even with that path wired,
COVERAGE still dominates: the keystone cannot produce the
stale-external-rewrite sequence.

## Remedy

OPEN 2026-07-22 — TRIAGE ONLY, no code changed. Remedy shape (recorded in
the vault tracker): before minting a fresh UUID for an ID-less incoming
headline, match it against the existing children by CONTENT + POSITION (i.e.
add a content/position fallback to the by-id `old_blocks` lookup at
`file_sync_controller.rs:1694`/1602 so a re-ingested ID-less headline
reconciles onto its already-minted twin instead of creating a duplicate) —
this makes the writeback idempotent under stale re-writes. Keystone gap to
close alongside the fix: a transition (driver rung) that generates the write
→ writeback → stale-rewrite cycle — an external-editor-write step that emits
an ID-less file, lets the app ingest+writeback, then re-emits the pre-mint
text — so the keystone can go red on the duplicate before the prod fix
lands.
