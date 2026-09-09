# Adversarial verification — lane `lowcode-inc3`

Workspace: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/lowcode-inc3`
`@-` = `830d794f878f` (asserted). Tree assert passed: `cook.rs` absent,
`persist_file_projection` present. Lane work lives in `@` (37 files, WIP).
`pwd` printed on every command.

## Overall: CONFIRMED WITH ONE REFUTED SUB-CLAIM (C5 classification evidence)

The refactor itself (C1–C4, C6–C8) holds up. The single refuted item is the
*evidence* the new bugfunnel entry uses to justify its ORACLE classification.

---

## C1 — bespoke parser gone. CONFIRMED

- `crates/holon-kitchen/src/` holds only `cookable.rs`, `lib.rs`,
  `shopping_sync.rs`, `shopping.rs`. Deleted and verified absent:
  `cook.rs`, `rows.rs`, `params.rs`, `file_format.rs`,
  `tests/rows_contract_differential_pbt.rs`,
  `holon-plugin-host/tests/cook_plugin_differential_pbt.rs`.
- `crates/holon-kitchen/Cargo.toml` has no `cooklang` dependency.
- `grep -c cooklang Cargo.lock` = **0**. (The guest keeps its own workspace lock
  at `guests/cooklang/Cargo.lock` — correct and expected.)
- `rg -n "cooklang|@[a-z]+\{" crates --type rust`: every hit classified — doc
  comments, test fixtures/assertions, the format-NAME string `"cooklang"`
  (`editor_view_model.rs:1738,1769` are test doubles building an
  `EditRefused::ReadOnlyFormat`), and `cookable.rs`, which is SQL over
  `ingredient_use` rows, not a parser. No Rust parses cooklang syntax.
- `BUNDLED_PLUGINS` at `holon-plugin-host/src/sidecar.rs:97` compiles in
  `plugins/cooklang.yaml` + `plugins/cooklang.wasm`.
  `holon-app/src/wiring.rs:356-375` builds `FormatRegistry` from the org adapter
  plus `PluginFormatAdapter::bundled(...)`.
- `.cook` string literals outside tests/fixtures/plugins are all in comments.

## C2 — behaviour parity. CONFIRMED (brief's command was wrong)

**The brief's command `-p holon-app cook_vault_ingest` targets the wrong
package** — the test lives in `crates/holon-integration-tests/tests/`. Run as
specified it spends ~6 min building and matches nothing. Correct run:

`cargo nextest run -p holon-integration-tests --test cook_vault_ingest`
→ **`Summary [6.633s] 12 tests run: 12 passed, 1 skipped`** (the skip is the
`#[ignore]`d residual). Log:
`scratchpad/verify-lowcode/cook-vault-ingest.log`.

Two tests read for substance — both assert parsed CONTENT, not mere existence:
`a_vault_recipe_produces_typed_rows_and_answers_cookable_now` and
`a_cook_file_in_the_vault_ingests_beside_org` (asserts de-sugared step prose,
`@eggs{2}` → "eggs", `~{3%minutes}` → rendered text, and the cooklang `title:`
metadata reaching the document block — `cook_vault_ingest.rs:130-180`).

## C3 — Fork A. CONFIRMED (one latent hazard, below)

`holon-app/src/turso_seams.rs:446-480`:

```
INSERT INTO file (id, name, parent_id, content_hash, document_id)
VALUES (?, ?, ?, ?, ?)
ON CONFLICT(id) DO UPDATE SET content_hash = excluded.content_hash,
                              document_id = excluded.document_id
```

- `persist_file_hash` no longer exists as a method anywhere — only in doc-comment
  prose. The old UPDATE path is deleted, not kept alongside.
- Sole production call site: `file_sync_controller.rs:4588`
  (`persist_disk_hash_for`), which always passes `document_id: Some(...)`, so the
  `excluded.document_id` overwrite cannot NULL a live value on this leg.
- Other implementer `holon-app/src/loro_seams.rs:187` is a deliberate no-op
  (no SQL `file` table under Loro). No conflict.
- `a_cook_file_records_its_document_on_the_file_row` passes for `.cook` AND org
  (12/12 run above).

## C4 — Fork B. CONFIRMED functionally; the "6→3" number is NOT measured

Gate is on the TYPED tier (`file_sync_controller.rs:2695`), ahead of the
substring pre-filter, exactly as claimed:

```
if self.adapter(path)?.write_tier() == WriteTier::ReadOnly {
    return Ok(ShareProbe::Ordinary);
}
```

`a_read_only_file_is_never_parsed_by_the_share_probe` passes and does assert a
guest-call COUNT (`assert_eq!(parses, 1, ...)`, using the `AtomicU64` at
`holon-plugin-host/src/adapter.rs:46`), with a recipe whose text contains the
probe's own `share-role` trigger so the pre-filter cannot mask the cost.

**DEFECT (evidence, not function).** The "first boot of three recipes went 6
guest parses → 3" claim appears in three places — `lane-report-lowcode-inc3.md:387`
("Effect, measured"), `docs/Plans/Kitchen.md` (R12 B2 and the Inc 3 section), and
the doc comment on `a_second_boot_re_parses_only_the_recipe_that_changed`. The
lane report cites `lane-logs/a2c-incremental.log`. That log contains no such
count. Counting the guest's own parse lines:

- `a2c-incremental.log`: Linsensuppe 6, Pancakes 2, Waffles 2 (across both boots)
- `a2d-incremental.log`: Linsensuppe 5, Pancakes 2, Waffles 2

i.e. 2 parses per unchanged recipe over two boots = **3 on the first boot** —
the POST-fix number only. No log anywhere records the pre-fix 6. The only
before/after pair in the logs is the second-boot counter, which went 7 (a2c) →
6 (a2d) — a reduction of one, not a halving. The "6" is an inference
(probe + ingest per file), not a measurement, and is labelled "measured".

## C5 — the residual. REFUTED (classification evidence)

The mechanical parts hold:
- `a_second_boot_re_parses_only_the_recipe_that_changed` is `#[ignore]`d with an
  inline reason: `"known red: in_tree answers false for a recipe's document root
  the Loro tree holds — see the bugfunnel entry"` (`cook_vault_ingest.rs:1201`).
  (Minor: the inline reason points at "the bugfunnel entry" without naming the
  file; the doc comment above it does name it.)
- `a_recipe_lands_in_every_active_store` passes and does assert the root is in
  the tree via `snapshot_blocks()` (`snapshot.contains_key(&root_id)`). Its
  SqlOnly early-return is NOT vacuous: `TestEnvironmentBuilder` defaults
  `enable_loro: true` (`test_environment.rs:311`) and the test never disables it.

**I reproduced the probe myself and got a DIFFERENT answer.**
`cargo nextest run -p holon-integration-tests --test cook_vault_ingest
--run-ignored all a_second_boot_re_parses_only_the_recipe_that_changed
--no-capture` with `RUST_LOG=holon_filesystem=debug`
(`scratchpad/verify-lowcode/ignored-repro.log`):

```
in_tree probe root=block:0790f49e-f9f7-85fc-6ac2-a154f8ae9874 present=Some(true)   # Pancakes.cook
in_tree probe root=block:7009cf16-c144-491f-2305-d67c9cbca5e1 present=Some(false)  # Waffles.cook
... the second boot ran the guest 5 times   (lane logs recorded 6 and 7)
```

The SAME root (`block:0790f49e-…`) that the lane recorded as `Some(false)`
answers `Some(true)` for me, in the same scan where a sibling recipe answers
false. So `in_tree` is not systematically false for a recipe root — the answer
is timing-dependent. That is the signature of a race (the Loro tree has not
absorbed the doc root when the probe runs), not of a wrong stable-id → tree-node
resolution.

**And the entry's decisive comparison does not exist.** The entry states:
"Org roots in the SAME vault, in the same scan, answer true (`block:journals`,
`block:fce3ebaf-…`), which is why the skip works for org and only for org."
Across **every** file in `lane-logs/`, there are exactly four `in_tree probe`
lines — both `.cook` roots, twice each, all `Some(false)`. **Zero org-root
probes.** Structurally there cannot be one in those runs: the probe sits inside
`stored == &disk_hash && self.content_present_in_all_stores(root).await?`
(`file_sync_controller.rs:2898`), and the org files in that vault had
`stored != disk` on the second boot (visible in my log for `Journals.org` and
`Journals/2026-09-08.org`), so they short-circuit before `in_tree` is ever
called. The org half of the comparison is asserted, never measured.

Consequence for the classification question the brief asks: the entry
`2026-09-08-in-tree-answers-false-for-a-recipes-document-root.md` is classified
**ORACLE** on the strength of "`in_tree`'s answer is WRONG, and org proves it".
Neither leg survives: the org comparison was never measured, and the cook answer
is not stably false. On the evidence available the residual looks like an
ordering/settling race between ingest and the Loro tree, which points at a
different gap class and a different remedy than "write an in_tree-vs-snapshot
differential". The entry's "Root cause: not yet located" is honest; its
"Missing piece" and the ruling-out of alternatives are not yet earned.


**Repeat runs settle it: the measurement is non-deterministic.** Three
back-to-back identical runs of the ignored test
(`scratchpad/verify-lowcode/repro3.sh`, log
`tasks/bmmykqlhy.output`) reported:

```
=== RUN 1 ===  the second boot ran the guest 7 times
=== RUN 2 ===  the second boot ran the guest 6 times
=== RUN 3 ===  the second boot ran the guest 5 times
```

Same tree, same vault, same command — 7 / 6 / 5. The lane's own logs recorded 7
(a2c) and 6 (a2d) and read that one-step difference as the EFFECT of fork A.
It is within this test's run-to-run spread. So the a2c→a2d delta the lane
attributes to a code change is not distinguishable from noise, and neither the
"6 → 3" figure nor the "7 → 6" improvement is supported by the evidence on hand.

## C6 — entries and plan docs. CONFIRMED

- `2026-09-08-every-boot-re-parses-every-cook-file-in-the-vault.md`: `status: OPEN`.
- The kitchen entries are untouched: `jj diff -r @ --name-only` adds only the two
  new 2026-09-08 entries; no existing kitchen/cook/shopping entry is modified.
  The four named ones remain OPEN.
- `/usr/bin/python3 scripts/bugfunnel.py check` → **`657 entries, 0 problems`**
  (exit 0). `counts` → 654 escapes, OPEN 272, FIXED 329.
- `docs/Plans/Kitchen.md` R11 (OPEN, `ADVANCED_UNITS` measured ON, not flipped),
  R12 (RULED, half-closed, A1 + B2 as implemented) and R13 (OPEN, in_tree) match
  the code. **Minor inconsistency:** the section header reads
  "Lowcode Inc 3 — … — **LANDED**" while the work is an uncommitted WIP in `@`
  and not on `main`; every other increment carries a date with its LANDED tag.
  The R12 B2 row repeats the unmeasured "6 guest parses → 3" (see C4).

## C7 — gates. CONFIRMED (reproduced independently)

- `cargo check --workspace --all-targets` → `Finished dev profile in 54.66s`,
  **0 errors** (warnings only). Log: `scratchpad/verify-lowcode/check-workspace.log`.
- `cargo nextest run -p holon-kitchen -p holon-plugin-host` →
  **`Summary [97.541s] 86 tests run: 86 passed (2 slow), 1 skipped`** — matches
  the claimed 86. Log: `scratchpad/verify-lowcode/kitchen-pluginhost.log`.
- `cargo nextest run -p holon-integration-tests --test cook_vault_ingest` →
  12/12 (C2).
- `just guests-verify`: queued behind other lanes past my window; NOT reproduced
  by me. Partial independent check: the tracked guest bytes hash to
  `b669a8ed…5f96d` (cooklang) and `e3658d72…21fd4` (testkit), which are exactly
  the hashes `lane-logs/final-guests.log` reports its rebuild produced. That
  confirms the tracked artefacts match the lane's own rebuild; it does not
  re-prove reproducibility from source.
- `just keystone-smoke`: queued past my window; NOT reproduced by me.
  `lane-logs/final-keystone.log` shows `test result: ok. 4 passed; 0 failed`.

## C8 — secrets / hygiene. CONFIRMED

- Secret scan over the full lane diff (`jj diff -r @` and `-r @-`) for
  the Anthropic key prefix and env-var name, `ghp_`, `AKIA…`, `PRIVATE KEY`,
  `export VAR=` (pattern names redacted for this bundle's own scan):
  **0 hits**. (One scan attempt was correctly blocked by `block-env-dump.py`
  because my grep pattern contained the literal guarded token; the guard is
  right, and I reran without it.)
- Fixtures are synthesized, not vault content: `PANCAKES_COOK` is
  "Fluffy Pancakes / @eggs{2} / @flour{200%g}";
  `holon-plugin-host/tests/fixtures/` holds only `pancakes.cook`, `testkit.wasm`,
  `testkit.yaml`.

---

## Defects (evidence only — not fixed, per role)

1. **The "6 → 3" guest-parse figure is labelled "measured" but no log records
   it.** Appears in `lane-report-lowcode-inc3.md:387`, `docs/Plans/Kitchen.md`
   (twice) and in the doc comment on
   `a_second_boot_re_parses_only_the_recipe_that_changed`. The cited log
   (`lane-logs/a2c-incremental.log`) holds only the post-fix count. Pre-fix "6"
   is an inference. Evidence: parse-line counts in a2c/a2d above.

2. **The in_tree bugfunnel entry's supporting comparison was never measured, and
   its central observation does not reproduce.** Zero org-root `in_tree probe`
   lines exist in any lane log (four probe lines total, all `.cook`, all false),
   and the org files in that vault short-circuit before the probe. My own run
   yields `Some(true)` for the very root the entry records as `Some(false)`.
   Classification ORACLE is not supported; a race is the better-fitting
   hypothesis.

3. **The a2c→a2d "improvement" is inside the test's own noise band** (7/6/5 on
   three identical reruns), so the second-boot counter cannot support any
   before/after attribution as currently used.

4. **`docs/Plans/Kitchen.md` marks Inc 3 "LANDED"** while it is an uncommitted
   working-copy change.

5. **The brief's own C2 command (`-p holon-app cook_vault_ingest`) selects no
   tests** — a false-green shape if anyone reuses it. Correct package is
   `holon-integration-tests`.

## Gaps (not defects)

- **`OrgModeSyncProvider` is a second writer of `file.document_id` that always
  writes `None`** (`orgmode_sync_provider.rs:218-224`, `File::new(..., None)`,
  emitted as both `Change::Created` and `Change::Updated` on every scan). Today
  this is harmless-looking because org resolves its root from CONTENT
  (`DocumentIdentity::Embedded` → `doc_id_from_content`,
  `file_sync_controller.rs:2847`), never from the column — so a stomped org
  `document_id` changes no behaviour. But it is exactly the "half-filled column
  is a trap for the next reader" the fork-A test's own doc comment warns about,
  and `a_cook_file_records_its_document_on_the_file_row` cannot catch it: the
  test polls until both rows are non-null and breaks immediately, so it never
  asserts the value SURVIVES a later provider scan. I did not trace the
  change-application path far enough to say whether the Update actually writes
  the column — unverified either way.
- The whole in_tree residual is measured only under heavy machine contention
  (my run also raised an unrelated `Actor channel closed` quarantine on
  `Linsensuppe.cook`). If the residual is a race, contention is a confound in
  every measurement taken of it so far, including the lane's.

---

# Rev 3 delta — CONFIRMED

Same workspace, `@-` still `830d794f878f`, now 38 changed files in `@`.
`pwd` printed on every command. Read-and-run only; nothing edited.

## The C5 rewrite — CONFIRMED

I re-extracted the distribution from the RAW logs myself rather than trusting
`DISTRIBUTION.txt` (a script-generated artefact), stripping ANSI and matching on
the panic text and the probe lines in `lane-logs/rev3-probe/run-1..5.log`:

```
RUN 1  6 times   block:0790f49e Some(false)   block:7009cf16 Some(false)
RUN 2  4 times   block:0790f49e Some(true)    block:7009cf16 Some(true)
RUN 3  5 times   block:0790f49e Some(true)    block:7009cf16 Some(false)
RUN 4  5 times   block:0790f49e Some(true)    block:7009cf16 Some(false)
RUN 5  7 times   block:0790f49e Some(true)    block:7009cf16 Some(false)
```

Byte-for-byte the table in the entry, in `docs/Plans/Kitchen.md` R13 and in the
lane report. Exactly two probe lines per run, both `.cook` — **zero org-root
probes in 5/5**, which is the claim the rev-2 entry got wrong and this one now
states as unmeasurable in this harness. My own rev-2 finding (`Some(true)` for
`block:0790f49e-…`) is reproduced by their runs 2–5.

`2026-09-08-a-cook-vaults-second-boot-re-parses-a-varying-number-of-recipes.md`:
`gap: ENVIRONMENT`, `secondary: ORACLE`, `status: OPEN`. The old
`…-in-tree-answers-false-…` file is gone. `bugfunnel.py check` →
`657 entries, 0 problems` (unchanged total: one deleted, one added).
The three root-cause candidates each carry a discriminating measurement, and
the entry now names run 2 as evidence that `in_tree` is *not* the only cause —
a point it would have been easier to omit.

`#[ignore]` reason (`cook_vault_ingest.rs:1278`) now reads
`"…the second boot ran the guest 6/4/5/5/7 times over five identical runs
(expected 1), with the in_tree probe answering inconsistently for the same
root…"` — the measured distribution, as claimed.

## The "6 → 3" withdrawal and the Fork B A/B — CONFIRMED, sha-proven

`lane-logs/rev3-forkb-ab.sh` mutates a TRACKED file, so I checked the restore
first, three ways:

```
sha-before.txt  55d00e1cfb267ff844b1b0efd5c3f2b14dfe7f36924424900e29e11ecc33a86a
sha-after.txt   55d00e1cfb267ff844b1b0efd5c3f2b14dfe7f36924424900e29e11ecc33a86a
live file NOW   55d00e1cfb267ff844b1b0efd5c3f2b14dfe7f36924424900e29e11ecc33a86a
```

and the guard is present in the tree at `file_sync_controller.rs:2695`. The
script asserts `s.count(guard) == 1` before substituting and uses copy-back, not
a VCS command. Clean.

Results, extracted from the six logs myself:

```
with-forkb    1,2,3:  1 test run: 1 passed          (assertion is parses == 1)
without-forkb 1,2,3:  "booting one recipe ran the guest 3 times"  → 1 failed
```

Deterministic 3/3 per side. So Fork B is **3 → 1 for a read-only file whose
bytes trip the `share-role` pre-filter**, and nothing for one that does not — I
confirmed neither three-recipe fixture (`PANCAKES_COOK`, the `recipe()` helper
at `cook_vault_ingest.rs:979`) contains `share-role` or `shared-tree-id`. The
vault-wide "6 → 3" is withdrawn in all three places that carried it
(`Kitchen.md:311`, the entry, `lane-report:535`), and named as withdrawn rather
than quietly dropped.

## The `document_id` stomp fix — CONFIRMED

The claimed mechanism is real, and I verified it rather than taking it:
`QueryableCache::build_batch_statements` builds `update_clause` as
`columns.iter().map(|c| format!("{} = excluded.{}", …))`
(`crates/holon/src/core/queryable_cache.rs:771-775`) — **every** column,
`document_id` included. A scan emitting `None` therefore cleared it.

The fix is at the provider boundary
(`orgmode_sync_provider.rs:229`): `File::new(…, parse_doc_id_any_carrier(&content))`
replaces the hardcoded `None`. It is the SAME carrier function the org adapter's
`doc_id_from_content` uses (`file_format.rs:93`), so provider and ingest cannot
disagree, and the single `File::new` feeds both the `Created` and `Updated`
arms — the stomp path was `Updated`, so this matters.

`a_scanned_org_file_carries_the_document_its_content_names`
(`orgmode_sync_provider.rs:770-792`) asserts exactly what the brief said it
should: `data.document_id.as_deref() == Some("notes-doc")` against content
carrying `#+ID: notes-doc` — the content's carrier id, not merely non-null.

Two things the lane volunteered that I checked and agree with, and which count
in its favour rather than against it:
- A `.cook` row was never stomped (the provider scans with
  `org_only_format_registry()`); the org row was the casualty. The brief's
  framing was wrong and the lane says so.
- `a_recorded_document_survives_a_later_org_scan` was **GREEN before the fix**
  (`lane-logs/rev3-red-stomp-21544.log`) and is kept only as a guard, explicitly
  NOT claimed as the red. The red that justifies the change is the provider unit
  test above. That is the correct call, honestly labelled.

Still open, and flagged by the lane for a ruling rather than hidden: an org file
carrying NO id still emits `None`, and the generic `QueryableCache` UPSERT was
deliberately not given never-clear semantics for one column. I agree with not
special-casing a generic writer; the residual belongs in a ruling.

## Kitchen.md — CONFIRMED consistent

- Heading (`:172`): "**implemented (unlanded, lane `lowcode-inc3`)**" — the
  "LANDED" overstatement is gone.
- R13 rewritten to the varying-parse framing, quotes 6/4/5/5/7, names run 2 as
  ruling out an in_tree-only cause, and states the org comparison as withdrawn.
- R12 B2 restated with the measured A/B (3 without the guard, 1 with) and the
  scope limit (nothing for a recipe not carrying the substrings).

## Gates — all reproduced by me this session

```
holon-orgmode --features holon-orgmode/di   201 tests run: 201 passed, 0 skipped
  incl. PASS a_scanned_org_file_carries_the_document_its_content_names
cook_vault_ingest                            13 tests run: 13 passed, 1 skipped
just guests-verify        exit 0 — cooklang b669a8ed…5f96d matches its source
                                   testkit  e3658d72…21fd4 matches its source
just keystone-smoke       exit 0 — test result: ok. 4 passed; 0 failed
```

(cook_vault_ingest is 13 now, up from 12, because of the retained guard test.)
Log: `scratchpad/verify-lowcode/rev3gates.log` and siblings.

## Verdict on the delta: CONFIRMED

Every rev-3 number I could re-derive independently, I did, and it matched. No
new defect found. The lane corrected the two things I refuted, reported three
limitations that weakened its own case, and left the residual open instead of
scoping the caller past it.
