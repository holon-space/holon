---
id: 2026-08-23-todoist-projects-second-write-authority-boot-panic
date: 2026-08-23
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The desktop app panics at boot with "entity 'todoist-projects' already has a
  write authority" because a connected MCP integration registers its entities as
  free-standing types and the boot sequence then derives a SECOND write
  authority over the mirror table.
---

## Bug

Reported by Martin dogfooding: starting the GPUI desktop app from a child of
`main` 449a269e over his own vault (`~/Workspaces/pkm/holon-pkm`) with the
Todoist integration enabled aborts before the window appears.

```
PANIC crates/holon/src/api/operation_dispatcher.rs:1395:21
  inside di.factory.FrontendSession.resolve_engine
[OperationModule] write authority for free-standing type 'todoist_projects':
[OperationDispatcher] entity 'todoist-projects' already has a write authority; …
```

The two spellings are not a second defect: `EntityName::new` folds `_` to `-`
(`crates/holon-api/src/types.rs:48`) so the type is `todoist_projects` and the
routed entity is `todoist-projects`.

## Root cause

Two boot steps describe ONE entity, and neither knows about the other.

1. `McpIntegrationsModule` publishes a `RegistryOperationProxy` per configured
   integration (`crates/holon-app/src/mcp_integrations.rs:574`). Its descriptors
   come from the server's tool list crossed with the sidecar's `tools:` block
   (`crates/holon-mcp-client/src/mcp_provider.rs:254`), so `add-projects`,
   `update-projects` and `find-projects` (`assets/integrations/todoist.yaml:111`,
   `:117`, `:130`) all claim entity `todoist-projects`. These providers are
   resolved FIRST, at the top of the `OperationDispatcher` factory
   (`crates/holon/src/api/operation_dispatcher.rs:1321`).
2. On connect, the same module calls `register_entity_types`
   (`mcp_integrations.rs:500`), which puts every sidecar entity into the
   `TypeRegistry` as a `TypeDefinition` built from its `schema:` columns. Such a
   type references nothing and has persisted fields, so
   `is_free_standing` accepts it (`crates/holon/src/di/schema_providers.rs:462`),
   and the factory's next loop derives a `SqlOperationProvider` write authority
   for it and hands it to `register_provider`, which refuses the second claim.

The provenance is what got lost: `EntityConfig::to_type_definition` built the
definition with the default `TypeSource::UserDefined`, and
`TypeSource::McpProvider(String)` — the arm that says exactly "these rows mirror
a connector" — was declared in `crates/holon-api/src/entity.rs:239` and
constructed nowhere in the tree. With no marker on the type, the loop cannot
tell a locally-owned type from a mirror, so it derives write machinery for both.

The panic needs the integration to actually CONNECT: a failed connect registers
an `EmptyOperationProvider` and never reaches `register_entity_types`, which is
why the app starts fine without `TODOIST_API_KEY`.

### Measurement

Two competing hypotheses were tested and REFUTED rather than argued away:

- *A declaration persisted in the vault DB is replayed at boot.* Nothing loads
  type definitions back out of Turso: every `TypeRegistry` write in the tree is
  `create_default_registry`, `File::type_definition`, the org profiles, the MCP
  sidecars, or a runtime `declare_type`. The reproduction below fires on boot-1
  against a fresh database.
- *The sidecar is loaded twice.* A duplicate registration would fail inside
  `register_entity_types` (warned) or add a second composed provider; neither
  raises this message, whose wrapper text names the free-standing loop as the
  SECOND registrant.

The mechanism was then reproduced through the production DI path, not inferred:
`crates/holon-integration-tests/tests/boot_suite/mcp_mirrored_entity_write_authority.rs`
against the prod-faithful fake connector produced the identical panic at the
identical site (`.lane-logs/04-RED-mirrored-entity.log`):

```
[OperationModule] write authority for free-standing type 'fake_probe':
[OperationDispatcher] entity 'fake-probe' already has a write authority; …
```

## Missing piece

Two independent reasons, both measured.

**The shared fake was not connector-shaped.** `fake_mcp_module` registered NO
entity type into the `TypeRegistry`, and its `FakeOperationProvider::operations()`
returned an empty vector — so in the tests that DO boot it
(`store_suite/turso_ivm_index_bug.rs`, via `TestEnvironmentBuilder::with_fake_mcp`)
no entity was ever claimed twice. That is the parity this fix closes.
`pbt_mcp_fake` is a second copy of the same fake with no users at all.

**The composed keystone never starts an app.** Its SUT
(`HeadlessFrontendComponent`) implements `SutAppLifecycle::start_app` as
`unimplemented!()`, so the `StartApp` transition — the only one that boots a
connector, and which hard-codes `enable_fake_mcp: true`
(`pbt/transitions/start_app.rs:90`) — is cap-gated OUT of the composed alphabet
(asserted in `pbt/composed/builder.rs:1230`). Measured: `just keystone-smoke`
stays GREEN with the boot guard fully disabled
(`.lane-logs/32-keystone-guard-off.log`). The `enable_fake_mcp: true` on that
transition is therefore not evidence that the keystone boots a connector.

The escape is ENVIRONMENT: booting is generatable in principle, but the code
path that fails — a connector seeding the type registry while holding
descriptors on the same entity — ran nowhere in the test wiring.

## Remedy

**Root cause, at the boundary.** A `TypeDefinition` now carries
`write_authority: WriteAuthority` (`crates/holon-api/src/entity.rs`), separate
from `source`. Origin and write ownership are different questions: a connector
can mirror a feed it only READS, and such a type still needs local write
machinery derived from its columns. `EntityConfig::to_type_definition` takes both
— the connector's name, and whether that connector declares a mutating tool for
the entity (`McpSidecar::write_ownership`, which classifies by the sidecar's own
`effect:`). The boot loop derives no SQL authority for a type whose writes the
connector owns, and logs the connector it deferred to
(`crates/holon/src/api/operation_dispatcher.rs`).

**The registry's ambiguity rule was too coarse.** `register_provider` refused any
second provider that shared an ENTITY. Dispatch selects by the (entity, op) PAIR
(`operation_dispatcher.rs:876-901`), so that pair — not the entity — is the unit
of ambiguity. Under the entity-level rule a connector's read-only op made the
entity unusable for the derived CRUD provider, which is how a read-only mirror
ended up with no write authority at all. The rule now refuses exactly what the
error text always described: a second provider offering an (entity, op) the
registry already answers.

Every case lands where it should:

- An entity the connector WRITES (`todoist_projects`) gets the connector's
  vocabulary and no local CRUD — a derived `set_field` would write the mirror
  table and never reach Todoist.
- An entity the connector only READS keeps its derived CRUD, and the connector's
  read op coexists because the two op sets are disjoint.
- An entity the connector declares no tool for (`gcal.yaml`, `gmail.yaml`,
  `jsonplaceholder.yaml` ship `tools: {}`) is unchanged.
- A genuine duplicate declaration — the same (entity, op) twice — is still
  refused, and the runtime re-declaration path keeps its own refusal
  (`crates/holon/src/core/type_declaration.rs`,
  `a_declared_type_cannot_be_redeclared_even_after_teardown`).

**Prod/test parity.** `fake_mcp_module` now boots like a real connector: the
in-memory server advertises a tool, the sidecar classifies it onto the entity,
the handle publishes the real `McpOperationProvider`, and the entity types are
seeded through `register_sidecar_entity_types` — one function now shared by the
connector and by every test that used to re-implement the loop
(`mcp_integration.rs`, `structural_pbt.rs`, `components.rs`,
`dense_patch_entity_links.rs`).

**Pin.** `boot_suite/mcp_mirrored_entity_write_authority.rs` boots twice over the
same persisted database and asserts, on each boot, that the connector-written
entity has exactly ONE authority (the connector's), and that both non-written
mirrors — the tool-less one and the read-only one — carry the derived CRUD triple
AND pass `assert_write_capability_for`. The read-only half is red under a guard
that defers for every mirrored entity: `ops present: ["find_readonly"]`
(`.lane-logs/37-RED-readonly-mirror.log`).

**An unclassified tool is now a load error.** Write ownership is read from the
sidecar's `effect:`, so silence there decides who writes an entity. A tool with
no `effect:` and no write metadata used to load and count as a read — meaning a
genuinely write-capable tool whose author declared none of the three would hand
its entity a LOCAL authority and send those writes to the mirror table instead of
the provider. `validate_write_policy` now refuses any tool without an `effect:`,
naming the tool and the vocabulary, and `McpSidecar::load` adds the file path.
The contradiction case (`effect: read` alongside `affected_fields`/`undo`) did
NOT exist before and is refused too; the pre-existing half — write metadata with
no `effect:` — is unchanged.

This made the shipped `assets/integrations/todoist.yaml` unloadable, which is the
point of the check: its four `find-*` tools carried no classification. They are
declared `effect: read` in this commit. `find-tasks` and `find-projects` are the
two entities' `sync.list_tool` calls, so the sync leg already depends on their
being reads; `find-tasks-by-date` and `find-completed-tasks` are the same query
family and carry no write metadata. No other shipped sidecar was affected —
`gcal`/`gmail`/`jsonplaceholder` ship `tools: {}` and `claude-history` already
classified both of its tools. Pinned by `every_bundled_sidecar_loads`, which is
red against the unclassified yaml (`.lane-logs/65-RED-flipB-shipped-yaml.log`);
the two refusals are red against the previous validation
(`.lane-logs/64-RED-flipA-strictness.log`).

**A failed entity registration is now an error, not a warning.**
`register_sidecar_entity_types` returns `Err` and the connect path in
`crates/holon-app/src/mcp_integrations.rs` fails the boot with the provider and
entity named. The refusal has real triggers, because `TypeRegistry::register`
INSERTS over whatever holds the name and returns `Ok` — so a sidecar entity
colliding with `block`, or with a second connector's entity, used to replace
that definition silently and change the shape every registry reader sees. A
reconnect re-registering the SAME provider's own entity stays a no-op. Pinned by
`an_entity_colliding_with_a_local_type_is_refused_by_name`,
`two_providers_claiming_one_name_is_refused` and
`the_same_provider_registering_twice_is_a_no_op`; both collision tests are red
against the previous warn-and-continue body (`.lane-logs/21-RED-registration.log`).

**Not covered.** A live GPUI double boot against the real Todoist server was not
run: the panic requires a successful connect, which requires Martin's
`TODOIST_API_KEY`. The reproduction and the fix are exercised through the same
`FrontendSession.resolve_engine` DI path with a connector that behaves like the
real one.

## Adjacent hazards

Found while fixing this, deliberately NOT fixed here — recorded for separate
triage.

**1. `entity_prefix` splits one entity in two.** A sidecar that declares
`entity_prefix` registers its type under `{prefix}{entity}`
(`McpSidecar::prefixed_name`) while `McpOperationProvider` builds its tool
descriptors under the BARE entity key from `tools:`. The two therefore never
meet: the connector's authority sits on `session`, the free-standing loop sees
`cc_session` as unclaimed and derives a SQL authority over the mirror table.
That is not this panic — it is the quieter failure the panic prevented in the
unprefixed case: a write routed to a local table the system of record never
sees. The fix belongs with the prefix design (one canonical name for an entity
across type, table and descriptor), not with this guard.

The three shipped sidecars that declare a prefix, and are therefore in scope:

| Sidecar | `entity_prefix` | Prefixed entities |
|---|---|---|
| `assets/integrations/gmail.yaml:192` | `gmail_` | label, thread, message |
| `assets/integrations/gcal.yaml:160` | `gcal_` | calendar, event |
| `assets/integrations/claude-history.yaml:11` | `cc_` | project, live_session, pending_question, session, task, agent, message, agent_message |

`gmail.yaml` and `gcal.yaml` ship `tools: {}`, so their entities have no
connector authority at all and the derived SQL one is currently the only
candidate — latent, and armed the moment either declares a write tool.
`claude-history.yaml` is already in the split state: its `send_message` and
`answer_question` tools claim the BARE `live_session` / `pending_question`,
while the registered types are `cc_live_session` / `cc_pending_question`, each
carrying a derived SQL authority. A write dispatched to the prefixed name lands
in the local mirror and never reaches the server.
`assets/integrations/todoist.yaml` declares NO prefix, which is why it produced
the loud panic instead of this quiet one.

A named TODO citing this entry sits next to the guard in
`crates/holon/src/api/operation_dispatcher.rs`.

**2. `TypeRegistry::register` silently overwrites.** It is an unconditional
`HashMap::insert` returning `Ok(())`
(`crates/holon-profiles/src/type_registry.rs:135`), so no caller can learn that
it replaced a live definition. The connector boundary now checks before
registering (above), but every other caller — profiles, `File`, `declare_type` —
is still on the honour system. Making the registry itself refuse is a
wider-blast-radius change than this lane should take.
