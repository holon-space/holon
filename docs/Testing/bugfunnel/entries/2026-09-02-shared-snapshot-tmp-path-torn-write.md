---
id: 2026-09-02-shared-snapshot-tmp-path-torn-write
date: 2026-09-02
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Four call sites publish a share's snapshot through one deterministic
  `<id>.loro.tmp` path with no mutual exclusion, so two concurrent saves
  interleave into a torn file that is then fsynced and renamed into place.
---

## Bug

`SharedSnapshotStore::save` is documented as an atomic publish, and it is
atomic against a crash. It is **not** atomic against a second writer, and the
share backend has four call sites that reach it for the same share.

The temp path is derived from the share id alone and carries no per-writer
suffix (`crates/holon-loro/src/shared_snapshot_store.rs:84`,
`{shared_tree_id}.loro.tmp`), and `save` takes no lock. `File::create` is
`O_CREAT|O_TRUNC`, so a second writer truncates the file the first is still
writing while the first keeps writing at its own offset. The result is fsynced
and renamed into place, passing every check the code makes.

Call sites, all in `crates/holon-loro/src/loro_share_backend.rs`:

| Line | Caller |
| ---: | --- |
| 177 | the debounced save worker's work call |
| 675 | `flush_all` |
| 1116 | `sync_with_peers`, the save-before-push barrier |
| 1722 | `accept_shared_subtree`, the save-before-mount barrier |

Line numbers are as of the `subtree-share-race` tree; match on the
`snapshot_store.save(` call text if they have drifted.

The save worker and `sync_with_peers` are the routine pair: both are armed by
the same commits, on debounces of 150 ms and 500 ms, so their windows overlap
under ordinary editing rather than only at shutdown.

Found by a fresh-context verifier auditing this claim in the
`subtree-share-race` lane report, which had scoped the overlap's damage to a
spurious degraded banner. The verifier reproduced the interleaving directly
(`lane-logs/subtree-share-race-verify.md` §5):

```
published len=300000 (writer1 intended 300000)
bytes 0..1000 are writer2's: True
bytes 1000..150000 are NUL holes: True
published == a valid snapshot from either writer: False
```

## Root cause

Not yet fixed, so this is the mechanism rather than a post-mortem. One temp path
per share, shared by four unsynchronised writers. A per-writer unique temp name,
or a per-share write lock held across `create → write → fsync → rename`, would
each close it; the choice is a design call and is not made here.

Severity is a judgement call this entry does not make. A shared subtree is
pruned from the global `holon_tree.loro` at share time, so the per-share
snapshot is the only copy of its content on the device
(`crates/holon-loro/src/shared_snapshot_store.rs:7-10`), and a torn file that
still imports is a silent corruption rather than a loud one. The quarantine path
only triggers when `LoroDoc::import` fails.

## Missing piece

No invariant covers a concurrently-published snapshot. `P-NO-SILENT-CORRUPT` in
`subtree_share_round_trip_pbt` checks for **zero-byte** `.loro` files only, so a
NUL-holed file of plausible length passes it. The interaction is generatable —
the PBT already drives edits, restarts and syncs that overlap these writers —
but no oracle would flag the result, which is what makes this ORACLE rather than
COVERAGE.

A byte-level oracle would be: after every settle, every `.loro` under `shares/`
must import cleanly AND round-trip to the doc the writer intended.

## Remedy

FIXED in `share-lifecycle` rev 3, taking the unique-temp-name option of the two
this entry named. `SharedSnapshotStore::stage_tmp` gives every write a private
tmp sibling (`<final name>.<pid>-<seq>.tmp`) and `publish_tmp` renames it; all
four publish paths (`save`, `save_peers`, `save_port`, `save_generation`) go
through them, so no two writers of the same file can share a tmp. The trigger
was the same race surfacing on the peers sidecar under load —
`2026-09-09-concurrent-peer-sidecar-writes-share-one-tmp-file`, which carries
the full analysis and the second half of the fix (the peer set is now persisted
under the `known_peers` guard).

Covering test, red for the right reason first against a probe that restores the
fixed tmp name (`lane-logs/r3-red-3-save-probe.log`, the probe reverted with a
matching sha256 in `lane-logs/r3-probe-{green,restored}-sha.txt`):
`holon_loro::shared_snapshot_store::tests::concurrent_snapshot_saves_publish_a_loadable_file`
— two concurrent `save` calls for one share, the slower held inside its publish
window; it asserts both that the slower writer's publish succeeds and that the
published file still imports, which is this entry's byte-level oracle at the
unit level.

The suggested PBT oracle is NOT added: `P-NO-SILENT-CORRUPT` still checks only
for zero-byte files, so the "every `.loro` imports cleanly after every settle"
invariant remains unwritten. That is a residual coverage gap, not a live defect.

`sweep_stale_tmps` now collects any `*.tmp` under `shares/` rather than four
fixed suffixes, so a failed publish's orphan is still swept at the next startup
under the new naming. The note below about orphans is therefore still accurate:
a temp file must not be relied on to survive.

## Keystone repro

Not attempted. `general_e2e_composed_pbt` has no share or publish transition in
its catalog.
