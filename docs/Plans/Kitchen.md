# Kitchen — recipes / pantry / shopping / nutrition (plan, 2026-09-01)

Driving use case for custom datatypes (BG) and the wife-adoption/sharing driver.
Rulings K1–K5 (`~/.claude/projects/-Users-martin-Workspaces-pkm-holon/memory/kitchen-feature-rulings-2026-09-01.md`) are BINDING and are cited inline.

## 1. Goals / non-goals

| | |
|---|---|
| **G1** | `.cook` files in the vault are AUTHORITATIVE for recipes (K1). Holon parses, projects, renders; never becomes a second source of truth. |
| **G2** | ALL kitchen datatypes + logic in a separate `holon-kitchen` crate (K1) — the first crate to own domain datatypes. |
| **G3** | Pantry inventory + a live "what can I cook now" query. |
| **G4** | Shopping list bidirectional with Martin's shopping app as a **peer** (K4) — item-level reconciliation, Holon is NOT master. |
| **G5** | Nutrition on a `product` datatype (static bundled table); calorie/macro rollups via **aggregates grown into the Computation dual-compile subset** (K3), not chained matviews. |
| **G6** | Thermomix = structured guided steps (temp/speed/time) rendered as mobile step cards. |
| **NG1** | **No Cookidoo import/scraping.** Ever, in this plan. |
| **NG2** | **No site importers** (K5). Manual copy/paste → `.cook` now. |
| **NG3** | **No sharing implementation.** The shared Kitchen container is named as the future driver for BG-6 and is OUT of scope here. |
| **NG4** | No OpenFoodFacts network lookup (K3: "later"). Bundled static table only. |
| **NG5** | No barcode scanning, no meal planning calendar, no recipe scaling UI beyond what cooklang gives free. |

## 2. Premise table

Every row is an anchor a reviewer can check. `UNVERIFIED` rows must be closed before the increment that depends on them starts.

| # | Premise | Anchor / status |
|---|---|---|
| P1 | Sidecar transports today are `child_process`, `http`, `rest` — three optional fields on one struct, NOT an enum. | `crates/holon-mcp-client/src/integration_config.rs:39-43`; dispatch `:286-320`, `crates/holon-mcp-client/src/mcp_integration.rs:383-434` |
| P2 | **K2's "first-class `http` transport" ALREADY EXISTS as `rest`** — a UTCP-manual-style direct-API transport, base_url + named `calls`. `http` means MCP-over-HTTP (historical name). gmail + gcal ALREADY converge on `rest`. | `integration_config.rs:71-89` and its doc comment at `:30-37`; `assets/integrations/gmail.yaml:118-125`, `gcal.yaml` |
| P3 | **`rest` is READ-ONLY: `GET` only, no request body.** This is the actual gap for K4, not the transport's existence. | `RestCallConfig` `integration_config.rs:149-172` — `method` doc says "Only `GET` is supported today (fails loud otherwise)"; `RestTransport` doc `:66-70` "Read-only for now" |
| P4 | Write machinery (effect classes `idempotent`/`once_only`, `undo`, `triggered_by`, `affected_fields`, `WritesPolicy`, `OnceOnlyAuthorization`) already exists at the sidecar level and is transport-agnostic. | `integration_config.rs:224-231`; example `assets/integrations/todoist.yaml:78-115` |
| P5 | Sync is **upsert + tombstone, identity-keyed; convergence from the engine, never a diff**. | `crates/holon-mcp-client/src/mcp_vtable.rs:58, 878-891` |
| P6 | Auth: static header or OAuth2 refresh-grant, credentials by env/file/keychain reference only, 0600 enforced, Debug redacted. | `integration_config.rs:104-115`, `crates/holon-mcp-client/src/rest_oauth2.rs:20-27, 60-108` |
| P7 | A free-standing type is a pure YAML data declaration (name, primary_key, fields with sql_type/nullable/indexed). Registered at boot from `BUNDLED_TYPES`. | `assets/default/types/person.yaml`, `organization.yaml`; `crates/holon-profiles/src/type_registry.rs:335-344, 348-400` |
| P8 | **There is NO general typed-reference / FK field in a type yaml.** Only `id_references` — an FK on the PRIMARY KEY, the extension-table shape. A `product_id` column would be plain TEXT with no declared relation. | `crates/holon-api/src/entity.rs:282`; `organization.yaml:4` |
| P9 | Declaring a type creates BOTH a `<name>_raw` write table and a `<name>` derived matview. | `crates/holon-turso/src/turso_adapter.rs:306-361` |
| P10 | `computed_persisted` IS already expressible in a `*_profile.yaml` as `tier: computed_persisted`. | `assets/default/types/person_profile.yaml:4-6` |
| P11 | **I3-2 is what makes P10 real in production**: its first step wires `TypeDefinition::persisted_derived_plan()` (`crates/holon-api/src/entity.rs:694`) into the production reconciler; I3-1 left the DDL sink test-harness-only. | `docs/Plans/BlockGeneralization.md:86-105` |
| P12 | `Computation` variants are a CLOSED set: `Lit, Field, Arith, Compare, Case, Concat, And, IsDefined, Predicate, Script`. No aggregates, no method calls, no field access. | `crates/holon-api/src/computation.rs:225-327` |
| P13 | **The language is strictly ROW-SCOPED**: `pub type Context = HashMap<String, Value>`. Eval cannot reach another row. | `computation.rs:64`, eval entry `:537` |
| P14 | There is **no function registry** — one hardcoded call form, `is_def_var("x")`. | `crates/holon-api/src/expr_parser.rs:614-637` |
| P15 | SQL seat: `compile_sql() -> SqlFragment` (`computation.rs:641`); plantable iff `compile_sql` AND `inline_sql` succeed (`:1065-1094`); planted as `({sql}) AS {name}` into the matview SELECT. **The sink accepts any parameter-free scalar SQL expression — a correlated subquery qualifies.** | `crates/holon-turso/src/schema_modules.rs:477-502` |
| P16 | Rhai fallback is live: subset parse first, Rhai on `ExprParseError`, both errors reported loudly if both fail. | `crates/holon-petri/src/lib.rs:350-363` |
| P17 | `FileFormatAdapter` exists and is **Block-shaped, org-only** today (`parse`/`render_document`/`render_blocks`/`build_block_params`). | `crates/holon-core/src/file_format.rs:60-171` |
| P18 | **BG Inc-5 (first non-org adapter) is BLOCKED BY Inc-4 (Loro adapter), itself blocked by NV-1/S2.** K1 names cooklang as the Inc-5 vehicle — this is a real ordering conflict, see §6/R1. | `docs/Plans/BlockGeneralization.md:118-133` |
| P19 | `cooklang` is NOT in `Cargo.lock` — a brand-new external dependency, subject to `deny.toml`. | `Cargo.lock` (no match) |
| P20 | cooklang-rs is **MIT, actively maintained**; parser yields ingredients/cookware/timers/metadata/steps; extensions include aliases, advanced units + ranges, ingredient/recipe references, intermediate preparations, modes. VERIFIED in-tree at `cooklang 0.18.7`: entry `cooklang::parse(&str) -> PassResult<Recipe>`; `Recipe{metadata, sections, ingredients, cookware, timers, inline_quantities}`; `Section.content: Vec<Content>`; `Content::{Step,Text}`; `Step{items, number}`; `Item::{Text,Ingredient,Cookware,Timer,InlineQuantity}` (component variants carry an INDEX into the recipe-level vec, not the value). Only 4 transitive deps. | RESOLVED. Registry source read directly. |
| P21 | Thermomix temp/speed have **no native cooklang syntax**. RULED: Inc E encodes them as OUR inline step convention, parsed at the Holon boundary into typed step fields. Files stay canonical cooklang, so other tools see plain text. | RULED by team-lead. Inc E's concern, NOT Inc A's. |
| P29 | **cooklang SILENTLY LOSES quantities AND whole ingredients on a misplaced brace.** FIVE MEASURED shapes at 0.18.7, none producing an error or warning: (a) unclosed `@flour{200%g` → quantity dropped; (b) `@flour{200%g @sugar}` → unit `"g @sugar"`, `@sugar` gone; (c) `@flour{200%g with a }` → unit `"g with a"`; (d) `@flour{200 @sugar}` → the sigil lands in the VALUE, `Text("200 @sugar")` with NO unit at all; (e) `#pot{1 @salt}` → COOKWARE swallows the ingredient, recipe ends with an EMPTY ingredient list. (b)–(e) all have BALANCED braces, so no counting guard can see them. Inc A refuses (a) by source scan and (b)–(e) by post-parse validation over ingredients AND cookware, across both the unit and a text value. | `crates/holon-kitchen/src/cook.rs` (`reject_unclosed_component_brace`, `reject_swallowed_components`); pinned by `an_unclosed_quantity_brace_...`, `a_late_closing_brace_that_swallows_a_component_is_refused`, `..._swallows_prose_is_refused`, `a_sigil_swallowed_into_the_value_is_refused`, `a_component_swallowed_by_cookware_is_refused` |
| P34 | `~{10 @salt}` — the timer form of the same swallow — is a HARD cooklang parse error ("Timer value is text"), so it needs no guard arm. | MEASURED at 0.18.7; noted in `a_component_swallowed_by_cookware_is_refused` |
| P33 | The swallow guard's word-count rule is a **stated bound, not a proof**: a two-word swallow (`{200%g with}`) still passes, because real units reach two words (`fl oz`) and over-refusing a valid recipe is worse than the narrow miss. The SIGIL rule has no such gap — a swallowed component always carries its sigil. | Pinned deliberately by `a_two_word_swallow_is_a_known_miss` and `a_two_word_unit_is_accepted` |
| P30 | A bare `@word` captures **one word only**; multi-word ingredients need `@maple syrup{}`. Recipes authored without the braces silently ingest a truncated ingredient name. | MEASURED at 0.18.7. Authoring hazard, not a code defect. |
| P31 | **There is NO multi-adapter routing.** `FileSyncController` holds ONE `format: Arc<dyn FileFormatAdapter>`; the watcher filter is hardcoded to `.org`; no registry type exists anywhere. The `FormatRegistry` is a PROPOSAL only. | `crates/holon-filesystem/src/file_sync_controller.rs:604`; `crates/holon-orgmode/src/file_watcher.rs:79-81`; proposal `docs/Proposals/ForeignVaultCompat-2026-07-12.md:93-102` |
| P32 | The landed PRECEDENT for a non-org adapter is `holon-markdown`: obsidian + logseq implement `FileFormatAdapter`, are exercised ONLY by their own crate's tests, and are wired into no production routing. Read-only tiers panic/bail loudly on every write method. | `crates/holon-markdown/src/obsidian.rs:120,342-393`; `crates/holon-markdown/tests/obsidian_ingest.rs` |
| P22 | **Shopping READ leg MEASURED** (source: Martin's Garmin watch app, a read-only consumer): `GET /list/{listId}` → `{data:{items:[…]}}`. | `ShoppingItemSyncDelegate.mc:8,25`, `GarminShoppingView.mc:60-71` in `/Users/martin/Workspaces/garmin/garmin-shopping-list` |
| P23 | Item shape: `name` string (required) · `cat` string category code (enumerated: MuF/FuV/DuH/R/D/P/B/S/CuT/SuD/Ca/I/C/Sn/F/Cu/O) · `count` number (optional). **No id. No timestamp. No etag. No `checked` field.** | same anchors as P22 |
| P24 | **The BASE URL IS A CREDENTIAL** — no auth headers; an opaque token segment is embedded in the URL PATH. | same anchors as P22. This plan never quotes the URL. |
| P25 | `base_url` already supports `${VAR}` expansion, so credential-in-URL is mechanically supported today with no engine change. | `integration_config.rs:73`, expansion at `:340` |
| P26 | **`redact_url` strips ONLY the query string** (`split_once('?')`) — a token in a PATH segment survives redaction into every error and log line. Eight call sites in the REST path would leak it. | `crates/holon-mcp-client/src/rest_oauth2.rs:571-576` (+ test `:603-609` pinning query-only); call sites `rest_transport.rs:252,267,307,315,336,344,353,387` |
| P27 | Observed sync model is **fetch-all-replace**: an unpaginated full snapshot of one list, no incremental cursor. | P22 anchors |
| P28 | **WRITE LEG UNKNOWN** — add/check/delete endpoints, item identity, and whether `checked` exists server-side at all. The watch app never writes. | Martin has been asked for the write-side spec. Hard block on C2. |

## 3. Data model

New crate `holon-kitchen` ships type yamls + profile yamls (P7) plus the cooklang adapter. Nutrition reference unit is **decided below**; relations are plain TEXT columns until P8 is closed (§7 R2).

### 3.1 Types

| Type | Fields (sql_type) | Notes |
|---|---|---|
| `product` | `id` TEXT pk · `name` TEXT idx · `brand` TEXT? · `kcal_per_100g` REAL? · `protein_per_100g` REAL? · `fat_per_100g` REAL? · `carb_per_100g` REAL? · `density_g_per_ml` REAL? · `unit_weight_g` REAL? | Static bundled table (K3). |
| `recipe` | `id` TEXT pk · `title` TEXT · `source_path` TEXT idx · `servings` INTEGER? · `total_time_min` INTEGER? | Projected from a `.cook` file; `source_path` is the authority link (G1). |
| `ingredient_use` | `id` TEXT pk · `recipe_id` TEXT idx · `raw_name` TEXT · `quantity` REAL? · `unit` TEXT? · `product_id` TEXT? idx · `step_index` INTEGER | The child relation Inc D aggregates over. |
| `pantry_item` | `id` TEXT pk · `product_id` TEXT idx · `quantity` REAL · `unit` TEXT · `opened_at` TEXT? · `best_before` TEXT? | |
| `shopping_item` | `id` TEXT pk · `name` TEXT idx · `cat` TEXT · `count` REAL? · `product_id` TEXT? · `checked` INTEGER · `deleted_at` TEXT? · `last_seen_remote` TEXT? | **No `remote_id` — the peer issues none (P23).** `name` + `cat` mirror the wire shape verbatim; `checked`/`product_id` are LOCAL-ONLY until P28 resolves. `deleted_at` is the local tombstone; `last_seen_remote` is the timestamp of the last complete fetch that contained this item (§4). |
| `meal_log` | `id` TEXT pk · `recipe_id` TEXT? · `product_id` TEXT? · `servings` REAL · `eaten_at` TEXT idx | Exactly one of recipe_id/product_id set — asserted at the write boundary, fail-loud. |

### 3.2 Decisions

**D1 — nutrition reference unit: per 100 g, mass-canonical, no alternatives.** Every nutrition figure is per 100 grams. Volume units convert via `density_g_per_ml`; count units (`1 egg`) via `unit_weight_g`. Rejected: storing per-serving or per-package (forces a second reference field and a units war inside every aggregate). **A conversion with no factor is NOT zero and NOT skipped** — it is a loud unconvertible state (§3.4).

**D2 — how a cooklang `@ingredient` binds to a `product`: an explicit mapping table, never a fuzzy match at read time.** `ingredient_use.raw_name` always holds the cooklang text verbatim. `product_id` is filled from a `product_alias` mapping (`alias TEXT pk · product_id TEXT`) shipped bundled and extended by the user. **An unmatched ingredient leaves `product_id` NULL and the recipe renders a visible "unmatched ingredient" affordance** — per fail-loud, it never silently drops from a nutrition rollup and never guesses. Any rollup over a recipe with an unmatched or unconvertible ingredient is reported as *incomplete*, not as a smaller number (§3.4).

**D3 — recipe identity.** `recipe.id` derives from the `.cook` file's stable id the same way org files derive theirs (`doc_id_from_content`, P17); absent one, from the vault-relative path. Re-parse is upsert-keyed on that id (P5).

### 3.3 Computed fields

| Type.field | Tier | Expression shape |
|---|---|---|
| `ingredient_use.grams` | computed (row-scoped, EXISTS TODAY) | `Case` over `unit` × `density_g_per_ml`/`unit_weight_g` — expressible in P12's subset **only if** the product's factors are on the row; they are not (P8/P13). See R2. |
| `recipe.kcal_total` | computed_persisted | **`Sum(ingredient_use, kcal_of_use)` — NOT expressible today.** This is Inc D. |
| `recipe.protein_total` / `fat_total` / `carb_total` | computed_persisted | same shape |
| `recipe.unmatched_count` | computed_persisted | `Count(ingredient_use, product_id == ())` — drives D2's incompleteness banner |
| `pantry_item.grams_on_hand` | computed | same conversion shape as `ingredient_use.grams` |

### 3.4 What the aggregate subset must grow (Inc D)

Minimal growth — one new `Computation` variant, one grammar rule, both seats:

```
Agg { kind: AggKind, rel: RelName, inner: Box<Computation>, filter: Option<Box<Computation>> }
AggKind = Sum | Count
```

**Everything below is MEASURED, not assumed.** The D.0 spike
(`crates/holon-turso/tests/agg_subquery_matview_spike.rs`, 8 probes green;
lane report `lane-report-kitchen-d0.md`) **refuted this section's original
correlated-subquery lowering**. What follows is the proven replacement — do not
re-derive the old one.

* **Grammar** (`expr_parser.rs`, extends the single `is_def_var` call form at P14 into a *small closed* call table — still no open registry): `sum(<rel>, <expr>)`, `count(<rel>)`, `count(<rel>, <pred>)`. `<rel>` is a bare identifier naming a declared child relation of the enclosing type.
* **SQL seat — the correlated subquery is REJECTED by the fork.** A scalar correlated subquery in a matview SELECT list fails at DDL: *"Correlated scalar subqueries in materialized view SELECT lists are not yet supported by the IVM compiler. Rewrite as a LEFT OUTER JOIN with GROUP BY…"* (`SUM` and `COUNT` alike, so the verdict is about the shape). The original claim of "**no change to `schema_modules.rs:477-502`**" therefore does not hold; see the plant-site cost below.
* **SQL seat — the proven lowering** is the fork's own prescription, and it is the shape `block` already ships for its edge fields (`schema_modules.rs:528-532` generalized from junctions to declared relations): one GROUP BY **side matview** per relation, `LEFT OUTER JOIN`ed into the type's matview, with the outer column `COALESCE`d. Verified IVM-maintained across child insert / update / delete / FK re-parent / emptied relation, and through the three-level chain §3.3 needs (`child_raw → child mv → relation agg mv → parent mv`) — which matters because `recipe.kcal_total` sums `ingredient_use.grams`, itself a computed column. **The `COALESCE` is load-bearing, not cosmetic**: without it a parent with no children drops out of its own matview.
* **`N` aggregates over ONE relation share ONE side view and ONE join.** §3.3's four recipe rollups plus `unmatched_count` are a single extra join on `recipe`, not five.
* **Filtered count lowers to `SUM(iif(<pred>, 1, 0))`, never `COUNT(*) FILTER (WHERE …)`.** The `FILTER` clause the fork's own error message suggests is itself unsupported (*"FILTER not supported with Count in incremental views (v1 limitation)"*) — taking that message at face value produces a second dead end. `iif` is already the lowering `Computation::Case` uses, for the same class of reason.
* **The inner expression is compiled with the child alias applied at compile time**, not left to SQL name resolution. Under the rejected subquery lowering, an inner column the child does not own bound silently to the OUTER row — a plausible, wrong number. Under the JOIN lowering the same mistake is a hard DDL error (*"SUM column 'servings' not found in input"*), so the lowering removes a bug class rather than guarding against it.
* **Plant-site cost — this is where the work moved.** `PlantedColumn` is a `{name, sql}` pair rendered as `(<sql>) AS "<name>"` and `TursoAdapter::matview_select` emits a flat `SELECT … FROM "<raw>"`; neither can express a join. Three bounded changes: `DerivedFieldPlan` gains a third bucket for relation aggregates; `matview_select` emits the joins and `COALESCE`d outer columns; `TursoAdapter::schema_modules` emits one side-matview module per relation, ordered before the type's own matview so the existing `requires` DDL gate is satisfied.
* **Eval seat — P13 is NOT a breaking change** (measured: 4 exhaustive matches over `Computation`, all in `computation.rs`; 3 production `eval` callers, of which only `derived_reconciler.rs` must migrate). `eval(&Context)` stays; add `eval_scoped(&EvalScope)` where `EvalScope` pairs the outer row with a relation resolver yielding each child row's own `Context`. `eval` delegates with a resolver that **fails loud** on `Agg` (`NoRelationResolver { rel }`) — never zero, never an empty set. `inner` evaluates in the CHILD context; every other variant is untouched.
* **The PARSER scope changes too, not only the two seats**: `parse_typed` resolves `+` as concat-vs-add from declared column types, and inside `sum(<rel>, …)` those are the CHILD's `FieldTypes`. Forgetting this mis-types an operator silently instead of failing.
* **`result_kind`**: both `Sum` and `Count` yield `FieldKind::Numeric`, independent of the inner. Declare-time checks, all fail-loud: `Sum` requires a `Numeric` inner, `filter`/`<pred>` requires `Boolean`, and a nested `Agg` is refused (there is no second scope level).
* **`Sum` yields REAL on BOTH seats** (`CAST(… AS REAL)` in SQL, `Value::Float` in eval) — ruled 2026-09-01. One rule with no type-dependence beats type-faithful summation here, because the dual-seat design's purpose is minimizing desync points and nutrition sums are real anyway. Revisit if a future integer-domain aggregate makes it awkward. Pin it with an integer-column case in the differential; the D.0 probe used a REAL column throughout and did not cover it.
* **Relation declaration**: `rel` must resolve to `(child_table, fk_column)` — see R2 below, a hard prerequisite of Inc D, not an optional cleanup.
* **Fail-loud, no zero-substitution**: `COALESCE(...,0)` is legitimate only over rows that are all convertible. The incompleteness signal is carried by `unmatched_count`, and the UI must refuse to present a total as authoritative when it is non-zero (D2).
* **Rhai burn-down alignment** (K3): aggregates in the subset must NOT be reachable via `Script` fallback, or the two seats silently diverge — the Rhai seat has no relation resolver. `Agg` inside a `Script` is a loud refusal.
* **The standing oracle**: extend `derived_field_eval_vs_sql.rs` with generated parent/child sets and assert the seats agree **after every mutation of the child set**, not only at the end. A retraction the IVM gets wrong is invisible to a final-state-only assertion.

## 4. K4 — shopping app peer sync

Holon is a peer, never master. Poll-based. **The measured contract (P22–P28) removes the two things a clean reconciler normally stands on: a server-side item id and any timestamp.** Everything below is shaped by that.

| Concern | Rule |
|---|---|
| Identity | **`(name, cat)` is the reconciliation key** — there is no id (P23). Chosen over `(name)` alone because `cat` is always present and makes "Milk/DuH" and "Milk/Ca" distinct items rather than a collision. |
| What name-keying BREAKS — state it plainly | (a) **Duplicate names collapse.** Two "Milk" entries in the same category are indistinguishable; Holon will see one item. Mitigation: fold duplicates into `count` on ingest, and never round-trip a second row with the same key. (b) **A rename is indistinguishable from a delete + add.** Editing "Milk"→"Oat milk" on the peer arrives as one removal and one addition, so any local-only state attached to the old key (`checked`, `product_id`) is LOST. There is no fix without a server id; it is a disclosed limitation, not a bug to chase. (c) **`checked` and `product_id` cannot survive a peer-side rename** for the same reason. |
| Reconciliation | Item-keyed, field-wise, over the upsert+tombstone contract (P5). |
| Both sides add | Key present remotely, absent locally ⇒ local insert. Key present locally, absent remotely ⇒ push (C2 only; in C1 it simply cannot happen, see Inc C). |
| Absence vs deletion — the P5 tension | P5 says absence is never deletion, but the peer offers **fetch-all-replace with no tombstones** (P27), so absence is the ONLY deletion signal it can give. Resolution: absence is authoritative deletion **only within a fetch that completed successfully and in full**. A failed, partial, or truncated fetch is discarded whole and changes nothing — it never reaches the reconciler. `last_seen_remote` records the last complete fetch that carried the item, so "absent" is always evaluated against a known-good snapshot rather than against silence. |
| Local deletes | Still need a local tombstone (`deleted_at`, window ≥ 2× poll interval, proposed 7 days) so a local delete is not immediately resurrected by the next pull before it has been pushed. |
| `checked` | **May not exist server-side at all (P23/P28).** The observed schema has no such field, so checking an item on the peer may simply BE removal from the list. Until P28 lands, `checked` is local-only and never pushed. IF the write leg exposes it, the rule is **check-off wins over un-check** — the cost of a wrong check ("you skip something you have") is far below a wrong uncheck ("you re-buy every trip until someone notices"). Asymmetric cost, so an asymmetric rule rather than last-write-wins. |
| `count` | Last-writer-wins; there is no timestamp to arbitrate with, so the LOCAL write wins only if it is newer than the last complete fetch, else the remote value is taken. |
| Cadence | `rest.poll_interval` (P2), proposed 60s while the shopping view is open, 5m otherwise. |
| What the certifier can certify | For C2's sidecar: `writes: enabled`, per-tool `effect` classes (`add-item` = `once_only`, `check-item` = `idempotent`, `delete-item` = `once_only`), `undo.reversible`. **The certifier cannot certify the conflict rule, the name-keying, or the complete-fetch discipline** — all three are Holon-side reconciler logic, not profile clauses. They need their own PBT; claiming certifier coverage here would be false comfort. |

**Security — credential-in-URL (P24/P26).** The URL is the only secret and it lives in a path segment. Two obligations, both in C1:
1. The sidecar takes it as `base_url: ${SHOPPING_LIST_URL}` (P25 — works today). The bundled sidecar in `assets/integrations/` carries the `${VAR}` reference ONLY, never a resolved URL, so bundled-sidecar portability (`bundled_sidecars.rs` generation rules) is preserved and the token never enters the repo, a config export, or a support bundle.
2. **`redact_url` must learn to redact path segments before any shopping traffic runs** (P26). Today it strips only `?query`, so all eight REST error/log sites would print the token verbatim. This is a prerequisite of C1, not a follow-up — it is the one place where a plain error message becomes a credential leak. Proposed shape: for a URL whose sidecar declared it credential-bearing, redact ALL path segments (`https://host/<redacted>`); do not attempt to guess which segment is the token.
3. Corollary: the credential-bearing URL must never be quoted in a bugfunnel entry, a PBT fixture, or a plan. Fixtures use a synthetic localhost URL.

## 5. Increments

Risk-eliminating order. Each is independently landable with a red-first surface (`holon-feature` skill: red-for-the-right-reason BEFORE implementation).

### Inc A — cooklang read adapter + recipe types + recipe page — **LANDED (this lane)**
* **Scope.** New crate `holon-kitchen`; `cooklang 0.18` dependency; `recipe` + `ingredient_use` type yamls + `recipe_profile.yaml`; `CookFormatAdapter: FileFormatAdapter` (P17) claiming `.cook`; the recipe page as the type's default render variant.
* **What of the per-format adapter architecture it de-risks (K1/BG Inc-5) — R1 GRANTED, PARTIAL claim.** It proves, on a real second format: **extension claiming, document identity, and round-trip (read) authority**. It **explicitly does NOT** discharge C7 — making `FileFormatAdapter`/`FileFormatParseResult` type-generic — nor does it wire multi-adapter routing (P31). BG's doc takes this wording at landing.
* **Scope correction from P31/P32.** "`.cook` files in the vault are authoritative" is the DURABLE design statement, not something Inc A can deliver: no routing exists to carry a `.cook` file to any adapter. Inc A therefore proves the adapter through crate-level tests exactly as `holon-markdown` does (P32) — the landed precedent for this staging. Live vault ingest arrives with the `FormatRegistry`, not here.
* **Read-only tier.** `render_document`/`render_blocks` panic and `writeback_drops` bails, mirroring `ObsidianMarkdownAdapter` — Inc A ships no cooklang renderer, so a write would be loss, not a round trip.
* **Riskiest thing (retired).** cooklang-rs API + license — RESOLVED, see P20.
* **Red-first (captured).** `lane-logs/red1-adapter-missing.log`: every rung fails as `unresolved import holon_kitchen::{CookFormatAdapter, IngredientUse, ingredient_uses, RECIPE_TYPE_YAML, …}`. Green: 15/15.
* **Out of scope, held.** Ingredient→product binding stays NULL and visibly unmatched (Inc D).

### Inc A2 — the `FormatRegistry` — **LANDED 2026-09-01 (D55.a, discharges R6)**
* **Scope.** `FormatRegistry` in `holon-core` (ordered adapters + lowercased-extension index; a contested extension is a CONSTRUCTION error, per parse-don't-validate). `FileSyncController` holds `formats: Arc<FormatRegistry>` instead of one adapter and routes per file at every parse / identity / render / write-back site; the watcher, the directory scan, `poll_new_files` and the notify adapter's rename-pairing buffer filter on the registry's extension union. `OrgFileWatcher` → `VaultFileWatcher`. Production registers **org + cook** (D56.a); `register_kitchen_types` is now called from the bundled-type declaration site, so the `recipe` table exists.
* **`WriteTier` — the increment's real design change.** `FileFormatAdapter` gained `fn write_tier(&self) -> WriteTier { ReadWrite | ReadOnly }`, no default impl. Routing a read-only format INTO the controller also exposes it to the controller's write half, and the adapter's own `unreachable!()` guards would have turned that into a PANIC in a spawned task — an abort that discloses nothing. The gate sits at `write_back_or_skip_readonly`, the ONE chokepoint every projection write passes, and refuses at ERROR through `WritebackDisclosure`.
* **What the red test caught (would have been a P0 in a real vault).** With routing live but before the gate, a `.cook` recipe was DELETED and replaced by a `Pancakes.org` projection of itself: cooklang embeds no id, so the page is name-chain-identified, `page_file_from_name_chain` appends `.org`, the twin was materialized and the original retired as a stale home. Two fixes: the write-tier gate, and recording `doc_home` at ingest for read-only formats (it was otherwise written only where the controller WRITES a file, so the document read as homeless and every gate read that as "its file is ours to mint").
* **Red-first.** `lane-logs/red3-registry-missing.*.log` (unresolved `FormatRegistry` / `WriteTier`, before implementation); `lane-logs/red1-red2-no-cook-registration.*.log` (cook adapter unregistered → no page minted for `Pancakes.cook`; wiring restored byte-for-byte, sha verified).
* **Out of scope, held.** Markdown registration (both flavors claim `md`; needs the vault-flavor discriminator) · a cooklang renderer / any `.cook` write leg · C7 type-genericity · populating `recipe` ROWS (Inc B/D — the adapter emits blocks, nothing writes that table yet) · carrying the cooklang `title:` metadata onto the persisted page (pages are titled from the filename; `sync_document_metadata` is the seam) · the `.org`-hardcoded page-file derivation (see R7).

### Inc B — pantry + ops + cookable-now live query — **LANDED (query + ops); ingest leg is D58**
* **Scope.** `pantry_item` type; add/consume/adjust ops; the "what can I cook now" live query (a recipe is cookable iff every `ingredient_use` has a `pantry_item` with sufficient converted quantity).
* **Riskiest thing — DISCHARGED.** The cookable-now predicate is an aggregate in disguise ("ALL children satisfy…"). **It must be written as a query, not as a computed field**, or it silently front-runs Inc D and forces the language growth early. Ruling for the executor: query. Delivered as two SQL constants in `holon-kitchen/src/cookable.rs`, both composed from ONE `SATISFIES` fragment so the cookable list and the blocker list cannot disagree.
* **Ops.** `add` = the declared type's generic `create`, `adjust` = `set_field` — no kitchen-named aliases over the same write. Only `consume` is bespoke (`holon/src/core/pantry_operations.rs`): it is a read-modify-write with two refusals the generic path cannot make (past empty; an unconvertible unit), and its inverse restores the exact prior amount rather than re-adding the delta.
* **Query shape.** `NOT EXISTS` — correct rows, served by DISCLOSED eager re-execution rather than an incrementally maintained matview (`sql_ivm_maintainable` routes every subquery predicate there). Chosen over the incremental anti-join PAIR because that needs two permanently-registered named matviews wired as `SchemaModule`s in DI, materialized for every user whether or not they own a recipe. The disclosure is asserted, not assumed.
* **Conversion (F3 ruling).** Same-unit only; `product.density_g_per_ml` is Inc D. A differing-unit pair is UNCONVERTIBLE — not cookable AND named as a blocker, never silently skipped, never counted satisfied.
* **Join key (F2 ruling, §3.1 deviation).** `pantry_item` carries `name TEXT`; cookable joins `pantry_item.name = ingredient_use.raw_name`. `product_id` stays nullable and present so Inc D's binding needs no migration.
* **Red-first (captured).** `lane-logs/red1-no-pantry.*.log`: unresolved `holon_kitchen::{COOKABLE_RECIPES_SQL, COOK_BLOCKERS_SQL, CookBlockReason}`. Inversion proof `lane-logs/red2-inversion.*.log`: deleting the `EXISTS(ingredient_use)` vacuity guard reds `a_recipe_with_no_known_ingredients_is_not_cookable`; deleting the same-unit clause reds `an_unconvertible_unit_blocks_the_recipe_by_name`. Restored byte-for-byte (sha verified), 7/7 green.
* **What Inc A shipped unwritable (fixed here).** `recipe` and `ingredient_use` declared neither `properties` nor `property_kinds`, so the engine's `_provenance` stamp had nowhere to land and EVERY `create` on them was refused at the write boundary. Inert while nothing wrote them; both columns are now declared on all three kitchen types.
* **Out of scope, held — the ingest leg (D58).** Nothing turns a parsed `.cook` recipe into `recipe`/`ingredient_use` ROWS. Cookable-now is proven over rows written through the PN surface; it does not yet answer over a real vault file. **Hazard for whoever builds that leg: `ingredient_use.recipe_id` must hold the MINTED id (`recipe:<local>`), not the bare one — the write path prefixes ids with the entity's kebab name, and a bare id joins to nothing silently.**

### Inc C — shopping peer. **SPLIT: C1 read-only (startable now) / C2 write+reconcile (blocked).**
Per P2/P3 neither half is a new transport kind.

#### C1 — read-only pull (contract known, CAN START)
* **Scope.** `shopping_item` type; `assets/integrations/shopping.yaml` using the EXISTING `rest` GET transport (P2/P3) with `base_url: ${SHOPPING_LIST_URL}` (P24/P25); one call `GET /list/{listId}`; `sync.extract_path: data.items`; ingest keyed on `(name, cat)` (§4) with duplicate-folding; a read-only shopping view. **Plus the `redact_url` path-segment fix (P26) — a hard prerequisite, not a follow-up.**
* **Why it is worth landing alone.** It proves the credential-in-URL pattern, the category vocabulary, the complete-fetch discipline, and the `(name, cat)` key against the real peer — every one of which C2 would otherwise have to debug simultaneously with write semantics.
* **Riskiest thing.** The credential leak through `redact_url` (P26) — a plain error message today prints the token.
* **Red-first.** Keystone against a mock HTTP server: a served list projects `shopping_item` rows; a truncated/failing response changes nothing (complete-fetch discipline). Expected red: no shopping sidecar exists, zero rows. Second red: assert no log or error line contains the token path segment — expected red because `redact_url` keeps it.
* **Designed so C2 slots in without rework.** C1 must, from the start: (a) put reconciliation in a `ShoppingReconciler` seam that takes `(local_rows, complete_remote_snapshot)` and returns intents — C1 simply has no push intents to emit, rather than having no seam; (b) store `checked`/`product_id` as local-only columns already present in the schema, so C2 adds no migration; (c) write local tombstones on local delete even though nothing pushes them yet; (d) keep the category codes (P23) as a parsed enum, not strings, per parse-don't-validate.

#### C2 — write leg + bidirectional reconcile (**BLOCKED on P28**)
* **Scope.** Grow `RestCallConfig` beyond `GET` (POST/PATCH/DELETE, request-body template, response-id extraction), then bind the `writes`/`tools`/`effect` machinery (P4) and turn on the §4 push intents.
* **Generic enough for gmail/gcal convergence (K2).** The write leg is declared in the sidecar, not in kitchen code — gmail "mark as read" or gcal "create event" become authorable with zero further engine work. That is the reason to spend the increment on the transport rather than a bespoke shopping client.
* **BLOCKED — Martin must supply the write-side spec** (add/check/delete endpoints, item identity, whether `checked` exists). K1: code against it if locked down, else wait. Do not start C2 on a guessed API.
* **Riskiest thing.** Bidirectional conflict semantics under a key that is not an id (§4) — no existing sidecar exercises anything like it, and the rename-loses-local-state hole (§4) may prove unacceptable to Martin, which would force asking the peer's author for an id field.
* **Red-first.** Mock-HTTP PBT: both peers mutate the same item across a poll; assert §4's rules incl. the rename limitation as an EXPECTED, asserted outcome. Expected red: `rest` refuses a non-GET method loudly.

### Inc D — product/nutrition + AGGREGATE GROWTH (riskiest increment)
* **D.0 — LANGUAGE-DESIGN SPIKE — DONE 2026-09-01.** It paid for itself: §3.4's correlated-subquery SQL seat was **refuted** and the replacement proven, before any nutrition code existed. The eval-seat scope change (P13) — the escalation trigger this spike was built around — did **not** fire; it is additive and touches one production caller. What the spike did NOT do: no `Agg` variant in `holon-api`, no plant-site change, no nutrition tables. Record: `crates/holon-turso/tests/agg_subquery_matview_spike.rs` (8 probes green) + `lane-report-kitchen-d0.md`. Probes A (correlated subquery rejected, `SUM` and `COUNT`) and the `COUNT(*) FILTER` probe are **retained as fork-capability guards** — a future fork bump flips them red and the lowering gets revisited deliberately rather than by accident; probe B's childless-parent `0.0` assertion is a permanent guard on the `COALESCE`.
* **Then, in this order** (splitting the plant-site risk from the language risk, so a plant-site bug and a language bug can never be diagnosed as each other):
  1. **R2 alone** — `fields[].references` + the relation registry. Prerequisite of both the aggregates and `ingredient_use.grams`, closes the D6 seam, carries no language change.
  2. **The plant-site generalization** (§3.4), exercised by a **stored** child column — no new `Computation` variant yet. This is where the risk moved after D.0, and it can be de-risked without touching the language.
  3. **`Agg` + grammar + `eval_scoped`**, with the extended differential.
  4. **Then** `product` type + bundled nutrition table + `product_alias`; the §3.3 computed_persisted fields; the D2 unmatched banner.
* **Inc A left a stated seam Inc D must close (D6).** `IngredientUse` is a parse-time value, NOT yet a row: its `name` field maps to the schema's `raw_name` column, and it carries no `id` or `recipe_id` at all. Inc A never persists ingredient uses, so the gap is inert there; Inc D is what writes them and must supply the id minting, the `recipe_id` foreign key (per R2's `fields[].references`), and the `name`→`raw_name` rename at the boundary. Anchors: `crates/holon-kitchen/src/cook.rs` (`IngredientUse`) vs `crates/holon-kitchen/assets/types/ingredient_use.yaml`.
* **Riskiest thing.** The eval-seat scope change (P13) — it touches every `Computation` consumer, and getting it wrong desynchronizes the two seats, which is precisely the class of bug the dual-compile design exists to prevent.
* **Red-first.** A differential PBT asserting eval and SQL agree on `sum(...)` over generated recipes — the standing dual-seat oracle.
* **Depends on I3-2** (P11) for `computed_persisted` to reach production at all.

### Inc E — cook-this composite op + Thermomix step rendering
* **Scope.** `cook_this(recipe, servings)` = decrement pantry by each converted `ingredient_use`, append a `meal_log`, in ONE transaction. Plus step cards from cooklang steps/timers.
* **Riskiest thing.** Thermomix temp/speed have no native syntax (P21) — the encoding decision. Second: `cook_this` must be atomic and undoable as a unit, and partial application on an unconvertible ingredient is a loud refusal, not a partial decrement.
* **Red-first.** GPUI PBT for step cards; keystone for the transactional op incl. the refusal path.

## 6. Dependencies

```
   Inc A (LANDED) ──> A2 FormatRegistry (LANDED, R6 discharged) ──> Inc B
                                                             │
   redact_url path fix (P26) ──> C1 (READY NOW) ──> C2       ├──> Inc E
                                        [Martin: write spec]─┘     ▲
   I3-2 (BG:86-105) ──┐                                            │
                      ├──> R2 (relation decl) ──> Inc D ───────────┘
```
C1 is on NO critical path — it is startable today and independent of A/B/D.

| Dependency | Kind | Note |
|---|---|---|
| I3-2 landing | internal, concurrent lane | Inc D's `computed_persisted` fields are inert without it (P11). Name it in Inc D's PR. |
| R2 relation declaration | internal, this plan | Hard prerequisite of Inc D's aggregates AND of `ingredient_use.grams` (§3.3). |
| `redact_url` path redaction | internal, security | Hard prerequisite INSIDE C1 (P26). |
| Shopping **write-side** spec | **external, Martin** | Hard block on C2 ONLY (P28). C1 is unblocked. |
| cooklang-rs | **external, network** | Hard block on Inc A start (P19/P20). `deny.toml` license review required. |
| BG Inc-4 / NV-1/S2 | internal | Only if Inc A attempts C7 — which it does not, by R1. |

## 7. Open risks / rulings needed

* **R1 — GRANTED.** Inc A claims PARTIAL Inc-5 de-risking (extension claiming, identity, read authority), explicitly not C7. Wording lands in the BG doc.
* **R2 — CLOSED 2026-09-01 by the D.0 spike.** The FK is declared on the **CHILD**, and the parent's relation set is **derived** from it — two authorable declarations that can disagree is how the two seats come to disagree.

  ```yaml
  # ingredient_use.yaml
  fields:
    - name: recipe_id
      sql_type: TEXT
      references:
        type: recipe          # target type; the target column is ALWAYS its pk
        as: ingredient_uses   # OPTIONAL — the name the aggregate uses
  ```

  `references.type` names the parent type; a general column-to-column FK buys nothing Kitchen needs and doubles the resolution surface. `as` defaults to the **child type's name verbatim** (`ingredient_use`) — **no pluralization**, ruled 2026-09-01: guesses in identifier position are how declarations silently stop resolving. Two FKs from the same child type to the same parent without distinct `as` names is a **construction error**, refused when the registry is built rather than at first use (same discipline as `FormatRegistry`'s contested-extension rule). Resolution seat: a `RelationRegistry` mapping `(parent_type, rel_name) → { child_table, fk_column, child_types: FieldTypes }`, read by the parser as well as by both seats (§3.4). Closing R2 also closes the D6 seam — `references` is exactly what turns `recipe_id` from a plain TEXT column into the relation an aggregate can name.
* **R6 — DISCHARGED 2026-09-01 by Inc A2.** The registry is landed and `.cook` files in a real vault ingest; Inc B's "cookable now over MY recipes" is no longer fixture-bound. Obsidian and logseq are NOT unlocked by it: both claim `md`, so they additionally need the vault-flavor discriminator before they can be registered.
* **R7 (NEW, from Inc A2) — `VaultPath::page_file_from_name_chain` hardcodes `.org`.** A page homed in another format derives an ORG path as "its own file". Inc A2 makes this safe for READ-ONLY formats by refusing the write, but the refusal is not available to a writable non-org format. *Recommendation:* the derivation must ask the owning adapter for its extension before any SECOND WRITABLE format is registered. Not on the kitchen critical path (cook is read-only); it blocks markdown write-back and any future writable adapter.

* **R3 — the check-off-wins rule (§4) is asserted, not measured**, and may be moot: the observed schema has no `checked` field at all (P23). It is a product judgment; if Martin disagrees, only the reconciler PBT changes.
* **R4 — name-keying loses local state on a peer-side rename (§4).** No fix exists without a server-issued item id. *Recommendation:* accept it as a disclosed limitation for C1/C2 and, in parallel, ask the shopping app's author whether an `id` field can be added — that single field would delete this entire risk class. Needs Martin's call on whether the rename hole is acceptable before C2 is designed around it.
* **R5 — `redact_url` is a live credential-leak hazard the moment a path-token URL is configured (P26)**, and it is not kitchen-specific: any future sidecar using a capability URL inherits it. Fixing it in C1 is correct, but it may deserve landing on its own ahead of C1.

## 8. Stays out of scope

Cookidoo (NG1) · site importers (NG5/K5) · OpenFoodFacts network (NG4) · sharing / BG-6 (NG3) · barcode scan · meal planning calendar · webhook push for Inc C · making the certifier cover the conflict rule (§4, last row).

## 9. Staleness guard — re-run before acting on this plan

| Claim | Guard |
|---|---|
| P2/P3 `rest` GET-only | `grep -n "Only \`GET\` is supported" crates/holon-mcp-client/src/integration_config.rs` |
| P8 no general FK | `grep -rn "id_references\|reference_target" crates/holon-api/src/entity.rs` |
| P12/P14 closed variant set, no registry | `grep -n "pub enum Computation" -A 110 crates/holon-api/src/computation.rs` · `grep -n "is_def_var" crates/holon-api/src/expr_parser.rs` |
| P13 row-scoped Context | `grep -n "pub type Context" crates/holon-api/src/computation.rs` |
| P15 matview plant sink | `grep -n "block_matview_select_with_computed" -A 30 crates/holon-turso/src/schema_modules.rs` |
| P11 I3-2 open | `grep -n "I3-2" -A 20 docs/Plans/BlockGeneralization.md` |
| P18 Inc-5 blocked | `grep -n "Inc 5" -A 8 docs/Plans/BlockGeneralization.md` |
| P19 cooklang absent | `grep -in cooklang Cargo.lock` |
| P25 base_url ${VAR} | `grep -n "base_url" crates/holon-mcp-client/src/integration_config.rs` |
| P26 redact_url query-only | `grep -n "fn redact_url" -A 8 crates/holon-mcp-client/src/rest_oauth2.rs` — if it still only splits on `'?'`, the leak stands |
| P31 adapter routing — LANDED | `grep -n "formats: Arc<FormatRegistry>" crates/holon-filesystem/src/file_sync_controller.rs` · `grep -n "fn write_tier" crates/holon-core/src/file_format.rs` — absence means A2 was reverted |
| R7 page-file derivation still org-only | `grep -n 'push(format!("{segment}.org"))' crates/holon-filesystem/src/vault_path.rs` — a hit means R7 is still open |
| P32 markdown precedent unwired | `grep -rn "ObsidianMarkdownAdapter" crates/ \| grep -v holon-markdown` — hits outside its own crate/tests mean it went live |
| P29 brace guard | `cargo nextest run -p holon-kitchen -E 'test(unclosed_quantity_brace)'` |
| §3.4 correlated subquery still rejected by the fork | `cargo nextest run -p holon-turso --test agg_subquery_matview_spike` — probes A / `COUNT(*) FILTER` are the fork-capability guards. A RED there means a fork bump changed what plants, and §3.4's lowering must be revisited (it does NOT mean the tests are broken) |
| P13 eval-seat blast radius still 4 matches / 1 migrating caller | `grep -c "Computation::Lit" crates/holon-api/src/computation.rs` · `grep -rn "computation.eval(\|comp.eval(" crates --include='*.rs' \| grep -v /tests/` |
