---
id: 2026-07-13-edit-reload-permanently-loses-root-layout
date: 2026-07-13
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  Edit+reload permanently loses root-layout ("No root layout found"); banner
  promise "Refreshing keeps your data" false — vault bricked
source_line: 986
---

## Bug

Edit+reload permanently loses root-layout ("No root layout found"); banner
promise "Refreshing keeps your data" false — vault bricked

## Missing piece

keystone runs headless in-process on native turso (which already carries the
fix), never exercises SQLite-over-OPFS persist→reopen; no invariant asserts
the `block` matview row-set equals `block_raw` after a
close→reopen(+checkpoint) cycle

## Remedy

FIXED (B1). Root cause was NOT WAL loss — `block_raw` (base) stays fully
intact across reopen; the `block` IVM matview DESYNCS from it after the
reopen-triggered autocheckpoint: rows dropped (incl. `block:root-layout`,
`block:root-layout::src::0`, `holon-app-layout::render::0`,
`default-left-sidebar`) while `block:journals`/`block:welcome` were TRIPLED
— non-deterministic DBSP MergeOperator rowids. seed skips (its existence
check reads the same broken matview) so it never self-heals. The worker's
turso pin lagged at stale `e2737c65` (0.7.0-pre.6); native/root already runs
the fix via a working-tree path patch. Remedy: bumped
`frontends/holon-worker/Cargo.toml` turso [patch] to a SURGICAL cherry-pick
(turso rev `84bb233a`, branch `holon-worker-b1-fix`) of the two IVM
matview-reopen fixes' SOURCE files (`8517e30647` antijoin-ghost-row +
`ce1504b8` deterministic MergeOperator rowids) onto e2737c65's
wasm-buildable dep graph — a straight branch-bump to the fork tip pulls
newer deps that re-enable tokio net/fs on wasm and break the worker build.
Verified via playwright over live OPFS: edit→blur→reload survives (×2),
multi-edit+Enter-split→reload survives, unicode 你好🐍foo_bar survives;
relay-probe confirms matview == block_raw post-reload (0 dropped, 0 dup).
KEYSTONE PARITY (not implemented here): the one composed keystone PBT
(`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`) could
grow a `persist_reopen_matview_consistency` arm that boots a file-backed DB,
mutates, closes+reopens (forcing a WAL checkpoint), and asserts every
base-table row surfaces exactly once in each dependent matview — the
invariant that would have caught this in-process
