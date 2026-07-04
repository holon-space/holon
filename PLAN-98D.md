# PLAN-98D — entity-scheme registration vs. boot org scan (ruling D)

Base: `961b2598`. Workspace `agent-a5d080b66a209b4b9`. All file:line as of that rev.
**Planning artifact — nothing implemented yet.**

---

## 0. One escalation the orchestrator must rule on before Increment 3

The ruling's step 4 was *"registration-triggered idempotent `block_links` repopulation"*.
Reconnaissance says that step can be **dissolved entirely**, and that dissolving it is
what D actually asks for.

**Why.** `derive_block_links` (`crates/holon-api/src/inline_mark.rs:362-390`) already emits
a junction row for `EntityRef::Internal { id }`, deriving `kind` from `id.scheme()`
(`tag`/`block`/else `entity`) and setting `resolved = Some(id)`. It skips only `External`
and `UnknownScheme`. `block_links` is documented as a **soft-target** junction —
no FK on `target`/`resolved_id`, dangling is representable
(`crates/holon-turso/sql/schema/block_links.sql:5-14`). The `backlinks` matview is
`block_links ⋈ block_raw ON b.id = bl.source_block_id WHERE bl.resolved_id IS NOT NULL`
(`crates/holon-turso/src/schema_modules.rs:1099-1112`) — it joins the **source** block, never
the target. So a junction row whose `resolved_id` names an entity that does not exist yet
is (a) already a supported state today (a stale `[[block:uuid]]` produces exactly it), and
(b) **self-healing**: the moment the scheme registers and the entity rows appear, the
`WHERE target_id = <uri>` backlink query finds the row that was written at ingest time.

Therefore: if the *persisted mark* stops encoding the registration verdict, the junction is
derived from a registry-independent fact and **no sweep, no registry broadcast, and no new
write path are needed at all**. That removes the ruling's single most dangerous piece — the
"new write path that mutates state without a file change" the brief itself flagged as the
echo-suppression hazard (brief §2 option B).

**Corollary — the enum merge.** `EntityRef::Internal { id: EntityUri }` is *already* the
neutral structural variant the ruling asks for: an `EntityUri` is exactly "scheme + opaque
path", which is the parse fact. `UnknownScheme { uri: String }` is the same fact plus a
verdict. So D step 1 is best realized as a **merge**, not a rename-in-place:

```rust
/// Scheme-shaped `[[…]]` target (`[[block:uuid]]`, `[[tag:x]]`, `[[cc-session:8f21]]`,
/// `[[Areas:Work]]`). The scheme SHAPE is a parse fact; whether any entity claims the
/// scheme is answered LIVE by the classifier at read time and is never stored here.
Scheme { uri: EntityUri },
```

`EntityRef::UnknownScheme` and `EntityRef::Internal` both disappear into it.
Wire compatibility: `#[serde(rename = "scheme", alias = "internal", alias = "unknown_scheme")]`
on the variant, `#[serde(alias = "id")]` on the `uri` field; the Loro backend's hand-written
tag reader accepts all three spellings and writes `"scheme"`.

**Decision requested (pick one, I will not guess):**

- **D-min (recommended)** — merge `Internal` + `UnknownScheme` → `Scheme { uri: EntityUri }`;
  `derive_block_links` stays a *pure* function (no classifier, no plumbing); live
  classification only in the GPUI decoration + click paths; **ruling step 4 dropped as
  unnecessary**. Blast radius ≈ 38 `EntityRef::Internal` sites (mechanical) + 12 `UnknownScheme`
  sites. Zero new async machinery, zero new write path.
- **D-lit (literal ruling)** — keep two variants (`Scheme { uri }` neutral + `Internal`),
  give `derive_block_links` a `&LinkTargetClassifier`, and plumb a classifier into
  `SqlOperationProvider` (struct at `crates/holon/src/core/sql_operation_provider.rs:240-251`
  — **currently holds nothing registry-like**, so all 12 prod ctor call sites +
  ~40 test ctor sites must change: `turso_seams.rs:761,823,983`,
  `event_infra_module.rs:126,180`, `loro_module.rs:166,404`, `sql_block_operations.rs:1203`,
  `operation_engine.rs:1917`, `backend_engine.rs:1346,1440`, `action_watcher.rs:364`,
  `holon_rule_watcher.rs:610`, `frontends/holon-worker/src/lib.rs:315`), **plus** a
  `TypeRegistry` broadcast + a sweep task (ruling step 4).
- **D-min-conservative** — D-min but keep the variant *named* `Internal` (0 renames, ~12 sites
  total). Cheapest, but leaves a variant named "Internal" holding `[[Areas:Work]]`, which
  mis-steers the next agent (CLAUDE.md: the code is the strongest signal).

Everything below is written for **D-min**; §7 lists the deltas if D-lit is chosen.

---

## 1. Red-first PBT design (the contract)

The defect is *ordering*: blocks carrying scheme-shaped links are ingested **before** the
scheme registers. Two layers get teeth, because the observable splits across the headless/
windowed boundary — `link_decoration` is `pub(crate)` inside `frontends/gpui`
(`text.rs:242`) and is structurally invisible to the headless keystone.

### 1a. Composed teeth (primary) — headless, reuses keystone structure

New test in the keystone-structured suite, next to the existing entity-link teeth:

`crates/holon-integration-tests/src/pbt/frontend_slice/structural_pbt.rs`
→ `entity_scheme_registered_after_org_scan_still_links()`

It is the **ordering-inverted twin** of the existing
`sidecar_entity_link_resolves_through_the_intent_boundary` (`structural_pbt.rs:1836-1972`)
and reuses that test's whole rig verbatim — `HeadlessFrontendComponent::new_with_clock`,
`keystone_boot_clock()`, `comp.clone().register(&mut caps)`, `OpDispatchWriter`, the same
`SIDECAR_YAML` (`:1852-1860`) declaring the multi-word entity `t_widget`, the same
`t-widget:abc123` URI (`:1861`), the same two SQL assertions (`:1930-1971`).

The **only** difference — and it is exactly the bug's shape:

| | existing test (`:1836`) | new test |
|---|---|---|
| boot fixture | `structural-page.org` with **no** entity link | `structural-page.org` **already contains** `See [[t-widget:abc123][Widget]] here` in a block with an `:ID:` drawer |
| registration | post-boot, **before** the link is authored | post-boot, **after** the org scan has ingested the link |
| how the link arrives | `set_field(content)` through the intent boundary | the **boot org scan** (`FileSyncStarted` → `on_file_changed`) |
| quiescence | `sleep(SETTLE)` after the edit | `sleep(SETTLE)` after registration, no further writes |

Assertions at quiescence (identical to `:1930-1971`):

1. exactly one `block_links` row with `source_block_id = <the linking block>`,
   `kind = 'entity'`, `target = 't-widget:abc123'`;
2. `SELECT ... FROM backlinks WHERE target_id = 't-widget:abc123'` returns that block.

**Predicted red on `961b2598` (red-for-the-right-reason):** assertion 1 fails with an
**empty** row set — `assertion `left == right` failed: expected exactly one entity link row
for t-widget:abc123, got []`. Cause chain to record in the red log: boot scan →
`OrgFormatAdapter::with_classifier` → `strip_link` (`crates/holon-org-format/src/inline_marks.rs:580`)
→ `classify("t-widget:abc123")` → `is_registered_entity_scheme` false (registry holds only
defaults) → `LinkTarget::UnknownScheme` → persisted `EntityRef::UnknownScheme`
(`inline_marks.rs:583`) → `derive_block_links` `continue`s on that arm
(`crates/holon-api/src/inline_mark.rs:367`) → no INSERT at
`crates/holon/src/core/sql_operation_provider.rs:1022-1035`.
It must **not** be red because the file failed to parse or the block was missing — the test
asserts the source block exists and its `marks` column is non-empty *before* the junction
assertion, so a parse failure is distinguishable from the ordering defect.

### 1b. Generative teeth (composed keystone proper) — closes the COVERAGE gap

The triage class is COVERAGE: **no transition in `E2ETransition`
(`crates/holon-integration-tests/src/pbt/transitions/mod.rs:220-296`) can register an entity
type at runtime** — confirmed, the only registration paths are the pre-boot DI hook
(`frontend_slice/components.rs:620-639`) and hand-written test code. So the keystone cannot
*generate* the interleaving at all.

New transition, file-per-variant per the arch rule (`transitions/mod.rs:304`):

- `crates/holon-integration-tests/src/pbt/transitions/register_entity_scheme.rs`
  → `RegisterEntityScheme { entity_name: String }`, added to the `declare_e2e_transitions!`
  enum. Generator draws from a tiny fixed alphabet of **multi-word** names
  (`t_widget`, `cc_session`) so it also keeps the #71 hyphen/underscore join under test.
- SUT side: needs a new capability `SutTypeRegistry` exposing
  `TypeRegistry::register` (`crates/holon-profiles/src/type_registry.rs:135-143`);
  `HeadlessFrontendComponent` already surfaces `comp.type_registry().await`
  (used at `structural_pbt.rs:1889`), so the cap is a thin wrapper, not new wiring.
- Precondition: `StartApp` has happened and this scheme is not yet registered.
- `apply_to_ref`: records the scheme in a ref-side `registered_schemes: BTreeSet<String>`
  (see §3 — used only for a *negative* assertion; the junction oracle deliberately does
  **not** consult it).

The teeth come from the **existing** derived-links oracle
(`structural_pbt.rs:1619-1665`: `holon_api::derive_block_links(ref_marks)` vs
`SELECT target, kind FROM block_links`) now being reachable under arbitrary
`WriteOrgFile` / `RegisterEntityScheme` interleavings. Under D-min the oracle's expectation
is registration-**independent**, so any interleaving that changes the junction is a red.

**Gating probe (P3, §6):** the composed keystone's generator must actually emit at least one
scheme-shaped Link mark, otherwise 1b is vacuous. `generate_org_file_content_with_keywords`
must be checked; if it emits no `EntityRef::Internal` link marks, 1b additionally needs a
link-bearing content arm in the generator (`crates/holon-block-roundtrip-testing/src/lib.rs:368`
already has a `Just(EntityRef::Internal { … })` strategy to reuse). **If P3 shows the arm
must be added, 1b moves behind 1a and is its own increment** — 1a alone is a valid
red-for-the-right-reason for the fix.

### 1c. Decoration teeth (windowed/GPUI) — for ruling step 2

`frontends/gpui/src/render/builders/text.rs`, unit tests beside the existing
`an_unknown_scheme_link_is_disclosed_as_unresolved` (`:311`):

- `a_scheme_link_is_unresolved_while_its_scheme_is_unregistered` — classifier without the
  scheme → `LinkDecoration::Unresolved`;
- `the_same_scheme_link_is_healthy_once_the_scheme_registers` — **same `EntityRef` value**,
  classifier built `with_schemes(["t-widget"])` → `LinkDecoration::Healthy`.

The second test is the red: on `961b2598` `link_decoration` takes only `&EntityRef`
(`text.rs:242`), so the test **cannot even be written against the current signature** —
that is the red-for-the-right-reason (a compile-level "the observable is not expressible",
which the holon-feature skill accepts as a cannot-go-red gap only if reported; here it is
fixable, so I report the signature change as the red and land the two tests together with
Increment 4). A same-value/different-classifier pair is the honest formulation: it asserts
the decoration is a function of (mark, live registry), not of the persisted mark alone.

---

## 2. Exact change inventory (D-min)

### 2.1 `EntityRef` merge — `crates/holon-api/src/inline_mark.rs`

| site | change |
|---|---|
| `:50-70` (enum) | delete `Internal { id }` and `UnknownScheme { uri }`; add `Scheme { uri: EntityUri }` with `#[serde(rename="scheme", alias="internal", alias="unknown_scheme")]` and `#[serde(alias="id")]` on the field. Rewrite the type-level doc comment (`:37-49`) — the new sentence is the whole ruling. |
| `:362-390` `derive_block_links` | `EntityRef::Internal { id }` arm → `EntityRef::Scheme { uri }`, body unchanged (`kind` from `uri.scheme()`, `resolved: Some(uri.clone())`). `UnknownScheme` arm at `:367` **deleted** — this single deletion is the whole junction fix. `External` still skipped. Update the fn doc at `:357-361` ("unknown-scheme links are NOT block links" is now false). |
| `:969`, `:1022`, `:1029-1032` | test fixtures + the wire-tag pin test. Pin test becomes: `Scheme` ⇒ tag `"scheme"`, **plus** two new round-trip asserts that `{"type":"internal","id":…}` and `{"type":"unknown_scheme","uri":…}` both deserialize to `Scheme`. |

### 2.2 Org round-trip — `crates/holon-org-format/src/inline_marks.rs` (**byte stability**)

| site | change | byte impact |
|---|---|---|
| `:580-590` `strip_link` | `LinkTarget::Resolved(uri) => Scheme { uri }` **and** `LinkTarget::UnknownScheme(uri) => Scheme { uri: EntityUri::from_raw(&uri) }` (needs an `ALLOW(entity_uri_from_raw)`, same as `link_parser.rs:169`). The classifier is still called (External/CreationIntent still need it) but its scheme *verdict* is no longer consumed. | none |
| `:731-737` writeback | `Internal { id }` arm → `Scheme { uri }`; body already emits `id.as_str()`, and the deleted `UnknownScheme` arm (`:748-753`) emitted `uri` — **the two arms were byte-identical**, including the `== label` short-circuit. Merging them is provably byte-stable. | **none** |
| `:377-400` `((uuid))` block-ref parse | mints `Internal` → `Scheme`. | none |
| `:778-790` `is_block_ref_link`, `:933`, `:1439` | `Internal` arms → `Scheme`. **⚠ These are the byte-stability risk (R1):** each must gate on `uri.scheme() == "block"`, otherwise `[[Areas:Work]]` starts rendering as `((Areas:Work))`. Probe P2 verifies; if a gate is missing today it must be **added**, not inherited. | none *after* P2 |
| `:1517`, `:1598` | test fixtures. | — |

### 2.3 Loro backend — `crates/holon-loro/src/loro_backend.rs`

- `:118-145` reader: the `"internal"` arm (`:123`) and the `"unknown_scheme"` arm (`:136-141`)
  collapse into one arm matching `"scheme" | "internal" | "unknown_scheme"`, reading the value
  from `"uri"` **or** `"id"`. This is the Loro half of the compat alias.
- `:219` doc comment: `"external"|"internal"|"name"|"unknown_scheme"` → add `"scheme"`.
- `:242-255` writer: the two arms collapse; writes `type="scheme"`, key `"uri"`.
- **Wire note:** newly written Loro payloads use `"scheme"`. A peer on an older build reads an
  unknown tag. Martin wipes Loro often (ruling premise) and sharing is not yet multi-version,
  so this is accepted, **not** mitigated. Recorded here so it is a decision, not an accident.

### 2.4 Renderer / click — `frontends/gpui`

- `text.rs:233-249`: `link_decoration(target: &EntityRef)` →
  `link_decoration(target: &EntityRef, classifier: &LinkTargetClassifier)`.
  New body: `Scheme { uri }` → `Healthy` iff
  `classifier.is_registered_entity_scheme(&link_scheme_shape(uri.as_str())?)`, else `Unresolved`;
  `External`/`Name` → `Healthy` (unchanged).
- `text.rs:278` (inside `merge_marks(active, ctx: &GpuiRenderContext)`) — pass
  `ctx.link_classifier()`.
- `rendered_text.rs:280-298` click handler: the `Internal { id }` arm (navigate) and the
  `UnknownScheme` arm (caret only) merge into one `Scheme { uri }` arm that classifies live —
  registered ⇒ the existing navigate path, unregistered ⇒ `set_focus_with_caret`.
  `rendered_text.rs:448` fixture.
- **Classifier seam:** add `fn link_classifier(&self) -> &LinkTargetClassifier` to
  `holon_frontend::reactive::BuilderServices` (`crates/holon-frontend/src/reactive.rs:82`),
  reached as `ctx.services.link_classifier()`. **No default method** — a defaulted
  built-ins-only impl is precisely the silent-wrong-answer shape that caused #98. Eight impls
  must each supply one: `reactive.rs:2947` (`ReactiveEngine` — the real one, holds DI),
  `reactive.rs:3576` (`StubBuilderServices`), `crates/holon-app/src/headless_builder_services.rs:52`,
  `crates/holon-integration-tests/src/pbt/reference_state.rs:2520`,
  `crates/holon-frontend/tests/sidebar_creation_slot.rs:61`,
  `frontends/gpui/tests/support/mod.rs:105,264`.
  DI already publishes the registry-backed classifier
  (`crates/holon/src/di/registration.rs:274-280`, `:324-332`) — `ReactiveEngine` resolves that
  one; the stubs use `LinkTargetClassifier::default()` **explicitly at the call site**.
- `frontends/dioxus-web/src/render/builders/rendered_text.rs:185`: `Internal` → `Scheme`
  (no live classification there — dioxus-web has no decoration path today; noted, not extended).

### 2.5 Remaining `Internal`/`UnknownScheme` sites (mechanical, ast-grep)

`crates/holon-frontend/src/link_segments.rs:114`, `editor_view_model.rs:1215`;
`crates/holon-markdown/src/inline.rs:147`; `crates/holon/src/api/operation_engine.rs:775`;
`crates/holon-app/src/turso_seams.rs:250`;
`crates/holon-block-roundtrip-testing/src/lib.rs:368`;
`crates/holon-org-format/src/dense.rs:287` (comment claims the unknown-scheme junction row is
"silently lost" — now false, must be rewritten);
`docs/Reference/ORG_SYNTAX.md:43` and `docs/Plans/EntityUriLinks-F1.md:152-165` (prose).
Tests: `crates/holon-mcp-client/tests/entity_link_scheme_join.rs:65-69`,
`frontends/mcp/tests/dense_patch_entity_links.rs:100` — both assert on `LinkTarget`
(the live-classification type), which is **unchanged**, so they keep passing as-is.

### 2.6 What deliberately does **not** change

- `LinkTarget` (`crates/holon-api/src/link_parser.rs:17-51`) keeps `Resolved` and
  `UnknownScheme`. It is the *read-time* answer, and being registry-dependent is its job.
  Only its persistence mapping changes (§2.2).
- `SqlOperationProvider` — **no new field, no ctor churn** (this is the D-min payoff).
- `TypeRegistry` — **no broadcast, no subscribers, no sweep task.** Ruling step 4 is
  dissolved (§0). Consequently there is **no new write path**, so the echo-suppression
  question the brief raised does not arise: `block_links` rows are written only by
  `block_link_statements` inside the existing block-write transaction, and org write-back is
  driven by block/marks changes — which this change does not produce.
- Options A (boot gate) and C (registry-aware projection hash): not touched, not added.

---

## 3. Reference-lens parity

The keystone reference model has **no `block_links` table**; the junction oracle
(`structural_pbt.rs:1619-1665`) derives the expectation by calling the *production*
`holon_api::derive_block_links` on the reference block's `marks`
(`ReferenceState.domain.block_state.blocks[id].marks: Option<Vec<MarkSpan>>`,
`holon-api/src/block.rs:354`). Reference marks are stored **as generated**, never re-parsed
(`WriteOrgFile::apply_to_ref` → `RefDocumentsMut::seed_org_file`).

That is the mirror-class trap, and D-min **removes** it rather than adding to it:

- **Today** the ref side mints `EntityRef::Internal` (generator,
  `holon-block-roundtrip-testing/src/lib.rs:368`) while the SUT side re-parses through the
  classifier and may mint `UnknownScheme`. The two lenses disagree *because of registration
  timing* — a divergence that would be reported at the junction layer but originates in the
  parse layer.
- **After** the merge, both sides mint `Scheme { uri }` unconditionally. The ref needs **no**
  model of the registry for the junction oracle, and the oracle's expectation becomes
  registration-independent by construction. Parity is structural, not maintained.

Required ref-side edits:
1. `crates/holon-integration-tests/src/pbt/reference_state.rs:1733-1824` and the generator at
   `crates/holon-block-roundtrip-testing/src/lib.rs:368`: `EntityRef::Internal` → `Scheme`.
2. `reference_state.rs:2520` (`impl BuilderServices for ReferenceState`): supply
   `link_classifier()` — `LinkTargetClassifier::default()` **plus** the schemes any
   `RegisterEntityScheme` transition has applied, so the *decoration* lens (if the windowed
   slice ever asserts it) agrees. Backed by the new `registered_schemes: BTreeSet<String>`
   field (Increment 5 only).
3. `crates/holon-integration-tests/src/pbt/types.rs:146` — `LinkTargetClassifier::default()`
   stays; the ref's parse lens keeps knowing only built-ins, which is now *correct* rather
   than merely tolerated, because the persisted shape no longer depends on the scheme set.

Explicitly **not** added: a ref-side `block_links` mirror. The derived-oracle approach is the
standing pattern here and a mirror would be a second source of truth to keep in sync.

---

## 4. BugFunnel row (draft, for `docs/Testing/BugFunnel.md`)

Header edit — bump the COVERAGE counter `68 → 69` (line 10) and append at the **top** of the
increment log (newest first, after line 21):

```
- (+1 COVERAGE 2026-08-01: entity-scheme registration racing the boot org scan — see #98 row)
```

Ledger row (6 columns: date | bug | primary | secondary | missing piece | remedy):

```
| 2026-08-01 | A `[[<entity>:<id>]]` link whose scheme is declared by an MCP sidecar renders muted+wavy forever and produces no `block_links` row / no backlink, on every vault where the block was ingested before the provider connected — which is near-deterministically *every* boot: `BackendEngine` spawns the org scan at `holon-app/src/turso_seams.rs:898-905` while entity types only register when the MCP provider connects (`holon-app/src/mcp_integrations.rs:272-273`, first resolved at `wiring.rs:333`, i.e. later). `LinkTargetClassifier` (`holon-api/src/link_parser.rs:154-176`) asks the live registry, but its verdict is **persisted** as `EntityRef::UnknownScheme` in the block's marks (`holon-api/src/inline_mark.rs:50-70`), `derive_block_links` skips that variant (`inline_mark.rs:362-368`), and the GPUI decoration reads the stored variant rather than re-classifying (`frontends/gpui/src/render/builders/text.rs:242-249`). Not self-correcting: the cold-boot fast path skips any file whose `sha256(RENDERER_VERSION ‖ consolidator ‖ disk_bytes)` is unchanged (`holon-filesystem/src/file_sync_controller.rs:845-855, 1942-1966`), so the poisoned marks are permanent across restarts. Found by agent exploration of the F2a provider lane, not by any test. | COVERAGE | — | No transition in `E2ETransition` (`holon-integration-tests/src/pbt/transitions/mod.rs:220-296`) can register an entity type at runtime — the only registration paths are the pre-boot DI hook (`frontend_slice/components.rs:620-639`) and hand-written test code — so the keystone structurally cannot *generate* a registration-after-ingest interleaving. The nearest test, `holon-integration-tests/tests/boot_projector_gated_on_scan.rs`, gates the projector on the scan and says nothing about entity registration; `structural_pbt.rs::sidecar_entity_link_resolves_through_the_intent_boundary` registers *before* the link is authored, i.e. exactly the passing order. Missing piece = a `RegisterEntityScheme` transition plus an ingest-then-register ordering case. | OPEN 2026-08-01 — ruling D ratified (see PLAN-98D.md). Red-for-the-right-reason to be captured by `structural_pbt.rs::entity_scheme_registered_after_org_scan_still_links` (predicted: empty `block_links` result set) before the fix. |
```

Secondary gap: none. ORACLE was considered and rejected — the existing junction oracle
(`structural_pbt.rs:1619-1665`) *would* have gone red had the interleaving been generated,
so the invariant catalog is not the weakness. This keeps the distribution honest per the
skill's own note that ORACLE is historically over-attributed.

---

## 5. Increments and gates

Every increment is independently green-able. Cargo runs go through the semaphore
(`--id holon-build -j4 --fg`; keystone `--id holon-keystone -j1 --fg`), output `tee`d.

| # | content | gate |
|---|---|---|
| **0** | Probes P1–P3 (§6). No source change. | probe outputs recorded in the plan |
| **1** | BugFunnel row + header increment (§4). Docs only. | `just archlint` / doc gates only |
| **2** | **RED**: add `entity_scheme_registered_after_org_scan_still_links` (§1a). | `cargo test -p holon-integration-tests --features pbt entity_scheme_registered_after_org_scan_still_links` → **must fail with the predicted empty-junction signature**; the log is the PR's red evidence. Also confirm `sidecar_entity_link_resolves_through_the_intent_boundary` is still green (proves the rig, not the fix, is what differs). |
| **3** | `EntityRef` merge + serde aliases + Loro + org writeback + all mechanical sites (§2.1–2.3, 2.5) and the ref-side rename (§3.1). | Increment-2 test **green**. `cargo test -p holon-api -p holon-org-format -p holon-loro`; `cargo test -p holon-integration-tests --features pbt org_ingest_link_marks_survive_full_catalog org_ingest_entity_link_resolves_and_backlinks`; **byte-stability gate**: `just pbt general 16` must not regress the org round-trip invariants. |
| **4** | Live decoration + click (§2.4) + `BuilderServices::link_classifier` across all 8 impls + the two GPUI tests (§1c). | `cargo test -p holon-gpui --features pbt text::tests`; `cargo test -p holon-frontend`; windowed smoke `just gpui-windowed-smoke` (or the repo's equivalent) green. |
| **5** | `RegisterEntityScheme` transition + `SutTypeRegistry` cap + ref `registered_schemes` (§1b), gated on P3. | `transitions::arch_tests::every_variant_has_a_dedicated_file` green; `just keystone-smoke`; `just pbt general 64`. |
| **6** | Acceptance. | (a) Increment-2 teeth green; (b) `just keystone-smoke` green; (c) `just pbt general 64` green with no NEW signature vs. the four known reds; (d) `just hand-authored` green; (e) `cargo test -p holon-mcp-client entity_link_scheme_join` + `-p holon-mcp dense_patch_entity_links` green; (f) `fmt --check` clean, `jj fix -s @` no-op. |

Not in scope, by ruling: option A boot gate, option C hash change, any migration beyond the
serde/Loro aliases.

---

## 5b. Increment 0 — probe + audit outcomes (RUN 2026-08-01)

**Rulings applied:** §0 resolved to **D-min** (Martin). §8 resolved: the new transition drives
the **`create_entity_type` MCP tool**, not bare `TypeRegistry::register`; if the harness's
real-`TypeRegistry` wiring (parallel #98 mechanical lane) has not landed, code against the
registry-carrying variant and note the dependency.

| probe | verdict | evidence |
|---|---|---|
| **P2 / R1** — `((block-ref))` writeback scheme gating | **FALSIFIED (no risk)** | `is_block_ref_link` (`crates/holon-org-format/src/inline_marks.rs:782-795`) is **doubly** gated: the label must start `((` / end `))` **and** `id.as_str().strip_prefix("block:")` must match the inner text. `[[Areas:Work]]` (label `Areas:Work`, no parens) fails the first gate. The other two grep hits (`:933`, `:1439`) are inside that same fn and inside a test. Merging `Internal` + `UnknownScheme` **cannot** turn a non-block scheme into `((…))`. No added gate needed. |
| **P1 / R2** — `EntityUri` + serde aliases | **FALSIFIED (no risk)** | Throwaway `crates/holon-api/tests/probe_p1_scheme_variant.rs` (since deleted), 3/3 green: `EntityUri::parse` accepts `Areas:Work` (scheme `"Areas"`), `t-widget:abc123`, `cc-session:8f21`, `block:x`, all byte-round-tripping; `#[serde(rename="scheme", alias="internal", alias="unknown_scheme")]` + `#[serde(alias="id")]` deserializes **all three** wire shapes into one variant and serializes to `{"type":"scheme","uri":…}`. Log: `scratchpad/98-P1-run.log`. **D-min keeps `Scheme { uri: EntityUri }`** — the `String` fallback is not needed. |
| **P3 / R3** — keystone generator emits scheme-shaped links | **CONFIRMED (risk real, cheap fix)** | `typing_text_strategy` (`crates/holon-integration-tests/src/pbt/generators.rs:236-253`) is the **only** editor-driven path that mints a `Link` mark, and its link arm is `2 => "[a-z]{2,5}".prop_map(\|w\| format!("[[{w}]]"))` — a bare **wiki-name** link (`EntityRef::Name`), never scheme-shaped. So Increment 5 would be vacuous as-is. **Mitigation (one `prop_oneof` arm):** add a scheme-shaped arm minting `[[t-widget:{w}]]`, flowing through `TypeChars → set_field("content") → extract_inline_marks`, i.e. the same mark-aware write path the existing arm already validates. Non-vacuity to be pinned with the oracle's existing `assert!(!expected.is_empty())` floor. |

### Consumer audit of `block_links` / `backlinks` (the added gate) — **CLEAN, proceed**

No consumer treats a junction row as evidence that its target entity is registered or exists.
All 5 read paths, and why each is inert for an unregistered-scheme row:

1. **The only prod UI consumer** — `assets/default/index.org:23` ("Linked references", mirrored
   in `crates/holon-integration-tests/scripts/seed_wide/index.org:22` and pinned by
   `crates/holon-turso/tests/backlinks_section_matview.rs:24`):
   `SELECT bl.* FROM backlinks bl JOIN focus_roots fr ON bl.target_id = fr.root_id …`.
   A row surfaces **only when the user is focused on that exact target**, which requires the
   entity to be navigable. An inert `Areas:Work` row is unreachable until the entity exists —
   and at that point surfacing it is the desired behaviour, not a defect.
2. **`backlinks` matview** (`crates/holon-turso/src/schema_modules.rs:1101-1112`) —
   `block_links ⋈ block_raw ON b.id = bl.source_block_id WHERE bl.resolved_id IS NOT NULL`.
   Joins the **source** block; the target is never resolved or existence-checked.
3. **Org write-back upgrade path** (`crates/holon-app/src/turso_seams.rs:216`, `CacheBlockReader`) —
   `SELECT … FROM block_links WHERE kind = 'page' AND …`. **Filtered to `kind='page'`.**
   *This is the echo-suppression proof the ruling asked for:* the only route from `block_links`
   back into org bytes is page-kind-only, and D-min adds `kind='entity'` rows exclusively.
4. **Page re-resolution** (`crates/holon/src/core/sql_operation_provider.rs:1259-1262`) —
   `UPDATE … WHERE resolved_id IS NULL AND kind = 'page' AND …`. Doubly excluded: the new rows
   carry a non-NULL `resolved_id` **and** `kind='entity'`.
5. **Re-pointing / sharing rewrite** (`sql_operation_provider.rs:3141`,
   `crates/holon-sharing/src/alias_ledger.rs:254`) — `WHERE resolved_id = '<exact block id>'`.
   An unregistered-scheme URI can never equal a block id being re-pointed.

No `COUNT`/aggregate/ranking consumer of either relation exists anywhere in the tree
(searched `*.rs`, `*.sql`, `*.prql`, `*.org`, `assets/`); every other hit is a writer, a test
assertion, or a comment. **Gate passed — D-min's inert-row tradeoff is approved as specified.**

Two comments become false and are rewritten in Increment 3:
`crates/holon-org-format/src/dense.rs:287` and `frontends/mcp/src/main.rs:495`
(both assert an unknown-scheme link "loses its `block_links` row").

## 6. Top risks, each with a cheap falsification probe run first (Increment 0)

> Outcomes recorded in §5b: R1 falsified, R2 falsified, R3 confirmed (mitigation scoped).

**R1 — the merge breaks org byte-stability via the `((block-ref))` writeback.**
`inline_marks.rs:778-790`, `:933`, `:1439` all match `EntityRef::Internal`. If any of them
does **not** gate on `id.scheme() == "block"`, then after the merge `[[Areas:Work]]` and
`[[cc-session:8f21]]` would be re-emitted as `((Areas:Work))` — a silent vault-byte
corruption, the single worst outcome here.
*Probe P2 (2 min, read-only):* read those three regions and confirm each guards on the
`block` scheme. If a guard is missing, add it **in Increment 3 as an explicit gate** and pin
it with a round-trip test on a non-block scheme before the merge lands.

**R2 — `EntityUri` cannot hold an unregistered scheme, or the serde aliases don't work.**
D-min stores `[[Areas:Work]]` as `Scheme { uri: EntityUri }`. `EntityUri::parse` validates
RFC 3986 (`entity_uri.rs:27`) and `Deserialize` likely routes through it; and internally-tagged
enums with both a variant `alias` and a field `alias` are the compat mechanism for every
persisted mark in the vault. If either fails, the whole D-min shape is wrong.
*Probe P1 (one throwaway test, ~1 build):* assert `EntityUri::from_raw("Areas:Work")` and
`EntityUri::parse("t-widget:abc123")` succeed and report `.scheme()` correctly, and that
`serde_json::from_str::<EntityRef>` accepts all three of
`{"type":"scheme","uri":…}`, `{"type":"internal","id":…}`, `{"type":"unknown_scheme","uri":…}`
against a scratch enum with the proposed attributes. **If P1 fails**, fall back to
`Scheme { uri: String }` and make `derive_block_links` call `EntityUri::from_raw` — a
one-line delta, no change to any other section.

**R3 — Increment 1b is vacuous: the keystone generator emits no scheme-shaped Link marks,**
so `RegisterEntityScheme` interleaves with nothing and the composed arm proves nothing while
looking like coverage. This is the exact failure mode the #71 row already recorded ("the
probe could not distinguish a working join from a broken one").
*Probe P3 (grep, free):* search `generate_org_file_content_with_keywords` /
`crates/holon-integration-tests/src/pbt/generators/` for `InlineMark::Link` / `EntityRef`.
If absent, Increment 5 must **first** add a link-bearing content arm (reusing the existing
strategy at `holon-block-roundtrip-testing/src/lib.rs:368`) and demonstrate non-vacuity with
the same `assert!(!expected.is_empty())` floor the existing oracle uses
(`structural_pbt.rs:~1630`); otherwise Increment 5 is deferred and 1a carries the contract.

---

## 7. Delta if the orchestrator rules **D-lit** instead

- §2.1: keep `Internal`; add `Scheme { uri }` as a third variant (no merge, no 38-site rename).
- `derive_block_links(marks: &[MarkSpan], classifier: &LinkTargetClassifier)` — every caller
  updated (`sql_operation_provider.rs:1022`, `structural_pbt.rs:1625`).
- `SqlOperationProvider` gains a required `classifier: LinkTargetClassifier` ctor param
  (**not** an `Option`, **not** a `Default` — a defaulted seam silently reproduces #98 at any
  site that forgets). 12 prod + ~40 test ctor sites, listed in §0.
- New: `TypeRegistry` broadcast channel + a sweep task that, on register/unregister,
  re-runs `block_link_statements` for blocks whose marks contain a scheme-shaped target.
  **Junction rows only** — it must not touch `block_raw.marks`, must run inside the existing
  block-write transaction seam, and must be proven not to enqueue an org write-back
  (write-back is driven by block/marks changes; a junction-only statement set produces none —
  this needs an explicit test, not an argument).
- Increments 3/4 unchanged in shape; add Increment 3b for the sweep, with its own teeth:
  register **after** ingest with the app already quiescent, assert the junction appears
  **and** that no org file on disk changed mtime/bytes.
- Estimated cost: ~4× Increment 3, plus one new async subsystem.

---

## 8. Open question for review

Increment 5's `SutTypeRegistry` capability exposes `TypeRegistry::register` to the PBT. Two
of the three real registration paths in prod go through
`holon-mcp-client/src/mcp_integration.rs:180` and `frontends/mcp/src/tools.rs:457`
(the `create_entity_type` MCP tool), not through a bare `register`. Driving the bare registry
is the *lower* driver rung and risks an environment gap of its own. Preference?
(a) bare `TypeRegistry::register` — cheapest, matches `structural_pbt.rs:1889`;
(b) drive `create_entity_type` through the MCP tool surface — higher rung, closer to prod,
more wiring. I lean (a) for Increment 5 and a follow-up row for (b), but this is a
test-fidelity call that belongs to the reviewer.
