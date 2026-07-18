# Leases & Read-Write for External Connectors

Status: RECOMMENDED (RW-3 + L-C/L-B layering) — increments 1–3 in progress; open questions 1–3 pending Martin; increment 4+ gated on Q1.

Governing ADR: [0024 — Unified Action Execution](../adr/0024-unified-action-execution.md), especially P4 (effect taxonomy: internal/idempotent vs external/once-only; UUIDv5 intent keys; execution logs are retry bookkeeping, never cross-replica dedup).

## 1. Problem

Every write to an external MCP connector fails loud today. `McpOperationProvider::read_only` builds a peer-less provider whose `execute_operation` denies all calls (`mcp_provider.rs:323-335`, denial at `:495-505`), and the live provider has no policy gate at all — the single dispatch chokepoint (`execute_operation`, `:489`) will happily call any advertised tool. `McpSidecar` (`mcp_sidecar.rs:12-34`) carries no write-policy field. Meanwhile `docs/integrations/todoist.yaml:69-122` already declares mutating tools (`complete-tasks`, `update-tasks`, `add-tasks`, `delete-object`, …) with `affected_fields`, `undo`, and `triggered_by`. `triggered_by` (`mcp_sidecar.rs:246` → `mcp_provider.rs:273`) is a reactive double-fire vector: a projected field change can re-dispatch a write.

We want to open external writes **safely and incrementally**, honouring ADR 0024's taxonomy: idempotent/keyed effects converge by construction (naming discipline), once-only external effects (send email, create-without-key) require *asymmetry* — a lease/writer — which no CRDT can provide and which therefore must be an explicit, disclosed, gated mechanism.

## 2. Design axes

Two independent axes.

### Read-Write policy surface (RW-*)

- **RW-1 — status quo (never allow):** all writes denied. Correct, useless.
- **RW-2 — global on/off:** one boolean; when on, any advertised tool executes. Simple, but erases the idempotent-vs-once-only distinction that ADR 0024 P4 makes load-bearing. A `create` with no idempotency key and a `set-field` are treated identically; reactive `triggered_by` storms become real duplicate emails. Rejected.
- **RW-3 — effect-classified policy surface (RECOMMENDED):** a master `writes: enabled|disabled` switch (absent = disabled = today's behaviour) **plus** a per-tool `effect: read | idempotent | keyed | once_only` classification, parsed at sidecar load. Enforcement at the single `execute_operation` chokepoint keys off the effect class:
  - `read` — always allowed.
  - `idempotent` / `keyed` — allowed when `writes: enabled`.
  - `once_only` — allowed only once a **writer/lease** is configured (increment 4); until then, blocked loud even when writes are enabled.
  A write-shaped tool (declares `affected_fields` or `undo`) with **no** explicit `effect` is a loud config error at connect — same discipline as a failing `views:` reconcile. This makes the policy legible per tool, keeps `read` traffic unaffected, and gives once-only effects their own gate instead of hiding them behind the global switch.

### Lease / writer designation (L-*)

Applies only to `once_only` effects (ADR 0024 P4: exactly-once world effects need an ownership token).

- **L-A — global single-writer:** one device holds a global write lease for the whole vault. Coarse; a phone that can complete a task must also own every connector.
- **L-B — per-connector lease:** the lease token is scoped to a connector (todoist, gmail…). Matches how connectors are provisioned per-device.
- **L-C — per-effect once-only lease:** the send transition consumes/holds an executor token per ADR 0024 P4's net vocabulary; the finest grain, and the one that maps cleanly onto the effect taxonomy.

**Recommended layering — L-C over L-B:** model once-only effects as per-effect leases (L-C) but *acquire and reconcile* them at per-connector granularity (L-B), so a device provisioning todoist takes the todoist writer role and every once-only effect on that connector rides the same lease epoch. TTL + reconciliation and manual takeover ("take over on this device") are stated honestly per ADR 0024 P4; on partition, duplicates are possible, disclosed, and surfaced in the automation journal.

## 3. Recommendation

**RW-3 + L-C/L-B layering.** Ship the policy surface and the idempotency machinery first (increments 1–3), which are safe and useful on their own (idempotent/keyed writes converge by naming discipline and need no lease). Gate once-only effects behind writer designation (increment 4), which is where the genuinely hard distributed-systems question lives and where an open user decision (Q1) blocks.

## 4. Increment plan

- **Increment 1 — policy surface + loud denial (this change).** `writes` master switch + per-tool `effect` enum on the sidecar, parsed at load (parse-don't-validate). Write-shaped-without-`effect` = loud config error. Enforcement at `execute_operation`: disabled → non-read denied naming the policy + fix; enabled → `once_only` denied "pending increment 4", `idempotent`/`keyed` pass, `read` always passes.
- **Increment 2 — mock write scenarios (harness before behaviour).** `holon-mcp-mock` gains a write tool and scenarios `WriteHappy`, `WriteDuplicateDetected` (echoes the idempotency key, dedups on repeat), `WriteConflict` (CAS/precondition failure), `WriteSlowAck` (accepts, delays the ack). Same `Scenario` + `MOCK_MCP_SCENARIO` idiom as the read scenarios.
- **Increment 3 — idempotency keys + sent-intents ledger (`keyed` tools).** Mint `UUIDv5(HOLON_CONNECTOR_NAMESPACE, connector ‖ tool ‖ entity-id ‖ intent-fingerprint)` per ADR 0024's naming discipline; the fingerprint reuses `effect_id::FiringKey` (canonical, type-tagged, order-independent serialization of the params). A `key_param:` sidecar field names which tool argument carries the key; the connector injects it into the outgoing call. A local sent-intents ledger records sent keys **strictly as retry bookkeeping** (ADR 0024 P4: an execution log is not a cross-replica dedup mechanism). Dedup authority is the remote — a retry-storm of N identical dispatches yields exactly one remote effect (asserted against `WriteDuplicateDetected`).
- **Increment 4 — writer designation (OUT OF SCOPE, gated on Q1).** Lease/writer token for `once_only` effects (L-C/L-B). Blocked pending the open user question on how the writer device is designated.

## 5. Open questions (pending Martin)

1. **(gates increment 4) Writer designation.** How is the once-only writer chosen — explicit per-device opt-in, first-provisioner-wins, or a vault-stored role? What is the lease TTL and the manual-takeover UX ("take over on this device")? ADR 0024 P4 requires manual override as a first-class capability, not a violation.
2. **Entity-id for create-shaped keyed effects.** `keyed` dedup mints from `connector ‖ tool ‖ entity-id ‖ fingerprint`. For a `create` there is no server id yet; increment 3 folds the whole params set into the fingerprint (so entity-id may be empty). Is a client-minted stable correlation id wanted instead, and should it be surfaced back into the cache row?
3. **Sent-intents ledger durability.** Increment 3's ledger is in-memory (per-process retry bookkeeping). Should it be vault-backed for cross-restart retry idempotency, or is remote dedup + deterministic keys sufficient (ADR 0024 P4 says the log is bookkeeping only, which argues in-memory is fine)?

## 6. Files

- Policy surface: `crates/holon-mcp-client/src/mcp_sidecar.rs` (`WritesPolicy`, `ToolEffect`, `key_param`, load-time validation).
- Enforcement: `crates/holon-mcp-client/src/mcp_provider.rs` (`execute_operation`).
- Key minting: `crates/holon-api/src/effect_id.rs` (`HOLON_CONNECTOR_NAMESPACE`, `deterministic_intent_key`).
- Mock scenarios: `crates/holon-mcp-mock/src/lib.rs`, `crates/holon-mcp-mock/src/bin/mock_mcp_server.rs`.
- E2E: `crates/holon-mcp-mock/tests/mcp_mock_e2e.rs`, fixtures under `crates/holon-mcp-mock/tests/fixtures/`.
