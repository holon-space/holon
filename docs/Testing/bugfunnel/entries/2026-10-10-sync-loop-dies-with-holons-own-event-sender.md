---
id: 2026-10-10-sync-loop-dies-with-holons-own-event-sender
date: 2026-10-10
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An integration's sync loop ended as soon as Holon's own `SyncEvent` sender dropped, even
  though the peer's bounded inbound queue was still open — the peer's resource-updated notices
  then changed nothing and the rows stayed stale while the UI showed them as current.
---

## Bug
Found by the security verifier of the MCP peer hardening lane `jaq-harden`
(`lane-logs/jaq-verify6.md`, D1) by reading and then probing the shared PBT fake, not by a test
in the suite. Build `PbtMcpIntegration` and have the peer send
`notifications/resources/updated`: the notice is accepted into the bounded inbound queue and
never read. Measured through the fake (`lane-logs/jaq-r10-red-d1.log`): the mirror holds
**0 rows** five seconds after the peer announced its update.

## Root cause
`spawn_sync_event_loop` (`crates/holon-mcp-client/src/mcp_integration.rs`) selected over three
sources — Holon's `SyncEvent` channel, the peer's bounded URI queue, and the collapse watch —
but only the first one's `None` arm ended the whole task. The two peer-driven arms set a flag
and kept the loop alive; the `SyncEvent` arm `break`ed, so the lifetime of every source was
the lifetime of ONE sender. `PbtMcpIntegration::new` dropped its `SyncEvent` sender when it
returned, which killed the loop before any notice could be read.

## Missing piece
`PbtMcpIntegration` had no consumer anywhere in the repo, so no test observed its sync loop at
all, and the production loop's own tests drive it through the `SyncEvent` channel — the one
source whose closure was handled. Nothing exercised "one source closes while another is still
open", which is exactly the shape a live connection has: a peer keeps pushing notices long
after any single Holon-side sender is gone.

## Remedy
The loop now tracks `events_open` beside `inbound_open` and `collapses_open` and ends only when
ALL THREE are closed, draining whatever is pending first. A closed `SyncEvent` channel merely
disables that select arm, so an integration whose peer is still announcing changes keeps
syncing them.

Pinned by
`crates/holon-integration-tests/src/pbt_mcp_fake.rs::tests::a_notice_sent_after_the_integration_was_built_is_synced`,
which announces an update ONLY through the peer's notice (no direct `resync_by_uri`) and waits
for the row. The fake keeps dropping its `SyncEvent` sender on purpose: that is the property
the test pins. Teeth: on the unfixed loop the test fails with "the peer announced one updated
resource and nothing synced it: the mirror holds 0 rows" (`lane-logs/jaq-r10-red-d1.log`),
green after (`lane-logs/jaq-r10-green-d1.log`).
