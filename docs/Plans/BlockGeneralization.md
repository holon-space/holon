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

Status verified against `main` on 2026-08-31.

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

---

## 3. Open, in dependency order

### NV-1 — kind fidelity via `property_kinds` (D29.a)
- **Scope.** A per-block `property_kinds` column carries `DateTime` / `Json`
  kinds for the schemaless properties bag. The `Value` serde-representation
  fix (Loro leg S2, derived sidecar S3) is a **separately scheduled** half.
- **Anchors.** `property_kinds` does not exist in the tree (zero hits).
  Loss cause: `crates/holon/src/core/sql_operation_provider.rs` re-types
  `Value::DateTime` to string and parses `Value::Json` into the blob. Claim
  site: `assets/default/capability/holon-native.yaml` (`types:` omits
  `date_time`).
- **Red-first surface.** Flip the two yaml lines; `certify_property_values`
  (`crates/holon-capability/src/certify.rs`) goes red with named
  `Violation{clause: TypeDeclared(DateTime|Json)}`. Plus a keystone
  extension drawing `DateTime`/`Json` payloads through `SetField`.
- **Blocked by.** Nothing — nv2 has landed (`25e384ee`). **Caveat that
  gates the yaml claim:** `Value` is `#[serde(untagged)]`
  (`crates/holon-pattern/src/value.rs:54`), so Loro's on-disk form destroys
  the kind (`crates/holon-loro/src/loro_backend.rs`). `property_kinds`
  alone buys fidelity on the SQL-authority leg only. **The profile must not
  claim `date_time`/`json` are representable until both halves are in.**
- Plan: `~/.claude/plans/d29-property-kinds-plan.md`.

### I3-1 — route the type registry through `Computation`
- **Scope.** `FieldLifetime::Computed` carries a `Computation` instead of a
  bare Rhai `CompiledExpr`, with `Computation::Script` as the disclosed
  fallback. Classify tiers with the existing `DerivedFieldPlan`
  (`sql_planted` ⇒ `computed_persisted`, `stage_evaluated` ⇒
  `computed_live`) — do not write a second classifier.
- **Anchors.** `crates/holon-api/src/entity.rs:115-119` —
  `FieldLifetime::Computed { expr: holon_expr::CompiledExpr }` is Rhai-only;
  registry compiles yaml `computed:` straight to Rhai in
  `crates/holon-profiles/src/type_registry.rs`.
- **Ruling executed.** BG-4 (a) / D28.a.
- **Red-first surface.** `crates/holon-integration-tests/src/pbt/typed_entity_schemas.rs:22-29`
  — `TypedEntitySchema` has `id_column` + `value_columns` only. Add
  `computed_columns`, evaluate the field's `Computation` in the oracle
  (`pbt/ref_caps/typed_entities.rs`), compare it in
  `pbt/composed/invariants/typed_matview_matches_ref.rs`. Expected red: the
  SUT matview has no such column.
- **Blocked by.** Two prerequisites: (1) declared column types must be
  reachable at parse time — typing `+` as numeric `Arith` vs string
  `Concat` cannot be guessed without them; (2) the S3 sidecar loss —
  `crates/holon-turso/src/derived_reconciler.rs:149` stores computed values
  via `serde_json::to_string` over the untagged enum, so a
  `computed_persisted` `DateTime`/`Json` is lossy into its own storage.
  Fix it or declare the loss explicitly; do not build the tier on it.

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
- **Red-first surface.** A test declaring such a type and expecting a named
  refusal; expected red = `declare_type` accepts it.
- **Blocked by.** **I3-1** — the tier answer comes from `DerivedFieldPlan`.
  Doing I3-2 first means inventing a second, weaker classifier.
- **Done when.** All four `CV-E lands in 2b.6` citations are gone.

### 2b.5 — refuse content at the write boundary on profile clauses
- **Scope.** Turn certified profile clauses into write-boundary refusals.
- **Anchors.** `crates/holon-capability/src/certify.rs:1931` and
  `crates/holon-org-format/tests/profile_certification.rs:401` both state
  the law: 2b.5 may not refuse content on a clause nobody drove. The
  UNKNOWN-resolution obligation is discharged by
  `docs/Testing/capability-ledger/entries/2026-08-22-org-block-constructs-promoted-by-measurement.md`.
- **Blocked by.** Nothing structural; independent of the I3 chain.

### 2b.6 — see **I3-2** (same item).

### BG-5 — delete `graph_eav`
- **Scope.** Delete the unused EAV storage module and its SQL schema; remove
  the DI registration and the Schema.md section. GQL is unaffected.
- **Anchors.** `crates/holon-turso/src/schema_modules.rs:1264-1283`
  (`GraphEavSchemaModule`, `Resource::schema("graph_eav")`),
  `crates/holon/src/di/schema_providers.rs:134`,
  `crates/holon-turso/sql/schema/graph_eav.sql`,
  `docs/Architecture/Schema.md:175-194`.
- **Ruling executed.** BG-5 (a).
- **Red-first surface.** None needed — this is a deletion of code with zero
  readers. The gate is that boot and the keystone stay green with the
  module and its `DbReady` marker gone.
- **Blocked by.** Nothing. Independent of everything else in this program.

### Inc 4 — Loro format adapter
- **Scope.** Layout inferred from `crdt_backing` metadata (hierarchical →
  tree, flat → map-per-entity; text-fidelity fields → `LoroText`, scalars →
  LWW); a generated per-type projector; adapter capability declarations;
  the OQ-2 authority rule. BG-2's provisional status is re-tested here.
- **Blocked by.** NV-1's S2 half touches the same on-disk representation —
  settle the `#[serde(untagged)]` kind loss before generalizing the leg.

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

## 4. Lanes running now (2026-08-31)

| Lane | Item | State |
|---|---|---|
| `.claude/worktrees/bg-i3-1` | I3-1 | in flight, not landed (working rev empty at check) |
| `.claude/worktrees/bg-eav` | BG-5 | in flight, not landed (working rev empty at check) |

---

## 5. How to update this doc

The landing orchestrator moves a row from §3 to §2 with the commit sha, the
date, and the test or invariant that locks it — at the moment the increment
lands on `main`, not when its PR opens. Nothing here is a total; nothing
here is mirrored anywhere else that would need a second edit. If an
increment's scope changes, edit its §3 row rather than appending a note:
this doc states the current plan, not its history.
