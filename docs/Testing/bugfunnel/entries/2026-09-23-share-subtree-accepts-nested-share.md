---
id: 2026-09-23-share-subtree-accepts-nested-share
date: 2026-09-23
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  share_subtree accepted a page that already held a share mount, moving the
  mount into a second shared doc, so owning_page of the inner share failed.
---

## Bug
A verifier of the readauth-2 lane (Inc 0c, D197.a) probed a nested share:
share block `gamma` (mount M1 under page `host`), then share page `host`.
The second share succeeded and moved M1 into shared doc 2. After that,
`owning_page(gamma)` and `owning_page(M1)` returned internal errors, because
the Loro walk looks for a mount only in the global doc. ADR 0028 A7 forbids
nested and overlapping shares in v1, but nothing enforced it.

## Root cause
`LoroShareBackend::share_subtree` (crates/holon-loro/src/loro_share_backend.rs)
refused only a target that IS a mount. `extract_for_share` forks the whole
subtree, mounts included. A block inside a shared subtree was refused only
by accident, with a misleading "not found in Loro tree" error.

## Missing piece
The keystone PBT has no `share_subtree` transition (only the two-instance
suite shares), so no generated sequence can share twice along one path.

## Remedy
`share_subtree` now refuses both shapes with a typed
`NestedShareRefusal` (`ContainsShare` / `InsideShare`,
crates/holon-loro/src/shared_tree.rs), checked under the global write lock.
Pinned red-first by `share_subtree_refuses_nested_shares` in
loro_share_backend.rs (red log: readauth-2 lane-logs/inc0c3-nested-red.log).
Open: a keystone share transition would close the coverage gap.
