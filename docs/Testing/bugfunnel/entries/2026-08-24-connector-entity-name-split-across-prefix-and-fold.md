---
id: 2026-08-24-connector-entity-name-split-across-prefix-and-fold
date: 2026-08-24
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A connector's operation descriptors name entities by the raw sidecar key while
  the type, table and dispatch identity use the prefixed, hyphen-folded form, so
  a prefixed mirror gets a local SQL write authority the connector never sees and
  every underscored entity fails its own id-column and undo lookups.
---

## Bug

Found by agent exploration while fixing
`2026-08-23-todoist-projects-second-write-authority-boot-panic`, recorded there
as an adjacent hazard and triaged here on its own.

One entity is known by three different names, and the code moves between them by
assumption rather than by parsing:

| name | shape | who uses it |
|---|---|---|
| sidecar key | raw YAML key, underscores, no prefix (`live_session`) | `sidecar.entities`, `entity_readers`, the sync engine's `caches`, `tools[].entity` |
| canonical `EntityName` | prefix applied, `_` folded to `-` (`cc-live-session`) | dispatch routing, `has_provider`, the boot write-authority guard |
| table name | prefix applied, underscores (`cc_live_session`) | `TypeDefinition.name`, the Turso table, the ID scheme |

`McpOperationProvider` builds its descriptors from the RAW key
(`crates/holon-mcp-client/src/mcp_provider.rs:274-282`, used at `:314`), while
`register_sidecar_entity_types` registers the type under the PREFIXED table name
(`crates/holon-mcp-client/src/mcp_integration.rs`, via
`McpSidecar::prefixed_name`, `mcp_sidecar.rs:615`). Two consequences, both live
today:

**1. A prefixed connector entity is split in two.** The connector claims
`live-session`; the registry holds `cc_live_session`, whose canonical name is
`cc-live-session`. Nothing relates them, so `write_ownership` marks the type
connector-owned but the boot guard's `has_provider("cc-live-session")` is false,
and the free-standing loop derives a local SQL write authority over the mirror
table. A write dispatched to the prefixed name lands in the local mirror and
never reaches the provider — the "silently degrades to look fine" case the repo's
error philosophy puts last. `assets/integrations/claude-history.yaml` is already
in this state (`send_message` → `live_session`, `answer_question` →
`pending_question`, both under `entity_prefix: cc_`).

**2. Every underscored entity misses its own lookups, prefix or not.** Dispatch
hands `execute_operation` the canonical `EntityName`, whose string is
hyphen-folded, and the provider uses that string directly as a raw-key index:

- `mcp_provider.rs:688` — `sidecar.entities.get("todoist-tasks")` against keys
  spelled `todoist_tasks` misses, and the id column silently falls back to
  `"id"` (`unwrap_or_else(|| "id".to_string())`). The idempotency key for
  `keyed`/`once_only` writes is then minted from the wrong column's value.
- `mcp_provider.rs:796` passes the same folded string to `build_undo_action`,
  where `:534` misses `sidecar.entities` and degrades mirror undo to
  `Irreversible` (warned), and `:466` misses `entity_readers` and errors. So
  `update-tasks` / `update-projects` undo capture never worked for todoist.

This half needs no `entity_prefix` at all — any entity key containing `_` has it.

## Root cause

There is no parse step between the three name spaces. `EntityName::new` folds
`_`→`-` (`crates/holon-api/src/types.rs:48`) and `prefixed_name` adds the
prefix, but nothing converts BACK, so every site that receives a canonical name
and needs the sidecar key just uses the canonical string and takes whatever
`HashMap::get` returns — `None` handled as a default, a warning, or an error
depending on the site. Exactly the "be suspicious of `_ => default`" shape
CLAUDE.md names.

## Missing piece

No test drives a write to the PREFIXED name of a connector-claimed entity. The
shared fake connector (`fake_mcp_module`) declares `entity_prefix: None`
(`:348`) and entity keys without underscores, so it inhabits the one corner
where all three names coincide and every lookup accidentally succeeds. The
prefixed and underscored corners were unreachable in the test wiring:
ENVIRONMENT primarily (the failing path exists nowhere in the harness), COVERAGE
secondarily (no transition dispatches a connector write at all).

## Remedy

**FIXED.** One canonical external identity, parsed at the boundary.

`McpSidecar::entity_key_of(&EntityName) -> Result<&str>` converts a canonical
name back to the sidecar key — strips the prefix, unfolds the separator, and
fails loudly naming both spellings when it resolves to nothing. Descriptors now
carry `prefixed_name(key)` so a connector's routing identity is the same
`EntityName` the type, table and boot guard already use, and two connectors that
both declare `session` no longer collide. `execute_operation` resolves the key
ONCE at the top and passes it down, so the id-column and undo lookups index the
map with the spelling it is keyed by.

The fake connector now declares `entity_prefix` (`fk_`), which moves it out of
the corner where the three names coincide, and
`boot_suite/mcp_mirrored_entity_write_authority.rs` asserts on the prefixed
names. Red log: `.lane-logs/71-RED-prefix-split.log`.

### The contract, made explicit

`execute_operation(entity)` accepts exactly the `entity_name` the provider
advertises in `operations()`. That is not a new rule — the dispatcher already
selects a provider by matching a descriptor and then calls it with that
descriptor's own entity (`operation_dispatcher.rs:561` and the main dispatch
path) — but nothing stated it, so callers that constructed the name by hand
drifted the moment the descriptor's spelling changed. Accepting both spellings
was rejected: it would restore the two-identities-for-one-entity condition this
entry exists to remove.

`holon-mcp-mock`'s three test files were such callers (they dispatched by the
raw key) and are migrated to the advertised name. They were missed in the first
pass because the crate was not in the gate list; 25 of its 48 tests were red.
The failure was loud, which is why nothing silently mis-wrote — but the fix was
not landable as claimed until they were migrated.

### Two latent holes closed at the same boundary

- Two entity keys that canonicalize alike (`x_y` and `x-y`) are a LOAD ERROR:
  `EntityName` folds `_` to `-`, so they are one entity, and `entity_key_of`
  would answer with whichever the map held.
- Two CONNECTORS resolving to one canonical name are refused before anything
  connects (`assert_no_cross_sidecar_entity_collisions`, called from
  `McpIntegrationsModule`). Prefixing the descriptors widened this surface —
  `entity_prefix: todoist_` + key `tasks` now collides with an unprefixed
  `todoist_tasks` — and registration alone cannot catch it, because it refuses
  on the (entity, op) pair and two connectors naming their tools differently
  overlap on none. The check reads the loaded configs directly, so it also
  covers schema-less entities, which register no type and are therefore
  invisible to the type registry's own collision check.

Neither is reachable from the shipped sidecars; both are pins of a new
invariant rather than red-first proofs, and
`the_bundled_sidecars_claim_distinct_entity_names` keeps the shipped set honest
as it grows.

The cross-connector refusal returns an `Err` from `Module::configure`, not a
panic: the input is user-editable sidecar YAML, `BootError::from_bootstrap_error`
has a branch for exactly this module, and this module's established policy for
bad user config is a disclosed boot error rather than a crash.

`mcp_vtable` no longer re-derives the canonicalization it documented as "the
same normalization" — its id scheme and its cache-table name both come from
`canonical_entity_name`, so the claim is obtained rather than asserted. That
also corrects the table it builds for a hyphen-bearing entity key, which
previously kept the hyphen while the declared table does not.
