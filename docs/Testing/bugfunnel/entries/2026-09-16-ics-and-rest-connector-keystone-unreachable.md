---
id: 2026-09-16-ics-and-rest-connector-keystone-unreachable
date: 2026-09-16
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The keystone PBT drives no configured connection, so nothing in it fails if
  the new `ics` feed codec stops decoding, if recurrence expansion breaks, or if
  a REST connector's write leg stops reaching the wire. All three are pinned
  only by dedicated tests.
---

## The gap

Google Onboarding Increment 1 adds an `ics` response codec
(`crates/holon-mcp-client/src/ics.rs`) and the bundled `ics-calendar` sidecar.
Increment 2w will add the reachability leg that lets a `rest` connection's
declared write tools reach the dispatcher. Both are behaviours of the CONNECTOR
path, and the composed keystone drives none of it: a feed that stopped decoding
into occurrences, a recurrence that stopped expanding, an `EXDATE` that stopped
removing an occurrence, or a write that stopped reaching the wire would each
leave the keystone green.

Martin's standing rule (D118 note, 2026-09-12) is that a behaviour pinned only
outside the keystone is a reportable coverage gap, so this entry states it
rather than letting the FeatureMap row read as covered.

This is the same shape as
`2026-09-14-remote-list-sync-keystone-unreachable` (same directory) and shares its closing
rung: a keystone transition over an IN-PROCESS fixture peer. One entry covers
both increments because closing one closes the other, and two entries for one
rung would double-count the same work in the COVERAGE total.

## Why the keystone cannot reach it today

The keystone composes over a vault and its projections. A connector round needs
three things the keystone has none of:

1. **A configured connection.** `configured_lists` and the MCP integration
   registry build one only from a sidecar resolved through `McpIntegrationsModule`,
   which the keystone wires not at all. The keystone's only integration-shaped
   transition is `EmitMcpData` (`crates/holon-integration-tests/src/pbt/transitions/emit_mcp_data.rs`),
   a faithful no-op that NO invariant observes.
2. **A peer or a call surface.** In production the `utcp:` path is satisfied by
   `RestCallSurface` over HTTP. A keystone transition needs the peer inside the
   composed SUT, which is a second source of truth the composed model would then
   have to mirror.
3. **A mirror table.** The sync path writes the entity's cache table, and the
   keystone's vault-derived projections do not include an integration's mirror.

## What would close it

The honest rung is a keystone transition over an IN-PROCESS fixture peer
declared by a test sidecar, with the reference model holding the peer's records:
the peer becomes part of the composed state rather than an external service,
which is what the composed model needs to predict an outcome. Concretely:

- one transition (`SyncRemoteList` for the remote-list path, or a
  `FetchFeed`-shaped one for a `sync` entity), added as a file under
  `crates/holon-integration-tests/src/pbt/transitions/` plus a `pub use` and a
  variant line in `src/pbt/transitions/mod.rs`;
- one invariant that JUDGES the decoded rows, wired with a `pub fn wire()` line
  in `composed_invariant_catalog()` (`crates/holon-integration-tests/src/pbt/composed/catalog.rs`);
- a capability and its registration in the composed SUT builder.

Sizing that transition is the open work. It was not attempted in Increment 1,
which is why this entry is filed rather than the gap left silent.

## Interim pins

| What | Where |
|---|---|
| The `ics` codec end-to-end through the real sync path (recurrence, EXDATE, override, all-day, folding, TEXT escaping) | `crates/holon-mcp-client/tests/rest_transport_mock.rs` |
| Refusals: an unresolvable `TZID`, a `VTODO` | same file |
| The Settings key configuring the connector, and the URL never reaching an error string | `crates/holon-app/tests/settings_ics_calendar_url_credential.rs` |
| The reachability leg of a REST write (Increment 2w) | `crates/holon-mcp-client/tests/rest_transport_write.rs` and the 2w-a dispatch test |
