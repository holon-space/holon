# Settings as layout data: a `state_toggle` over a non-block entity

**Status:** proposal, awaiting Martin's ratification.
**Date:** 2026-08-18. **Base:** `main` @ 66c90c4b.
**Ruling it implements:** D5.b (Martin, 2026-08-18) — the hardcoded GPUI
Settings→Integrations section is interim; build a `state_toggle`-style
LAYOUT-DATA widget plus a store-writing operation so settings become
user-arrangeable layout like everything else.
**Relates to:** [Model.md](../Architecture/Model.md) (five layers, invariants
1–12), [UI.md](../Architecture/UI.md) (Cell vs Mutable),
[Operations.md](../Architecture/Operations.md) (cells vs reflective ops),
[ADR 0024](../Adr/0024-unified-action-execution.md) (PN is the sole action
language), [ADR 0030](../Adr/0030-birth-atomicity-authority-and-mirror-contract.md)
(authority-only births).

---

## 0. The headline

The stated constraint that forced the interim hardcoding was:

> today no widget binds to a NON-BLOCK signal (`IntegrationConfigStore`'s
> futures-signals cells) and no PN operation writes the store.

**The first half of that premise is refuted by the code.** The render pipeline
is already entity-generic end to end: the widget→operation binding derives the
entity from the row's `id` URI scheme, not from a block assumption. The second
half is true and is the real work.

Concretely, three seams that everyone assumed were block-shaped are not:

| Seam | Code | What it actually does |
|---|---|---|
| Entity resolution | `ReactiveViewModel::entity_name`, `crates/holon-frontend/src/reactive_view_model.rs:820-831` | Splits the row's `id` on `:` and returns `EntityName::Named(scheme)`. A row with `id = "integration:gmail"` yields entity `integration`. No block special-case. |
| Toggle → intent | `state_toggle_intent`, `crates/holon-frontend/src/operations.rs:67-86` | Takes `entity_name` and `row_id` as parameters and emits `OperationIntent::set_field(entity, op, row_id, field, value)`. Never mentions blocks. |
| Op discovery | `create_profile_resolver`, `crates/holon/src/di/registration.rs:412-419` | `for op in dispatcher.operations()` grouped by `entity_name`. Any registered provider's ops reach `resolve_profile(row).operations` automatically. |

So the widget already exists, generically. `ViewKind::StateToggle { field,
current, label, states }` (`crates/holon-frontend/src/view_model.rs:305-312`) is
the layout-data toggle the ruling names, and the GPUI builder
(`frontends/gpui/src/render/builders/state_toggle.rs`) dispatches its click
through `services.dispatch_intent` with no block dependency.

**What is missing is not a widget. It is (a) an `integration` entity that the
query pipeline can see, (b) an `OperationProvider` registered for it, and (c) a
two-state appearance for `state_toggle` (today its glyph vocabulary is
hard-wired to task states).**

That reframing is what makes this cheap. The recommendation below is roughly
**one new provider + one projector + one table + one widget prop + a seed-layout
edit**, and it deletes 350 lines of hardcoded GPUI.

**And most of that cost is already owed.** The left-sidebar Integrations
discovery section is independently broken (§2.6, an OPEN bugfunnel entry
verified against the code and a cold-boot log), and its recorded remedy is the
*same* projection table this design needs. So the settings surface is close to
free: one table, one entity, one writer, serving **one section** that is both
the discovery list and the settings list (§4.1a).

---

## 1. First principles

Before any design: what is the system actually optimizing for here, and what
does "machine state as layout data" mean inside Holon's model?

### 1.1 The goal

Not "make settings prettier". The goal is **one rendering pipeline, one action
language, one state model** — so that:

- a user can rearrange, delete, or re-query the settings surface exactly as they
  can any other panel (the ruling's stated aim);
- an agent, an MCP client, a test driver, and a human all reach integration
  enablement through the *same* operation, rather than the GPUI mouse handler
  being the only door;
- the headless keystone PBT can see a surface it is structurally blind to today.

### 1.2 The constraints that are real

**Layer discipline (Model.md §Five layers).** Layer 3 (Turso) is "exactly one
writer per mode; verbatim and total; **ephemeral by contract**". Layer 4 is the
reactive pipeline; Layer 5 (UI) "displays fields and captures intent; **owns no
entity values**". Today the Integrations section violates Layer 5: the GPUI
switch owns the write (`vm.set_enabled(provider, !enabled)`,
`frontends/gpui/src/integrations_ui.rs:326`) and the store owns the value
outside the reactive pipeline entirely, with its own hand-rolled
signal→`window.refresh()` pump (`spawn_integrations_bridge`, same file, :82-129).

**The `Cell<T>` bar (UI.md §Three kinds of reactive state).** The documented
test is: *"has identity (uri+field), could be queried/persisted/synced"* → it is
entity field state, not widget state. Integration enablement has a stable
identity (`integration:gmail` + field `enabled`), is persisted (a
`.state.toml` file), and every consumer must agree on it. **It is entity field
state by the project's own definition.** The current `Mutable<IntegrationState>`
in `IntegrationConfigStore`
(`crates/holon-mcp-client/src/integration_state.rs:160-163`) is a private
signal cell standing in for a `Cell` — the FU-1 mistake in the other direction.

**ADR 0024.** PN/operations are the sole action language for user intent. A
mouse handler calling a store method directly is outside it. Note the routing
table in Operations.md already lists non-block entities —
`"orgmode.sync" → OrgModeSyncProvider`, `"todoist-tasks" → McpOperationProvider`
— so a non-block provider is precedent, not novelty.

**ADR 0030 D1/D2.** A birth fires atomically in **exactly one authority store**,
and mirroring is orthogonal. This is the constraint that decides where
enablement *lives*: whatever we do, there must remain exactly one authority, and
everything else must be a re-derivable mirror.

### 1.3 The precedent that settles the shape: navigation

**Navigation is already exactly this problem, already solved, already in the
render DSL.** Navigation state is machine state, not user content. It:

- has its own non-block `OperationProvider`
  (`NavigationProvider`, `crates/holon/src/navigation/provider.rs:271-278`) with
  a declared descriptor set
  (`navigation_operation_descriptors`, same file, :43-248);
- persists into its own **native Turso tables** (`navigation_history`,
  `navigation_cursor`) rather than into blocks;
- is projected through matviews (`focus_roots`) into the reactive pipeline;
- is **read back by the render DSL as ordinary layout data** — see the seeded
  right-sidebar backlinks query in `assets/default/index.org:23`, which joins
  `focus_roots` and `navigation_cursor` inside a `live_query`;
- is **written by a DSL-level action** — `navigation_focus(#{region: "main",
  block_id: col("id")})` inside a `selectable(...)` in
  `assets/default/index.org:12`.

Machine state as layout data is therefore not a new capability to invent. It is
a pattern already load-bearing in the default layout. Integrations should join
it.

---

## 2. Premise check — what exists today

Everything below was read at `main` @ 66c90c4b.

### 2.1 The hardcoded section (what we are deleting)

`frontends/gpui/src/integrations_ui.rs`, 350 lines:

- `IntegrationsSettingsGlobal` (:37) — a GPUI global holding
  `Arc<IntegrationsSettingsVm>`, installed from DI in `main.rs`.
- `spawn_integrations_bridge` (:82-129) — a bespoke futures-signals →
  `window.refresh()` pump, parallel to the reactive pipeline.
- `render_section` / `render_row` / `render_switch` (:137, :238, :288) — hand-built
  GPUI elements with hand-rolled track/knob geometry, explicitly duplicating the
  preferences toggle ("Same track/knob geometry as the preferences toggle
  (`pref_field.rs`), so the two switch kinds in one Settings modal read as one
  control", :295-296 — a comment that is itself the smell). The file it names is
  `frontends/gpui/src/render/builders/pref_field.rs`.
- The click (:326) calls `vm.set_enabled(provider, !enabled)` directly and
  surfaces failure as a `DegradedToast`.
- Three `TransparentTracker` ids (`integration_toggle_id`,
  `integration_row_id`, `integration_status_id`, :42-56) exist purely so a
  windowed test can find controls that are invisible to every other tier.

The call site, `frontends/gpui/src/lib.rs:1024-1042`, states the constraint in
its own words:

> Integrations live in the SAME modal as the preferences, but not in the same
> render pipeline: their switch writes to `IntegrationConfigStore`, not to the
> preference file, and `pref_field` has no seam for a second destination.

### 2.2 The half that is already layout data

The preferences half of the same modal is **already** render-DSL data:
`self.session.preferences_render_data()` (`frontends/gpui/src/lib.rs:1011`,
implementation `crates/holon-frontend/src/lib.rs:705-714`) returns a
`(RenderExpr, rows)` pair that goes through `interpret_and_render`. It builds
`pref_field(...)` calls grouped into `section(...)`s
(`crates/holon-frontend/src/preferences.rs:296-352`).

**And it already renders a section literally titled "Integrations"** —
`PrefSection::new("Integrations")` at :122, carrying at least one real
preference (a Todoist API-key path, :160-163), pinned by a test at :451.

So the Settings modal currently shows **two different sections both called
"Integrations"**: one that is layout data driven by `pref_field`, and one that
is 350 lines of hardcoded GPUI. That is the clearest possible statement of the
problem, and it means this work also removes a live user-facing confusion, not
only an architectural one.

The modal is therefore half-migrated already. This proposal finishes the
migration, and does it by moving integrations onto the *general* entity pipeline
rather than by bolting a second destination onto `pref_field`.

### 2.3 The store (the current authority)

`crates/holon-mcp-client/src/integration_state.rs`:

- On-disk authority is `<integrations_dir>/<provider>.state.toml`, a strict
  `StateFile { schema_version, enabled, configuration }` with
  `deny_unknown_fields` (:79-83) — deliberately fail-loud, so a truncated file is
  a parse error, never a silent "off".
- `IntegrationState { enabled: bool, configuration: Configuration }` (:60-63) —
  two orthogonal axes. `Configuration` is `Unconfigured | Configured { … }`
  (:49-53) and records credential *locations*, never secrets (:31-35).
- `IntegrationConfigStore { dir, states: HashMap<&'static str,
  Mutable<IntegrationState>> }` (:160-163) — one futures-signals cell per
  bundled provider.
- Writes go through `write_atomically` (:138-156) — temp sibling + rename.

`crates/holon-app/src/integrations_settings.rs` is the thin VM over it:
`IntegrationRow { provider, enabled, status }` (:46-53), `rows()` (:86-101),
`set_enabled`, and `ConfigStatus::{Unconfigured, Configured}` (:22-25) — the
display half of `Configuration` with credential locations dropped.

### 2.4 The generic render/operation machinery

- Widget kind: `ViewKind::StateToggle { field, current, label, states }`
  (`crates/holon-frontend/src/view_model.rs:305-312`); name registered at :522,
  :682, :873; live construction at
  `crates/holon-frontend/src/reactive_view_model.rs:1122`.
- GPUI builder: `frontends/gpui/src/render/builders/state_toggle.rs` — reads
  props `field`/`current`/`states`, calls `state_toggle_intent`, and on failure
  **discloses** a display-only glyph with a `tracing::warn!` rather than a dead
  control (:48-68). Good precedent; keep it.
- Intent: `state_toggle_intent`
  (`crates/holon-frontend/src/operations.rs:67-86`) →
  `find_set_field_op(field, ops)` (:296-310) → `OperationIntent::set_field`.
- Ops on a node come from the entity profile:
  `shared_render_entity_build` (`crates/holon-frontend/src/render_interpreter.rs:772-802`)
  → `resolve_profile(row)` → `ProfileResolver::materialize`
  (`crates/holon-profiles/src/lib.rs:1036-1050`), which looks operations up **by
  the row id's URI scheme** (:1041-1043, and the comment at :1033-1035 says so
  explicitly).
- `EntityUri` is scheme-generic (`crates/holon-api/src/entity_uri.rs:38-61`);
  `integration:gmail` is a legal URI today.

### 2.5 The seeded section that already exists

`crates/holon-app/tests/integrations_section_seed.rs` pins a **left-sidebar
Integrations discovery section** as ordinary, deletable, seeded layout data
(`assets/default/index.org:12`): `divider()`, an "Integrations" header, and a
`live_query` over `sync_states`. Its third test —
`deleted_integrations_section_does_not_resurrect_on_reseed` — pins the
"user deletion sticks" property.

**That section is read-only today** (it lists providers that have *synced*). The
design below turns it into the read-*write* settings surface, which is why this
work absorbs rather than duplicates it.

### 2.6 The discovery section is BROKEN — and its missing piece is this design's table

Independently of D5.b, that seeded section is defective. Recorded and verified:
`docs/Testing/bugfunnel/entries/2026-08-18-integrations-discovery-section-lists-only-orgmode.md`
(gap ENVIRONMENT, secondary ORACLE, **status OPEN**, blocked on a **pending D7
ruling**).

The section lists only `orgmode` even with gcal, gmail, todoist and
claude-history enabled and syncing, because `sync_states` is the sync-**cursor**
table, not a registry of integrations:

- `sync_states` is `(provider_name PRIMARY KEY, sync_token, updated_at,
  _change_origin)` — `crates/holon-turso/sql/schema/sync_states.sql`;
- it is written only by `SyncTokenStore::save_token`, on the **incremental**
  branch of `McpSyncEngine::sync_entity_inner`
  (`crates/holon-mcp-client/src/mcp_sync_engine.rs:399-412`). Cursorless
  strategies take the full-sync arm and never write a row — claude-history is
  `list_resource`-only, gcal and gmail declare no `cursor:`;
- worse, the key is `format!("{provider}.{entity}")`
  (`mcp_sync_engine.rs:372`), so todoist would surface as
  `todoist.todoist_tasks` / `todoist.todoist_projects`, never as one row;
- `orgmode` is the sole provider that writes a bare provider name
  (`crates/holon-orgmode/src/orgmode_sync_provider.rs:355,445`).

**The entry's own "Missing piece" section states the fix in the same terms this
proposal does:**

> There is **no queryable projection of integration enablement or connection
> state**. `IntegrationConfigStore` holds `Mutable<IntegrationState>` cells
> backed by filesystem state files … and is never mirrored into a Turso table,
> so no `live_query` — the only mechanism the seeded layout has — can read it.
> `sync_states` was picked as the nearest available table, and it is the wrong
> one.

and its prod/test-parity remedy:

> project integration enablement + connection status into a queryable table,
> then assert in the keystone that the discovery section's rows equal the
> enabled set.

That is Increment 1 plus Increment 4 of §5, arrived at by an independent route
from a live dogfooding defect. **This is the single most important input to the
fork below: the projection table is required whether or not D5.b happens.**

---

## 3. THE FORK

Three candidate homes for integration state, analysed honestly.

### Option A — a non-block entity: `integration:` rows in a native Turso table

Give integrations an entity scheme. A projector mirrors
`IntegrationConfigStore` into a native Turso table
`integration_state(id, provider_name, enabled, config_status, updated_at)` where
`id = 'integration:<provider>'`. An `IntegrationsOperationProvider` registers
`set_field` for entity `integration`. The render DSL reads the table with
`live_query` and renders each row through an `integration` entity profile whose
template contains `state_toggle(#{field: "enabled", …})`.

**This is the navigation pattern, verbatim.**

*For:*
- Every generic seam already resolves correctly (§0 table). Estimated frontend
  changes: **one optional prop on one widget.**
- Machine state stays out of the user's block tree — it is not content, does not
  belong in an org file, does not want backlinks, tombstones, or a name-chain.
- The state becomes queryable: a user can write their own `live_query` over
  `integration_state` and build their own settings panel. That *is* the ruling.
- MCP, tests, and the keystone reach enablement through `execute_operation`,
  the ADR-0024 door.
- It is the same move as the D2-era "swap the store backend to Turso later"
  note — see §3 Recommendation.
- **It is required anyway.** §2.6's OPEN defect needs exactly this projection to
  be fixable at all. Under Option A the settings surface costs *nothing extra*
  beyond a fix that must be built regardless — the discovery section and the
  settings surface become **one section over one table** (§4.1a).

*Against (stated honestly):*
- **Integration state is not a PN token.** ADR 0024's terminology section fixes
  "a token *is a block* (the Digital Twin)". Under Option A, integrations are
  operable (dispatcher ops) but not PN-*markable* — you could not write a
  Petri-net rule with an input arc over "gmail is enabled" without extending the
  token model beyond blocks. Navigation has the identical limitation today, so
  this is a consistent boundary rather than a new one, but it is a real cost and
  Martin should decide whether it is acceptable. If the deliberative layer later
  wants to reason over integration state, the token model must widen (which is
  arguably the right fix anyway, and is orthogonal to this ADR).
- One new native Turso table and one new projector to keep converged.
- Does not sync across devices. (Neither does the file today; see §6 R4.)

### Option B — project integration state into BLOCKS

Seed a hidden `Integrations` page whose children are one block per provider,
carrying `enabled` as a property. Render with existing block widgets; the
existing `state_toggle` needs no entity generalisation at all because the rows
really are blocks.

*For:*
- Maximum uniformity: PN tokens, backlinks, sharing, org round-trip, undo,
  templates, the whole block toolbox — all free.
- Zero new entity kind, zero new provider, zero new table.
- The keystone already models blocks completely, so oracle work is smallest.

*Against — and these are disqualifying:*
- **It makes machine state a replica.** A block lives in the block tree, which
  Layer 1 projects to an org file. Every toggle click would rewrite an org file,
  which the file watcher re-ingests as inbound intent, which diffs against a
  base, which the consolidator merges. Enablement is *device-local machine
  config*; running it through the vault's convergence machinery means a second
  device's vault sync flips integrations on the first device. This is precisely
  the class of failure invariant 11 exists to prevent.
- **Tombstones (invariant 9) apply.** Deleting a provider block could not be
  GC'd until every replica's base advanced past it; a stale replica would
  resurrect a provider the user removed.
- **The user can edit it.** Blocks are user content by construction. A
  hand-edited `enabled: yes` in an org file becomes an untyped string reaching
  the MCP launcher, and the strict `deny_unknown_fields` guarantee the store
  currently gives us (§2.3) is lost.
- **ADR 0030 authority conflict.** Blocks' authority is Loro (or Turso-LWW in
  SqlOnly). Enablement's authority is the state file, read at boot by the MCP
  client fleet *before* the block store is necessarily up. Making blocks the
  authority means the launcher must wait on the block pipeline; keeping the file
  authoritative while blocks mirror it means two writers to one logical value,
  violating "exactly one writer per store".
- The seeded blocks would need `ADR 0030` birth treatment and would show up in
  search, backlinks, and the Pages sidebar unless specially filtered — the
  `block:__default__` exclusion hack in `index.org` is the precedent for how
  ugly that gets.

**Option B trades a small implementation saving for a large architectural
liability.** The saving is also smaller than it looks, because §0 shows the
generic path costs almost nothing.

### Option C — a new widget kind binding the `IntegrationConfigStore` signal

Add e.g. `integration_toggle(...)` to the DSL, bound directly to the store's
`Mutable<IntegrationState>`, with a click handler calling `set_enabled`.

*For:* smallest diff; no table, no provider, no projector; the store stays
exactly as D2/D4 left it.

*Against:*
- It hardcodes the *contents* while making only the *placement* data. The user
  could move the widget but not query, filter, re-template, or aggregate it —
  it is the hardcoded section with a DSL wrapper.
- It creates a **second reactive path** into the UI, bypassing Layers 3–4. That
  is what `spawn_integrations_bridge` already is, and generalising it invites
  every future non-block store to add its own widget kind and its own pump.
- The click still bypasses the dispatcher, so ADR 0024 stays violated and MCP,
  tests, and the keystone still cannot reach the write.
- It fails the derived-data contract: a store-bound widget has no retraction /
  re-snapshot story when the store reloads from disk.
- **Decisively: it does not fix §2.6, and cannot.** A widget bound to the store's
  in-process signals does nothing for the *discovery* section, which is a
  `live_query` — a SQL surface. So Option C leaves the OPEN defect needing the
  projection table anyway, and we end up building **both** the projection *and*
  a bespoke signal-bound widget, with integration state living in two places
  that can disagree. Option C is therefore not the cheap arm; it is the most
  expensive one once §2.6 is priced in.

**Reject C.** It is the interim solution wearing a DSL costume, and the
sidebar-gap defect removes its only advantage.

### Recommendation

**Option A**, with one refinement that keeps ADR 0030 clean:

> **The `.state.toml` file remains the authority. Turso holds a projection.**

Do *not* move authority into Turso. Layer 3 is "ephemeral by contract"
(Model.md), the DB is rebuildable from the vault, and losing a user's
integration configuration on a DB rebuild is unacceptable in a way that losing
navigation history is not. Instead:

- authority: the state file (unchanged — D2/D4's work stands);
- one writer of the projection: an `IntegrationStateProjector` subscribed to the
  store's existing signals;
- the operation writes the **authority**, and the projection follows — "sinks
  never re-merge" (invariant 5), "exactly one writer per store" (invariant 4).

This also makes the D2-era "swap the store backend to Turso later" note a
*one-file* change: only the provider's write leg and the projector's source
move; the table, the profile, the DSL, and every test stay identical. See §3 Recommendation.

---

## 4. Architecture of the recommended path

### 4.1a ONE table, ONE entity, TWO surfaces that become ONE section

Because §2.6's fix and D5.b's fix are the same projection, they must not be
designed twice. Naming it once, explicitly:

| | |
|---|---|
| **Entity** | `integration` (URI scheme; row ids are `integration:<provider>`) |
| **Table** | `integration_state` |
| **Writer** | `IntegrationStateProjector` — **sole** writer, subscribed to `IntegrationConfigStore`'s per-provider `Mutable<IntegrationState>` signals |
| **Authority** | the `.state.toml` files (unchanged); the table is a mirror, re-derivable at any time |
| **Read by (discovery)** | `live_query` over `integration_state` — replaces the broken `sync_states` query at `assets/default/index.org:12` |
| **Read by (settings)** | the same `live_query`; each row's `state_toggle` binds `field: "enabled"` |
| **Written by (settings)** | `integration.set_field` → `IntegrationsOperationProvider` → the state file → the projector → back into the table |

**The two surfaces collapse into one section.** There is no separate "discovery
list" and "settings list": one query over one table, where each row shows the
provider, its status, and a toggle. A user who wants them apart writes two
queries with different `WHERE` clauses — which is precisely the
user-arrangeability D5.b asks for.

**Column shape — designed for D7 option (c), "enabled with status"** (the
default target per the pending ruling; the entry names the three candidates as
enabled-set / connected-set / enabled-with-status):

```sql
CREATE TABLE IF NOT EXISTS integration_state (
    id            TEXT PRIMARY KEY NOT NULL,  -- 'integration:<provider>'
    provider_name TEXT NOT NULL,
    enabled       INTEGER NOT NULL,           -- the stored decision
    config_status TEXT NOT NULL,              -- 'unconfigured' | 'configured'
    updated_at    TEXT NOT NULL,
    _change_origin TEXT
);
```

The row set is **every bundled provider**, not only the enabled ones —
`IntegrationsSettingsVm::rows()` already documents this as deliberate ("The list
is the PRESENCE axis in full: a provider that is off, or that the user has never
touched, is exactly what the settings surface exists to show",
`integrations_settings.rs:86-101`). Option (c)'s *discovery* reading is then a
`WHERE enabled = 1` in the seeded query, so the same table serves the strict
enabled-set reading too if D7 lands on (a) instead.

**How each D7 outcome maps** — the table absorbs all three, so this design does
not need to wait on the ruling:

| D7 lands on | Change required |
|---|---|
| (a) enabled-set | seeded query gains `WHERE enabled = 1` — a **layout-data** edit, no code |
| (c) enabled-with-status | the shape above as-is |
| (b) connected-set | needs a *third* axis (live connection), which neither the store nor this table has today — see §8 R9 |

Note the deliberate omission: **no `sync_token`, no cursor, no credential
reference.** `sync_states` stays exactly as it is and keeps its own job; this
table never tries to be it. That separation is the actual lesson of §2.6.

### 4.1 New and changed types

| Thing | Where | What |
|---|---|---|
| `integration_state` table | new DDL alongside `sync_states` (schema provider at `crates/holon/src/di/schema_providers.rs:87`, initialized `crates/holon/src/di/registration.rs:116`) | `id TEXT PRIMARY KEY` (`integration:<provider>`), `provider_name TEXT`, `enabled INTEGER`, `config_status TEXT`, `updated_at TEXT`. Native Turso table, not a matview — it has no upstream block source. |
| `IntegrationStateProjector` | `crates/holon-app/src/` | Subscribes to `IntegrationConfigStore`'s per-provider signals (the same `vm.signals()` the GPUI bridge consumes today) and upserts rows. The **sole** writer of the table. Replaces `spawn_integrations_bridge` wholesale. |
| `IntegrationsOperationProvider` | `crates/holon-app/src/` | `OperationProvider` for `EntityName::new("integration")`. Exposes one op: `set_field { id, field, value }`, `#[affects("enabled")]`, `id_column: "id"`. Refuses any other entity, op, or field — loud `Err`, mirroring `OrgModeSyncProvider`'s "rejects any other entity/op" shape (Operations.md dispatcher routing table) and `focus_target_is_a_block`'s prose-carrying refusal style (`crates/holon/src/navigation/provider.rs:257-268`). |
| `integration` entity profile | `assets/default/types/integration.yaml` | `entity_name: integration`, one variant whose `render` is the row template (label, status, toggle). Follows `person_profile.yaml`. |
| `appearance` prop on `state_toggle` | `crates/holon-frontend/src/view_model.rs` + `frontends/gpui/src/render/builders/state_toggle.rs` | `"task"` (default, unchanged) \| `"switch"`. See §4.3 — this is the one genuinely new widget capability. |

### 4.2 The operation

```
entity:  integration
op:      set_field
params:  id: "integration:gmail"   (id_column = "id")
         field: "enabled"
         value: Bool(true)
```

Nothing about this shape is new: it is the same `set_field` signature
`state_toggle_intent` already builds for blocks
(`crates/holon-frontend/src/operations.rs:79-85`).

**PN wiring through `operation_dispatcher`** is by registration, not by code
change. `create_profile_resolver` iterates `dispatcher.operations()` and buckets
by `entity_name` (`crates/holon/src/di/registration.rs:412-419`); registering
the provider in the DI graph is therefore sufficient for the descriptor to reach
`resolve_profile(row).operations` and thence `find_set_field_op`. Guard/arc
declaration follows ADR 0031's catalog shape — declare
`OpGuard`/`TransitionArcs` explicitly rather than leaving them `Undeclared` as
the navigation descriptors currently do.

**Refusal semantics (fail loud, per CLAUDE.md).** `set_field` must `Err` with
an enriched message when: the id's scheme is not `integration`; the provider is
not in the bundle; the field is not `enabled`; the value is not a bool; or the
atomic file write fails. No `.ok()`, no default-to-off. The existing
`DegradedToast` path in GPUI already surfaces dispatch failures, so a refusal
stays visible.

### 4.3 How the render DSL declares the toggle

The seeded left-sidebar section (`assets/default/index.org:12`) grows from a
read-only `sync_states` list into the settings surface:

```
live_query(#{
  sql: "SELECT id, provider_name, enabled, config_status FROM integration_state
        ORDER BY provider_name ASC",
  item_template: render_entity()
})
```

**`render_entity()`, not an inline `row(...)`.** This is a load-bearing choice.
`shared_render_entity_build` is what resolves the profile and therefore what
attaches `operations` to the node
(`crates/holon-frontend/src/render_interpreter.rs:783-792`). An inline `row(...)`
template produces a node with an empty `operations` vec, `find_set_field_op`
returns `None`, and the toggle renders display-only with a warning. Letting the
`integration` profile own the row template is both the working path and the
architecturally right one — the entity owns its presentation, the layout owns
its placement.

The profile's variant render:

```yaml
entity_name: integration
variants:
  - name: default
    render: 'row(text(col("provider_name")), spacer(8),
                 text(col("config_status"), #{muted: true}), spacer(8),
                 state_toggle(#{field: "enabled", current: col("enabled"),
                                states: "off,on", appearance: "switch"}))'
```

**Why `appearance` is needed.** `state_toggle`'s two-state cycling already
works: `cycle_state` falls through to `states.iter().position(...)` for
non-task keywords (`crates/holon-api/src/render_eval.rs:154-169`), so
`"off,on"` cycles correctly, and note the value crossing the wire is the
*state string* (`OperationIntent::set_field(… Value::String(next))`,
`operations.rs:84`), so the provider must parse `"on"`/`"off"` into a bool at
the boundary — parse, don't validate — rather than the DSL carrying a raw bool.
What does *not* work is the appearance:
`state_icon` (:207-222) and `state_display` (:224-239) are hard-wired to the
task vocabulary, so both `"off"` and `"on"` would render as `○` in the
`primary` colour — indistinguishable. `appearance: "switch"` selects a
track/knob rendering (the geometry currently hand-written at
`frontends/gpui/src/integrations_ui.rs:294-315`, moved into the builder where it
belongs and shared with
`frontends/gpui/src/render/builders/pref_field.rs`'s toggle, killing the duplication its own
comment admits to).

### 4.4 The click round-trip

```
user clicks the switch
  → state_toggle builder reads props, calls state_toggle_intent
      (entity from row id scheme = "integration", row_id = "integration:gmail")
  → services.dispatch_intent → session.execute_operation
  → OperationDispatcher routes entity "integration"
  → IntegrationsOperationProvider::set_field
  → IntegrationConfigStore writes <provider>.state.toml atomically   [AUTHORITY]
  → the store's Mutable<IntegrationState> fires
  → IntegrationStateProjector upserts integration_state              [PROJECTION]
  → CDC → LiveData → the live_query's ReactiveRowSet applies the change
  → the row node's data Mutable updates → state_toggle re-interprets
  → repaint
```

Every arrow after the first two is machinery that already exists and is already
exercised by every block field write. Note that the write is *not* optimistic:
the switch reflects the projection, so a refused write leaves the switch where
it was and raises a toast — which is the correct fail-loud behaviour and is
strictly better than today's `window.refresh()` after a possibly-failed
`set_enabled`.

### 4.5 What gets deleted

At the end of the increments below, **all 350 lines of
`frontends/gpui/src/integrations_ui.rs` are deleted**, plus:

- its `IntegrationsSettingsGlobal` install (`frontends/gpui/src/main.rs:267`);
- the `render_settings_integrations` call and its `SectionTheme` construction in
  `frontends/gpui/src/lib.rs:1024-1042`;
- `spawn_integrations_bridge` and its per-signal tokio pump;
- the three `TransparentTracker` id helpers and the two notice constants
  (`NEXT_LAUNCH_NOTICE`, `UNAVAILABLE_NOTICE`).

Per CLAUDE.md ("refactor completely, leave only the new approach"), the old path
is removed in the same increment that proves the new one, not left as a
fallback.

Two of those deletions carry real content that must be re-homed, not dropped:

- **`NEXT_LAUNCH_NOTICE`** ("saved immediately, takes effect at next launch")
  is a genuine disclosure of a genuine limitation. It becomes a `text(...)` in
  the seeded layout beside the section header — still visible, now user-movable.
- **`UNAVAILABLE_NOTICE`** (the fail-loud wiring-bug arm) is subsumed: under
  Option A a missing provider means `find_set_field_op` returns `None`, and the
  builder's existing disclosure path (display-only glyph +
  `tracing::warn!`, `state_toggle.rs:48-68`) fires. Verify this covers the
  "table exists but provider unregistered" case before deleting the notice; if
  it does not, the increment adds the missing disclosure rather than losing it.

### 4.6 What this does to the keystone — the big win

**Today the keystone cannot see settings at all.** The Settings modal is a GPUI
`AppModel` flag (`frontends/gpui/src/lib.rs:394`, toggled at :1197) rendering
GPUI-only elements; the headless keystone's SUT boundary stops at the shared
ViewModel. Only a windowed GPUI test can reach the integrations switch, and
issue #22 records that *no gate currently executes GPUI windowed tests*. So the
interim section is effectively **ungated**.

**Under this design the integrations surface moves into the seeded left-sidebar
render tree**, which the keystone already builds, mutates, and asserts on. The
keystone additionally already has `state_toggle` machinery on both sides — the
reference models it (`crates/holon-integration-tests/src/pbt/reference_state.rs:145,
:1246`), there are ref capabilities for it
(`rendered_state_toggle_ids`, `crates/holon-integration-tests/src/pbt/ref_caps/toggle.rs:98`),
and an invariant asserts it
(`InvViewmodelStateToggleCorrect`,
`crates/holon-integration-tests/src/pbt/invariants/bodies/viewmodel_state_toggle_correct.rs`).

So the answer to the brief's question is **yes, and it is the strongest argument
for this design**: integration enablement moves from an ungated windowed-only
surface to one the keystone can drive and oracle. That converts a whole class of
settings bugs from "dogfood finds it" to "the keystone finds it".

**Honest split — the win is the ORACLE half, not the ENVIRONMENT half.** §2.6's
entry classifies the discovery defect as primarily **ENVIRONMENT**: the failing
path is `McpSyncEngine::sync_entity_inner` running a cursorless strategy against
a real sidecar, and the keystone's only MCP wiring is
`fake_mcp_module::register_fake_mcp`
(`crates/holon-integration-tests/src/test_environment.rs:438,939`), a
concurrent-DDL race stressor — no `ToolSync`/`ResourceSync` strategy ever runs.
This design does **not** fix that; sidecar-driven MCP sync stays outside the
keystone. What it fixes is the **ORACLE** half the entry names secondarily
("no invariant anywhere relates the contents of the Integrations section to the
set of enabled integrations"): enablement, its projection, and its rendering all
become keystone-visible, so the section's rows can be asserted against the
enabled set. Claiming more than that would be overclaiming.

**A second gap the entry surfaces, which changes §5.** It states: *"There is no
enable-integration transition in the catalog, so the state 'N integrations
enabled' is not generatable."* So Increment 4 needs a **new keystone
transition** (enable/disable an integration), not merely a new invariant — an
invariant over a state the generator can never reach is vacuous by
construction. This is folded into Increment 4 below and is the direct cause of
risk R3.

Caveat to verify at increment start: the keystone generates its own `index.org`
layouts rather than always using the bundled one
(`crates/holon-integration-tests/src/pbt/reference_domain_state.rs:43,104`), so
the generator must actually *draw* an integration section for the oracle to
have teeth. Issues #23/#25 are the standing warning that a generator gate can
silently make an invariant vacuous — the Increment 4 rung below therefore
carries an explicit vacuity guard.

---

## 5. Increments

Each is independently landable and strictly better than its predecessor. Each
names its red-first PBT rung per the `holon-feature` skill.

### Increment 1 — the `integration` entity exists and is queryable

Add the `integration_state` DDL and `IntegrationStateProjector`. Register the
projector in DI beside the existing store wiring. No UI change; the hardcoded
section stays exactly as it is.

*Rung — headless harness tier* (new
`crates/holon-app/tests/integration_state_projection.rs`, conventions from
`integrations_section_seed.rs`).
**Red for the right reason:** the table does not exist, so the query errors.
Then: a store write for provider P produces exactly one
`integration_state` row with `id = 'integration:P'` and matching
`enabled`/`config_status`; a second write updates in place (no duplicate row);
a store reload from a hand-edited file re-converges the row. That last case is
the derived-data convergence contract and is the one a naive projector fails.

*Strictly better because:* integration state becomes inspectable from the
`holon` MCP and from any user query, immediately, before any UI moves.

**Increment 1b — repoint the discovery query, closing the OPEN defect.**
Change `assets/default/index.org:12` from the `sync_states` query to
`SELECT … FROM integration_state ORDER BY provider_name ASC` (plus
`WHERE enabled = 1` if D7 lands on (a)), and update
`crates/holon-app/tests/integrations_section_seed.rs` accordingly. This is a
**layout-data edit plus a test update** — no Rust changes beyond Increment 1.

*Rung:* the existing `integrations_section_seed.rs` tests, retargeted; its
`integrations_query_lists_synced_providers_in_order` becomes
`…lists_enabled_providers_in_order` over fixture rows written the way the
projector writes them.

*Strictly better because:* it resolves
`docs/Testing/bugfunnel/entries/2026-08-18-integrations-discovery-section-lists-only-orgmode.md`
from OPEN, and does so **before** any of the settings work lands. If D5.b were
cancelled tomorrow, Increments 1 and 1b would still be the right change. That is
the cleanest possible evidence that this is the correct arm of the fork.

### Increment 2 — the operation exists and is the only writer

Add `IntegrationsOperationProvider`, register it. Change
`IntegrationsSettingsVm::set_enabled`'s **caller** — the GPUI switch at
`integrations_ui.rs:326` — to dispatch `integration.set_field` instead of
calling the VM directly. The hardcoded section still renders; only its write leg
moves onto the ADR-0024 door.

*Rung — headless harness tier.* **Red:** `execute_operation(EntityName::from
("integration"), "set_field", …)` returns "no provider for entity". Then:
dispatching flips the file and the projected row; and the refusal matrix
(wrong scheme / unknown provider / wrong field / a value that is neither
`"on"` nor `"off"` / unwritable dir) each `Err`s with a message naming the
offending value — never silently no-ops.

*Strictly better because:* MCP, tests, and agents can now toggle an integration.
This alone closes the ADR-0024 violation, independent of everything else.

### Increment 3 — `state_toggle` can render a switch

Add the `appearance` prop through `ViewKind::StateToggle`, the reactive
constructor, and the GPUI builder; move the track/knob geometry from
`integrations_ui.rs:294-315` into the builder and point
`frontends/gpui/src/render/builders/pref_field.rs`'s toggle at
the same code. Still no layout change.

*Rung — windowed GPUI tier* (this is a pixel/geometry concern the headless
snapshot cannot see; it is also the tier that must be un-blocked — see §8 R2 and
issue #22). **Red:** a `state_toggle` with `appearance: "switch"` renders the
task glyph. Then: on and off are visually distinct, and the snapshot's
`ViewKind::StateToggle` carries the appearance so the headless tier can assert
the *prop* even where it cannot assert the pixels.

*Strictly better because:* it removes the duplicated switch geometry the
existing comment apologises for, and it makes `state_toggle` a general
two-state control rather than a task-only one.

### Increment 4 — the seeded layout carries the live section; the hardcoded one dies

Ship `assets/default/types/integration.yaml`; change the seeded left-sidebar
render (`assets/default/index.org:12`) from the read-only `sync_states` list to
the read-write `integration_state` + `render_entity()` section; re-home
`NEXT_LAUNCH_NOTICE` as layout text; **delete `frontends/gpui/src/integrations_ui.rs`
and its call sites** (§4.5).

*Rung — keystone (headless), extending
`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`.* This is the
rung that pays for the whole design.

**Prerequisite inside this increment: a new catalog transition.** §4.6 records
the entry's finding that there is *no enable-integration transition in the
catalog, so "N integrations enabled" is not generatable*. Add an
`enable_integration` / `disable_integration` transition that dispatches
`integration.set_field` (the real op, per the drive-via-drivers directive) and
mirror it in the reference model. Without this the invariant below can only
ever observe the seeded state and is vacuous by construction — which is
precisely failure mode R3.
**Red:** the keystone's rendered tree for a vault with the seeded sidebar
contains no `state_toggle` bound to an `integration:` row; the new oracle
(reference: "toggling provider P flips P's enabled in the reference model, and
the rendered toggle for `integration:P` shows the new state") fails because the
widget is not drawn.
Then green. **Vacuity guard required** (issues #23/#25 are the precedent for a
generator gate silently zeroing a rung): the rung must assert a non-zero count
of drawn integration toggles across the run and fail the run if it is zero,
rather than passing on an empty set.

Also update `crates/holon-app/tests/integrations_section_seed.rs` — its
`sync_states` assertions become `integration_state` assertions. Keep its third
test (`deleted_integrations_section_does_not_resurrect_on_reseed`) unchanged and
green: user deletion of the settings section must still stick, which is exactly
the ruling's "user-arrangeable" property.

### Increment 5 — dogfood gate

Per CLAUDE.md, `dogfood-explorer` is the final gate. Drive the live app: toggle
each provider, confirm the projection and the file agree, kill and relaunch,
confirm persistence, hand-edit a state file and confirm re-convergence, delete
the section from the layout and confirm it stays deleted. Any finding sends the
feature back to a PBT rung first (red-for-the-right-reason as proof), then a
fix, then re-dogfood.

---

## 6. Interaction with the in-flight OAuth lane

A **Configure** button is being added to the hardcoded section right now by
`lane-oauth` and neighbours. **Premise check: no Configure button exists in
`main` @ 66c90c4b** — `grep -rn "Configure" frontends/gpui/src` returns only
`BootStage::ContainerConfigure` and `ConfigStatus::Configured`. So this design
is not currently in conflict with anything landed; the coordination below is
forward-looking.

The OAuth machinery that a Configure click would drive does exist:
`crates/holon-mcp-client/src/rest_oauth2.rs`,
`crates/holon-mcp-client/src/credential_store.rs`, and
`crates/holon-api/src/auth.rs`.

**It absorbs cleanly, because `configuration` is already the second axis of the
same state** (`IntegrationState { enabled, configuration }`,
`integration_state.rs:60-63`), and `config_status` is already a column in the
projected row (§4.1). A Configure button is therefore **a second action on the
same row**, not a second surface.

**Caveat on the widget — do not assume `op_button` drops straight in.**
`op_button` exists (`crates/holon-frontend/src/shadow_builders/op_button.rs`,
`frontends/gpui/src/render/builders/op_button.rs`) but it is **not** a
free-standing "call this op" button: it takes the op name positionally or from
a `name` column and *hard-requires* a `target_id` column on the current row,
with a documented contract that call sites "must drive `op_button` from a
`chain_ops`/`ops_of` row source" (shadow builder, :26-36 — both are `expect`s,
so a wrong call site panics rather than degrading). Two admissible routes,
to be chosen when the OAuth lane converges:

- **(a) no widget change** — nest an ops collection in the integration row
  template: `row(#{collection: ops_of(...), item_template: op_button(col("name"))})`,
  the shape the mobile action bar already uses
  (`crates/holon-frontend/src/view_model.rs:426`);
- **(b) small generalization** — let `op_button` take an explicit target from
  props when no `chain_ops`/`ops_of` row source is present, replacing the two
  `expect`s with a loud typed error.

Recommendation: **(a)** first, because it needs no widget change and keeps the
op-discovery path uniform; fall back to (b) only if `ops_of` cannot be pointed
at a non-block entity. Verify that before committing — it was not confirmed
during this design pass.

Either way the `integration` provider gains a second descriptor (`begin_oauth`)
beside `set_field`.

**Coordination rules for the lanes, in dependency order:**

1. **The OAuth lane should keep building against the hardcoded section.**
   Increments 1–3 touch neither `render_section` nor `render_row`; only
   Increment 2 changes what the *switch's* click does, and Increment 4 deletes
   the file. So the OAuth lane is unblocked for the entire runway and only
   needs to converge at Increment 4.
2. **Increment 4 must not land before the OAuth flow is functional**, or a
   half-migrated Configure affordance is lost in the file deletion. Order:
   OAuth flow lands on the hardcoded section → Increment 4 ports the button to
   `op_button` + a `begin_oauth` descriptor in the same change that deletes the
   file.
3. **The OAuth lane owns the `Configuration` write path**; this design must not
   let `integration.set_field` accept `field: "configuration"`. Enablement and
   configuration stay orthogonal axes with separate operations — the store's
   own doc comment for `set_enabled` says "leaving its configuration axis
   untouched", and that separation should survive into the operation vocabulary.
4. **Shared with `lane-keychain-store`:** nothing in this design touches
   credential storage. `config_status` is the *display* half (`ConfigStatus`,
   `integrations_settings.rs:22-25`) with credential locations dropped, and the
   projected column must stay that way — **no credential reference, path, or
   secret may ever reach the `integration_state` table**, because the table is
   user-queryable by construction. This is a hard boundary; §8 R1.

---

## 7. Out of scope

Explicitly NOT part of this work:

- **Starting/stopping the running MCP client fleet on toggle.** The section's
  own notice says a switch takes effect at next launch; that limitation is
  preserved and re-homed (§4.5), not fixed here.
- **Migrating `pref_field` / preferences onto entity rows.** Preferences already
  go through the render DSL (§2.2); unifying their *storage* with the entity
  pipeline is a separate, larger question.
- **Moving the Settings modal itself into the block tree.** This design makes
  the integrations *section* layout data in the sidebar. Whether Settings as a
  whole becomes a page is a separate ruling.
- **Making integration state syncable across devices.** Out of contract today
  (invariant 11); nothing here makes it harder later.
- **Widening the PN token model beyond blocks** so integrations become
  markable (§3 Option A, *Against*). Flagged, not attempted.
- **Turso as the authority for enablement.** Deliberately deferred; §3 Recommendation keeps
  it a one-file change.
- **Credential storage, OAuth transport, keychain backends.** Owned by the
  in-flight lanes.

---

## 8. Risk register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | A credential location leaks into the queryable `integration_state` table | Low | **High** — user-queryable table, MCP-readable | Project only the `ConfigStatus` display enum, never `Configuration`. Add a test asserting the table's column set exactly, so a later field addition is a failing test, not a silent leak. Route the increment through `security-executor`. |
| R2 | Increment 3's windowed rung cannot run — no gate executes GPUI windowed tests (issue #22) | **High** — already true today | Medium | Do not let Increment 3 claim green on an unexecuted rung. Either fix the gate first or run the windowed tests manually and paste the output. Note that the *net* effect of this design is to move coverage from the windowed tier to the keystone, so this risk shrinks over the runway. |
| R3 | The Increment 4 keystone rung goes vacuous — the generator never draws an integration section, so the invariant passes trivially (issues #23, #25 precedent) | Medium | High — a false green on the design's main payoff | Mandatory non-zero-draw assertion in the rung itself; report the draw count in the PR alongside the red log. |
| R4 | Projector divergence: the file and the table disagree after an external edit or a partial write | Medium | Medium | The projector must re-derive from the store's signal, never accumulate deltas — the stateful-regrouping law from the derived-data contract. Increment 1's third case (hand-edited file re-converges) is the pinning test. |
| R5 | `render_entity()` in the item_template silently yields no operations (missing profile, unregistered provider) and every toggle renders display-only | Medium | Medium | The builder already warns and renders a visibly inert glyph (`state_toggle.rs:48-68`) rather than a dead-looking live control — keep that. Add an assertion in the Increment 4 rung that the drawn toggles have non-empty `operations`. |
| R6 | Increment 4 lands before the OAuth Configure flow, losing in-flight work | Medium | Medium | §6 rule 2: strict ordering, and Increment 4's PR must name the OAuth rev it builds on. |
| R7 | `appearance` on `state_toggle` drifts into a general theming escape hatch | Low | Low | Keep it a closed two-value enum parsed at the boundary (`"task" \| "switch"`), fail loud on anything else — parse, don't validate. |
| R8 | Deleting `integrations_ui.rs` loses the fail-loud `UNAVAILABLE_NOTICE` arm | Medium | Medium | §4.5: verify the builder's disclosure covers the unregistered-provider case *before* deleting; if not, add it in the same increment. |
| R9 | **D7 lands on (b) "connected-set"**, which needs a live-connection axis that neither `IntegrationConfigStore` nor `integration_state` has | Low–Medium | Medium | §4.1a maps (a) and (c) to a layout-data edit or the shape as-shipped. (b) needs a genuinely new third axis (connection health), sourced from the MCP client fleet rather than from the store — a separate increment, not a column rename. Do not pre-build it; the table takes an extra column without disturbing anything else. Flag to Martin in §10. |
| R10 | Increment 1b lands the discovery fix while D7 is still pending, and the ruling then contradicts the shipped query | Medium | **Low** | The difference between (a) and (c) is a `WHERE enabled = 1` in seeded layout data — reversible in one line with no code change and no migration. Landing 1b early is therefore cheap even if the ruling moves; say so in the PR rather than blocking on D7. |
| R11 | This design and the `lane-sidebar-gap` fix are built twice, in two lanes, with two tables | Medium | High — two disagreeing sources of integration state | §4.1a names the one table, one entity, one writer. Whichever lane lands Increment 1 first owns `integration_state`; the other builds on it. Requires an explicit handoff between this lane and `lane-sidebar-gap` before either starts coding. |

---

## 9. Staleness guard

Five parallel lanes are touching this area. **Re-run these at the start of every
increment**; any changed answer means re-reading this document's §2 before
writing code. (Run from `/Users/martin/Workspaces/pkm/holon`.)

```sh
# §0 — is the widget→operation binding still entity-generic?
grep -n "fn entity_name" -A 12 crates/holon-frontend/src/reactive_view_model.rs
grep -n "fn state_toggle_intent" -A 20 crates/holon-frontend/src/operations.rs
grep -n "for op in dispatcher.operations()" -A 8 crates/holon/src/di/registration.rs

# §2.1 — is the hardcoded section still shaped as described (and how big)?
wc -l frontends/gpui/src/integrations_ui.rs
grep -n "set_enabled\|render_switch\|spawn_integrations_bridge" frontends/gpui/src/integrations_ui.rs
grep -n "render_settings_integrations" frontends/gpui/src/lib.rs

# §6 — what has the OAuth lane added? (Configure button, new ops)
grep -rn "Configure\|begin_oauth\|oauth" frontends/gpui/src crates/holon-app/src crates/holon-mcp-client/src

# §2.3 — is the store still the file-backed authority with these two axes?
grep -n "struct IntegrationState\|struct IntegrationConfigStore\|struct StateFile" -A 6 \
  crates/holon-mcp-client/src/integration_state.rs

# §4.3 — is the seeded section still the sync_states live_query?
grep -n "sync_states\|Integrations" assets/default/index.org

# §2.6 — has the discovery defect been ruled on (D7) or fixed by another lane?
grep -n "^status:" docs/Testing/bugfunnel/entries/2026-08-18-integrations-discovery-section-lists-only-orgmode.md
grep -rn "integration_state" crates frontends assets --include='*.rs' --include='*.sql' --include='*.org'

# §4.6 — does the keystone still model state_toggle on both sides?
grep -rn "state_toggle\|StateToggle" crates/holon-integration-tests/src/pbt/ref_caps/toggle.rs \
  crates/holon-integration-tests/src/pbt/invariants/bodies/viewmodel_state_toggle_correct.rs
```

---

## 10. Open questions for Martin

1. **PN token model.** Option A leaves integrations operable but not
   PN-markable (§3). Navigation has the same boundary. Is that acceptable
   indefinitely, or should widening tokens beyond blocks go on the roadmap?
2. **Authority.** §3's refinement keeps the `.state.toml` file authoritative and
   Turso a projection, on the grounds that Layer 3 is ephemeral by contract. The
   alternative — Turso as authority, like `navigation_history` — is simpler
   (no projector) but loses enablement on a DB rebuild. Recommendation: keep the
   file. Confirm.
3. **D7 (the pending discovery-section ruling) and D5.b are the same table.**
   §4.1a designs for option (c) "enabled with status" as the default target, and
   shows (a) "enabled-set" costing a one-line seeded-query edit. Only (b)
   "connected-set" needs new machinery (§8 R9). Recommendation: rule (c), and
   let Increment 1b land the discovery fix without waiting — the (a)/(c)
   difference is layout data, not code. Confirm, or tell us to hold 1b.
4. **Lane ownership.** `lane-sidebar-gap` and this lane would otherwise both
   build `integration_state` (§8 R11). Recommendation: one lane owns Increments
   1 and 1b — they close the OPEN bugfunnel entry and are valuable standalone —
   and the settings increments 2–5 stack on top. Confirm which lane.
5. **Scope of the surface.** This moves integrations into the **left sidebar**
   section that `integrations_section_seed.rs` already seeds, which means
   integrations leave the Settings modal entirely. The alternative is to render
   the same `live_query` inside the modal. Recommendation: the sidebar, because
   it is the surface that is already layout data and already deletable —
   but it is a visible UX change and is Martin's call.
