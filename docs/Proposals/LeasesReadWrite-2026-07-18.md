# Leases & Read-Write for External Connectors

Status: **LANDED — increments 1–4 complete.** RW-3 policy surface + per-tool effect taxonomy (inc 1), mock write harness (inc 2), keyed idempotency + sent-intents ledger (inc 3), and **writer designation for once_only effects (inc 4)** are all in the tree. Q1 resolved by Martin's 2026-07-19 ruling (below). Q2 and Q3 remain open but are disclosed, non-blocking iteration-1 residuals.

## 0. Writer-designation ruling (Martin, 2026-07-19) — supersedes the Q1 L-* options

Q1 asked how the once_only writer device is chosen (per-device opt-in vs first-provisioner vs vault role). **Ruling:** a **configurable rule system defining which device may do what**, behind a **trait** so additional designations slot in later without touching the dispatch chokepoint. First iteration ships exactly **two policies**: `confirm_manually` (safe default) and `always_run`.

Implemented (increment 4, three sub-increments 4a/4b/4c):

- **Trait** `WriteAuthorizationPolicy` (`crates/holon-mcp-client/src/write_authorization.rs`) — pure: `authorize(&WriteIntent) -> WriteDecision { Allow | RequireConfirmation | Deny }`. Two impls: `AlwaysRun`, `ConfirmManually`. Selected per-connector by the sidecar's `once_only:` field (`OnceOnlyAuthorization` in `mcp_sidecar.rs`), which maps to an impl via `policy_for`. Future designations (vault-block role, TTL lease) are new config variants + new impls; the chokepoint is untouched.
- **Config home = the device-local sidecar YAML**, not `holon.toml` (colocated with the `writes:`/`effect:` fields already there; the integration configs in `{config_dir}/integrations` are per-device by construction, so there is no CRDT race). Absent `once_only:` = `confirm_manually`.
- **At-most-once state machine** `PendingWriteStore` (same module): `AwaitingConfirmation → Confirmed → Dispatching → Sent | OutcomeUnknown`. The dispatch-owning transition is taken **before** the remote call; a post-dispatch failure lands in `OutcomeUnknown` and is **never auto-retried** — it is surfaced for the human to verify on the remote (fail-loud). Compare-and-take `confirm`/`take_for_dispatch`/`begin_dispatch` are single-winner (no double-fire under racing approves). One shared store is DI-injected into every MCP provider so all once_only chokepoints and the frontend approve panel coordinate through the same machine.
- **Confirm-manually mechanics (4c):** on enqueue the store broadcasts a `PendingWriteEvent`; a GPUI bridge (mirroring the degraded bus) surfaces a disclosure toast and re-renders. A **pending-writes approval panel** lists `AwaitingConfirmation` intents (with an **Approve** button) and `OutcomeUnknown` intents (disclosed, no auto-retry). Approve does `confirm` + re-dispatches through the SAME chokepoint via `session.execute_operation` with the stored call and the SAME intent key — dedup-safe. Nothing fires on a timer. Live-verified in the real GPUI app over MCP: once_only write queued → panel + toast shown → Approve → single dispatch succeeded → panel cleared.

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
- **Increment 4 — writer designation (LANDED).** Per Martin's 2026-07-19 ruling (§0), NOT the L-* lease options: a configurable `WriteAuthorizationPolicy` trait with two iteration-1 impls (`confirm_manually` default, `always_run`), a shared `PendingWriteStore` at-most-once state machine, and a GPUI approve panel + disclosure toast + bus bridge.
  - **4a** trait + decision + chokepoint gate (replaces the inc-1 "pending increment 4" hard block); intent-key minting generalized to `{keyed, once_only}` with injection conditional on `key_param`.
  - **4b** `PendingWriteStore` state machine + `approve` re-dispatch; mock-tested (queue / approve-once-effect / double-approve no-op / triggered_by coalesce / post-dispatch-failure → OutcomeUnknown, never auto-retried / no-key_param local at-most-once).
  - **4c** DI shared-store singleton installed on every provider; store broadcast; GPUI `PendingWritesGlobal` + `spawn_pending_writes_bridge` + approval panel with per-entry Approve. Live-verified end-to-end.

## 5. Open questions

1. **(RESOLVED by the 2026-07-19 ruling, §0) Writer designation.** Answered: a configurable `WriteAuthorizationPolicy` trait, iteration-1 = `confirm_manually` (default) + `always_run`, config in the device-local sidecar YAML. Manual override is first-class (the approve panel). TTL leases and vault-role designation are future trait impls, not iteration-1.
2. **(OPEN, disclosed, non-blocking) Entity-id for create-shaped keyed effects.** `keyed` dedup mints from `connector ‖ tool ‖ entity-id ‖ fingerprint`. For a `create` there is no server id yet; increment 3 folds the whole params set into the fingerprint (so entity-id may be empty). Is a client-minted stable correlation id wanted instead, and should it be surfaced back into the cache row?
3. **(OPEN, disclosed, non-blocking) Pending-write / sent-intents ledger durability.** Both the inc-3 sent-intents ledger and the inc-4 `PendingWriteStore` are **in-memory per process**. Iteration-1 accepts this (ADR 0024 P4: the log is bookkeeping, not cross-replica dedup). Two disclosed residuals a human should confirm before relying on it beyond a session:
   - **Restart durability:** pending / `OutcomeUnknown` state is lost on restart. A crash *after* the remote call but *before* `mark_sent`/`mark_outcome_unknown` loses the record; on restart the same intent would `begin_dispatch` fresh and could double-fire. Vault-backing the store closes this; deferred.
   - **Deterministic-key dedup semantics:** the intent key is `UUIDv5(connector ‖ tool ‖ entity-id ‖ FiringKey(params))`, so two once_only writes with *identical params* collapse to one intent (correct for "send once"; a user who deliberately wants to send the same payload twice would be blocked as a duplicate). Confirm this is the desired semantics for once_only.
   - Cross-device once-only is explicitly **not** solved: iteration-1 gives at-most-once **on one device** via the local store; multi-device / partition duplicates are the lease-epoch work (future), honestly disclosed per ADR 0024 P4.

## 6. Files

- Policy surface: `crates/holon-mcp-client/src/mcp_sidecar.rs` (`WritesPolicy`, `ToolEffect`, `key_param`, load-time validation).
- Enforcement: `crates/holon-mcp-client/src/mcp_provider.rs` (`execute_operation`).
- Key minting: `crates/holon-api/src/effect_id.rs` (`HOLON_CONNECTOR_NAMESPACE`, `deterministic_intent_key`).
- Mock scenarios: `crates/holon-mcp-mock/src/lib.rs` (incl. `WriteAcceptedThenError` for the post-dispatch-failure test), `crates/holon-mcp-mock/src/bin/mock_mcp_server.rs`.
- E2E: `crates/holon-mcp-mock/tests/mcp_mock_e2e.rs`, fixtures under `crates/holon-mcp-mock/tests/fixtures/` (`write_once_only.yaml`, `write_once_only_always_run.yaml`, `write_once_only_keyed_confirm.yaml`).
- Writer designation (inc 4): `crates/holon-mcp-client/src/write_authorization.rs` (trait, decision, policies, `PendingWriteStore` + broadcast). Chokepoint + `set_pending_store`/`approve`/`pending_writes`: `crates/holon-mcp-client/src/mcp_provider.rs`. Sidecar `once_only:` field: `mcp_sidecar.rs` + `integration_config.rs`.
- DI shared store: `crates/holon-app/src/mcp_integrations.rs` (singleton + install on every provider), re-exported from `crates/holon-app/src/lib.rs`.
- Frontend (inc 4c): `frontends/gpui/src/share_ui.rs` (`PendingWritesGlobal`, `spawn_pending_writes_bridge`, `render_pending_writes_panel`, `dispatch_approve`, toast kinds), `frontends/gpui/src/lib.rs` (bridge install + render wiring), `frontends/gpui/src/main.rs` (resolve store + install global).
