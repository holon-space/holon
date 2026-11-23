# Does a written keystone case replay its red?

Question: when the keystone writes its failing case (the original draw and the
shrunk one) as a `Fixture` JSONL line with the failing invariant id as its lock,
does a replay of that line fail again with the same id, reliably enough to
replace seeds?

Short answer: probably yes, provided the replay checks the whole invariant catalog. The sample of injected defects is too small to call the pass line met.
- Every red the spike wrote replays 10 of 10 times with the same id through the whole catalog: two injected defects, three reds of the base itself and one known red.
- The unchanged sidecar replay path misses the pass line. One of the two injected defects replays 0/10 there, because its narrowed checks change what the system under test does.
- Only two of the six injected defects were shown to cause their red: fewer than the five to ten the measurement asks for.

Three further results bear on the next slices:
- The keystone drew three deterministic reds of the base that match no known-red row.
- A known-red row keyed on message text absorbed an injected defect in every run.
- The two-peer runner cannot write a case at all.

## Method

- Spike tree: a copy of the base outside the workspace. It carries three changes:
  - a harness patch on the keystone test file;
  - a defect injector, switched on by an environment variable per defect;
  - nothing else. Nothing of it lands.
- Harness patch:
  - Every failing draw is appended, flushed, to an attempt record: the full case line, the tick, the failing ids and whether a known-red row matched.
  - When the shrink ends, the original line (the last draw before the shrink) and the shrunk line are written.
  - A case line is a hand-authored regression line with these fields:
    - `locks` = the failing ids;
    - `boot: "fresh"`;
    - `also_check_after` = the failing tick when it is not the last.
- Defects: six reverted landed fixes, one per layer (table below). A seventh candidate (frontend builders) was dropped at the desk: no keystone transition authors builder input (desk read, not measured).
- Each defect: up to 20 keystone runs to the first red. Then each written case is replayed 10 times per arm, without seeds:
  - **H**: the sidecar replay as it is (`HOLON_HAND_AUTHORED_SIDECAR`): it checks the locks plus the scope floor.
  - **S**: H with a whole-catalog lock added, so every check runs the full catalog while only the locks judge.
  - **C**: the keystone's own `test_sequential` over the written line, the path of the keystone's static examples.
  - **X**: C plus the drawn initial-state fields that the written line drops (timing, peer ids, boot fault).
- A replay counts only when it fails the same first invariant id as the written case.
- Build profile for every run and every time below: test (dev, unoptimized).
- Logs: under the spike archive, written here as `$SPIKE`. Its path is in the change summary. Each log carries `BASE:`/`TREE:` headers.

Baseline: the spike tree with no defect injected ran the keystone green
(`$SPIKE/r1/none-2/red-1.log`).

## Results per defect

"Defect off" is the shrunk case replayed 10 times through C with no defect
injected. Green there means the injected defect caused the red. Red there means
the run drew a red of the base instead.

| Defect (layer) | First red | Written case: transitions (failing tick) | Failing ids | Slippage | Original H/S/C/X | Shrunk H/S/C | Defect off | Replay median in C: original / shrunk |
|---|---|---|---|---|---|---|---|---|
| d1 unparseable render takes the generic path (render) | run 5 of 20 | 20 (8) → 1 | inv-no-observed-errors | none | **0**/10/10/10 | **0**/10/10 | green 10/10: defect | 8 s / 4 s |
| d2 stale-rename guard on a vanished `.cook` home (file-sync ingest) | not red in 20 runs | — | — | — | — | — | — | — |
| d3 ops offered that the write tier refuses (frontend ops) | every run, 20 of 20, absorbed as a known red | 27 (0) → 1, see below | inv-offered-ops-pass-the-write-tier | not shrunk | 10/10/10/10 | 10/10/10 | green 10/10: defect | 4 s / 4 s |
| d5 empty render of a deleted page written back (org write-back) | run 7 of 20 | 9 (9) → 2 | inv-task-state-matches-ref + 7 more | none | 10/10/10/10 | 10/10/10 | **red 10/10: base** | 14 s / 8 s |
| d6 unarmed projection withholds tombstoned deletes (Loro projection) | run 2 of 20 | 22 (21) → 2 | inv-shape-sim-matches-authority | none | 10/10/10/10 | 10/10/10 | **red 10/10: base** | 14 s / 3 s |
| d7 born creation slot arms caret offset 0 (editor caret) | run 1 of 20 | 36 (16) → 5 | inv-live-children-match-ref + 2 more | none | 10/10/10/10 | 10/10/10 | **red 10/10: base** | 26 s / 12 s |

Evidence:
- First red and written cases:
  - `$SPIKE/r1/d1/red-05.log` and `$SPIKE/r1/d1/dump-05/`;
  - `$SPIKE/r1/d2/red-01..20.log`;
  - `$SPIKE/r1/d3/red-01..20.log` and `dump-*/attempts.jsonl`;
  - `$SPIKE/r1/d5/red-07.log` and `$SPIKE/r1/d5/dump-07/`;
  - `$SPIKE/r1/d6/red-02.log` and `$SPIKE/r1/d6/dump-02/`;
  - `$SPIKE/r1/d7/red-01.log` and `$SPIKE/r1/d7/dump-01/`.
- Replay counts: the `TALLY` lines in the job log of each defect, and `$SPIKE/r1/<defect>/replay-<case>-<arm>/NN.log`.
- Times: `$SPIKE/r2/replay-wall-times.log` (the `replay-original-C` and `replay-shrunk-C` rows of each defect).

Notes on the rows:
- **d1**: the failing id is the cross-cutting error invariant, not the defect's own invariant. The error is logged by the UI watcher's render of the block. That render runs only when the whole catalog is checked, so H, whose scoped check set does not render, stays green 10/10 for both cases. Runs 1 and 4 were red on known reds before run 5 drew the defect.
- **d2**: every failing draw in the 20 runs matched a known-red row; the defect itself was never reached (`$SPIKE/r1/d2/dump-*/attempts.jsonl`). An earlier round on the same defect went red at run 4. That red was not the defect. It was the known split-on-decision refusal: it reached the panic through the error invariant, so the known-red row (message plus site) did not match it. Its shrunk case (12 transitions) replays 10/10 in H, S and C (`$SPIKE/r1/d2-first-tally.log`).
- **d3**: every run failed the right invariant at tick 0. The known-red row `cooklang-read-only-write-refusal-any-op` matches on the message text `cooklang is a read-only format`, which the invariant message quotes, so the keystone spent each draw as a known red and wrote no case. Each of the 20 runs then went red a second time and exited 101: `[shape reach] NOT REACHED` at `shape_sim_matches_authority.rs:84` (`$SPIKE/r1/d3/red-01.log`:588; all 20 runs in `$SPIKE/r2/d3-second-panic.log`). The two cases were rebuilt from the attempt records:
  - the original is the first failing draw;
  - the "shrunk" case is the shortest failing draw, one transition, not shrunk by proptest (`$SPIKE/r1/d3-cases-from-attempts.log`).
- **d5, d6, d7**: the shrunk case fails the same id set as the original, so no slippage. With the defect off, the shrunk case still fails the same ids 10 of 10 times. So these runs drew reds of the base, not the injected defect; whether the keystone reaches those three defects at all is not shown. None of the three reds matched a known-red row in any failing draw. The baseline is 1 green keystone run (`$SPIKE/r1/none-2/red-1.log`), so how often the keystone draws these reds on the base is not measured; once written, they replay every time.
- Evidence for "Defect off": the `ATTRIB` lines in the attribution job log, and `$SPIKE/r1/none/replay-<defect>-shrunk-defect-off-C/NN.log`. Each defect-off log header shows `SPIKE_DEFECT=<unset>`, and every injection is guarded on its own tag (`$SPIKE/spike/patches/defect-injections.diff`). Through H with the defect off, the d5, d6 and d7 shrunk cases also fail 10 of 10 times (`ATTRIB-H` lines in the second attribution job log).

Original replays take longer than shrunk ones. The largest original + shrunk median of one arm is d7 in C: 26 s + 12 s = 38 s. So 10 replays of both cases of one defect through one arm cost at most 10 × 38 s = 380 s, about 6.3 minutes, in the test profile (`$SPIKE/r2/replay-wall-times.log`). X replays only the original case; its largest median is d7, 27 s. N = 10 per case is affordable for a history with a denominator.

## Pass line

Pass line: at least 90% of the injected defects reproduce in at least 9 of 10
replays with the same invariant id.

Injected defects shown to cause their red: d1 and d3.
- **Unchanged replay path (H)**: 1 of 2 passes (d3); d1 replays 0/10. 50%: below the pass line.
- **Whole-catalog replay (S, C, X)**: 2 of 2 pass, 10/10 each, original and shrunk.
- **Sample size**: two defects, against the five to ten the measurement asks for. d2 was not reached in 20 runs. The d5, d6 and d7 runs drew base reds first.

Every red the spike wrote, whatever its cause: the two defects, the three base reds and the known split-on-decision red.
- **Whole-catalog replay**: all six reproduce 10/10.
- **Unchanged path**: five of the six reproduce; d1 does not.

Verdict: the unchanged path is below the pass line. The whole-catalog replay
meets it on every red measured, but two injected defects are too few to call it
met. Six reds of known cause support the result; they do not replace injected
defects.

How the unchanged path differs from the keystone driver:
- **Driver**: H checks only the locks and the scope floor. The check set is not a passive observer: some invariants render, query or settle, and that work produces the red (d1). A replay that runs the whole catalog and judges only the locks (S) reproduces it.
- **Initial-state capture**: the written line keeps the wiring only. It drops the drawn timing, peer ids and boot fault. X restores them; X equals C for every defect, so the gap decided nothing here. It may for a defect that lives in a boot fault.
- **Quiescence waits**: no replay flipped between runs of one arm. No difference was seen.

## Collisions

- **On one invariant id**: d1 and the known split-on-decision red both fail as `inv-no-observed-errors` / `inv-no-errors`. A lock on a cross-cutting id holds any later error red, so it cannot tell two defects apart.
- **On a known-red message pattern**:
  - d3: a novel invariant red was absorbed by a row whose pattern matches text quoted inside the invariant message.
  - The split-on-decision red escaped its row because the site differed.

  Both rows have no invariant id; both errors disappear under a typed key (invariant id against panic site).

## Two peers

Cannot hold today; zero tries were made.
- The two-instance keystone is a `prop_state_machine!` macro with its own runner, so there is no point to write a case from.
- The replay (`LockedReplay`) exists only for the one-instance machine, and the sidecar test is fixed to it.

The case itself would fit: the two-instance init state is a constant wiring and
its transitions are the same enum. What is missing is a hand-rolled runner like
the keystone's, plus a two-peer replay. Evidence: `$SPIKE/r1/c4-two-peer-desk.log`.

## Known-red rows by class

Desk classification of `docs/Testing/KeystoneKnownReds.md`: 198 rows
(`$SPIKE/r2/c5-classify-by-panic.log`).

Rule: a row is an invariant-failure panic when the panic it describes is an
oracle invariant failure. That is the case when its Match pattern names an
`inv-*` id, "diverged from the oracle" or an `inv-sql-budget` key, or when its
other cells name one of these and reading the row shows that the test red is
that invariant failure. All other rows are other panics.

The pattern test alone gives 48 invariant-failure rows
(`$SPIKE/r1/c5-classify-02.log`). The row cells name an invariant in 21 more
rows; each was read:
- **Moved to invariant-failure panic (15)**: the row text reports the test red as an invariant failure.
  - 265 `failed-slot-create-sqlonly-keystrokes-run-anyway`: oracle verdict `inv-no-observed-errors`.
  - 274 `loro-stable-id-missing`: the worker panic surfaces as `inv-no-observed-errors` + `inv-org-render-fixed-point`.
  - 277 `join-then-stale-second-join`: `inv-no-observed-errors`.
  - 295 `drawer-open-matches-ref`, 296 `drawer-open-matches-ref-reverse`: `inv-drawer-open-matches-ref` as the sole violation.
  - 304 `windowed-shape-gate-split-anchor-not-simulated`: `[inv-shape-gate-refuses-illegal-writes]`.
  - 373 `lib-focus-roots-mismatch-right-sidebar`: lockstep test fails on `inv-focus-roots`.
  - 388 `lib-type-chars-home-profile-derived`: lockstep test fails on `inv-home-profile-matches-derived`.
  - 396 `bulk-add-sibling-order`: `inv-live-children-match-ref` and three other arms.
  - 652 `catalog-advice-step6-weave-dismiss`: catalog test fails on `inv-advice-rows-woven`.
  - 666 `settle-budget-reboot-under-load`: `[inv-settle-budget]`.
  - 667 `logseq-parity-caret-fold-lost-loro`, 687 `reaper-deletes-saved-headless-newborn`, 688 `edited-copy-cut-paste-raises-unedited-two-files`: "diverged from the oracle".
  - 772 `folder-companion-subdir-fileless-page-not-homed-load`: structural test fails on `inv-every-page-has-its-own-file`.
- **Kept as other panic (6)**: the row names an invariant only as context.
  - 263 `syn-real-mint`: harness assertion; the id names another log.
  - 272 `state-toggle-row-absent`, 283 `sidebar-focus-bind`, 300 `jump-typechars-no-row`, 327 `typechars-no-focused-block`: driver panic before any invariant reports.
  - 653 `catalog-class-one-pinned-list`: `assert_eq!` on a pinned census.

| Scope | Invariant-failure panic | Other panic (SUT unwrap, harness or driver assert, fork wrapper, timeout) | Total |
|---|---|---|---|
| Registry | 42 | 27 | 69 |
| Per-binary sections | 21 | 108 | 129 |
| Whole file | 63 | 135 | 198 |

By status: known-red 165, fixed-pending-soak 28, no-longer-fires 3, proposed 2
(`$SPIKE/r1/c5-classify-02.log`).
So about a third of the rows (63 of 198) are invariant reds that a written case
with an id lock can hold. The other 135 need another key: a panic site, a fork
wrapper or a timeout.

## Every instance of the class

`rg -n 'failure_persistence|max_shrink_iters' crates -g '*.rs'` (`$SPIKE/r1/grep-shrink-config.log`):
- **Keystone** (`tests/general_e2e_composed_pbt.rs`, the `draw_sample` config): covered by the harness patch.
- **Live-MCP keystone** (same file, `max_shrink_iters: 0`): not covered. It has its own runner and drives a live app, and no `LockedReplay` exists for its SUT.
- **Two-instance keystone** (`tests/two_instance_composed_pbt.rs`, macro config): not covered (see Two peers). Its other two configs are `proptest!` blocks over drawn data, not transition sequences.
- **`src/pbt/composed/wide_e2e.rs` shrinker walk**: not applicable; it fails by design to exercise the shrinker.
- **`src/pbt/known_reds.rs` configs**: the shared sampler and its self-tests. The patch hooks the keystone's closure around the sampler, not the sampler itself, so a second user of the sampler would need the same hook.
- **`src/pbt/ui_harness.rs`, frontend slices, editing suite, `keystone_atom_counts.rs`**: not covered. They draw their own alphabets or are measurements, not gate reds.
- **Every other hit**: a property test over drawn data in another crate, not a transition machine. Not a case-writing target.

`rg -n 'HOLON_.*(CASE|REPLAY|SIDECAR)' crates scripts` (`$SPIKE/r1/grep-replay-entry.log`):
- **`src/pbt/hand_authored.rs` `HOLON_HAND_AUTHORED_SIDECAR` and `HOLON_HAND_AUTHORED_CASE`**: the sidecar replay. It is not the keystone driver: it runs scoped checks, not `test_sequential`.
- **`scripts/gate-integrate-steps.sh`**: selects sidecar cases by name; same path as above.
- **`scripts/check-gate-scripts.sh`**: echoes the variable in a probe; not a replay.
- **`scripts/run_live_mcp_pbt.sh`**: runs the live-MCP keystone; not a replay.
- **`hand-authored-regressions/*.jsonl`, `transitions/declare_typed_schema.rs` and `composed/reseed_observer.rs`**: comments only.

## Clause form

There are 212 `InvariantResult::Fail` sites in 95 files. Of the 86 invariant
bodies that hold Fail sites, 36 have more than one. Distribution of sites per
body file: 1 site in 50 files, 2 in 12, 3 in 4, 4 in 7, 5 in 5, 6 in 3, 7 in 2,
8 in 2, 9 in 1 (`$SPIKE/r1/fail-sites.log`). For those 36 invariants, the id
alone does not name the failing clause. In this spike the id was enough as a
lock: no replay failed the same id through another clause, as far as the first
id shows. It is not enough as a registry key for the cross-cutting ids (see
Collisions).

## Recommendations

- **Before the slices**: fix the three base reds d5, d6 and d7 (each has its own fix card). Their written cases are in the spike archive. And measure three or more further injected defects through the whole-catalog replay, each with the defect-off control, to bring the sample to five.
- **Slice A (typed record, case writer, history with a denominator)**:
  - Build the case writer as patched here: append every failing draw, flushed, before the panic; write the original and the shrunk case at the end of the shrink.
  - Write the case also when a known-red row absorbs the red (d3), so the history can show the absorbed reds.
  - Replay through the whole catalog and judge only the locks (arm S): not through the scoped sidecar check.
  - Capture timing, peer ids and boot fault in the line; it costs nothing and closes the one capture gap.
  - N = 10 replays per case fits the measured times.
- **Slice B (registry on typed keys)**:
  - Key rows on the invariant id for invariant reds (63 of 198 rows) and on the panic site for other panics (135): never on message text alone. Message text absorbed an injected defect in 20 of 20 runs here, and let a known red escape through another path.
  - Treat the cross-cutting error ids as too coarse to key on alone. Add the clause (or the logged site) for them and for the 36 multi-site invariants.
- **Slice C (replay annotation outside the gate)**:
  - Annotate a red with the reproduced count from the whole-catalog replay; a scoped replay would have marked d1 as not reproduced.
  - Two-peer reds stay out until the two-instance keystone has a hand-rolled runner and a replay.
