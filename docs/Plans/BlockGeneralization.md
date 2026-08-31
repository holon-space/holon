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
- **Still open.** The capability check itself (below). `declare_type` takes no
  home, so refusing a `computed_persisted` declaration against a
  `string_only`/`none` home first needs a ruling on which home a type
  declaration binds to.
- **Red-first surface.** A test declaring such a type and expecting a named
  refusal; expected red = `declare_type` accepts it.
- **Blocked by.** Nothing — I3-1 has landed (`8d07c182`) and supplies the
  tier answer via `DerivedFieldPlan`.
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
