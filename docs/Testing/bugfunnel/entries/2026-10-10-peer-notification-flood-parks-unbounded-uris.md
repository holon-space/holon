---
id: 2026-10-10-peer-notification-flood-parks-unbounded-uris
date: 2026-10-10
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  An MCP peer sending `notifications/resources/updated` unasked decided how much memory Holon
  parked: 1024 notices with a 128 KiB URI each held 134 MB, with nothing refusing anything.
---

## Bug
Found by the security verifier of the MCP peer hardening lane `jaq-harden`
(`lane-logs/jaq-verify5.md`, D1) — a code audit plus a hostile mock peer, not a test in the
suite. A peer answers `initialize` and then pushes 1024 `notifications/resources/updated`,
each with a peer-chosen 128 KiB URI. Measured through the production stdio connect
(`lane-logs/jaq-r7-red-all.log`): **134,227,882 bytes parked, 1024 notices**, twice
`MAX_RESPONSE_BODY_BYTES`, and nothing refused any of it.

## Root cause
The whole inbound path was unbounded, in three places in a row:
`crates/holon-mcp-client/src/mcp_notification_handler.rs:29` created an
`mpsc::unbounded_channel()` and `on_resource_updated` pushed the peer's URI with
`let _ = self.sender.send(...)`; `crates/holon-mcp-client/src/mcp_integration.rs:1216` relayed
it into a SECOND unbounded channel; and `PendingSyncWork.uris` (same file) was an unbounded
`HashSet<String>`. Nothing drains any of them until the boot org scan opens the sync gate
(`gate_watchdog` 600 s), so the parking window is a peer-chosen amount of memory held for
minutes. Each notice is a complete message, so the per-connection partial-event allowance
never saw these bytes.

## Missing piece
No test ever played a peer that pushes unsolicited notices in VOLUME: every mock emits one
`resources/updated` to prove the resync wiring works (`pbt_mcp_fake.rs::emit_update`), which
is the happy path of exactly this mechanism. Secondary ENVIRONMENT: the parking only exists
before the sync gate opens, i.e. in the boot wiring the headless keystone does not stand up.

## Remedy
The inbound leg now goes through the connection's budget. `PeerBudget.inbound`
(`crates/holon-mcp-client/src/peer_budget.rs`) carries `InboundNotices`, and
`PeerBudget::notifying_handler()` (`mcp_notification_handler.rs`) is the only way to build the
handler, so a handler cannot exist without the bounds. The queue is bounded
(`MAX_PENDING_SYNC_URIS` = 256, the channel's capacity), a URI past
`MAX_NOTIFICATION_URI_BYTES` (2048) is refused, and `PendingSyncWork::absorb` carries the same
256 bound for one batch. The second unbounded channel and its forwarder task are gone: the
sync loop reads the bounded queue directly.

Neither bound drops a signal — a dropped `resources/updated` leaves rows stale while the UI
shows them as current. Hitting one COLLAPSES the connection's owed per-URI re-syncs into ONE
full re-sync, announced on `InboundNotices::collapse` and disclosed by the app as
`ConditionKind::IntegrationChangeSignalsCollapsed` (boot-always: degrade and disclose).

Pinned by `crates/holon-mcp-client/tests/inbound_bounds.rs` (oversized URIs, more pending
notices than the bound) and
`crates/holon-app/src/mcp_integrations.rs::collapsed_change_signals_are_disclosed_on_the_degraded_bus`.
Teeth: widening both bounds turns both tests red (`lane-logs/jaq-r7-teeth-inbound.log`), with
the constants restored byte-for-byte (`lane-logs/jaq-r7-sha-before.log` /
`-sha-after.log`).
