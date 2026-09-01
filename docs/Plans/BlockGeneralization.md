# Block Generalization — program status

**Goal.** Block is one datatype among many, not a privileged one. The
`TypeDefinition` (the *datatype*) is the primary object; Turso, Loro, org,
CSV/YAML are each a thin **serialization adapter** derived from it. A new
datatype must cost a runtime declaration and a profile — not code.

**Design law (per-format, never per-type).** Machinery may exist per FORMAT
(few, thin adapters). Per-type artifacts — tables, matviews, PRQL stdlib
entries, Loro layouts, projectors — are legal only as *generated outputs* of
`adapter(TypeDefinition)`, regenerable and deletable. Test: delete the
artifact, re-register the type, get it back byte-identical.

Design record: `~/.claude/plans/block-generalization-design-2026-08-21.md`
(architecture, T1/T2/T3 candidates, rulings) and
`~/.claude/plans/block-generalization-next-plan-2026-08-26.md` (premise
check with file:line evidence, NV/I3 step plan).

Status verified against `main` on 2026-09-01.

---

## 1. Rulings

| ID | Ruling | Consequence | Date |
|---|---|---|---|
| **BG-1** | REFRAMED — no substrate is preferred; the **datatype is primary**, every store is a serialization of it | "typed tables" survive only as what the Turso adapter emits; every layer derives from `TypeDefinition` | 2026-08-21 |
| **BG-2** | (c) per-format file projection, **PROVISIONAL** | Nothing hardcoded. Machinery per FORMAT is allowed; per TYPE never. Re-tested at Inc 4 | 2026-08-21 |
| **BG-3** | (a) hierarchy is an **opt-in capability** | `Entity` / `HierarchicalEntity` split; `BlockOperations` becomes generic over it (Inc 6) | 2026-08-21 |
| **BG-4** | (a) computed tiers named `computed_persisted` / `computed_live`; ONE expression language dual-compiled to SQL and to a runtime evaluator | `Computation` (`crates/holon-api/src/computation.rs`) IS that language; Rhai stays the authoring surface, parsed into it | 2026-08-21 |
| **BG-5** | (a) **delete** the unused `graph_eav` EAV storage module + schema | GQL is unaffected — it compiles against typed tables via `GraphSchemaRegistry` | 2026-08-21 |
| **BG-6** | (a) **defer** sharing-unit generalization; lift `ContainerId → EntityUri` opportunistically only | No sharing work in this program until a non-block sharing demand exists | 2026-08-21 |

Follow-on rulings that bind open work:

| ID | Ruling | Date |
|---|---|---|
| **OQ-1** | (b) Turso-less is a CONSTRAINED PROFILE (CRUD + `computed_live` + simple filters); IVM-grade reactivity requires Turso | 2026-08-21 |
| **OQ-2** | (a) fidelity-ordered authority matrix; secondary durable formats are base-diffed REPLICAS, consolidator sole authority | 2026-08-21 |
| **OQ-3** | Resolved in substance: `Computation` is the language; out-of-subset sources land in `Script` = runtime-only, legal ONLY for `computed_live` | 2026-08-21 |
| **CV-A..E** | Capability profiles are DATA; the durable HOME's profile binds the entity; re-homing is the first-class escape hatch | 2026-08-22 |
| **D27.b** | Explicit null: add `Value::Removed`; `Value::Null` becomes a real value | 2026-08-26 |
| **D28.a** | Extend the `Computation` language; `person_profile.display_name` stays the acceptance case | 2026-08-26 |
| **D29.a** | Kind fidelity via **schema+data** (a per-block `property_kinds` column), not an in-band envelope | 2026-08-26 |

---

## 2. Landed

| Increment | What landed | Commit | Date | Locked by |
|---|---|---|---|---|
| **Inc 1** | The Turso adapter derives raw table + matview + PRQL access from a `TypeDefinition`; `register()` returns an artifact inventory, `teardown()` disposes; person migrated free-standing; keystone gains the datatype axis | `a7b391b9` | 2026-08-21 | regeneration idempotence (register→teardown→register byte-identical); `pbt/composed/invariants/typed_matview_matches_ref.rs`; `pbt/transitions/create_typed_entity.rs` |
| **Inc 2** | **Generic write authority** — a typed entity's writes route by its own `TypeDefinition`, never by a block default; `declare_type` = adapter → registry → authority; `OperationDispatcher` gains runtime `register_provider` | `dc8eaa95` | 2026-08-22 | keystone SUT dispatches `create_typed_entity` through the real `OperationDispatcher` (red-first: "No provider registered for entity person") |
| **2b.1** | `holon-capability` crate: certifier with teeth, `ProfileRevision` = content hash, org profile certified | `7303bcd5` | 2026-08-22 | `crates/holon-org-format/tests/profile_certification.rs` |
| **2b.2** | Capability vocabulary COMPLETE; coverage law (`enforced_by`, driven-at-layer-or-marked) is a gate | `b329e689` | 2026-08-22 | the coverage gate itself |
| **2b.3** | Vocabulary answers by MEASUREMENT; `holon-native` certified against the production SqlOnly wiring; `CapabilityProfile::diff()` total over `ClauseId` | `3e3ca491` | 2026-08-22 | `crates/holon/tests/capability_certification.rs` |
| **2b.3/I1** | `logseq-db` is the third certified profile | `05552b8d` | 2026-08-22 | `crates/holon-logseq-db/profile.yaml` certification |
| **2b.4** | Re-homing is a real, driven operation (`rehome_entity`) | `843c5ce0` | 2026-08-23 | `pbt/composed/invariants/home_profile_matches_derived.rs` (CV-D stage A) |
| **NV-0** | ONE JSON→`Value` parse at the properties boundary; five ad-hoc converters deleted | `7105bfcf` | 2026-08-26 | `crates/holon/src/core/json_value_parse_differential_test.rs`; mutation-proven `stored_containers_keep_their_kind_through_the_merge_leg` |
| **I3-0** | `Computation` extended to `person_profile.display_name` (D28.a): `Concat`, short-circuit `And`, `IsDefined`, string + unit literals, Eq/Ne-against-Null lowering; `FieldIdent` newtype closes the DDL injection surface | `70ae275a` | 2026-08-26 | `crates/holon-api/tests/derived_field_dual_eval_pbt.rs` (eval-vs-SQL differential oracle); `person_profile_display_name_seat_a.rs` |
| **NV-2** | Removal intent is an explicit `Value::Removed`, not a null-string sentinel (D27.b) | `25e384ee` | 2026-08-26 | `Value::Removed` wire shape pinned in `crates/holon-pattern/src/value.rs` |
| **BG-5** | `graph_eav` module, DDL, DI registration and Schema.md section deleted; GQL `validate_typed_shape` refuses unknown labels, untyped edges and unlabelled nodes at compile time, default resolvers pointing at a sentinel table so residual paths fail loud and identically on fresh and legacy files | `0a10b5a5` | 2026-08-31 | fresh-boot inventory over all 14 table names in `crates/holon-app/tests/fresh_db_boot_seed_smoke.rs` |
| **I3-1** | `ComputedSpec::parse` is the single door: typed parse over `FieldIdent`, tier enforcement in the constructor, registry cross-check that declared types match the owning `TypeDefinition`'s columns; `computed_persisted` lowers through the ONE formatter (`PlantedColumn::select_expr`, duplicate unvalidated sink deleted), `computed_live` evals without Turso behind a boot-once disclosed WARN for out-of-subset fields | `8d07c182` | 2026-09-01 | `crates/holon/tests/computed_tier_dual_path.rs` (dual-path agreement, proptest); `crates/holon-profiles/tests/computed_tier_declaration.rs`; `crates/holon-api/tests/typed_computed_field_declaration.rs` |
| **NV-1** | Per-block `property_kinds` column records the kind for keys whose JSON spelling is ambiguous; ONE writer (`bag_and_kinds_set_clause`) owns bag and kinds in a single UPDATE across create, update and `set_field` — `set_field` was a third bag writer that bricked rows — with a workspace-wide tripwire keeping it the only `json_set`/`json_remove` site; reads parse at one boundary and a kind/value disagreement is a loud query error; additive sniffed ALTER migration, idempotent, downgrade inert. SQL leg only — the profile claims `date_time`/`json` with the Loro leg disclosed pending S2 | `e19a726f` | 2026-09-01 | `crates/holon/src/core/set_field_property_kinds_test.rs`; `crates/holon-turso/tests/property_kinds_migration.rs`; `crates/holon/tests/capability_certification.rs` (certifier drives the types clause on every live route and PINS the two `set_field` routes as expected-undriven) |

---

## 3. Open, in dependency order

### NV-1/S2–S3 — kind fidelity on the Loro leg and the derived sidecar
- **Scope.** The `Value` serde-representation half that `property_kinds` did
  not buy: S2 the Loro on-disk form, S3 the derived sidecar.
- **Anchors.** `Value` is `#[serde(untagged)]`
  (`crates/holon-pattern/src/value.rs:54`), so Loro's on-disk form destroys
  the kind (`crates/holon-loro/src/loro_backend.rs`). S3:
  `crates/holon-turso/src/derived_reconciler.rs:149` stores computed values
  via `serde_json::to_string` over the untagged enum, so a
  `computed_persisted` `DateTime`/`Json` is lossy into its own storage.
- **Red-first surface.** Extend the certifier's types clause to the Loro
  route; expected red = the disclosed-pending marker becomes a named
  `Violation{clause: TypeDeclared(DateTime|Json)}`.
- **Blocked by.** Nothing. **The profile's `date_time`/`json` claim stays
  disclosed as SQL-leg-only until S2 lands.**
- Plan: `~/.claude/plans/d29-property-kinds-plan.md`.

### I3-2 — enforce the capability clause (2b.6 / CV-E)
- **Scope.** Declaration-time refusal when a type declares a
  `computed_persisted` field against a home whose profile says
  `string_only` or `none`.
- **Anchors.** `crates/holon/src/core/type_declaration.rs` has no capability
  check; `holon_capability::supports` already carries the
  `Feature::ComputedPersisted(kind)` arm. Four pending citations:
  `assets/default/capability/holon-native.yaml:141`,
  `crates/holon-org-format/profile.yaml:127`,
  `crates/holon-logseq-db/profile.yaml:183`,
  `crates/holon-capability/src/fixture.rs:146`.
- **Production wiring: DONE.** `TursoAdapter::matview_select` consumes
  `TypeDefinition::persisted_derived_plan()`, so every registered type's read
  matview carries its `computed_persisted` columns. Pinned end to end by
  `crates/holon-app/tests/computed_persisted_boot_column.rs` (a real boot
  reads `person.display_name` off the matview) and by the keystone, whose
  datatype axis now compares computed columns against an oracle that
  EVALUATES the same `Computation`.
  - The block-scoped sidecar reconciler
    (`spawn_derived_field_reconciler`) stays test-only DELIBERATELY: it
    maintains `block_derived` for seat-B fields, and no declaration in the
    tree routes a `computed_persisted` field to seat B — the tier refuses a
    computation that does not lower to SQL, so `matview_select` asserts
    `stage_evaluated` is empty. Spawning it would be a worker over an empty
    field set. It becomes production wiring when a declaration surface for
    block-scoped derived fields exists.
- **LANDED.** Ruling D54.a binds a declaration to a home: types carry a
  type-level `home:` (`TypeDefinition::home`) with an optional per-field
  `fields[].home` override, both parsed into `holon_api::HomeProfileId`.
  - **The seat is `crates/holon-app/src/type_admission.rs`, NOT `declare_type`.**
    `holon` is a `PROFILED_FORMAT_CRATE`
    (`crates/holon-architecture-tests/tests/architecture_rules.rs:330`) and may
    not link `holon-capability` in its production graph, because a format that
    read its own profile would make its own certification circular. Profiles
    DESCRIBE formats; the layer that COMPOSES formats and profiles decides
    admission. So enforcement (holon-app) and certification (format vs profile)
    stay two independent measurements, and admission against the default
    `holon-native` home is not self-confirming. `declare_type` is unchanged in
    signature and crate deps. Like `move_block` vs `MoveGuard`, the guarded call
    is still reachable: the direct `declare_type` callers that remain are its
    own unit tests.
  - **The kind checked is the kind the COMPUTATION PRODUCES**, not the column's
    declared SQL type. `Computation::result_kind` infers it over the
    SQL-plantable AST, and `TypeRegistry` now derives a fresh computed column's
    `sql_type` from that same answer — the old hard-coded `"TEXT"` made every
    computed field look String-kinded and would have answered `string_only`
    with a "yes" it had not earned. An uninferable kind is a loud error for
    `computed_persisted`; `computed_live` keeps neutral TEXT, since it plants no
    column and every Rhai-only field is uninferable by construction.
  - A `computed_persisted` field whose home nobody named is REFUSED — a silent
    default would make the check vacuous. An unknown `home:` is refused for any
    type, computed fields or not, so a typo fails the day it is authored.
  - **BOTH doors are guarded, and the second one is the important one.** A type
    becomes real two ways: the `declare_type` op, and REGISTRY SEEDING at boot
    (`create_default_registry`, `holon_kitchen::register_kitchen_types`, an MCP
    sidecar), which reaches the same end state — registry entry, Turso
    artifacts, write authority — without touching the op. Guarding only the op
    left every bundled type unchecked, and `person.display_name` (the tree's one
    `computed_persisted` field) shipped with no home at all.
    `type_admission::sweep_registry` runs `admits()` over the WHOLE registry and
    `holon_app::new_from_config_with_di` calls it, refusing startup loudly on any
    offender. Sweeping the REGISTRY rather than a list of seeding call sites is
    deliberate: a future door is covered by construction, because every seeder
    shares the one registry. `assets/default/types/person.yaml` now declares
    `home: holon-native`.
    **Ordering:** the sweep runs before the session and engine handles reach any
    caller, and those handles are the only route to dispatching a write, so no
    caller-served write can precede it. Write AUTHORITIES are already registered
    at that point (`FreeStandingTypeViews` derives them during engine
    construction), which is why a refusal aborts startup rather than unwinding
    them — there is no undeclare.
- **The rehome-time re-check (D54.a's second half) is implemented at LIBRARY
  LEVEL ONLY and has NO production driver.**
  `check_computed_persisted(.., HomeSeat::Destination(id))` applies the
  destination to every field and deliberately ignores field/type homes, so a
  `fields[].home` can never exempt a field from a move's check. Nothing calls it
  in production, and that is measured, not assumed: `RehomeTarget` accepts only
  `holon-native` (`crates/holon-app/src/rehome_entity.rs:53`), which is
  `full_algebra`; `MoveGuard` returns `Confirm` for anything that is not
  `block` + `move_block` (`crates/holon-app/src/move_guard.rs:208`); and `block`
  declares no `computed_persisted` field — repo-wide, `tier: computed_persisted`
  appears only in `assets/default/types/person_profile.yaml:5`. So no op today
  can move a `computed_persisted`-bearing entity into a lossy home. **Wiring it
  belongs to whichever increment introduces an entity-level home-changing op**;
  wiring it now would add a branch that short-circuits on every reachable input.
- **Coverage.** `crates/holon-app/tests/type_admission_cve.rs` drives the seat.
  The discriminating shape is a COMPARISON (`a == b` → Boolean), which
  `string_only` refuses; a concatenation — the only shape the keystone's
  datatype axis draws — infers to Text and is offered by every home, so it
  cannot tell an enforced check from an absent one. A keystone rung is feasible
  on the existing `datatype` axis but needs a Boolean draw, a drawn home, and an
  outcome return on `SutTypedEntity::declare_typed_schema` (which panics on a
  declare error today); that is a follow-up increment, not this item.
- **Type onboarding is a PN action (ADR 0024, ruling D57) — THIN version
  landed.** The seat is a guard/body pair (`admits` / `declare_type_admitted`)
  and registers a `declare_type` `OperationDescriptor` under entity `type`,
  `TargetScope::Global`, no `id_column` — reachable through the existing
  generic surface (`list_operations` / `execute_operation`, MCP included) with
  no bespoke tool. It declares itself `UndoAction::DeclaredIrreversible`:
  declaration is one-way, so there is no inverse to offer. Entity `type` is a
  name no table backs, which the dispatcher already supports — `navigation` and
  `identity` are existing precedents. NOT the wildcard `*`: that arm broadcasts
  to every provider and its ADR 0031 guard argument holds only for ops taking
  no parameters. Bundled types do NOT come through this op — see the boot
  sweep above.
- **Deferred follow-ups.**
  - Boot-path unification: bundled types declared through the PN rather than by
    direct call (bootstrap staging of a self-registering PN).
  - An inverse/undo design for `declare_type` (today: declared irreversible).
  - An entity-level home-changing op — the production driver
    `HomeSeat::Destination` currently lacks.
  - DDL-transaction hardening for runtime declarations: `declare_type` mutates
    the registry after the SQL artifacts exist, so a step-3 failure is
    unrecoverable for that name.
  - A keystone rung for CV-E: needs a Boolean-producing draw, a drawn home, and
    an outcome return on `SutTypedEntity::declare_typed_schema`.
  - **BootLadder debt:** the no-Turso boot path
    (`holon-app/src/no_turso.rs:79` → `loro_ui_watcher.rs:68`) builds its OWN
    `create_default_registry()` and is not swept. Out of scope for CV-E today —
    it derives render profiles for a session with no Turso, so there is no
    planted column for a `computed_persisted` field to be lost from, and it
    parses the same bundled yaml that every Turso boot sweeps. It joins the
    other steps that path owes (`docs/Plans/BootLadder-2026-07-18.md`) rather
    than getting one shared-wiring step while missing the rest.
- **Blocked by.** Nothing — I3-1 has landed (`8d07c182`) and supplies the
  tier answer via `DerivedFieldPlan`.
- **Done when.** All four deferral citations in the profile yamls (and the
  fixture) are gone — the marker they carried is now absent from the tree. ✅
  They cite `crates/holon-app/src/type_admission.rs`.

### 2b.5 — refuse content at the write boundary on profile clauses
- **Scope.** Turn certified profile clauses into write-boundary refusals.
- **Anchors.** `crates/holon-capability/src/certify.rs:1931` and
  `crates/holon-org-format/tests/profile_certification.rs:401` both state
  the law: 2b.5 may not refuse content on a clause nobody drove. The
  UNKNOWN-resolution obligation is discharged by
  `docs/Testing/capability-ledger/entries/2026-08-22-org-block-constructs-promoted-by-measurement.md`.
- **Blocked by.** Nothing structural; independent of the I3 chain.

### 2b.6 — see **I3-2** (same item).

### Inc 4 — Loro format adapter
- **Scope.** Layout inferred from `crdt_backing` metadata (hierarchical →
  tree, flat → map-per-entity; text-fidelity fields → `LoroText`, scalars →
  LWW); a generated per-type projector; adapter capability declarations;
  the OQ-2 authority rule. BG-2's provisional status is re-tested here.
- **Blocked by.** NV-1/S2 touches the same on-disk representation — settle
  the `#[serde(untagged)]` kind loss before generalizing the leg.

### Inc 5 — first non-org file adapter (CSV or YAML)
- **Scope.** Proves the per-format rule end to end. Re-opens C7:
  `FileFormatAdapter` / `FileFormatParseResult`
  (`crates/holon-core/src/file_format.rs`) must become type-generic, with
  org remaining the Block-only instance.
- **Blocked by.** Inc 4 (an adapter needs a durable-format contract to
  declare against).

### Inc 6 — trait decomposition + Block as an instance
- **Scope.** Split `HierarchicalEntity` out of `BlockEntity`
  (`crates/holon-core/src/traits.rs`); make `BlockOperations` generic over
  it; derive the intent vocabulary from `TypeDefinition`. Then dissolve the
  hand-written Block Turso serialization: `crates/holon/src/di/schema_providers.rs:455-466`
  excludes `block` by name from `is_free_standing` because
  `CoreSchemaModule` + `BlockMatviewSchemaModule` still hand-write it — the
  comment states the literal dissolves once block becomes an adapter
  instance. `SqlBlockOperations` remains the block-only write path
  (`crates/holon-loro-wiring/src/event_infra_module.rs:110-197`).
- **Ruling executed.** BG-3 (a), and BG-1's K6 (Block must BECOME an
  instance, not survive beside the generic machinery).
- **Blocked by.** Inc 4. Largest mechanical surface in the program;
  delegable once the semantics above are fixed.

### Sharing (BG-6) — deferred
No work until a non-block sharing demand exists. `ContainerId → EntityUri`
is lifted opportunistically only.

---

## 4. Lanes running now (2026-09-01)

No lane in this program is in flight; BG-5, I3-1 and NV-1 have all landed on
`main`. The only live lane in the tree, `.claude/worktrees/integ-views4`, is
unrelated to Block Generalization.

---

## 5. How to update this doc

The landing orchestrator moves a row from §3 to §2 with the commit sha, the
date, and the test or invariant that locks it — at the moment the increment
lands on `main`, not when its PR opens. Nothing here is a total; nothing
here is mirrored anywhere else that would need a second edit. If an
increment's scope changes, edit its §3 row rather than appending a note:
this doc states the current plan, not its history.
