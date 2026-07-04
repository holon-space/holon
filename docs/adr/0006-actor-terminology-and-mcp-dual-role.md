# ADR 0006: Actor terminology and MCP dual role

**Status:** Accepted (2026-05-27; shipped. Note: sync adapters were materialized as manifest variants + generic YAML sidecars, not per-integration structs.)
**Deciders:** Martin
**Context:** Naming + role assignment for the third tier of ADR 0004

## Problem

Calling the third tier "Consumers" is too narrow — UI, MCP server, and the autonomous Action engine all both **read from** and **mutate** the domain. They are not passive consumers. We need a term that:

- Includes both reading and mutating.
- Includes autonomous components (Action engine), not just human-facing ones.
- Reads naturally next to "Action engine".
- Distinguishes from the SerDe sense of "Adapter" (Loro/Org/Markdown/Turso).

Separately, **MCP appears in two architectural roles** that must not be conflated:

1. Holon **runs an MCP server** that AI agents connect to in order to drive the Holon domain.
2. Holon **acts as an MCP client** to talk to external systems (GCal, GMail, etc.) so it can sync its domain with those systems' state.

## Decision

### Terminology: Actors

The third tier is called **Actors**. An actor is a component that interacts with the domain — reading from it, mutating it, or both. Synonyms considered and rejected:

| Candidate | Rejection reason |
|---|---|
| Consumers | Reading-only connotation; misses mutation |
| Agents | "Agent" is overloaded with AI/LLM semantics |
| Frontends | Too UI-specific; doesn't fit headless Action engine |
| Drivers | Already taken by `UserDriver` and would conflict |
| Clients | Too generic; doesn't carry the read+write meaning |
| Participants | Vague |
| Primary adapters (hexagonal) | Technically precise but heavy and unfamiliar in this codebase |

"Actor" is domain-neutral, captures both directions, and "Action engine" reads naturally as *"an actor that performs actions autonomously."*

### MCP dual role

| Role | Tier | Why |
|---|---|---|
| MCP server (Holon serves AI agents) | Actor | Receives input from an AI, drives the domain — same shape as a UI driving the domain on behalf of a human |
| MCP client (Holon connects to GCal/GMail/…) | Tier 2b sync adapter | Two-way sync of the Holon domain with an external system; the *external system* is the "storage" being synced with |

Concretely:

- `MCPServerActor` lives in Tier 3, owns subscription/emission state.
- `GCalSyncAdapter`, `GMailSyncAdapter` (each using MCP-client transport) live in Tier 2b, own per-integration cursor + remote-id-to-domain-id mappings.

This means swapping `{MCP server}` in or out of the wiring manifest controls "is there an AI agent connection point?", while swapping `{GCalSyncAdapter}` controls "are we syncing with Google Calendar?" — two independent decisions.

### Naming of actor-state types

- `UIActorState`
- `MCPServerActorState`
- `ActionActorState`

Plain `Actor` is the umbrella term; concrete types carry the qualifier. The trailing `State` mirrors the existing `ReferenceState` / `*BackendState` convention.

## Consequences

- Documentation, PBT module names, and DI module names use "actor" consistently.
- The wiring manifest (ADR 0007) distinguishes `storage_adapters`, `sync_adapters`, and `actors`.
- Todoist-like integrations already in the codebase (Todoist itself) move under Tier 2b sync adapters in Phase 11.
- The third-party MCP feature roadmap (GCal, GMail) drops in as Tier 2b sync adapters, not as Tier 3 actors.

## Known weaknesses / open questions

1. **Actor mutations vs sync-adapter mutations — only one path.** An MCP-server tool call "create calendar event" must NOT bypass the GCal sync adapter. The architectural rule: **actors mutate the domain only**; sync adapters observe the domain (or external system) and produce mutations on the *other* side. Two paths to the same outcome is a bug. Phase 11 must verify this discipline.
2. **Action engine parsing DSL crosses tiers.** ✅ **Resolved (Phase 6): DSL parser is a Tier-1 domain helper.** The render/action DSL parser is a pure function over content with no actor state, so it lives in the domain tier (`holon-api`): `render_dsl` (render expression parsing) and `action_dsl` (action-block parsing) were relocated from the `holon` orchestration crate into `holon-api`. The Action-engine actor (`action_watcher`) and the UI both call the same `holon_api::render_dsl` / `holon_api::action_dsl` helpers rather than each owning a private parser.
3. **`UIActorState` mixes per-tab and per-user state.** ✅ **Resolved (Phase 6): split into `UITabState` + `UIUserState`.** `UIActorState` now composes two sub-fragments: `UITabState` (per-tab, ephemeral — focus, cursor, navigation history, expand/drawer widget state, active-editor mirror) and `UIUserState` (per-user — open pins, active view profile). The per-user fragment is the seam for a future cross-device sync adapter, avoiding the re-split this weakness warned about.

## References

- ADR 0004 — defines the three tiers.
- ADR 0007 — wiring manifest uses these tier names.
