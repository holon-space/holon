---
id: 2026-09-14-remote-list-sync-keystone-unreachable
date: 2026-09-14
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The keystone PBT drives no configured remote-list connection, so nothing in it
  fails if a sync round stops reconciling. The generic reconciler that replaced
  the bespoke shopping client is pinned only by dedicated tests.
---

## The gap

Lowcode Inc 5 replaced the bespoke shopping client with one generic
`RemoteListReconciler` and one `remote_list_sync` operation
(`crates/holon-connections`). The behaviour is pinned by
`crates/holon-connections/tests/remote_list_reconcile_pbt.rs` (both declared key
shapes) and by the mock-peer round in
`crates/holon-app/tests/shopping_pull_mock.rs`.

Neither is the keystone. Martin's standing rule (D118 note, 2026-09-12) is that
a behaviour pinned only outside the keystone is a reportable coverage gap, so
this entry states it rather than letting the FeatureMap row read as covered.

## Why the keystone cannot reach it today

The keystone composes over a vault and its projections. A remote-list round
needs three things the keystone has none of:

1. **A configured connection.** `configured_lists` builds one only from a
   sidecar that declares `holon.list_sync`, resolved through the MCP integration
   registry. The keystone wires no integration registry.
2. **A peer.** `RemoteListPeer` is satisfied in production by a `utcp:` call
   surface over HTTP. A keystone transition would need a fixture peer inside the
   composed SUT, which is a second source of truth the composed model would then
   have to mirror.
3. **A mirror table.** `SqlMirrorRows` reads the table the sidecar names; the
   keystone's vault-derived projections do not include an integration's mirror.

## What would close it

The honest rung is a keystone transition `SyncRemoteList` over an IN-PROCESS
fixture peer declared by a test sidecar, with the reference model holding the
peer's list. That makes the peer part of the composed state rather than an
external service, which is what the composed model needs to predict an outcome.
Sizing that transition is the open work; it was not attempted in this increment.

## Interim pins

| What | Where |
|---|---|
| Reconciler over both key shapes | `crates/holon-connections/tests/remote_list_reconcile_pbt.rs` |
| One round over a mock HTTP peer | `crates/holon-app/tests/shopping_pull_mock.rs` |
| The shopping sidecar's mapping | `crates/holon-kitchen/tests/shopping_mapping_differential.rs` |

## What closed it

The rung described above landed: a generic `RemoteListPeer` fixture family
(`crates/holon-connections-testing`) drives the REAL `holon_connections::sync_once`
round and reconciler, and the composed keystone owns one such peer as a SUT
component (`SutRemoteListSync`) with the peer's list as the reference model
(`RemoteListRefState`). The `RemoteListSync` transition mutates the peer and runs
two rounds (the second must decide nothing), and
`inv-remote-list-mirror-matches-ref` compares the SUT's mirror table against the
peer's declared list. The keystone's non-vacuity guard
(`wide_cap_presence_guard`) now requires the cap, so the connector cannot go
unseen again.
