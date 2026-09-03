# Adversarial verification — lane `mcp-authority-fix`

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/mcp-authority-fix`
(identity grep `overflow_pair` in `crates/holon-mcp-client/src/mcp_sidecar.rs` = OK;
`@` empty, lane commit inspected as `@-`). Logs: `mcp-authority-verify-logs/`.
No jj/git write command was run. The one file mutation (claim 3 probe) was
cp-backed and restored — sha256 identical before and after.

---

## Claim 1 — Bisect (main `4e2ee368` FAILS, `3a070e88` PASSES) — **CONFIRMED**

Two independent read-only trees extracted with
`git -C /Users/martin/Workspaces/pkm/holon archive <rev> | tar -x`
(4360 and 4347 files — non-empty sentinel), each with its own target dir,
semaphored, cold-built.

| rev | tree-identity header | verbatim summary | log |
|---|---|---|---|
| `4e2ee368` | `STAMP_HITS=5`, `OVERFLOW_PAIR_IN_SIDECAR=0` | `Summary [   5.522s] 1 test run: 0 passed, 1 failed, 20 skipped` | `v1-main-4e2ee368.log` |
| `3a070e88` | `STAMP_HITS=0`, `OVERFLOW_PAIR_IN_SIDECAR=0` | `Summary [   3.529s] 1 test run: 1 passed, 20 skipped` | `v2-parent-3a070e88.log` |

Both verdicts match the lane's. Two controls the lane did not state, which
close the obvious escape routes:

- The test file is **byte-identical** in both trees
  (`cbf0eb151cfd6ddc41d6cee68c9be051b003ea9b5dd7c6fdaad8acc3183ec9b4`), and
  `boot_suite/main.rs` declares `mod mcp_mirrored_entity_write_authority;` in
  both — the PASS is not a vacuous "test absent" or "0 tests run".
- `crates/holon-mcp-client/src/mcp_sidecar.rs` is **identical** between
  `3a070e88` and `4e2ee368`, so the flip cannot be attributed to the sidecar
  file; it tracks `require_engine_stamp_has_a_home` appearing (0 → 5 hits).

The failure predates the chain. The lane's premise-refutation stands.

## Claim 2 — Root cause / boot panic — **CONFIRMED**

The fix-absent tree `4e2ee368` panics at boot with exactly the claimed message
(`v1-main-4e2ee368.log`):

    thread '…mirrored_entity_keeps_the_connector_as_its_only_write_authority'
    panicked at crates/holon/src/api/operation_dispatcher.rs:1592:21:
    [OperationModule] write authority for free-standing type 'fk_fake_shadow':
    type 'fk_fake_shadow' declares no `properties` overflow column …
    Declared persisted fields: ["id", "data"]

This is a `panic!` in production `OperationModule::configure`, not a test
assertion — the blast-radius claim holds.

Restored (fix present, lane tree): whole `boot_suite`
`Summary [  26.553s] 21 tests run: 21 passed, 0 skipped` (`v6-gate2.log`).

Boot loop read at `crates/holon/src/api/operation_dispatcher.rs:1637-1660`
confirms the "skips only types whose connector holds an authority" claim
verbatim: the `continue` fires only when
`type_def.owning_integration()` is `Some` **and** `dispatcher.has_provider(entity)`.
A read-only mirror satisfies neither conjunct's second half, so it reaches
`register_write_authority` and the `unwrap_or_else` panics.

## Claim 3 — Fix scope + existing-table handling — **CONFIRMED (with defects, below)**

Scratch tests appended to the lane tree's `mod tests`, run, then restored
(`v3-probe3.log`; `SHA_BEFORE == SHA_AFTER ==
3488c565b330dd593e57af7cb6313c987d2289fcfea91dda5bbd1f0562711020`, so the
deletion is proven):

    VPROBE_A names=["id", "data", "properties", "property_kinds"]
    VPROBE_A kinds=[… "properties:OverflowProperties", "property_kinds:OverflowPropertyKinds"]
    VPROBE_B names=["id", "properties"]        # declared `properties` → unchanged
    VPROBE_C names=["id", "property_kinds", "properties", "property_kinds"]   # DUPLICATE

A and B are exactly as claimed. C is a new defect (see DEFECTS).

Mirror-table DDL: `QueryableCache::initialize_schema`
(`crates/holon/src/core/queryable_cache.rs:109-175`) generates the table from
the same `TypeDefinition` via `generate_create_table_sql_with_change_origin`
(`:1267-1304`), which iterates `type_def.fields` — so the two columns do reach
the DDL.

**Existing-table question, answered: it SILENTLY KEEPS THE OLD TABLE.**
`generate_create_table_sql_with_change_origin` emits
`CREATE TABLE IF NOT EXISTS …` (`queryable_cache.rs:1300-1304`) and
`initialize_schema` does no `PRAGMA table_info` / `ALTER TABLE` reconciliation
— the only pre-check is the matview short-circuit at `:124`. So the second boot
neither fails nor migrates: the CREATE is a no-op, the registry now carries
`properties`/`property_kinds` while the table does not, and
`require_engine_stamp_has_a_home` inspects only the *type definition*, so it
passes. The loud boot panic is therefore converted into a **deferred,
write-time** `no such column: properties`. Evidence is a code read, not a
double-boot run — I did not execute the two-boot experiment.

## Claim 4 — Gates — **CONFIRMED**

Run in the lane tree, sidecar sha256 `3488c565…` (unmodified) — `v5-gate.log`,
`v6-gate2.log`, `v7-smoke-rerun.log`.

| gate | verbatim | verdict |
|---|---|---|
| `cargo fmt --all -- --check` | `FMT_OK` (exit 0) | CONFIRMED |
| `cargo nextest run --no-fail-fast -p holon-mcp-client -p holon-loro-wiring -p holon-loro -p holon-frontend -p holon-app` | `Summary [  76.708s] 1492 tests run: 1492 passed (1 slow), 4 skipped` | CONFIRMED — failure set **empty**, trivially ⊆ the allowlist. This is the 5-crate set (1492), a superset of the lane's reported 4-crate 1131. |
| `just keystone-smoke` | run 1: `test result: FAILED. 3 passed; 1 failed`; runs 2 and 3: `test result: ok. 4 passed; 0 failed` | CONFIRMED **with note** — see below |
| bugfunnel | `646 entries, 0 problems` | CONFIRMED |

keystone-smoke note: my first run went red on `inv-drawer-open-matches-ref`
(`drawer block:default-right-sidebar rendered open=true but reference says
open=false`, sole violation, self-reported `NOT proven first-divergent`). That
is a **registered known red** — `docs/Testing/KeystoneKnownReds.md:125`,
`drawer-open-matches-ref`, pre-existing at `6eec7606`, A/B'd by population at
~1/10, with the explicit instruction that a single run is not evidence. Two
subsequent runs were green. Not a lane regression. It is, however, a **different**
rung from the `inv-sql-budget=30/31` one the lane report names, so the lane's
"4/4" is one sample of a flaky gate, not a stable green.

Bugfunnel entry
`docs/Testing/bugfunnel/entries/2026-09-08-sidecar-mirrored-type-has-no-stamp-home-panics-boot.md`
conforms: front-matter `gap: ENVIRONMENT` / `secondary: COVERAGE` /
`status: FIXED`, and both halves are argued (harness existed with the right
oracle but ran in no gate = ENVIRONMENT; composed keystone implements no
`SutAppLifecycle` so `StartApp` is cap-gated out = COVERAGE). The COVERAGE half
I verified only as a reading of the entry's own argument, not by inspecting the
keystone's capability gating.

## Claim 5 — `holon-mcp::describe_ui_deferred` reds are pre-existing — **CONFIRMED**

Run on the `4e2ee368` scratch tree (`v4b-mcp-main.log`), the fix absent:

    Summary [   2.704s] 6 tests run: 4 passed, 2 failed, 0 skipped
    FAIL … unexpanded_live_query_is_marked_unevaluated
    FAIL … expanded_live_query_renders_the_querys_real_rows
      panicked at frontends/mcp/tests/describe_ui_deferred.rs:89:10:
      insert integration_state fixture row: DatabaseError("Failed to execute
      statement: NOT NULL constraint failed: integration_state.display_name")

Same two names, same line, same error, with the lane's change absent.
`frontends/mcp/tests/describe_ui_deferred.rs` is byte-size-identical (18651) in
both trees. Pre-existing, unrelated.

Method note: a bare positional filter (`nextest run -p holon-mcp
describe_ui_deferred`) yields `0 tests run: 0 passed, 84 skipped` — it matches
test names, not binary ids. `-E 'binary(describe_ui_deferred)'` is required.

---

# DEFECTS

**D1 — the fix introduces a NEW boot-failure path (duplicate column).**
`overflow_completed_schema` guards on `properties` alone, but appends the whole
pair. A sidecar whose `schema:` declares `property_kinds` and not `properties`
gets `["id", "property_kinds", "properties", "property_kinds"]` (VPROBE_C,
measured). `generate_create_table_sql_with_change_origin` emits one column per
`type_def.fields` entry, so the mirror-table DDL becomes
`CREATE TABLE … "property_kinds" TEXT, …, "property_kinds" TEXT` — rejected as a
duplicate column name, at boot. `FieldSchema::overflow_pair`'s own doc
(`crates/holon-api/src/entity.rs:508-511`) states "Neither column means anything
without the other, so nothing declares one of them" — the guard does not enforce
what that doc assumes. Before the fix this shape was harmless.

**D2 — the P0 boot panic survives for a plausible sidecar shape.**
`FieldSchema` is `#[serde(default)]` with `value_kind: ColumnValueKind::Declared`
by default (`crates/holon-api/src/entity.rs:405-413, 441-447`). A sidecar that
mirrors a column literally named `properties` (a JSON blob from an external API
is the obvious case) parses to `value_kind: Declared`; VPROBE_B measured exactly
that (`properties:Declared`). `require_engine_stamp_has_a_home` then takes its
`Some(field)` non-overflow arm and returns `Err`, and the boot loop's
`unwrap_or_else` panics — the same whole-app boot crash, same blast radius, for a
sidecar author who never touched Holon internals. The lane report frames this as
deliberate ("the existing check names it precisely"), but the outcome is a panic
before any UI exists, not a disclosure.

**D3 — doc comment stolen from the public API.**
The ~19-line doc block on `EntityConfig::to_type_definition` (covering
`graph_label`, the `primary_key`-is-a-SCALAR warning "Do not read this scalar as
the table key", and the provider/`writes` separation) now sits above the new
private `overflow_completed_schema`; `to_type_definition` is left with **no** doc
comment. `crates/holon-mcp-client/src/mcp_sidecar.rs:559-605`.

**D4 — the guard hardcodes the column name.**
`fields.iter().any(|f| f.name == "properties")` duplicates
`WriteSchema::OVERFLOW_COLUMN`, the constant the check it is satisfying reads
(`crates/holon/src/core/type_declaration.rs:112`). The two can drift silently.

# GAPS

**G1 — no test covers the fix's own branches.**
The updated `to_type_definition_sets_graph_label_and_primary_key` pins only the
append case. Neither the "declares `properties`" branch (D2) nor the
`property_kinds`-only branch (D1) has a test; I had to write throwaway ones. Per
the `holon-feature` contract the behaviour change added a branch with no
covering assertion.

**G2 — the mirror-table reconciliation hole is recorded but unmeasured.**
The lane flags it in the bugfunnel entry and I confirmed the mechanism by code
read (Claim 3). Nobody has run the double boot, and the entry does not record
that the *observable* consequence changes shape — from a loud boot panic to a
silent registry/table divergence surfacing later as a write-time
`no such column: properties`. That is a "silently degrades to look fine" outcome
under the project's error-handling priority order.

**G3 — the ENVIRONMENT gap class rests on an unverified premise.**
The entry attributes the escape to `3f6e985e`'s gate crate set omitting
`holon-integration-tests`. I did not verify that commit's gate invocation. If
the crate was in the gate and the run was skipped for another reason, the class
would change.

**G4 — keystone-smoke is a 1-sample gate over a ~1/10 known flake.**
Both the lane and I hit non-determinism on the first try (different rungs). A
single `just keystone-smoke` is not evidence of green for this lane or any
other; `KeystoneKnownReds.md:125` says so explicitly for this signature.

---

# Rev 2

Workspace not stale (`jj workspace update-stale` → "the working copy is not
stale"). Rev 2 inspected as `jj diff -r @-`: 4 files, +286/-7
(`crates/holon-api/src/entity.rs`, `crates/holon/src/core/sql_operation_provider.rs`,
`crates/holon-mcp-client/src/mcp_sidecar.rs`, the bugfunnel entry).
Sidecar sha256 `fe94422f5de35afe549ea3bf71c45b0185f975e6651f474f800659354857c30f`,
identical before and after my probe (`r2-probe.log`) — restore proven.

## Shape probes — all six as claimed (`r2-probe.log`)

Driven through `McpSidecar::from_yaml`, the validating path:

    R2 KINDS_ONLY  => REFUSED  "…entity 'thing' declares one of `properties` and
                                `property_kinds` without the other…"
    R2 BAG_ONLY    => REFUSED  (same half-pair refusal)
    R2 BOTH        => OK ["id", "properties", "property_kinds"]        # unchanged
    R2 PLAIN_BAG   => REFUSED  "…declares a column `properties`, which is the engine's
                                own overflow column … Declare it
                                `value_kind: overflow_properties` or give the column a
                                different name."
    R2 PLAIN_KINDS => REFUSED  (same, naming `overflow_property_kinds`)
    R2 NONE        => OK ["id", "data", "properties", "property_kinds"]  # pair appended

Every refusal names the entity (`thing`), the column(s), and — for the
wrong-kind shape — the required `value_kind`. No duplicate arises in any shape.

Totality of `to_type_definition` independently probed by BYPASSING `from_yaml`
(constructing `EntityConfig` directly, so no validation runs):

    R2 BYPASS_property_kinds => ["id", "property_kinds", "properties"] dup=false
    R2 BYPASS_properties     => ["id", "properties", "property_kinds"] dup=false

`overflow_completed_schema` now filters `FieldSchema::overflow_pair()` by absent
name, so the duplicate-column DDL is unreachable even when validation is not on
the path. This is the right shape: the invariant is enforced structurally, not
only at the load boundary.

## Per-defect verdicts

| defect | verdict | evidence |
|---|---|---|
| D1 duplicate `property_kinds` column | **CLOSED** | `R2 BYPASS_*` dup=false (structural) + `R2 KINDS_ONLY/BAG_ONLY` refused at load (belt and braces) |
| D2 plain `properties` panics at boot | **CLOSED for the YAML leg, NOT for auto-discovery** — see below | `R2 PLAIN_BAG` / `R2 PLAIN_KINDS` refused with the required kind named |
| D3 doc comment stolen | **CLOSED** | the helper moved BELOW `to_type_definition`; `to_type_definition` keeps its original doc block, extended by three lines; the helper has its own |
| D4 hardcoded name | **CLOSED** | `FieldSchema::OVERFLOW_PROPERTIES` / `OVERFLOW_PROPERTY_KINDS` added in `crates/holon-api/src/entity.rs:510-514`; `overflow_pair` and `WriteSchema::OVERFLOW_COLUMN` (`sql_operation_provider.rs:264`) both read them; `validate_overflow_declarations` reads them too |

Tracked sidecars: `grep -l "name: propert" assets/integrations/*.yaml` over all
six (`claude-history`, `gcal`, `gmail`, `jsonplaceholder`, `shopping`,
`todoist`) matches **nothing**, so the new refusal breaks no shipped asset.
Confirmed.

## Gates

| gate | verbatim | log |
|---|---|---|
| `cargo check --workspace --all-targets` | `Finished \`dev\` profile … in 5m 16s`, zero `error`/`error[` lines | `r2-check.log` |
| `cargo fmt --all -- --check` | `FMT_OK` | `r2-check.log` |
| `boot_suite` | `Summary [  21.382s] 21 tests run: 21 passed (2 leaky), 0 skipped` | `r2-check.log` |
| `-p holon-mcp-client` | `Summary [   7.900s] 365 tests run: 365 passed, 0 skipped` | `r2-check.log` |

The `holon-api` public-const addition is additive, so nothing downstream broke.

# NEW DEFECT — D5: the load-time refusal is not on the only path that sets a schema

`validate_overflow_declarations` runs from `McpSidecar::from_yaml`, which is the
sole production YAML entry (`crates/holon-mcp-client/src/mcp_integration.rs:381`).
But an entity's `schema` is **also written after that point, from the remote MCP
server**, and that write is not validated:

- `crates/holon-mcp-client/src/mcp_integration.rs:641` —
  `existing.schema = meta.fields;` merges an auto-discovered schema into a
  sidecar entity whose YAML declared none.
- `crates/holon-mcp-client/src/mcp_integration.rs:~687` — a newly discovered
  entity is registered with `schema: meta.fields`.

`meta.fields` is built in
`crates/holon-mcp-client/src/mcp_resource_discovery.rs:60-80` with
`FieldSchema::new(name, sql_type)`, i.e. `value_kind: ColumnValueKind::Declared`.

So a remote MCP server whose resource-template metadata advertises a field named
`properties` (or `property_kinds`) produces exactly the shape `R2 PLAIN_BAG`
refuses — reaching `to_type_definition` unvalidated, with kind `Declared`, and
`require_engine_stamp_has_a_home`'s wrong-kind arm then panics the app at boot.
That is the original P0, unreached by the new guard, and its trigger is
*remote-controlled* data rather than a local file. D1 is unaffected (the
structural filter covers this path), and I did not execute this scenario — the
evidence is the three code sites above, not a run.

# Standing gaps, unchanged by rev 2

G2 (existing mirror table never reconciled — `CREATE TABLE IF NOT EXISTS`, no
`PRAGMA table_info`/`ALTER TABLE`, so the second boot silently keeps the old
table and the failure moves to write time), G3 (ENVIRONMENT class rests on the
unverified premise about `3f6e985e`'s gate crate set), G4 (keystone-smoke is a
1-sample gate over a ~1/10 registered flake). G1 is now closed: rev 2 adds four
unit tests covering the branches.

---

# Rev 3 — D5 CLOSED

Workspace not stale. Rev inspected as `jj diff -r @-` at `cf54fe230da0`:
7 files, +623/-120. Probe files restored/deleted, sha256 and existence checked
(`r3-probe2.log` `SHA_BEFORE == SHA_AFTER ==
6b3229ad814c54c9deb727a6f31902d1fecf9bfa40d11af4675821b519ffd8b8`;
`r3-seal.log` `DELETED=YES`).

## 1 + 2 — the discovery path (`r3-probe2.log`, `r3-probe.log`)

Driven through the real parser (`parse_resource_template_meta` on a
`ResourceTemplate`) into `absorb_discovered_entity`:

    R3 JSONSCHEMA_BAG   => REFUSED provider 'rogue-connector': entity 'thing',
        auto-discovered from resource template 'rogue://things', declares a column
        `properties`, … Declare it `value_kind: overflow_properties` or give the
        column a different name.        | entity_schema_empty=true
    R3 JSONSCHEMA_KINDS => REFUSED (…`property_kinds`… `value_kind:
        overflow_property_kinds`)       | inserted=false
    R3 FLAT_KINDS       => REFUSED (flat `schema: {id: string, property_kinds: string}`)
                                        | entity_inserted=false
    R3 CLEAN_MERGE  => ["id:Declared", "data:Declared",
                        "properties:OverflowProperties",
                        "property_kinds:OverflowPropertyKinds"]
    R3 CLEAN_INSERT => ["id", "properties", "property_kinds"]

Every refusal names **provider, resource template, column and the required
`value_kind`** — the four things a reader needs. `MirrorSchema::parse` runs at
`mcp_integration.rs:613`, **before** the `yaml_key` match, so a refusal leaves
the sidecar entity exactly as authored (`entity_schema_empty=true`) and inserts
nothing (`inserted=false`) — verified as state, not just as a returned `Err`.

Boot continues: the caller (`mcp_integration.rs:715-724`) is
`if let Err(err) = absorb_discovered_entity(…) { error!("… the entity is not
registered and does not sync; the rest of the integration continues") }` inside
the template loop. No `?`, no panic. One bad template costs that one entity.

Clean templates keep the completion on both legs — merge into a declared entity
and insert of a brand-new one — with the right `value_kind`s.

## 3 — every schema producer goes through `parse`

`grep -rn "MirrorSchema" crates/ frontends/ --include='*.rs'`, all sites:

- `mcp_sidecar.rs:785` — `config.schema = MirrorSchema::parse("sidecar entity '{entity}'", …)`, called from `from_yaml` via `parse_entity_schemas`
- `mcp_integration.rs:613` — `MirrorSchema::parse(…)`, its result the only value assigned at `:639` (`existing.schema = schema`) and at `:681` (`EntityConfig { schema, … }`)
- `mcp_integration.rs:1731` — `MirrorSchema::default()` (empty; `to_type_definition` returns `None` on empty, test helper)
- `fake_mcp_module.rs:228`, `pbt_mcp_fake.rs:219` — test support, both `parse`

`existing.schema = meta.fields` no longer type-checks: the field is
`MirrorSchema` and `meta.fields` is `Vec<FieldSchema>`. The old assignment is
gone from the diff.

Sealing, measured — a scratch test in `crates/holon-mcp-client/tests/`
(outside the defining module), then deleted (`r3-seal.log`):

    error[E0423]: cannot initialize a tuple struct which contains private fields
      --> crates/holon-mcp-client/tests/vseal_probe.rs:6:16

So `MirrorSchema(vec![…])` is unreachable from outside `mcp_sidecar`. The
`#[derive(Deserialize)] #[serde(transparent)]` bypass **is** real (the same probe's
`serde_json::from_str::<MirrorSchema>` line drew no error) — the lane declares it
as the wire form, and no production path uses it: the only production
deserialization is `McpSidecar::from_yaml` (`mcp_integration.rs:381`), which
runs `parse_entity_schemas` on every entity. `IntegrationFileConfig` carries
`sidecar_yaml` as a `String`, not a nested `McpSidecar`. Every other direct
`serde_yaml::from_str::<McpSidecar>` is in `tests/` or `#[cfg(test)]`.

## 4 — Gates (`r3-check.log`, `r3-check2.log`, `r3-boot-repeat.log`)

| gate | verbatim | verdict |
|---|---|---|
| `cargo check --workspace --all-targets` | `Finished \`dev\` profile … in 2m 23s`, zero `error`/`error[` lines | CLEAN |
| `cargo fmt --all -- --check` | `FMT_OK` | CLEAN |
| `-p holon-mcp-client -p holon-api -p holon-mcp` | `Summary [  12.009s] 972 tests run: 970 passed, 2 failed` | the 2 are `holon-mcp::describe_ui_deferred::{expanded_live_query_renders_the_querys_real_rows, unexpanded_live_query_is_marked_unevaluated}` — the reds I already confirmed PRE-EXISTING on main `4e2ee368` (Claim 5). Not a rev-3 regression. |
| `boot_suite` | run A `21 tests run: 21 passed`; run B `20 passed, 1 failed` | see flake note |
| `just keystone-smoke` ×2 | `test result: ok. 4 passed; 0 failed` ×2 | GREEN |

boot_suite flake note: run B failed
`junction_survives_reboot_repro::loro_written_edge_fields_survive_reboot_over_existing_db`
at **18.553s** (its normal time is ~2.9s), while a probe run and the smoke were
sharing the machine. Re-run in isolation **3/3 PASS** at 2.9–3.5s
(`r3-boot-repeat.log`). Load-sensitive, and its subject (Loro edge fields across
a reboot) has nothing to do with sidecar schemas. Not a lane regression.

## Verdict

**D5 CLOSED.** The remote-controlled boot panic is unreachable: the untrusted
schema is parsed before it can be stored, the refusal is disclosed rather than
swallowed, the integration degrades by one entity instead of crashing, and the
type that would have carried the defect can no longer be constructed outside
its module. D1–D4 remain closed (Rev 2 evidence unchanged by this rev).

# Residual notes (not defects in this rev)

**N1 — a resource-template form is silently dropped by the parser, not refused.**
A flat `schema:` map naming **both** `properties` and `property_kinds`
(`R3 FLAT_BOTH_PLAIN`) makes `parse_resource_template_meta` return `None`, so
the loop `continue`s and the entity is skipped with no log line at all — the
flat/JSON-Schema branch discrimination keys on a `properties` member. The
outcome is safe (nothing reaches the registry) but undisclosed, which sits at
priority 4 of the project's error-handling order rather than 2 or 3. It
pre-dates this rev.

**N2 — `junction_survives_reboot_repro::loro_written_edge_fields_survive_reboot_over_existing_db`
is load-sensitive and is not in `docs/Testing/KeystoneKnownReds.md`.**
Observed 1/4 under concurrency in this session. Unregistered, so a future lane
will have to re-characterise it from scratch.

**G2 unchanged** — `QueryableCache::initialize_schema` still never reconciles an
existing mirror table, so a vault whose table predates the fix keeps a table
without the pair while the registry has it.
