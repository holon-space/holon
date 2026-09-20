---
id: 2026-09-21-concurrent-loro-snapshot-saves-share-one-temp-name
date: 2026-09-21
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Two overlapping Loro snapshot saves wrote the same temp file, so the second
  rename found no source and failed the block operation with a bare
  "No such file or directory (os error 2)".
---

## Bug

`holon-app::cell_leg_delta_delivery::an_authoritative_write_wakes_the_editors_delta_stream`
failed 1 run in 10 in isolation during verifier round 6 of lane
`batch-identity` (`lane-logs/v6-cellleg-run6.log`):

```
block/set_field through the production dispatcher: Operation 'set_field' on
entity 'block' failed: No such file or directory (os error 2)
```

The error named no path, so the failing syscall was unknown. Found by a
verifier's repeat run, root-caused here by code audit plus a targeted
concurrency population test.

## Root cause

`write_atomic` (`crates/holon-loro/src/loro_document.rs`) minted its temp with
`path.with_extension("tmp-write")` — a name that depends only on the target,
so every saver of a given snapshot used the same temp. `LoroDocumentStore::save_all`
(`crates/holon-loro/src/loro_document_store.rs:301`) runs under a *read* lock
and is reached from `LoroBlockOperations::save_doc`
(`crates/holon-loro/src/loro_block_operations.rs:161`) on every block write, so
an ingest write-back and a user write are routinely inside `write_atomic` for
the same path at once. Interleaved, both write the shared temp, the first
`rename` consumes it, and the second gets `ENOENT`. Both `?`s were bare, so
neither the path nor the syscall reached the caller.

Measured: a new test driving two threads through `save_to_file` on one path,
300 rounds each, failed **258 of 600** saves with exactly the observed string,
the parent directory present (`lane-logs/d7-red.log`).

Pre-existing, not introduced by the lane: base rev `72a5e6a2` carries the same
shared-temp `write_atomic`.

## Missing piece

`holon-filesystem`'s `write_atomic_blocking` (`crates/holon-filesystem/src/fs_port.rs:231`)
already mints a per-process, per-call temp and names the path in its errors.
`holon-loro` carried a second, weaker copy of the same helper, and nothing
exercised either one from two threads.

The keystone PBT issues interactions sequentially and spawns no concurrent
writer, so it cannot place two savers in `write_atomic` together; the race is
pinned at the unit level instead
(`loro_document::tests::concurrent_snapshot_saves_do_not_steal_each_others_temp`).

## Remedy

`write_atomic` now delegates to
`holon_filesystem::fs_port::write_atomic_blocking`, which gives unique temps
and path-named errors. The population test is green 20/20 after the fix, and
the original flaky test 30/30 isolated (`lane-logs/d7-green.log`,
`lane-logs/d7-gates.log`).

Still open: the tree holds further atomic-replacement helpers.
`shared_snapshot_store.rs:118-150` (`stage_tmp` / `publish_tmp`) mints a
pid-and-sequence temp and fsyncs the file and the directory, so it does not
carry this race but does duplicate the mechanism;
`holon-mcp-client/src/integration_state.rs:138` (`write_atomically`) uses a
FIXED `.toml.tmp` temp, the same shape as the defect above, on a
single-writer config path. Consolidating them is queued separately.

The flake itself never reproduced on this tree — 0 failures in 30 isolated runs
and 0 in 10 full `-p holon-app` suite runs before the fix
(`lane-logs/d7-pop30.log`, `lane-logs/d7-suite10.log`) — so the tie from this
race to that one observed failure is by error shape and reachability, not by a
reproduction.
