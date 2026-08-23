# Root remodel feasibility (D4.c) — 2026-08-23

## RULING (Martin, 2026-08-23): NULL-parent REJECTED — read this first

**Model B below is NOT the design. Do not re-propose it.**

Martin vetoed storing NULL as the root parent. His reason, which the rest of
this document must be read against:

> the explicit `sentinel:no_parent` value exists BECAUSE a series of bugs came
> from `parent_id` being NULL for different, inconsistent reasons; the field was
> made non-optional in parse-don't-validate style with explicit values for each
> former meaning of null.

Model B reintroduces a NULL-with-meaning at the storage layer. The evidence for
that is in this very document: the "R4 seam" — the undo/inverse path meeting a
raw NULL — is that bug class reappearing, and the `block_fresh` view proposed to
fix it was re-deriving the meaning of NULL at every read site, which is exactly
what making the field non-optional was meant to end. One seam found by a test is
not evidence that the others do not exist.

**The chosen path is (a): the sentinel stays an explicit, typed, STORED value.**
The anchor row stays, the matview exclusion stays, `block_raw` keeps the
sentinel string; only `move_block`'s DESTINATION CONTRACT changes — see
"Implemented: ruling (a)" at the end.

**What survives as knowledge:** the IVM measurement is a FACT and remains
correct — the Turso fork *does* maintain NULL-parent rows through the
production matview chain, including `UPDATE parent_id → NULL` (7/7 checks,
`.lane-logs/rr-probe-6.log`). It is a fact about the engine, not a licence to
model root that way. The Loro finding also stands: Loro keeps a real
`TreeParentId::Root` and synthesizes the sentinel at its read boundary. That the
two legs spell root differently is a genuine asymmetry, and it is not licence
either — Loro's root is a structural position in a tree, which cannot be
confused with an absent field the way a SQL NULL can.

Everything below (i)–(v) is the original feasibility analysis, kept as the
record of what was measured and why the answer was still no.

---

**Original verdict (SUPERSEDED): GO.**

The question was whether rootness ("a block has no parent") can be modelled
without the hidden `sentinel:no_parent` row, so the write path can reach root
the same way the read path shows it.

It can, and the change is smaller than expected, because **most of the target
model is already built**. Two measurements decide it:

1. `block_raw.parent_id` is **already nullable**, and the Turso IVM fork
   maintains NULL-parent rows correctly through the exact production matview
   shape — including the `UPDATE parent_id → NULL` that a move-to-root *is*.
   Measured, 7/7 checks: `crates/holon-turso/examples/root_remodel_null_parent_probe.rs`.
2. The read boundary **already** maps a NULL `parent_id` to `EntityUri::no_parent()`
   (`crates/holon-api/src/block.rs:865-868`), and the Loro backend **already**
   keeps a real CRDT tree root (`loro::TreeParentId::Root`) and synthesizes the
   same sentinel at its own read boundary.

So the SQL sentinel row is a **projection artifact of one leg only — the SQL
write leg**. Loro never materializes it; the SQL reader already tolerates its
absence. Only the writer insists on it.

A prototype implementing the remodel takes the previously-failing move-to-root
from red ("Parent not found") to green.

---

## (i) Where the sentinel is load-bearing

`rg "sentinel:no_parent|no_parent"` over `crates/ frontends/ assets/` returns 871
hits, but that number is misleading. Broken down by token:

| Form | Count | What it is |
|---|---|---|
| `EntityUri::no_parent()` | 425 | The **domain value** for root — a typed constructor |
| `is_no_parent()` | 72 | Predicate on that value |
| `'sentinel:no_parent'` (SQL literal) | ~35 | The **string encoding**, in raw SQL |
| `sentinel:no_parent` (other) | rest | Comments, test names, fixture data |

This split is the heart of the finding. **The remodel targets the ROW and the
EXCLUSION, not the VALUE.** `EntityUri::no_parent()` is a legitimate typed
domain sentinel with a predicate — it is not a hidden row, and Loro already
synthesizes exactly it. Removing the value would be a 425-site refactor with no
benefit; removing the row is a bounded change.

### Classified: the genuinely load-bearing sites

**FK anchor** — `crates/holon-turso/src/schema_modules.rs:76-91` seeds a
self-parented row so roots satisfy the parent FK. *Justification is vacuous
under the remodel*: a NULL FK value is satisfied by definition, with no row to
point at. (Measured separately: the FK is DEFERRABLE INITIALLY DEFERRED and this
fork does not fire it for a single autocommit statement at all — it *is*
enforced inside `transaction()`. So the anchor was buying less than it appeared.)

**Matview exclusion** — `schema_modules.rs:477-484`, `WHERE b.id != 'sentinel:no_parent'`,
with a pinned test calling it "load-bearing for correctness". It is load-bearing
only *because the row exists*; delete the row and there is nothing to exclude.
Note this WHERE is also the exact `!=` shape our own
`turso_ivm_null_where_neq_repro.rs` documents as an IVM hazard — the remodel
deletes a known-hazardous predicate rather than adding one.

**Read-path filter (recursive CTE)** — `crates/holon-turso/sql/schema/blocks_with_paths.sql:17`,
base case `WHERE parent_id LIKE 'sentinel:%'`. Under the chosen design (below)
this stays correct unchanged.

**Loro projection / CRDT tree root** — *not load-bearing.* `LoroBackend` stores
blocks in a real `LoroTree`; root creation is `tree.create(None)`, move-to-root
is `tree.mov_to(target, loro::TreeParentId::Root, 0)`
(`crates/holon-loro/src/loro_backend.rs:3048`), and `EntityUri::no_parent()` is
synthesized on read when `parent_tree_id` is `None`
(`loro_backend.rs:1107-1110`). The string is never written to Loro. **Loro is
already the target model.**

**Org write-back** — *not load-bearing.* Org files store bare block IDs and
hierarchy; the sentinel never reaches disk. `home_authority.rs:292` and
`file_sync_controller.rs:1387` compare against `EntityUri::no_parent()`, i.e.
against the value, which survives.

**Focus/navigation roots** — `matview_focus_roots.sql` does **not** touch
`parent_id` at all (it filters `navigation_history` on `block_id IS NOT NULL`).
Unaffected.

**PBT reference model** — `composed/seed_primitives.rs:67`,
`transitions/start_app.rs:172`, `block_state.rs:83,212` all use the typed
`EntityUri::no_parent()`, never a raw string. Unaffected by a storage-encoding
change.

**MCP/SQL views** — the only other `parent_id` predicates in shipped SQL are
`journal_day_pages_matview.sql:39` (`= 'block:journals'`, a specific parent, not
root) and `assets/queries/action_discovery.sql:13`
(`query_src.parent_id = action_src.parent_id`, a sibling self-join). See the
risk register for the latter.

---

## (ii) The model, and what the IVM fork can actually do

### Measured: Turso IVM and NULL parents

`crates/holon-turso/examples/root_remodel_null_parent_probe.rs`, run at this
pin, log `.lane-logs/rr-probe-6.log` — **7/7 PASS**. It builds the real
production shape (an edge-aggregation matview, then `block` chained on top via
`LEFT OUTER JOIN`) and checks:

| Check | Result |
|---|---|
| Root row inserts with no anchor row present | PASS |
| `block` matview carries NULL-parent rows | PASS |
| `UPDATE parent_id → NULL` (**move to root**) reflected in the matview | PASS |
| `UPDATE NULL → parent` (move off root) reflected | PASS |
| A `WHERE parent_id IS NULL` roots matview tracks the plain SELECT across both moves | PASS |
| Recursive CTE anchored on `parent_id IS NULL` reaches every block, full paths | PASS |
| Two roots do not self-join as siblings under NULL | PASS (behaviour change, see risks) |

**There is no IVM wall.** The one documented IVM NULL hazard is `!=` against
NULL; `IS NULL` is the fork's correct predicate and the remodel *removes* the
`!=` rather than adding one.

### The chosen design: store absence, project the sentinel

Two candidate models were considered.

**Model A — NULL all the way up.** `parent_id IS NULL` in storage *and* in the
`block` matview; `Block.parent_id` becomes `Option<EntityUri>`.
This is the "most correct" model, but it forces `Block.parent_id: EntityUri` →
`Option<EntityUri>`, which is the 425-site change, and it silently flips every
`=`/`!=`/join on `parent_id` into three-valued logic.

**Model B — store NULL, synthesize the sentinel at the read boundary (CHOSEN).**
`block_raw` stores root as `parent_id IS NULL` (no anchor row, FK satisfied by
definition). The `block` matview projects
`COALESCE(b.parent_id, 'sentinel:no_parent') AS parent_id`. Every reader keeps
the exact spelling it uses today.

Model B is chosen because it is **precisely what Loro already does**: keep a
real root in storage, synthesize the sentinel on read. It makes the SQL leg
agree with the CRDT leg instead of inventing a third convention. It deletes the
hidden row and the exclusion — which is what Martin asked — without a
domain-type migration, and it keeps the sibling self-join semantics that Model A
would silently change.

The prototype implements Model B.

---

## (iii) Migration

Existing vault DBs carry a seeded `sentinel:no_parent` row and root blocks whose
`parent_id` is the sentinel *string*.

A one-shot rewrite at boot is acceptable and is both **detectable and
idempotent**:

```sql
UPDATE block_raw SET parent_id = NULL WHERE parent_id = 'sentinel:no_parent';
DELETE FROM block_raw WHERE id = 'sentinel:no_parent';
```

- **Detectable**: `SELECT 1 FROM block_raw WHERE id = 'sentinel:no_parent'` is
  the exact precondition.
- **Idempotent**: after it runs, both statements match zero rows.
- **Ordering**: the UPDATE must precede the DELETE, or the deferred FK fires at
  COMMIT. Run both inside one `transaction()` (the fork enforces deferred FKs
  there, so a mis-ordered migration fails loud rather than silently).

Not yet written — it belongs to the landable increment.

---

## (iv) Cost

**Production write paths: already funneled.** All op-driven parent writes now
render through one helper (`SqlOperationProvider::parent_column_literal`), so
production needed two call sites changed, not dozens.

**The real cost is raw-SQL test fixtures**: ~35 occurrences of the bare literal
`'sentinel:no_parent'` across ~20 files hand-write it as a parent value. With
the anchor row gone these fail the deferred FK inside transactions. This is
mechanical (`'sentinel:no_parent'` → `NULL` in the parent position) and is the
bulk of the remaining work.

**Invariants to re-prove**: `inv-matview-consistent-with-recompute` (runs inside
the keystone), `inv-no-orphan`, `inv-no-parent-cycles`, and the
`block_matview_select_exact_shape` pin (updated in the prototype).

### Risk register

| # | Risk | Status |
|---|---|---|
| R1 | IVM mishandles NULL in the chained matview | **Refuted by measurement** (7/7) |
| R2 | Loro cannot express root | **Refuted** — it already does, natively |
| R3 | Org write-back depends on the sentinel | **Refuted** — sentinel never reaches disk |
| R4 | Readers bypassing the `block` matview see a raw NULL | **REAL, OPEN** — see below |
| R5 | `action_discovery.sql` sibling self-join on `parent_id` | Neutralized by Model B (matview projects the sentinel, so roots still join). Would be a real semantic change under Model A. |
| R6 | Existing vaults carry sentinel rows | Bounded; migration above |
| R7 | Fixture migration misses a site | Caught loudly by the deferred FK, not silently |

**R4 is the one genuine open seam.** Two decoders for a block row exist and
disagree about NULL: the hand-written `TryFrom<StorageEntity> for Block`
(`block.rs:865`) handles it, while the derive-generated `TryFromEntity`
(`holon-macros/src/entity.rs:160-169`) requires a string, because
`Block.parent_id` is a total `EntityUri`. Model B's COALESCE covers every reader
that goes through the `block` matview, but **not** readers that touch
`block_raw` directly. Measured: 3 tests fail this way
(`undo_move_block_e2e` ×3 shape, `ingest_property_removal`), all via the
undo/inverse path, which reads the *old* raw column value
(`read_field_old_value(id, "parent_id")`) and carries a `Value::Null` back into
`set_field`. The fix is to apply the same NULL→sentinel mapping at that raw
read, not to weaken `TryFrom<Value> for EntityUri` (a probe doing that went
green but maps *every* null URI field to root — a silent fallback, rejected).

---

## (v) Recommendation and the strongest counter-argument

**GO**, on Model B, as a landable increment.

The decisive measurement: `UPDATE block_raw SET parent_id = NULL` is correctly
maintained by the IVM fork through the production matview chain, and the
previously-red move-to-root goes green with the hidden row deleted.

**The strongest counter-argument**: Model B does not remove the sentinel *value*
— it moves the place where the value is invented, from a seeded row to a
`COALESCE`. A reviewer can fairly say we swapped one synthetic root for another
and that Model A is the only honest remodel.

The answer is that a synthesized value at a read boundary is categorically
different from a row in the data table: it cannot be selected, joined,
mutated, orphaned, or accidentally rendered as a block, and it needs no
exclusion filter to stay hidden. That is exactly the shape Loro already ships,
and the SQL leg becoming consistent with it is a genuine simplification, not a
rename. Model A remains available later as a pure type change, and Model B does
not block it — it removes the row, which is the part that is hard to undo.

## Gates measured on the prototype

| Gate | Result | Log |
|---|---|---|
| NULL-parent IVM probe | **7/7 PASS** | `.lane-logs/rr-probe-6.log` |
| `move_block_to_root` BEFORE the remodel | **RED** — "Parent not found" | `.lane-logs/rr-red-3.log` |
| `move_block_to_root` AFTER | **PASS** | `.lane-logs/rr-green-2.log` |
| `just keystone-smoke` (incl. `inv-matview-consistent-with-recompute`) | **GREEN** | `.lane-logs/rr-keystone2.log` |
| `just hand-authored` | **GREEN**, 9 passed / 0 failed | `.lane-logs/rr-handauthored.log` |
| `-p holon-architecture-tests` | **GREEN**, 6/6 | `.lane-logs/rr-gate-arch.log` |
| `-p holon-turso -p holon-core` | 399/400 — the one failure (`matview_lease_actor::only_the_last_release_reaps`) is a load-dependent flake that passes in isolation and touches no changed file | `.lane-logs/rr-gate-turso-core-2.log`, `.lane-logs/rr-lease.log` |
| `-p holon-turso -p holon-core -p holon-api -p holon-app -p holon` (final) | 1410/1441; every remaining failure **also fails at base** | `.lane-logs/rr-g-suite.log` |
| `cargo fmt --all -- --check`, clippy on the touched crates | clean | `.lane-logs/rr-g-fmt-arch.log`, `.lane-logs/rr-g-clippy2.log` |

Which failures are pre-existing was established by MEASUREMENT, not argument:
the whole tree was reverted to `@-` content (`jj file show`, new files removed),
the suite replayed, and the tree restored with every file sha-verified against
its pre-probe hash. Base itself fails 26 + 1 timeout in `-p holon`. Diffing the
two failure sets leaves **zero** failures attributable to this change, and
identifies two that fail at base but pass here
(`stress_tests::test_parallel_sync_operations`, and
`turso_storage_repros::…cursor_filtered_main_panel_delivers_at_vault_scale`) —
load-dependent, not fixed by this work. Logs: `.lane-logs/rr-BASE-holon.log`,
`.lane-logs/rr-BASE-app.log`.

For the flake / known-red census, the pre-existing failures cluster as: the
`no such column: _version` group (the sync, stress, reliability, e2e-backend,
json-aggregation and turso-storage-pbt suites);
`create_page_from_link::recreating_a_renamed_pages_old_name_yields_a_distinct_page`,
which names its own reason (interim ADR 0029 D1b refuses what §5.3 requires);
and
`holon-app::backlinks_section_seed::fresh_seed_places_backlinks_section_below_outline`.

## Implemented: ruling (a)

Model B was reverted to base content in full (37 files restored byte-for-byte
against `@-`, 3 removed). What ships is (a), and it is small:

**`move_block_prefetched` (`crates/holon-core/src/traits.rs`)** — the root
sentinel becomes a legal explicit destination. The destination-existence read
(`get_by_id`) is skipped for that ONE id, because the sentinel is the FK anchor
row: it always exists in `block_raw` and is deliberately excluded from the
`block` matview the read goes through, so the lookup would search the one
relation built to hide it. Its page-ness is supplied directly as `false` — a
fact about the anchor, not a lookup. The no-pages-under-non-pages guard is NOT
skipped; it runs on that value like any other. Nothing else in the chokepoint
changes, and the "Parent not found" error now names the destination it could
not resolve.

Covered by `crates/holon-app/tests/move_block_to_root.rs`: a leaf under an org
page moves to root through the production read path, is still readable, the page
no longer holds it — and, in the same test, a move to a NON-sentinel unknown
destination is still refused with "Parent not found", pinning that the skip is
keyed on the exact sentinel id rather than on "unresolvable".

RED on base: `.lane-logs/rr-red-3.log` ("Parent not found"). GREEN after:
`.lane-logs/a-suite.log`.

### Two asymmetries this surfaced (not addressed by (a), flagged deliberately)

1. **A root block cannot be moved at all.** The chokepoint reads the moved
   block's own parent and refuses `None` with "Cannot move root block", and a
   sentinel parent reads as `None`. So `move_block` can now move a block TO
   root but never FROM root. The refusal-ordering is why the unknown-destination
   assertion in the test runs BEFORE the move to root — afterwards it is masked
   by this earlier refusal.
2. **A PAGE cannot be moved to root**, because the guard receives
   `parent_is_page = false` for the anchor and a page may only be reparented
   under a page. Pages are seed-only at `no_parent` today (ForkB-B1 R8), so
   nothing regresses — but "create a page at root" and "move a page to root"
   now differ, which the re-home story will have to rule on.

### Gates

`-p holon-core -p holon-turso -p holon -p holon-app --features
holon/test-helpers` 942/970 with every failure also failing at base;
`-p holon-architecture-tests` 6/6; `just keystone-smoke` ok (4/0);
`just hand-authored` ok (9/0); `cargo fmt --all -- --check` clean; clippy on
`holon-core` clean. The sibling-order invariants named in the brief
(`inv-live-children-match-ref`, `inv-loro-children-match-ref`,
`inv-blocks-match-ref/*`) run inside keystone-smoke and hand-authored and are
green there.

### A hazard worth recording (from the abandoned Model B work)

A blanket `sed` of `'sentinel:no_parent'` → `NULL` across the tree silently
damaged two things a compile could not catch: a `COALESCE` default became
`COALESCE(parent_id, NULL)` (a no-op), and `WHERE id != 'sentinel:no_parent'`
became `WHERE id != NULL` (never true). The matview's pinned exact-shape test
**co-mutated with the code** and stayed green — the one test written to catch
exactly this. Rule: a mass literal edit must never be trusted against a suite
that contains the literal; verify against the artifact the code produces (here,
the view SQL stored in `sqlite_master`), not against the suite.
