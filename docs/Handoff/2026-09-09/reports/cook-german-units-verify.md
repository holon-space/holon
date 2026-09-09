# Verify: lane `cook-german-units` (D100.a) — **CONFIRMED**

Workspace: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/cook-german-units`
`pwd` printed on every command below. `@` = `16e8420634d5`, `@-` = `91b1501d4016` (asserted).
Tree assert: `guests/cooklang/src/german.toml` present, `crates/holon-kitchen/src/cook.rs` absent.
All evidence produced in this session. Scratch: `.../scratchpad/verify-german/`.

## C1 — layering, `Extensions::all()`, no English shadowing — CONFIRMED
* `guests/cooklang/src/lib.rs`: `parser()` does `toml::from_str(include_str!("german.toml"))` →
  `Converter::builder().with_bundled_units().with_units_file(german).finish()` →
  `CooklangParser::new(Extensions::all(), converter)`.
* Equivalence to the old `cooklang::parse` proven from the crate source
  (`~/.cargo/registry/src/*/cooklang-0.18.7/src/lib.rs:153`, `convert/mod.rs:178`):
  `CooklangParser::default()` = `Extensions::default()` = **`Extensions::all()`**, and
  `Converter::default()` = `bundled()`. So the only delta is the German layer.
* `german.toml` sets **`aliases` only**. `apply_extend_groups` (builder.rs:282) touches
  `names`/`symbols` only when the entry supplies them; formatting is driven by `names[0]`/`symbols[0]`.
* Collisions: the builder errors on a duplicate index key (`ConverterBuilderError::DuplicateUnit`),
  so a successful build IS the no-collision proof. Independently exercised in a native probe
  (`.../verify-german/probe`): **200 consecutive converter builds succeed** — relevant because
  `Extend.units` is a `HashMap`, i.e. extend order is nondeterministic per process.
* English unchanged at the converter level, not just at the projection: probe compared
  `Converter::bundled()` vs the German-layered converter on
  `@flour{200%g} @milk{300%ml} @b{1%tbsp} @c{2%tsp} @d{1%l}` and `@x{200 g} @y{1%kilogram}` —
  `english-identical=true` for both.
* Aliases resolve to the RIGHT physical unit (probe, `Converter::convert`):
  `Minuten→60 s`, `Stunde→3600 s`, `Sekunden→1 s`, `Tag→86400 s`, `Kilogramm/Kilo→1000 g`,
  `Liter→1000 ml`, `EL→14.787 ml`, `TL→4.929 ml`, `Tasse→236.59 ml`, `Meter→100 cm`,
  `Zentimeter→1 cm`, `Millimeter→0.1 cm`. Every English `names`/`symbols` list came back
  unmodified with only the German words appended to `aliases`.

## C2 — red-first + golden provenance — CONFIRMED (reproduced independently)
Method: `rsync` of the worktree to `.../verify-german/basetree`, then the **base** wasm
(`jj file show -r @-`, sha `b669a8ed…`, 516 852 B) written over the tracked one; separate
`CARGO_TARGET_DIR`. Log: `.../scratchpad/verify-german/gateB.log`.

Result at the BASE wasm — `23 tests run: 22 passed, 1 failed`:
* `a_german_recipe_parses_with_its_own_timer_and_quantity_units` **FAILS** with
  `... the cooklang plugin refused kartoffelsuppe.cook: guest reported: cooklang source is not a
  valid recipe: cooklang parse failed: Unknown timer unit: Minuten; Unknown timer unit: Stunde`
  — verbatim the bugfunnel entry's message. Red for the right reason, reproduced by me.
* `the_english_projection_is_unchanged_by_the_german_units` **PASSES against the base wasm**.
  That is the decisive provenance check: the committed golden
  `crates/holon-plugin-host/tests/fixtures/pancakes.projection.txt` is genuinely a pre-change
  capture, and it is in the diff as a tracked fixture.
At the lane's wasm (`gateA.log`): `23 tests run: 23 passed, 0 skipped`.
The German test asserts parsed cells (`Kartoffeln` 500 `g`, `Mehl` 200 `g`), not merely "no error".

## C3 — reproducibility — CONFIRMED (stronger than claimed)
* Tracked bytes hash to `0cb3703f5ce281695a86e27917fb162089e7ee1f1c08efe4a05bb0982a2e13c2`
  (both on disk and via `jj file show -r @`). Base was `b669a8ed…`.
* `just guests-verify`: `guest cooklang: 0cb3703f… matches its source`,
  `guest testkit: e3658d72… matches its source`.
* That run reused a warm stage, so I re-ran after `rm -rf /tmp/holon-guest-build/cooklang/guests/cooklang/target`
  **and** the `.staged` key: `guests/build.sh cooklang <scratch>` rebuilt from scratch to
  `0cb3703f5ce281695a86e27917fb162089e7ee1f1c08efe4a05bb0982a2e13c2` — byte-identical.

## C4 — cost — CONFIRMED
* Numbers are script-emitted by the test itself (`200-step recipe: <fuel> fuel, <bytes> bytes`),
  and `lane-logs/gate-fuel.sh` prints the wasm sha in each half:
  base `b669a8ed…` → 150 550 597 fuel / 2 228 224 B; new `0cb3703f…` → 153 378 084 / 2 293 760.
  = **+1.88 % fuel, +2.94 % memory**. Wasm 516 852 → 731 936 B = **+215 084 B**, matching the diff stat.
* Attribution to the `toml` decoder: `wasm-objdump -h` on both — Code `0x5fc17 → 0x91e65`
  (**+205 390 B**, functions 1185 → 1717), Data +9 008 B. cooklang's `bundled_units` feature pulls
  `toml`+`syn`+`quote`+`prettyplease` at **build** time (units baked via `build.rs`), so the base
  wasm carried no runtime TOML parser and the new one does. Consistent; not symbol-level (stripped).

## C5 — non-green results attributed elsewhere — CONFIRMED (re-run by me)
* `rows_contract_differential_pbt`: the lane's `lane-logs/rows-{base,new}.log` show the same two
  timeouts, but `gate-rows.sh` never prints the wasm sha, so those logs carry no in-log proof of
  which wasm was in the tree. I re-ran the A/B myself in `basetree`, printing the sha in each half:
  BASE `b669a8ed…` → `4 tests run: 2 passed, 2 timed out`; then the NEW wasm copied into the SAME
  tree, `0cb3703f…` → `4 tests run: 2 passed, 2 timed out`. Identical signature
  (`both_scopes_reach_the_wire`, `cook_rows_survive_the_json_lines_contract`, 120 s). **Pre-existing.**
* `shopping_mapping_cost::a_ten_thousand_item_list_maps_inside_the_slo`, run isolated:
  `PASS [2.847s] … 1 test run: 1 passed`. It lives in `holon-kitchen`, not `holon-plugin-host`
  (the lane report does not say so). Load-sensitive, as claimed.

## C6 — gates and docs — CONFIRMED
* `cargo check --workspace --all-targets` in the lane: `Finished dev profile … in 3m 35s`,
  0 `^error` lines, **exit 0** (re-run `--quiet`, `exit=0`).
* `cook_vault_ingest` in its real crate (`holon-integration-tests`): `13 tests run: 13 passed, 1 skipped`.
* `just keystone-smoke`: my run went RED where the lane's log was green. `scripts/keystone-known-reds.sh`
  on my own `target/gate-logs/pbt-general.log`:
  `PASS-WITH-NOTE: 45 known-red panic(s), 0 novel, 0 collateral` — all one registered family,
  `opentab-sql-reads-budget` (`OpenTabViaModifierClick.sql_reads: 24 exceeds expected 23 + tolerance 0`),
  documented in `docs/Testing/KeystoneKnownReds.md:122` as PRE-EXISTING and load-independent, UNOWNED.
  Nothing cooklang-shaped. Within the known-reds rule this is a pass.
* Bugfunnel entry `2026-09-02-a-german-timer-unit-refuses-the-whole-recipe.md`: `OPEN → FIXED`,
  names both tests and the measured cost. `/usr/bin/python3 scripts/bugfunnel.py check` →
  `660 entries, 0 problems`.
* `docs/Plans/Kitchen.md` §7 R11: `CLOSED by D100.a (2026-09-10), which SUPERSEDES D91.a`; the
  Inc-3 "open, named" line updated to "STAYS on (D100.a)".
* No vault content: `kartoffelsuppe` appears nowhere under `/Users/martin/Workspaces/pkm/holon-pkm`,
  and that vault holds no `.cook` files at all.

## Defects
**None found.** No claim was refuted.

## Gaps / notes for the orchestrator (not blockers)
1. **Oracle gap in the German test.** It asserts the timer's rendered prose (`"20 Minuten"`,
   `"1 Stunde"`), never the resolved unit. A `german.toml` that filed `Minuten` under `h` would
   still pass. I closed this out-of-band by direct `Converter::convert` measurement (see C1), but
   the repo has no test that would catch a mis-filed alias. One assertion on converted seconds
   would fix it. Same for `Gramm`/`EL`/`Tasse`.
2. **German spoon sizes are US-sized.** `EL`/`Esslöffel` aliases cooklang's `tablespoon`
   (14.787 ml) and `TL`/`Teelöffel` its `teaspoon` (4.929 ml); the German convention is 15 ml / 5 ml.
   Not introduced by this lane — it is the bundled unit's ratio — but German recipes now feed the
   pantry / "cookable now" conversion path with US spoon sizes.
3. **The converter is rebuilt on every `parse_recipe` call** (TOML decode + `ConverterBuilder`).
   cooklang's own docs say to reuse a parser. No regression vs base (`cooklang::parse` also built
   one per call), but it is now strictly dearer per call — this is what the +1.9 % fuel buys. A
   `OnceCell` in the guest is a cheap follow-up, cheaper than the `build.rs` idea in the report's gap 1.
4. **Lane evidence hygiene.** `lane-logs/gate-rows.sh` prints no wasm sha, so its A/B logs are not
   self-proving (`gate-fuel.sh` does it right). Re-run by me above; nothing to fix in the diff.
5. `lane-logs/` and `lane-report-cook-german-units.md` are untracked — the red log will not land
   with the commit.

## Amendment — gateD completed after the verdict was written
Log: `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/tasks/bqo5l4q9y.output`
(`pwd` = the lane workspace, printed in the log header). Nothing changes above.

* **D1** clean-stage guest rebuild, in-log: `Compiling holon-guest-cooklang … Finished release in 23.18s`,
  `731936 bytes raw`, `rebuilt: 0cb3703f…` == `tracked: 0cb3703f…`. Confirms C3 in the script's own words.
* **D2** `cargo check --workspace --all-targets`: **`check-exit=0`**, warnings only
  (`unused IngestOutcome` ×12, unused imports/variables), no errors. Confirms C6.
* **D3** `just keystone-smoke`: exact outcome is
  `test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 10574.78s`
  (`recipe keystone-smoke failed … exit code 101`). To be precise: **my keystone run exited non-zero.**
  Its log is the one I classified — `PASS-WITH-NOTE: 45 known-red panic(s), 0 novel, 0 collateral`,
  the single registered family `opentab-sql-reads-budget` on `OpenTabViaModifierClick` — which is
  exactly the case `scripts/keystone-known-reds.sh` exists to adjudicate, so it is a pass under the
  project's own rule, not a green run. The lane's `lane-logs/keystone-1.log` finished green in 3.87s
  (`4 passed; 0 failed`); the family is probabilistic, matching the registry's measured rate
  (1/32 on pure main vs 1/31 on the chain, `docs/Testing/KeystoneKnownReds.md:122`).
  The 10 574 s wall time is the shrink loop, not the property — the registry's documented
  shrink hazard; hunt with `PROPTEST_MAX_SHRINK_ITERS=0` if anyone re-runs it.

## CORRECTION to C6 — keystone classifies FAIL, 1 novel (supersedes both statements above)
`pwd` = the lane workspace. My first classification ran against a log the keystone was **still
writing** (45 panics). Re-run on the COMPLETE log:

```
scripts/keystone-known-reds.sh target/gate-logs/pbt-general.log
[known-reds] FAIL: 1 novel panic(s), 144 known-red panic(s), 0 collateral (ignored).   exit=1
```

The novel signature:
`OpenTabViaModifierClick.sql_reads: 23 exceeds expected 22 + tolerance 0 = 22 (watches=0, docs=4)`
— same transition, same invariant, same `tolerance 0` tail as the registered family; it classifies
novel **only because row 122's pattern hard-codes the literal ceiling**:
`OpenTabViaModifierClick\.sql_reads: [0-9]+ exceeds expected 23 \+ tolerance 0`, and this draw's
pin is 22, not 23. (Both draws report `docs=4`, so the expectation varies on something the row's
prose — "the raw read count varies 22–24 while the pin is 23" — does not capture.) This is the
exact classifier brittleness the registry documents for itself at line 115 (`sidebar-focus-bind`:
a rewritten assertion "stopped matching and the classifier reported this known red as a NOVEL
regression").

**Not attributable to this lane, on structure rather than on judgement:** `jj diff -r @ --summary`
is a wasm blob, `guests/cooklang/{Cargo.toml,Cargo.lock,src/{lib.rs,german.toml}}`, ONE test file,
two test fixtures and two docs. **There is no host production Rust in the diff at all**, and
`OpenTabViaModifierClick` is a tab-open SQL read path that touches no `.cook` file. The lane has no
mechanism to move a host SQL read count. A base A/B is not tractable for it — the family fires at
roughly 1 run in 30 (registry: 1/32 on pure main, 1/31 on the chain), and this run cost 10 574 s.

**Consequence for landing:** `just keystone-smoke` exits 101 here and the known-reds gate exits 1,
so a land gate that runs the classifier would FAIL at this tip until row 122's pattern is widened
(drop the literal `23`, anchor on the transition + `tolerance 0` as the row's own prose says it
intends). That is an orchestrator call and a registry edit, NOT a change to this lane's diff.

**Overall verdict is unchanged: CONFIRMED.** C1–C5 are untouched by this, and C6's other members
(`cargo check` exit 0, `cook_vault_ingest` 13/13, bugfunnel 0 problems, R11 supersession, no vault
content) all hold. What is corrected is only my characterisation of the keystone gate: it is a
FAIL-with-one-novel that I judge non-causal, not the clean pass-with-note I first reported.
