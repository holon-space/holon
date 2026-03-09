# ADR 0004: Domain / Adapter / Actor split

**Status:** Proposed (2026-05-27)
**Deciders:** Martin
**Context:** Componentization for Turso-optional builds and PBT subset wiring

## Problem

The current architecture has no explicit, committed boundary between:

- The **block knowledge graph** itself (what blocks exist, how they're shaped, how they're related).
- The **SerDe mechanisms** that store the block graph (Loro CRDT, Org files, Markdown files, Turso matviews).
- The **observers and drivers** that read/mutate the graph (UI, MCP server, autonomous action engine, MCP-client integrations like GCal).
<!-- MCP-client integrations don't modifiy the blocks domain, they have their own domain -->
Today's symptoms:

- `holon-integration-tests::pbt::reference_state::ReferenceState` is a god-struct mixing block data with focus/nav/expanded-toggles (UI), peer state (Loro), file content (Org), CDC watermarks (Turso), MCP emissions, and harness flags.
- Turso is treated as the substrate (global schemas, hardcoded matview names in non-Turso modules), making "Turso-less Holon" architecturally impossible to express.
- The "Loro adapter" actually means "Loro CRDT + the bridge that writes into Turso matviews" — two concerns fused.
- PBT subset selection (e.g., the `_sql_only` slice) is implemented via `enable_*` booleans scattered through `TestContext::start_app`, not via a typed manifest.

## Decision

Holon is structured as three tiers plus a wiring manifest:

### Tier 1 — Domain (block knowledge graph)

Storage-agnostic, viewer-agnostic. The **logical canonical projection** of the knowledge graph — not a materialized struct. The bytes live in adapters; the domain is the agreement they converge to.

"Canonical" means: *for every wired adapter pair, after quiescence (defined below), their projections to the domain are equal modulo each adapter's declared loss relation.* When adapters disagree before quiescence, no in-memory struct "holds" the truth — disagreement is a transient state the PBT must observe and let settle.

- Lives in: `crates/holon-api::block::Block` (already exists; treat as canonical), plus `crates/holon/src/api/pbt_infrastructure` (`MemoryBackend`, `BlockTransition`) as the reference implementation.
- Contains: block identity / parent / content / content_type / source_language / task_state / tags / properties / marks / render config / default-collapsed.
- Excludes: `sort_key` (see ADR 0005), `focus`, `navigation_history`, peer state, file content, CDC watermarks.
- Domain transitions are the CRUD vocabulary of the graph; they are routed *through* adapters and actors but the domain-level effect is the same regardless of route.
- Domain invariants (always checked): no parent cycles, all references resolve, children-of-parent forms a valid ordered list, `source_language` iff `content_type == Source`, etc.

### Tier 2a — Storage adapters

Each adapter implements `domain ↔ adapter_state` with an explicit equivalence relation (often lossy).

| Adapter | Owns | Lossy aspects |
|---|---|---|
| Loro | peers, frontiers, op log, shared-subtree mounts | merge non-determinism under concurrent edits |
| Org | file content snapshot per doc, mtimes | trailing whitespace, property ordering |
| Markdown | file content snapshot per doc | no `:ID:`, no `#+TODO:`, fewer property semantics |
| Turso | registered schemas, CDC watermark, matview rows, scheduler available-set | CDC settling time |
<!--
Let's discuss the watermark and if it is really necessary again.
I know many systems use one (e.g. Spark Streaming).
When is a watermark strictly necessary, when can which alternatives be used?
-->
Per-adapter round-trip invariant: when wired, `read_via_adapter(domain) ≅ adapter_state` modulo that adapter's known-loss relation.

Cross-adapter convergence invariant: when ≥2 adapters wired, after quiescence all wired adapters project to the same domain.

**Loro's non-determinism under concurrent edits is *not* loss** in the round-trip sense — it loses user intent disambiguation, not bytes. Listed in the "Lossy aspects" column for compactness; tracked separately from byte-level loss in the invariant body.

#### Quiescence (operational definition)

A wired system is *quiescent* iff **all** of:
- No in-flight Turso CDC batches (`cdc_emitted_watermark` stable across two reads ≥ ε ms apart).
- No pending file-watcher events for any wired Org/Markdown adapter (watcher queue empty AND last debounce window elapsed).
- No unflushed Loro ops (`primary_oplog_frontiers == last_synced_frontiers`).
- No scheduled actor work (action engine watcher queue drained, MCP server emission queue drained).

Any invariant that depends on cross-adapter agreement MUST first wait for quiescence with a typed budget (see `transition_budgets.rs`). Invariants that fire pre-quiescence are bugs in the invariant, not the system.
<!--
An invariant might fire and succeed even pre-quiescence as it only verifies
a part of the system which might already have converged to the correct state before all quiescence.
-->
### Tier 2b — Sync adapters

Two-way sync between the Holon domain and an external system. Structurally the same shape as storage adapters; distinguished because the "storage" is remote and externally driven.

Examples: GCal-via-MCP-client, GMail-via-MCP-client, Todoist. See ADR 0006 for the MCP dual-role distinction.

### Tier 3 — Actors

State that exists only because something is observing or interacting. No actor wired → no actor state.

| Actor | Owns |
|---|---|
| UI | focus (per region), nav history + cursor, collapse overrides, pinned blocks, active view profile, editor cursors, slash-popup state, layout / regions / viewport |
| MCP server | active subscriptions, last-emitted per subscription, pending response cache |
| Action engine | registered watchers, watcher cursors |

Per-actor invariants (when wired): displayed text matches domain content modulo display normalization (UI); emitted deltas correspond to actual domain changes (MCP); etc.

See ADR 0006 for the "Actor" naming choice and why MCP-server-vs-MCP-client splits between Tier 3 and Tier 2b.

### Wiring manifest

A subset is declared as `Wiring { storage_adapters, sync_adapters, actors }`. The runtime composes only the named fragments; PBT alphabet and invariant set are derived mechanically from the manifest. See ADR 0007.

### Cross-adapter bridges

A component that **reads one adapter and writes to another** (e.g., the Loro→Turso write-through) is neither a storage adapter nor an actor. Call it a **bridge**. Bridges:

- Are wired only when *both* underlying adapters are wired.
- Are pure functions of one adapter's state into another adapter's mutations — they own no state of their own beyond a cursor.
- Have their own round-trip invariant: `read_via_target(domain) ≅ read_via_source(domain)` post-quiescence.

The Loro→Turso write-through, the Org→Block parse, and the Markdown→Block parse are bridges. (`crates/holon-markdown/` already implements the Markdown side — `parse_markdown_file` parser, frontmatter, renderer, wikilink — so it is no longer future work.)
<!--
We're just trying to get rid of direct coupling between Loro and Turso.
We previously had an event bus to do this but the ergonomics were suboptimal.
We are now trying to do this is to have Loro expose a `LiveData<Block>` that Turso
subscribes to and through which it can get the latest data stream-like.
-->

## Resolved OPEN items

| Concept | Tier | Rationale |
|---|---|---|
| `Document.filename` | **OPEN — see below** | Title-derived has collision / FS-portability / external-rename problems unresolved; needs its own ADR before commit |
| Pin-set per region | UI actor | Per-viewer bookkeeping, like focus |
| Initial collapse state | Domain (`Block.default_collapsed`) | Persistent, part of the block |
| Per-viewer collapse override | UI actor (`UIActorState.collapse_overrides`) | Per-viewer flip on top of the default |
| Tags | Domain | Knowledge metadata; Turso storage detail does not leak |
| Render config on blocks (author intent: table layout, default chart type) | Domain | Authored once, shared across viewers |
| Render config (viewer preference: this column hidden on my screen) | UI actor | Per-viewer flip on top of author intent — same split as default_collapsed vs collapse_overrides |
| `current_focus` / `focus_roots` matview | Turso adapter cache of UI actor concept | Only present when both `{Turso, UI}` wired |
| `block_with_path`, `block_tags`, `block` matview | Turso adapter (cache of pure domain) | Present whenever `{Turso}` is wired |
| Action engine | Actor | Reads + mutates, like UI and MCP server |
| Sort key | Adapter detail | See ADR 0005 |

## Consequences

- `holon-integration-tests::pbt::reference_state::ReferenceState` is factored across Phases 2–6 of the migration into domain + per-actor + per-adapter fragments. Each phase deletes the corresponding fields first and lets the compiler drive the rewrite.
- `holon_api::block::Block` retires `sort_key` (ADR 0005).
- Turso becomes one adapter among four. Schema registration becomes per-adapter (Phase 9).
- The Loro→Turso write-through bridge becomes its own component, wired only when both adapters are present (Phase 10).

## Known weaknesses / open questions

1. **`Document.filename` derivation.** Title-derived filenames must answer: collisions across docs, illegal FS chars (`/`, `\0`, `:`, leading `.`, length caps), macOS case-insensitivity vs Linux case-sensitivity, external renames (`git mv`). **Required follow-up ADR before Phase 6.**
2. **Serialization compatibility.** On-disk Loro blocks today carry `sort_key`. Removal (ADR 0005) requires either a one-shot migration on load or a backward-compat reader that drops the field. **Required: migration strategy doc before Phase 8 lands.**
3. **Indirection cost on hot paths.** Typing/scrolling cannot afford per-keystroke `Vec<EntityUri>` allocation through the adapter layer. Decision needed: do hot paths get a typed fast-path bypass, or is the adapter layer required to expose an `Iterator`-shaped API?
4. **Phases 2–11 ordering.** The migration phases are referenced by number throughout these ADRs but no dependency DAG is committed. Suspected partial order: 8 (sort_key) and 5 (filename) can precede 2–6 (state split) to reduce churn. **Required: phase DAG in the plan file.**
5. **Test green-line commitment.** The plan must commit to "e2e suite green at every phase boundary" (no multi-week red windows), or explicitly accept a red window with a planned re-green date.

## References

- ADR 0003 (LoroTree architecture) — Loro adapter shape comes from this.
- ADR 0005 (children-as-ordered-list) — domain order representation.
- ADR 0006 (actor terminology + MCP dual role).
- ADR 0007 (wiring manifest).
