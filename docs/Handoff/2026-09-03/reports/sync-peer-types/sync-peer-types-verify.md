# Verify `sync-peer-types` — REFUTED

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/sync-peer-types` (`pwd` printed in every driver log).
`@ = df4ba007`, parent `89e2efea main`. 17 files, +781/-20. `files.sql` on disk carries `properties`/`property_kinds` (lines 7-8).

## R1 (blocking) — `just keystone-smoke` is RED, caused by this lane

`lane-logs/verify-keystone.log:5192` → `test result: FAILED. 3 passed; 1 failed` (exit 101).

```
:940 Test failed: HeadlessFrontendComponent sql_query failed
(SELECT id, email, role, organization, phone, properties, property_kinds, display_name FROM "person"):
Query error: stored property kinds are corrupt: property_kinds is not valid JSON
(expected value at line 1 column 1): "a"
```

Chain: the datatype axis reads the persisted column list off the registry and
"a create fills every one of them"
(`crates/holon-integration-tests/src/pbt/typed_entity_schemas.rs:1-75`). This lane
added `property_kinds` to `assets/default/types/person.yaml`, so the generator now
fills a loud-parsing JSON-kind column with an arbitrary short string. The failing
SELECT names a column that exists only because of this diff — impossible on
`89e2efea`. The lane's one green keystone-smoke (`lane-logs/g2-keystone.log`) was a
lucky case draw; the gate is randomized.

## R2 — the two bugfunnel entries were never touched

Neither `docs/Testing/bugfunnel/entries/2026-09-02-a-shopping-item-can-never-be-added-in-holon.md`
nor `…-deleting-a-shopping-item-is-undone-by-the-next-sync.md` exists in this WS, and
neither appears in `jj diff --name-only`. They live only in unlanded lanes; in
`.claude/worktrees/kitchen-dogfood/` both still read `status: OPEN`. The lane report's
"Entries FIXED, with evidence" table cites files this tree does not contain.
`bugfunnel.py counts`: TOTAL 609, FIXED 314, OPEN 246 — unchanged by this lane.

## R3 — teeth: rule (1) has none; rule (2) has teeth on one of its two routes

Backups `scratchpad/{td,sop}.bak`; restore verified byte-identical against
`scratchpad/teeth-baseline.sha` (`RESTORE OK byte-identical`), no jj/git write used.

- Inversion A — `type_declaration.rs:95` `require_engine_stamp_has_a_home(type_def)?;`
  → `let _ = …;` (guard removed from the ONE seam). **All 6 tests still green**
  (`lane-logs/verify-teeth-inverted.log:125`). The declare-time tests call the two
  helper fns *directly* (`:205`, `:238`), so nothing pins that `register_write_authority`
  still calls them. Deleting the wiring is a silent no-op for the suite.
- Inversion B1 — `sql_operation_provider.rs:1694` `prepare_delete`'s tombstone branch
  forced to the hard-delete arm. **All 6 tests still green** (same log). There are two
  delete routes: `execute` (`:3649`) and `prepare` (`:4528` → `prepare_delete`); the
  soft-delete logic is duplicated and the prepare route is uncovered.
- Inversion B2 — `:3649` `"delete" if self.write_schema.tombstone_column().is_some()`
  → `if false`. **RED for the right reason** (`lane-logs/verify-teeth-b.log:117`):
  `assertion left == right failed: a soft-deleted row stays on the write table until the
  peer has been told; rows: [("Milch", None)] left: 1 right: 2`. Rule (2) has teeth here.

## Confirmed (reproduced this session)

- `just hand-authored` — **GREEN**, `lane-logs/verify-hand.log:6347`
  `test result: ok. 9 passed; 0 failed … 328.04s` (the gate the lane could not run).
- gate-tests (`-p holon -p holon-kitchen -p holon-app --features holon/test-helpers`) —
  `lane-logs/verify-gatetests.log:975` `759 tests run: 754 passed, 5 failed, 10 skipped`.
  The 5 reds are EXACTLY `holon::e2e_backend_engine_test` {basic_query_execution,
  create_and_delete_workflow, multiple_operations_sequence, operation_triggers_stream_update,
  query_and_watch_stream}; `cannot modify materialized view block` occurs 5× — the
  pre-existing signature, nothing new.
- Both new e2e tests and all 4 `type_declaration` tests PASS (`:328-344`, `:618-619`).
- `cargo check -p holon-gpui --all-targets` exit 0; `cargo fmt --all -- --check` exit 0
  (`lane-logs/verify-final-driver.log`).
- Generic-ness: `rg -i 'shopping|kitchen|cook'` over the three engine files yields only
  `type_declaration.rs:198` (a doc comment citing the bugfunnel slug) and `:257`
  (inside `#[cfg(test)] mod tests`). Clean.

## Design notes (reported, not remedied)

- `crates/holon-turso/src/schema_modules.rs` has **no derived-DDL emission path at all** —
  every in-tree entity table is hand-written SQL (`blocks.sql`, `files.sql`, `identity.sql`, …);
  only yaml-declared free-standing types get generated DDL. So `file` is not an exception,
  but `File`'s `#[derive(Entity)]` (`crates/holon-filesystem/src/file.rs:44-49`) and
  `files.sql` remain two unguarded sources of truth: the mismatch this lane fixed was
  caught only by a keystone run, not by any structural check.
- `tombstone_statements` (`sql_operation_provider.rs:1716`) builds SQL by string
  interpolation with `'`-doubling rather than binding.

## Verdict: REFUTED — R1 blocks landing; R2 is unfinished bookkeeping; R3 is a teeth gap.

---

# Rev 2 — CONFIRMED (with one classification)

Same WS, uncommitted, 25 files / +1220-48. Teeth files restored and
`shasum -a 256 -c scratchpad/rev2-baseline.sha` = **OK** for both (no jj/git write).

## R1 — CONFIRMED

`ColumnValueKind` is a real kind on `FieldSchema`; the axis filters on the KIND
(`typed_entity_schemas.rs:81` `!f.primary_key && !f.value_kind.is_engine_owned()`),
and `require_engine_stamp_has_a_home` matches on
`field.value_kind == ColumnValueKind::OverflowProperties`
(`type_declaration.rs:118`), refusing a same-named column that does not declare
the kind.

3× `just pbt general 8` with the lane's documented setting
(`HOLON_PBT_WEIGHTS='DeclareTypedSchema:300,CreateTypedEntity:300' HOLON_PBT_FORCE_FULL=1`):

| run | `property_kinds is not valid JSON` | result | log |
|---|---|---|---|
| 1 | **0** | FAILED (`inv-sql-budget ToggleState.sql_read_repeat` 80x over ratchet 64 — a known unrelated signature) | `lane-logs/v2-weighted-1.log` |
| 2 | **0** | `ok. 4 passed` (173.14s) | `lane-logs/v2-weighted-2.log` |
| 3 | **0** | `ok. 4 passed` (171.86s) | `lane-logs/v2-weighted-3.log` |

Special-case grep over the changed files: the only `property_kinds` string
literals are `FieldSchema::overflow_pair()` (`entity.rs:520`, the ONE place the
pair is modelled), two error-message strings (`type_declaration.rs:135-136`) and
the macro attribute token map (`holon-macros/src/entity.rs:75`). **No
special case in the axis or the read path — no finding.**

## R3a — CONFIRMED

Inversion A (both `?` calls in `register_write_authority` → `let _ = …`):
`lane-logs/verify-r2-invA.log` — `8 tests run: 7 passed, 1 failed`, sole failure
`core::type_declaration::tests::declaring_an_undeclarable_type_is_refused_by_the_public_path`
(`type_declaration.rs:329`). The seam wiring is now pinned; the Rev 1 gap is closed.

## R3b — CONFIRMED

Inversion B (`delete_plan`'s tombstone arm → `if false && op_name == "delete"`):
`lane-logs/verify-r2-invB.log` — `8 tests run: 6 passed, 2 failed`, both failures
being the two route pins at `sql_operation_provider.rs:5733`:
`single-op: the row must survive a soft delete until the peer has been told; rows: []`
and the same for `batch:`. Both delete routes now have teeth; the Rev 1 uncovered
prepare route is gone (one `delete_plan`, one `delete_statements`).

## Classification — `inv-main-panel-rows-match-focus DROPPED ROW(S) IN MAIN PANEL`

**PRE-EXISTING (rare flake) — NOT caused by this lane.**

Population: `just pbt general 8`, alternating lane / base, 6 complete runs each.
Base = `git -C /Users/martin/Workspaces/pkm/holon archive 89e2efea | tar -x` into
`scratchpad/base89` (`Cargo.toml` present and non-empty; its `person.yaml`
contains 0 occurrences of `property_kinds`). Counts from `scratchpad/count.sh`:

| tree | runs | runs with the signature | rate | runs red on any signature |
|---|---|---|---|---|
| lane (rev 2) | 6 | **0** | 0/6 | 3/6 (`inv-sql-budget`, `inv-editor-text/mirror`, `inv-drawer-open-matches-ref`) |
| base `89e2efea` | 6 | **0** | 0/6 | 2/6 (`SutOrgRender … NotFound`, `inv-sql-budget`) |

Neither arm reproduced it in 12 runs, so the population gives no evidence of
causation and bounds the rate low in BOTH trees. The decisive evidence is
positive control from a third tree: `.claude/worktrees/webpbt/lane-logs/
webpbt-r3-keystone-smoke-run1.log` carries 62 occurrences of the identical
`inv-main-panel-rows-match-focus] DROPPED ROW(S) IN MAIN PANEL — blocks the
reference model counts as editable rows of Main…`, and that tree's
`assets/default/types/person.yaml` has **0** occurrences of `property_kinds` —
it carries none of these changes. The signature therefore predates this lane.

It has no row in `docs/Testing/KeystoneKnownReds.md` and should get one; that is
registry work, not this lane's.

## Rev 2 verdict: CONFIRMED — R1, R3a and R3b are closed; the remaining keystone
## red is pre-existing. R2 (the two bugfunnel entry files) is out of this tree by
## the lane's own statement and must flip on the integration chain at weave.
