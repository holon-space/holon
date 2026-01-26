# ADR 0007: Wiring manifest for PBT subsets

**Status:** Proposed (2026-05-27)
**Deciders:** Martin
**Context:** PBT subset selection + production DI subset declaration

## Problem

Today's PBT slice declarations (`general_e2e_pbt_full`, `general_e2e_pbt_sql_only`, `org_create_ordering_pbt_full`, `loro_backend_pbt`) use ad-hoc constructs: a `VariantMarker` type, a few `enable_loro` / `enable_todoist` booleans on `TestContext`, and capability traits as the implicit gating mechanism. Symptoms:

- New subsets ("Loro + MCP only", "Turso-less Holon", "GCal-sync-only smoke test") cannot be added without rewriting the slice machinery.
- The transition alphabet for each subset is implicit — derived from generator `weighted_generator` returning `Fail(...)` reasons.
- Invariant gating is by trait bounds on the SUT type, which scales poorly past a handful of capabilities.
- Production DI has the same problem inverted: "can I build Holon without Turso?" has no architectural answer because there's no declarative wiring artefact.

## Decision

Introduce a single typed **Wiring manifest** that:

- Declares which storage adapters, sync adapters, and actors are wired.
- Is the input to both production DI (which fragments to construct) and PBT framework (which alphabet to generate from + which invariants to run).
- Lives in `crates/holon-pbt-core` (or a sibling crate shared with production DI).

### Type sketch

```rust
pub struct Wiring {
    pub storage_adapters: BTreeSet<StorageAdapter>,
    pub sync_adapters:    BTreeSet<SyncAdapter>,
    pub actors:           BTreeSet<Actor>,
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum StorageAdapter { Loro, Org, Markdown, Turso }

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum SyncAdapter { Todoist, GCal, GMail /* extensible */ }

#[derive(PartialEq, Eq, PartialOrd, Ord)]
pub enum Actor { UI, MCPServer, ActionEngine }
```

### Derived artefacts

From a `Wiring`, the framework mechanically derives:

1. **Reference fragments to construct.** `WiredReferenceState` builds a `ReferenceState` with each adapter/actor fragment as `Some(_)` if and only if it appears in the manifest.
2. **Active transition alphabet.** Every transition declares its dependencies as a `RequiredWiring`. The generator only includes transitions whose requirements are satisfied by the active manifest. `RequiredWiring` is *necessary, not sufficient* — generators retain dynamic preconditions (e.g., "a block exists to edit"); the manifest gates structurally, the generator gates dynamically.
3. **Active invariant set.** Every invariant declares its `RequiredWiring`. The registry runner skips invariants whose requirements aren't met (with the gate visible, not silent).

#### `RequiredWiring` expressiveness

A `RequiredWiring` is a boolean expression over tier-presence atoms, not just a subset:

```rust
pub enum RequiredWiring {
    Any,                                        // unconditional
    HasStorage(StorageAdapter),
    HasSync(SyncAdapter),
    HasActor(Actor),
    AnyStorageOf(BTreeSet<StorageAdapter>),     // disjunction
    All(Vec<RequiredWiring>),
    AnyOf(Vec<RequiredWiring>),
}
```

Disjunction (`AnyOf`, `AnyStorageOf`) is required: "edit content" needs *some* mutable storage adapter, not a specific one. Flat subsets force per-adapter transition copies, defeating the point of the manifest.

### PBT slice declaration becomes

```rust
pbt_slice! {
    name: loro_mcp_pbt,
    wiring: Wiring {
        storage_adapters: { Loro },
        sync_adapters:    {},
        actors:           { MCPServer },
    },
}
```

Today's `general_e2e_pbt_full` becomes a `Wiring` with every adapter and actor present. Today's `_sql_only` becomes `{ Turso, Org } + { UI }` (or similar — exact set decided in Phase 7).

### Production DI alignment

Production binaries also accept a `Wiring` at startup. "Turso-less Holon" is a binary built with `Wiring { storage_adapters: { Loro }, actors: { UI }, ... }`. The same composition logic runs in both contexts.

### Validity of a manifest

Some wirings are invalid (e.g., `actors: { UI }` with no storage adapter — there's nothing to display). A `Wiring::validate() -> Result<(), WiringError>` enforces the dependency graph between components. The set of validity rules is the architectural commitment: e.g., *"every wiring must contain at least one storage adapter"*, *"MCPServer requires at least one storage adapter"*, *"ActionEngine requires a query adapter"*.

## Consequences

- `VariantMarker` and the `enable_*` boolean soup are deleted in Phase 7 of the migration.
- New subsets are 4-line additions, not framework rewrites.
- Production gets a path to feature-reduced builds (Turso-less, etc.) using the same primitive.
- The wiring validity rules become a load-bearing architectural document — they define what combinations Holon can actually run as.

### Blessed vs valid manifests

A *valid* manifest passes `validate()`. A *blessed* manifest is one CI runs PBT against on every change. Today's commitments:

| Manifest | Status | Purpose |
|---|---|---|
| `{Loro, Org, Markdown, Turso} + {Todoist} + {UI, MCPServer, ActionEngine}` | Blessed | Replaces `general_e2e_pbt_full` |
| `{Turso, Org} + {} + {UI}` | Blessed | Replaces `general_e2e_pbt_sql_only` |
| `{Loro} + {} + {}` | Blessed | Replaces `loro_backend_pbt` |
| `{Org} + {} + {}` | Blessed | Replaces `org_create_ordering_pbt_full` |
| Other valid manifests | Unblessed | Smoke-tested only when added |

Adding a new blessed manifest is a deliberate decision, not an automatic consequence of validity.

### Storage vs sync — sharpened distinction

Both categories interact with state outside the in-process domain. The categorical line is **event-loss tolerance**:

- **Storage adapter:** authoritative state is local (file, embedded DB, in-process CRDT). Event stream MUST NOT be lossy — losing an event means losing user data. Watcher gaps are bugs.
- **Sync adapter:** authoritative state is remote, the protocol allows event loss (webhook misses, rate limits, polling gaps), and recovery is via re-fetch / reconcile.

By this rule Org and Markdown are storage (filesystem events SHOULD be reliable; gaps are debug-and-fix); Todoist/GCal/GMail are sync (gaps are routine and the adapter must reconcile).

## Migration

This ADR is realized by **Phase 7** of the componentization migration (after each tier's fragments are individually separable in Phases 2–6).

## Known weaknesses / open questions

1. **Closed enums lock out plugins.** `StorageAdapter`/`SyncAdapter`/`Actor` as flat enums means every new adapter is a core-enum bump. Acceptable if "all adapters live in-tree" is an explicit non-goal-of-extensibility commitment; otherwise needs a `&'static str` tag or trait-object identity.
2. **Validity grammar is unenumerated.** The ADR commits to validation rules without writing them. **Required follow-up:** a complete table of validity rules + a PBT generator over `Wiring` that exercises `validate()` (positive: blessed manifests valid; negative: rule-violating manifests rejected). Without this, the manifest concept is a vibe, not a contract.
3. **Authoritative-for-ordering is not in the type.** ADR 0005 says the manifest declares which adapter is authoritative for ordering when multiple are wired. The current `Wiring` sketch has no field for this. Either add `pub ordering_authority: StorageAdapter`, or derive it deterministically from a fixed priority (Loro > Org > Markdown > Turso) and document the rule.
4. **Combinatorial test surface.** 4×3×3 storage/sync/actor with subset semantics yields a large valid-manifest space. CI cost is bounded by the blessed list; an unenumerated tail of "valid but never tested" manifests is a footgun. Mitigation: add `#[must_bless]` annotation on `Wiring` literals constructed outside test helpers — refuse to compile binaries that construct an unblessed wiring without an explicit override.

## References

- ADR 0004 — defines the tiers the manifest names.
- ADR 0006 — defines the actor naming used in the enum.
- ADR 0005 — children-as-ordered-list applies inside the domain regardless of manifest.
