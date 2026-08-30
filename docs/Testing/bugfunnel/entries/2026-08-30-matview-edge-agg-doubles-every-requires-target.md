---
id: 2026-08-30-matview-edge-agg-doubles-every-requires-target
date: 2026-08-30
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  The `block_requires_agg` IVM aggregate accumulated every junction row twice,
  so the `block` matview served `["block:x","block:x"]` while the base
  `block_requires` junction held one clean row, and org write-back wrote the
  doubled target into Martin's vault drawers.
---

## Bug

Four `:REQUIRES:` drawers in Martin's live vault file
`/Users/martin/Workspaces/pkm/holon-pkm/Projects/Holon/Dogfooding & Agents.org`
came back from a Holon write-back with their target written twice — e.g.
`:REQUIRES: handoff-md-migration handoff-md-migration`. Every committed
revision of that file holds single values, so a Holon write produced it.
Found by Martin outside any automated test; investigated under task #10.

The vault file was only the visible tip. The defect is **vault-wide**: of the
edge arrays recovered from the store's matview state, **217 are doubled and 36
are clean**, and the 36 clean ones are all `contributes_to` targets
(`compass-*`). Every `requires` array in the store was doubled; no
`contributes_to` array was.

## Root cause

**Not a Holon writer. The store served the doubled value.**

The doubling exists in exactly one place: the persisted DBSP state of the
`block` matview, table `__turso_internal_dbsp_state_v1_block`. A recovered
materialized row for `block:ac6-dogfooding` carries, at **weight 1**, the value

```
["block:claude-sessions-under-topics","block:claude-sessions-under-topics",
 "block:handoff-md-migration","block:handoff-md-migration",
 "block:open-questions-inbox","block:open-questions-inbox",
 "block:petri-rank-mcp-claim-release","block:petri-rank-mcp-claim-release"]
```

while the base junction row set recovered from the same file is clean and
single (`block_requires` is `PRIMARY KEY (block_id, required_id)` —
`crates/holon-turso/sql/schema/block_requires.sql`, so it cannot hold a
duplicate at all).

The production hydration shape (`crates/holon-turso/src/schema_modules.rs`,
pinned by `block_matview_select_exact_shape` and
`edge_agg_view_select_groups_targets_by_source`) is a per-junction
pre-aggregate joined 1:1:

```
block_requires_agg = SELECT source_id, json_group_array(target_id) AS vals
                     FROM block_requires GROUP BY source_id
block               = block_raw b LEFT OUTER JOIN block_requires_agg
                        ON block_requires_agg.source_id = b.id
```

The 1:1 join cannot fan out, so the doubling was produced **inside the
`block_requires_agg` aggregate**: its input z-set carried each junction row
with multiplicity 2. The junction write is a coarse wipe-and-rebuild —
`edge_field_replace_sql`
(`crates/holon/src/core/sql_operation_provider.rs:1032-1053`) emits one
`DELETE FROM block_requires WHERE block_id = …` followed by one plain `INSERT`
per target. A `DELETE`+`INSERT` of an unchanged row set must net to zero in the
aggregate; a multiplicity of exactly 2, uniform across every block in the
vault, is the signature of the `INSERT` half reaching the persisted aggregate
state while the `DELETE`'s retraction did not.

Window and trigger: the DB snapshot taken 2026-08-28 22:10 contains **zero**
doubled arrays; the live DB (`~/.config/holon/holon.db`, mtime 2026-08-29
03:06) contains them. The v0.0.18 release binary booted on the real vault at
~01:06 in that window — a binary booting over matviews persisted by a
different build, the same version-skew boot recorded in
`2026-08-28-matview-version-skew-false-cycle-boot-fail`, and the same family as
BugFunnel 90 (`matview_reboot_duplicate_repro`: persisted DBSP state surviving
a restart and taking a re-ingest's deltas unconsolidated). The `requires`/
`contributes_to` asymmetry is consistent with per-junction aggregates being
recreated independently across that boot; which aggregates were recreated and
which survived was NOT established.

Why the doubled value reached disk: the org write-back renders from the block
cache, which is fed by an IVM watch over the `block` matview — so it read the
doubled array. `models.rs:931-941` then renders `self.requires` sorted and
space-joined with no fold, producing `:REQUIRES: x x`. The renderer is a
faithful carrier, not a producer: it has a single source for the drawer and
`insert`-overwrites any properties-bag copy.

Exonerated by direct reading, each a candidate before the store evidence
landed:

* **Org parser** — dedupes at three points (`parser.rs:692`, `:948`, and
  `ids.contains` in `resolve_dependency_edge` at `:1573`), and the same guards
  are present at the older revisions that shipped in the release binary. Its
  splitter (`parse_edge_targets`, `:1492`) is a single correct
  `split(',' | whitespace)`.
* **Every Loro edge writer** — `set_block_edge_field` / `set_block_requires`
  (`crates/holon-loro/src/loro_backend.rs:3097`, `:3153`),
  `loro_seams.rs:529`, `block_cell_registry.rs:769` all serialize a slice
  wholesale. None appends; no read-modify-write concatenation exists.
* **CRDT merge (H2)** — the edge field is a JSON *string* under one LoroMap
  key, an LWW register. Concurrent replicas resolve last-writer-wins; they
  cannot concatenate.
* **Two-source render (H3)** — `read_properties_from_meta`
  (`loro_backend.rs:454`) strips all four edge columns from the properties bag
  over `EdgeField::ALL`, and `read_edge_from_meta` (`:605`) is a faithful JSON
  decode. No Loro meta copy in the live DB is doubled.
* **`TursoSinkReader`** (`crates/holon/src/storage/turso_sink_reader.rs:47-55`)
  and `CacheBlockReader`'s fallback SQL (`holon-app/src/turso_seams.rs:66-81`)
  hydrate edges with **correlated subqueries over the base junction**, which is
  clean — these paths would not have doubled it.

## Missing piece

**ORACLE — but not for want of a matview oracle.**
`inv-matview-consistent-with-recompute`
(`crates/holon-integration-tests/src/pbt/invariants/bodies/matview_recompute_matches.rs`)
already enumerates EVERY materialized view from `sqlite_master` and compares
what IVM maintains against that view's own defining SELECT re-executed
(`SutMatviews::matview_recompute_snapshot`,
`frontend_slice/components.rs:2776`). `block_requires_agg`'s defining SELECT
reads the base junction, which stayed clean — so that invariant WOULD have gone
red on this state. It never got the chance: the composed keystone has no reboot
transition, so a run never reaches a second boot over persisted DBSP state.

The harness that DOES reboot —
`crates/holon-integration-tests/tests/store_suite/matview_reboot_duplicate_repro.rs`,
whose `block_matview_with_edge_fields_no_duplicates_after_reboot` boots twice
and writes `tags` + `requires` through the production Loro edge path — stopped
its assertions at ROW counts: no duplicate ids, and per-id matview count ==
`block_raw` count. A doubled target lives INSIDE one row, so `block` still has
exactly one row per id and every one of those assertions passes while the
served array reads `["block:x","block:x"]`. That harness reached the state and
had nothing to say about it.

**Secondary COVERAGE.** No transition in the composed catalog restarts the app
over an existing database, so the strong recompute oracle cannot be brought to
bear on any post-reboot state at all.

## Repro attempt — the mechanism did NOT reproduce

`crates/holon-turso/tests/edge_agg_retraction_across_reboot.rs` states the
retraction contract over the REAL production aggregate SQL (verified verbatim
against Martin's `sqlite_master`:
`SELECT block_id AS source_id, json_group_array (required_id) AS vals FROM
block_requires GROUP BY block_id`) and drives the real
`edge_field_replace_sql` wipe-and-rebuild shape through `handle.transaction`.
Three mechanisms were tried:

1. unchanged-set wipe-and-rebuild within one boot — **green**;
2. the same replay on a second boot, view and DBSP state persisted — **green**;
3. a later boot that re-issues `CREATE MATERIALIZED VIEW IF NOT EXISTS` over
   persisted state (what `ensure_schema` does every start) and then replays —
   **green**.

So the "DELETE's retraction is lost across a reboot" hypothesis, in its three
plausible forms, **does not hold** at this level. The three tests are kept as a
green contract: they pin the behaviour that must not regress and document what
was excluded.

Also excluded: the historical multi-junction cross-product view, which the
codebase records as a fixed fan-out bug producing exactly this corruption
(`schema_modules.rs:450-461` — "a block with 3 tags and 1 requires row yielded
`requires = ["R","R","R"]` … masked for `tags` because sets dedup at parse").
Martin's database carries the CURRENT chained shape (`block_requires_agg` plus
1:1 LEFT JOINs, read from its `sqlite_master`), so the old shape is not the
cause.

**ENVIRONMENT residue.** What still separates the harness from Martin's machine
is the version-skew boot — a binary opening matviews persisted by a DIFFERENT
build, the same condition as
`2026-08-28-matview-version-skew-false-cycle-boot-fail`. Test environments
always create their matviews fresh from the running binary, so that path does
not exist in any harness, and reproducing it needs the at-rest file surgery
`crates/holon-turso/tests/matview_version_skew_boot.rs` performs. That is the
untried lane.

## Remedy

Open — nothing fixed here; this entry records the diagnosis. The producer is
store-side (the fork's IVM aggregate maintenance), not a Holon writer, so no
small unambiguous Holon-side fix applies and none was attempted.

Note that the already-landed `EdgeField::param_value` fold
(`2026-08-30-edge-field-duplicate-target-wedges-write`) does NOT cover this: it
folds on the write leg, while this doubling enters on the read leg and reaches
org write-back through the renderer, which has no fold.

**Detector LANDED** (the gap's own remedy, not the engine fix):
`edge_array_multiset_mismatches` in
`crates/holon-integration-tests/tests/store_suite/matview_reboot_duplicate_repro.rs`,
asserted at the end of
`block_matview_with_edge_fields_no_duplicates_after_reboot` — the harness that
already reboots and already writes edge fields through the production path, per
the dedicated-PBTs-share-keystone-structure directive (no parallel harness).
For every block it expands the hydrated edge array (`json_each`) and the base
junction's targets, sorts both, and compares them element-for-element,
iterating `EdgeField::ALL` so a fifth edge field cannot be half-covered.

Full multiset equality, deliberately not a cardinality check: comparing
`json_array_length` against `COUNT(*)` would catch a doubled or dropped target
but would PASS a same-length substitution (`[x,y]` served as `[x,x]` is length
2 against count 2). Sorting is what makes the comparison meaningful rather than
flaky — order is not semantic for an edge set and the junction hydration has no
`ORDER BY`. `IS NOT` rather than `!=` so a block with no targets on either side
(both aggregates NULL) compares equal.

Teeth proven end-to-end on the SHIPPED comparator, with both failure modes:

* **Doubling** — the class the detector is named for. The matview side is
  expanded twice (`json_each(...) UNION ALL json_each(...)`), modelling an
  aggregate that serves each target twice over a clean junction. Fires with 6
  findings naming real values, e.g. `block:blk-a.requires: matview array =
  [block:blk-b␟block:blk-b], junction = [block:blk-b]` and `block:blk-a.tags:
  [proj␟proj] vs [proj]`; zero-target blocks stay correctly silent. Log
  `scratchpad/hunt/reboot_red3.log`.
* **Element substitution** — one junction target rewritten so the two sides
  differ in VALUE at equal length: fires with exactly one finding,
  `block:blk-a.requires: matview array = [block:blk-b], junction =
  [block:zzz]`, while the other three fields stay green. Proves the detector
  catches same-length corruption a cardinality check would miss, and that it is
  targeted rather than blanket-failing. Log
  `scratchpad/hunt/reboot_red2.log`.

Both probes reverted; green on true data afterwards
(`scratchpad/hunt/store_d4fix.log`).

An earlier probe (`reboot_red1.log`) is NOT cited as evidence for either: it
predates this comparator (it panics at the superseded count-based assertion) and
it inverted the predicate rather than corrupting data, so it demonstrated only
that `EdgeField::ALL` is walked and the panic path is reachable — not that any
corruption class is detected. The doubling red above replaces it.

### Disclosures on this lane's evidence

**The keystone was NOT green while this landed, and it is not this diff.** Across
four runs of identical code the composed keystone gave four outcomes: one pass
(178s), one failure classified 12 NOVEL panics
(`inv-embedded-page-collapsed-lazy` — an embedded page missing its
`expand_toggle`), one PASS-WITH-NOTE of 23 known-red `editor-text-mirror`, and
one 1200s timeout. Non-causality is structural, not asserted: `store_suite` is
its own test binary, `general_e2e_composed_pbt.rs` contains zero references to
it, this lane changed no production code in the keystone's path, and the FIRST
run passed with the change already present. The novel
`inv-embedded-page-collapsed-lazy` signature is a pre-existing unregistered red
needing its own owner (log `scratchpad/hunt/gate_keystone_smoke.log`); nearest
prior art is `2026-08-08-trailing-open-nested-page-chevron-never`.

**The contract tests transcribe constants; nothing enforces the copy.**
`edge_agg_retraction_across_reboot.rs` hard-codes `AGG_SELECT` and the
wipe-and-rebuild shape rather than calling the production builders. They were
checked character-for-character against the shipped descriptor
(`schema_modules.rs:379-385`) and against Martin's live `sqlite_master`, and the
detector's own four junction triples likewise match the shipped descriptors. But
the in-repo pin `edge_agg_view_select_groups_targets_by_source` uses a SYNTHETIC
descriptor whose columns are literally named `source_id`/`target_id`, so it does
NOT pin the real `block_id`/`required_id` names. The transcription is correct
today and would not fail if production drifted.

Still open, in order:

1. **The engine fix.** Reproduce under version skew — the one untried
   condition — using the at-rest file surgery from
   `matview_version_skew_boot.rs`, then hand it to the turso peer, same family
   as `turso-ivm-quoted-identifier-desync` and the false-cycle fix.
2. **Bring the strong oracle to the reboot.** The multiset check landed here is
   narrow by design — one harness, one state; `inv-matview-consistent-with-recompute`
   is the general statement and already exists. The composed keystone having no
   RESTART transition is the structural gap that let this whole family escape;
   adding one is a keystone-transition feature under `holon-feature`, tracked in
   the vault under Testing/Keystone, not here.
3. Martin's DB has since self-repaired (the matview rebuild landed with the
   turso false-cycle fix on `main`), so the live vault drawers heal on the next
   write-back. The stale doubled drawers already written to disk are single
   again on re-render because the parser folds on read.
