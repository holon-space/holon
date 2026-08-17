---
id: 2026-08-09-holon-own-page-rename-retire-deletes
date: 2026-08-09
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  Holon's own page-rename retire deletes a file, and the watcher's delete
  event for that self-inflicted removal CASCADE-DELETES a DIFFERENT live
  page's blocks.
source_line: 747
---

## Bug

(task #53 lane, found by agent exploration — flagged by the 090aac6650
retire lane in its own commit message and by the #40 verifier's finding F5;
no automated test produced it) **Holon's own page-rename retire deletes a
file, and the watcher's delete event for that self-inflicted removal
CASCADE-DELETES a DIFFERENT live page's blocks.** The retire
(`materialize_page_identity_file`,
`crates/holon-filesystem/src/file_sync_controller.rs`) removes the page's
stale home under an ownership proof, then calls `forget_file_state`, which
drops that path's `last_projection` BEFORE the watcher's `Remove` for it
lands. `on_file_deleted` therefore cannot read the vanished file's `#+ID:`
and resolves the document by the path's NAME CHAIN — a lookup that returns
whatever page answers to the just-vacated name TODAY, not the page whose
bytes went away. A FILELESS namesake page (rule-minted journal date, a fresh
`convert_block_to_page`, any page whose materialize has not run yet) answers
it; the id-based reunification scan cannot rescue it because it owns no
tracked file for the scan to find; and the D3 stale-rename guard reads "same
file" because that page's own `authoritative_name_chain` derives exactly the
vacated path. Every block of that live document then goes through
`BlockOrdering::delete_in_tree`. Data-loss class: the user's content is
destroyed in the store, and the next write-back renders the loss to disk.

## Root cause

task #53 lane, found by agent exploration — the 090aac6650 retire lane
flagged it in its own commit message and the #40 verifier's F5 named the
same class; no test run produced it: **Holon's OWN page-rename retire
deletes a file, and the watcher's delete event for that self-inflicted
removal cascade-deleted a DIFFERENT live page's blocks.**
`forget_file_state` drops the retired path's `last_projection` before the
event lands, so `on_file_deleted` cannot read the vanished file's `#+ID:`
and resolves the document by the path's NAME CHAIN — which finds whatever
page answers to the just-vacated name today. A FILELESS namesake
(rule-minted page, fresh `convert_block_to_page`, a page whose materialize
has not run) answers it, the id-based reunification scan cannot save it (it
owns no tracked file to be found at), and the D3 stale-rename guard reads
"same file" because that page's own authoritative chain derives exactly the
vacated path — so every one of its blocks went through `delete_in_tree`.
FIXED 2026-08-09 by a `CascadeAuthority` ownership proof, the block-side
twin of `StaleHomeOwner`.)

## Missing piece

ENVIRONMENT (primary): the keystone harness has no real file-event watcher —
it pumps the controller over a path a transition NAMES
(`pump_watcher_over_disk_path`), and no transition names a path the SUT
itself removed, so `on_file_deleted` for a Holon-initiated removal never
runs in the test wiring at all. COVERAGE (secondary): even with that event
delivered, nothing in the catalog mints a page at a FREED page name and
leaves it fileless at that instant — `CreatePageAtFreedPath` materializes
its file. The oracle was adequate: a cascade of a live page's blocks reds
`inv-blocks-match-ref` and `inv-every-page-has-its-own-file`.

## Remedy

FIXED 2026-08-09 — `CascadeAuthority` (`file_sync_controller.rs`), the
block-side twin of `StaleHomeOwner`: identity read from the vanished file's
OWN last projection is self-proving (`ItsOwnLastProjection`); identity
resolved by name chain is a GUESS and reaches `delete_in_tree` only when a
home record — `doc_home` in every mode, plus the Loro-only alias registry —
puts that document's file at exactly this path (`OurHomeRecord`). Anything
else is `Refused(reason)`, a disclosed WARN naming the vanished path, the
document, where our record actually homes it (or that it is fileless), and
the consequence (blocks stay in the store, the path is dropped from
tracking) — the reason travels as data, so a silent refusal is
unrepresentable. RED-FIRST:
`retiring_a_stale_home_does_not_cascade_delete_a_fileless_namesake_page`
(`crates/holon-orgmode/tests/page_rename_retires_old_file.rs`) drives the
real `FileSyncController` through create→rename→retire→delete-event with a
fileless namesake and asserts on a recording `BlockOrdering` that NOTHING
was deleted — red with `left: ["block:pgnamesakechild", "block:pgnamesake"]
right: []` (`lane-logs/red-cascade-namesake.log`). Mutation-proven on the
discriminating axis: forcing `cascade_authority` to return an ACCEPT variant
unconditionally (the `Refused` arm removed) reds exactly that one test and
nothing else — independently reproduced by the lane's verifier.
Discriminating control in the same file, green both ways:
`a_users_deletion_of_a_tracked_file_still_cascades` — a genuine external
deletion of a tracked file still cascades, so the gate refuses the guess and
not every deletion. NOT CLOSED: the keystone parity gap itself (no watcher
rung that replays Holon-initiated removals) stays open — this lane pins the
behavior at the dedicated-test tier only.
