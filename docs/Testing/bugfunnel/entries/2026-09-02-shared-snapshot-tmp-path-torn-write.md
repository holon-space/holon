---
id: 2026-09-02-shared-snapshot-tmp-path-torn-write
date: 2026-09-02
gap: ORACLE
secondary: null
status: OPEN
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

OPEN. Deliberately not fixed in the `subtree-share-race` lane, whose scope was
the `P-NO-TMP-LEFTOVER` flake. Needs an owner and a design call between a unique
temp name and a per-share write lock.

Note for whoever takes it: the retry sweep that lane added to `SettleSaves`
widens the window in which a genuine orphan self-heals, because a failed publish
leaves its temp file at the same path that the next successful publish renames
away. Any oracle written for this bug should not rely on the temp file surviving.

## Keystone repro

Not attempted. `general_e2e_composed_pbt` has no share or publish transition in
its catalog.
