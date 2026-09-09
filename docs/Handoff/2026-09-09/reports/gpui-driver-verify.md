# Adversarial verification — lane `gpui-driver` @ `db5069dc27d3`

`pwd` for every command below: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/gpui-driver`
(except the four register lookups, `pwd` = `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/gpui-reds`).

Tree identity asserted before every run: `grep -c click_point_when_painted … = 6`,
`grep -c miss_census … = 4`; `jj log -r @` → `db5069dc27d3`, `@-` → `7e00a965f01e`.
The suite script re-asserts the marker and exits 93 otherwise.

**Headline verdict: CONFIRMED with defects.** The central claim — a single-frame
bounds read, fixed by a frame-wait, taking the deterministic register from 16 to
12 — reproduces exactly in my own run. Five secondary claims do not survive:
one verb was missed, one is dead code, the entry contradicts the code on the
deadline, the deadline is not the derived bound the report calls it, and the
production defect is recorded only inside an entry marked FIXED.

---

## C1 — root cause and the fix's completeness: **CONFIRMED, with two gaps**

`click_point_when_painted` (`frontends/gpui/tests/pbt_harness/sim_windowed_replay.rs:246-274`)
does pump a full cycle until a frame paints the entity, bails at the deadline,
and the bail carries `miss_census` (`:174-224`) naming element count, engine
focus, the painted entity set, every element bound to the missing id with its
`CLIPPED` state, and a sorted full element table. That part holds.

**Gap 1 — a fifth bounds-taking verb still reads one frame, and swallows the
miss.** `scroll_entity` at `sim_windowed_replay.rs:751`:

```rust
let Some((cx, cy)) = self.bounds_center_f32(entity_id) else {
    return Ok(());
};
```

This is the same single-shot read the lane fixed elsewhere, and it is worse than
the ones fixed: on a miss it returns `Ok(())`, so a scroll gesture aimed at a
blank frame becomes a silent no-op and the test proceeds as if it scrolled.
The repo rule is explicit ("NEVER swallow errors", `CLAUDE.md`). The report and
the commit message both say "no verb can sample a single frame again" and "all
four bounds-taking verbs" — there are five, and the fifth is the silent one.
Pre-existing, not introduced here, but it directly falsifies the stated scope.

**Gap 2 — `mouse_point` is now dead code.** `sim_windowed_replay.rs:228` has no
callers (`grep -rn mouse_point frontends/gpui/` returns only the definition).
It does not warn, because `frontends/gpui/tests/pbt_harness/mod.rs:14` carries
`#![allow(dead_code)]` — so the leftover is lint-invisible. `CLAUDE.md` bans
leaving the old code path in place.

**Note, not a defect.** `set_block_expanded` (`:560-578`) does not call
`click_point_when_painted`; it duplicates the loop inline because it resolves a
chevron element id rather than an entity. It honours the same contract. Its
comment cross-references the shared helper rather than restating the rationale,
which is the right call.

## C2 — register 16 → 12: **CONFIRMED, independently reproduced**

My run (`lane-logs/verify-full-serial.sh`, semaphore-wrapped, `--test-threads=1`,
log `lane-logs/verify-full-serial-82669.log`):

```
Summary [1326.545s] 390 tests run: 378 passed (3 slow), 12 failed, 6 skipped
```

`grep -c TIMEOUT` on that log = **0**.

The failing **name set** is identical to the lane's final run
(`lane-logs/gate-full-serial-53638.log`); `diff` of the sorted name lists differs
only in per-test timings. Both are exactly register rows 1, 3, 4, 7, 8, 9, 10,
11, 12, 13, 14, 15.

Rows the brief required green, from my log:

| Register row | Test | My result |
|---|---|---|
| 2 | `benchmark_windowed_per_case_boot_cost` | `PASS [49.290s]` |
| 5 | `windowed_composed_sut_drives_a_click_gesture_sequence_green` | `PASS [15.421s]` |
| 6 | `windowed_composed_sut_replays_a_fixture_via_replay_steps_green` | `PASS [15.151s]` |
| 16 | `a_short_window_still_paints_the_outline` | `PASS [15.515s]` |

`grep -c 'not in bounds after'` = 0 — no test reaches the new deadline in this
run, so the gate is not merely converting one failure into another.

Row 16's caveat in the report is correct and should stay: the register recorded
it 1/3 loaded and FAIL serial, so three green serial runs (lane ×2, mine ×1)
make it *likely* fixed, not proven.

## C3 — is the harness fix masking a production render gap? **Yes, and the report says so; the entry's status does not**

Both cited facts check out.

`assert_content_fidelity` (`crates/holon-layout-testing/src/invariants.rs:238`):

```rust
if total_descendants > 0 && visible_leaves == 0 {
```

A `reactive_shell` with **zero** descendants — the exact frame the lane
measured — is exempt. The function's own doc comment (`:203`) states the
exemption.

So: after this change, a transient one-frame blank of the main panel is
observable by **no** oracle in the windowed suite. The four gesture verbs now
wait it out silently, and the one invariant shaped to catch an empty shell
exempts the empty case. The lane recorded this honestly as open items 2 and 3,
and it is the correct engineering trade (a driver that samples an arbitrary
frame is not the gesture seam it stands in for). But the masking is real and
should be stated as such at weave time.

The *persistent* variant of the same render gap is still red and still
attributable: register row 10, `an_opened_nested_page_paints_its_children`,
`nested_page_chevron_gate.rs:753`, "The gate is open and the content
materialised, so the row opened onto nothing"
(`lane-logs/verify-full-serial-82669.log:6428`). That is Family D, a different
lane, and it does not cover the transient case.

## C4 — the 2 s deadline: **REFUTED as a derived bound**

The report calls 2 s "still ~100× the few-frame wait the defect actually needs".
Nothing measures the satisfied-wait duration, and the evidence contradicts the
factor.

Baselines for `arrow_walk_keeps_focus_on_the_reference_outline_neighbour`
(all from `…/gpui-reds/lane-logs/`):

| Run | Condition | Result |
|---|---|---|
| `gpui-serial-iso.log` | base rev, serial, 58-test subset | `PASS [52.188s]` |
| `gpui-full-run1.log` | base rev, loaded | `PASS [102.392s]` |
| `gpui-full-run2/3.log` | base rev, loaded | `TIMEOUT [120.03s]` ×2 |
| lane, 2 s deadline, serial | this commit | `PASS [65.070s]` |
| mine, 2 s deadline, serial | this commit | `PASS [65.452s]` |

The frame-wait adds ~13 s to this row under **zero** load, reproducibly (two
independent runs, 65.070 s and 65.452 s against a 52.188 s serial baseline). The
row's nextest budget is 120 s, and it already timed out twice at base under load.

The mechanism matters more than the number. `pump_cycle`
(`sim_windowed_replay.rs:37-47`) does not only wait — it calls
`app.advance_clock(Duration::from_millis(500))` on **every** iteration. So the
deadline is not a patience knob; it is a fake-clock injection rate. Raising it
to 5 s advanced the simulated clock further per gesture and blew the row to a
120 s TIMEOUT. Lowering it to 2 s restored the pass. That is a semantic
dependency of the test on the deadline constant, and 2 s is where it happened to
pass, not a bound anyone derived.

Mitigating: the row IS in the register's Tier-2 load-sensitive regex
(`GpuiCrateReds-2026-09-10.md:258`), so a load red there is already excused — which
is also why a regression here would be absorbed silently.

Also latent, in the seam between changed and unchanged code:
`send_key_chord` keeps a 4 s `overall_deadline` (`:410`) that was written when the
click point resolved instantly. It now spends up to 2 s inside
`click_point_when_painted` (`:424`) plus 1 s on the landing poll (`:430`) per
iteration, so the re-click loop that budget exists for gets at most one slow
attempt; and a 2 s bail propagates with `?` before the 4 s budget is ever
consulted. Not biting today (`grep -c 're-clicking'` and
`grep -c 'never moved focused_block'` on my log are both 0), but the narrowing is
unadjusted.

## C5 — reverted probe residue: **CONFIRMED**

`jj status` and `jj diff --stat` both show exactly two paths: the new bugfunnel
entry and `sim_windowed_replay.rs` (+161/−25 by `--stat`; brief said +161/−25).
`git status --porcelain` is empty of anything else; `lane-logs/` is ignored via
`.gitignore:36` (`.claude/worktrees/`).

Independent revert proof, not taken from the report:

```
b9a5f053b9ce66bb227d4fb14976d59769fd4d4c8169cc6d8c4afce9f5e9b748  crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs
b9a5f053b9ce66bb227d4fb14976d59769fd4d4c8169cc6d8c4afce9f5e9b748  lane-logs/wide_e2e.base.rs
```

`frontends/gpui/src/views/reactive_shell.rs` and
`frontends/gpui/src/render/builders/mod.rs` are absent from `jj status`, so the
`HOLON_GPUI_RENDER_PROBE` instrumentation is gone.

## C6 — deferred claims: **CONFIRMED**

`grep -rn 'fn surface_chars_before_content' crates/ frontends/` returns exactly
three hits: the trait default (`crates/holon-frontend/src/user_driver.rs:293`,
body `Err("editable surface not projected by this driver")`), the sole impl on
`ReactiveEngineDriver` (`:1486`), and the mirror helper
(`crates/holon-frontend/src/headless_editor_mirror.rs:426`). Neither
`SimUserDriver` (`sim_windowed_replay.rs:378`) nor production `GpuiUserDriver`
(`frontends/gpui/src/user_driver.rs:644`) implements it.

From my run, rows 1, 3 and 4 fail past the new gate at
`crates/holon-integration-tests/src/pbt/op_write_cap.rs:381`:

```
[SplitBlock/keystroke] cannot place the caret for content byte 0 on block:c1:
editable surface not projected by this driver
```

(`verify-full-serial-82669.log:669` row 1, `:1121` row 3, `:6236` row 4.)

Rows 12–14 fail unchanged, same text as the register recorded:
`structural_chord_stale_flush_windowed.rs:351` "vacuity guard: the Tab chord
changed no parentage" (`:6561`, `:6601`) and
`task_keyword_blur_windowed.rs:357` "the blur dispatched NO operation (6 history
rows before and after)" (`:6637`).

## C7 — the three remaining gates: **CONFIRMED, and greener than the lane's**

`lane-logs/verify-gates.sh`, semaphore-wrapped.

- `cargo check --workspace --all-targets` — `Finished dev profile … in 2m 38s`,
  `grep -cE '^error'` = 0 (`lane-logs/verify-cargo-check-39288.log`).
- `just hand-authored` — `test result: ok. 9 passed; 0 failed`, exit 0
  (`lane-logs/verify-hand-authored-39288.log`). The lane saw one red here
  classified `known-red:org-blocks-ref-diverge`; my run had **no** red at all, so
  that row is intermittent rather than a standing known red. Note:
  `scripts/keystone-known-reds.sh` on this green log reports "4 novel panics" —
  it scrapes panic text from the log without consulting the test result, and
  these four are the hand-authored suite's own negative assertions (mistyped-key,
  non-round-tripping `initial_state`). The script's verdict is a false alarm on a
  passing run; do not read it as a red.
- `/usr/bin/python3 scripts/bugfunnel.py check` — `662 entries, 0 problems`,
  exit 0. Matches the lane.

## C8 — the bugfunnel entry: **PARTIALLY REFUTED**

The entry is one file, correctly named, schema-valid, and follows the skill's
section order (Bug / Root cause / Missing piece / Keystone repro / Remedy). Gap
classification ENVIRONMENT with secondary ORACLE is defensible and argued from
the litmus questions. It names the three covering tests. Two problems:

**The entry contradicts the code.** Line 100 says the helper "fails loud at a
**5s** deadline". Every call site in the shipped code passes
`Duration::from_secs(2)` (`sim_windowed_replay.rs:424, 469, 498, 504, 524, 542,
564, 764, 771`). The entry was written against the reverted 5 s revision and not
updated. It is the durable artifact a future reader will trust.

**The production defect is not pickable up.** The main-panel blank-frame defect
and the `assert_content_fidelity` oracle hole exist only as prose inside an entry
whose front matter reads `status: FIXED`, plus the lane report, which is not a
tracked deliverable. `bugfunnel.py list --status OPEN` will never surface them,
so the funnel counts this escape closed while the production defect it names is
untouched. Per the skill's own framing — one escape, one file, and the
distribution steers investment — the render gap and the oracle hole want their
own `status: OPEN` entries. The report's answer to "where does a fix lane pick
this up" is currently "nowhere tracked".

## C9 — comments and data hygiene: **CONFIRMED, one style note**

Secrets scan over the whole diff (`api_key|secret|password|token|PRIVATE KEY|`
user email `|` vault path) returns nothing. No vault content, no real data; the
census format string emits only synthetic seeded ids (`block:c1`, `block:parent`).

Comment density and length are in line with the surrounding harness, and the
census doc comment earns its place by naming the three distinguishable miss
shapes. One rule-1 wobble: `click_point_when_painted`'s doc opens "A single-shot
bounds read is the odd verb out in this driver" — after this change no such read
remains on these verbs, so the clause is oriented to the diff rather than the
tree. The following sentence (the shell is re-created with an empty item vec)
carries the actual reason and stands alone.

---

## Defects, for routing (not remedied here)

1. `sim_windowed_replay.rs:751` — `scroll_entity` still resolves bounds from one
   frame and returns `Ok(())` on a miss, silently. Fifth verb; contradicts the
   report's and the commit message's "all four" / "no verb can sample a single
   frame again".
2. `sim_windowed_replay.rs:228` — `mouse_point` is dead, and hidden from the
   lint by `pbt_harness/mod.rs:14`.
3. Bugfunnel entry line 100 says 5 s; the code says 2 s at nine call sites.
4. The 2 s constant is tuned, not derived. `pump_cycle` advances the app's fake
   clock 500 ms per iteration, so the deadline changes test semantics, not just
   patience. Costs ~13 s on `arrow_walk_keeps_focus_on_the_reference_outline_neighbour`
   at zero load, against a 120 s budget on a row that timed out twice at base
   under load.
5. `send_key_chord`'s 4 s `overall_deadline` (`:410`) is not adjusted for the
   up-to-3 s-per-iteration cost the frame-wait introduces; the re-click loop is
   effectively single-attempt now. Latent — fires in neither run.
6. The production blank-frame defect and the `assert_content_fidelity`
   `total_descendants > 0` exemption are recorded only as prose in an entry
   marked FIXED. No OPEN entry, so no fix lane will find them.

## Weave safety

Safe to weave. The diff touches one test-harness file and adds one doc; nothing
in `crates/` or `frontends/gpui/src/` changes, so no production behaviour moves
and no other lane's file is contended. `cargo check --workspace --all-targets`
is clean and `just hand-authored` is green in my run.

It must weave **after** `gpui-reds`, as planned: `docs/Testing/GpuiCrateReds-2026-09-10.md`
does not exist at `7e00a965f01e`, and the lane correctly declined to create a
competing copy. The register row edits the report specifies (rows 2, 5, 6 → FIXED;
row 16 → LIKELY FIXED; rows 1, 3, 4 failure text replaced; Family A hypothesis
replaced by the measured result; Family E de-linked; result table 16 → 12) are
all supported by my run and can be applied at rebase. Defects 1–3 above are
one-line follow-ups that need not block the weave; defect 6 should become an
OPEN entry before the funnel's numbers are read again.

---

# Rev 2

`@` re-read at verification time: **`b25c76bf08b2`** (parent unchanged,
`7e00a965f01e`). Working copy is now 4 paths: the harness file and three
bugfunnel entries. `pwd` for every command below:
`/Users/martin/Workspaces/pkm/holon/.claude/worktrees/gpui-driver`.
Tree-identity gate in both scripts: exit 93/94 unless
`BOUNDS_WAIT_PUMP_CYCLES` and `CHORD_FOCUS_CLICK_ATTEMPTS` are on disk.

**Verdict: five of six claims CONFIRMED; one new defect the lane did not
measure, because it ran no row that could show it.** Safe to weave after
`gpui-reds`.

## (1) `scroll_entity` routed through the shared wait — CONFIRMED

`sim_windowed_replay.rs:766-771` now calls `click_point_when_painted` and maps
the miss to an error; the `Ok(())` swallow is gone, and the comment says why.

Exhaustive grep for a remaining single-frame read: `bounds_center_f32` is called
at exactly one site (`:271`, inside the wait), `text_center` at exactly one
(`:267`, same), `element_info` at `:154` (inside `bounds_center_f32`) and `:577`
(inside `set_block_expanded`'s bounded loop). All five verbs route through the
8-cycle contract — `click_entity` (`:552`), `send_key_chord` (`:433`),
`set_block_expanded` (`:575`), `drop_entity` (`:780`, `:787`), `scroll_entity`
(`:768`). Defect 1 of rev 1 is closed.

## (2) `mouse_point` deleted — CONFIRMED

`grep -c mouse_point` on the file = 0; recursive grep over `frontends/gpui/`
returns no file with a non-zero count. Defect 2 closed.

## (3)+(4) Deadline counted in cycles — CONFIRMED, and the reasoning is sound

`BOUNDS_WAIT_PUMP_CYCLES: usize = 8` (`:61`) with a doc comment (`:48-60`) that
states the actual reason: `pump_cycle` advances the fake clock 500 ms per
iteration, so a wall-clock deadline is a clock-injection rate. That is precisely
the mechanism I identified in rev 1, and converting to a cycle count removes the
coupling rather than re-tuning around it. The comment derives 8 from the settle
chain (two frames for the shell refill, plus CDC → Loro → projection) and states
the headroom. This is a derived constant now, not a tuned one.

`from_secs(5)` and "5s deadline": **0 occurrences** in the diff. The entry
(`…reads-one-frame…:100-110`) states the budget as `BOUNDS_WAIT_PUMP_CYCLES` (8),
gives the fake-clock reason, and records the 5 s revision only as measured
history in the Remedy section, which is what a funnel entry is for. Rev 1
defect 3 (entry says 5 s, code says 2 s) is closed.

Arrow-nav timing, isolated, one test per nextest invocation
(`lane-logs/verify2-iso-arrow_walk_*-{1,2}-63822.log`):

| Revision | Condition | Result |
|---|---|---|
| base `830d794f` | serial, 58-test subset | `PASS [52.188s]` |
| rev 1, 2 s wall | serial, full suite (mine) | `PASS [65.452s]` |
| rev 2, 8 cycles | isolated run 1 | `PASS [33.103s]` |
| rev 2, 8 cycles | isolated run 2 | `PASS [24.566s]` |

The claimed 36.4 s is corroborated and if anything conservative. In my 11-test
serial batch the same row read `PASS [57.228s]`, which is contention on the box,
not the change.

## (5) `CHORD_FOCUS_CLICK_ATTEMPTS = 3` — CONFIRMED, arithmetic cannot re-narrow

`:419` seeds `attempts_left = 3`; the loop clicks, polls for landing, and on a
miss does `attempts_left -= 1` then bails at zero (`:461-468`). That is three
clicks performed, with the bail after the third fails — no off-by-one, and no
attempt is lost. Because the budget is a count, the cost of
`click_point_when_painted` inside each iteration cannot consume it, which is
exactly the failure mode of the 4 s wall budget it replaces. Rev 1 defect 5
closed.

Nit only: `attempts_left` is `usize`, so a `CHORD_FOCUS_CLICK_ATTEMPTS` of 0
would underflow. The const is 3 and private to this file, so it is unreachable.

## (6) Two OPEN entries and the disclosure — CONFIRMED

Both new entries exist with well-formed front matter:

- `…-main-panel-collection-shell-is-rebuilt-empty-each-projection` — `gap: PERCEPTION`,
  `secondary: ORACLE`, `status: OPEN`.
- `…-content-fidelity-exempts-a-shell-with-no-descendants` — `gap: ORACLE`,
  `status: OPEN`.

The FIXED entry carries the disclosure at line 119: "This fix makes a TRANSIENT
blank panel unobservable to every windowed oracle", states the trade, and links
both OPEN ids by name. That is the exact gap rev 1 flagged, and it is now
findable by `bugfunnel.py list --status OPEN`. Rev 1 defect 6 closed.

`/usr/bin/python3 scripts/bugfunnel.py check` → `664 entries, 0 problems`,
exit 0. The count moved 662 → 664, consistent with two added entries.

## Gates I ran

- `cargo check -p holon-gpui --features holon-gpui/pbt --all-targets` — `CHECK_EXIT=0`
  (`lane-logs/verify2-check-33803.log`).
- Eleven named rows, serial, semaphore-wrapped (`lane-logs/verify2-rows-33803.log`):
  `Summary [422.167s] 11 tests run: 5 passed (2 slow), 5 failed, 1 timed out, 385 skipped`.

The five claimed-green rows all pass: `a_short_window_still_paints_the_outline`
`PASS [18.236s]`, `windowed_composed_sut_replays_a_fixture_via_replay_steps_green`
`PASS [28.313s]`, `windowed_composed_sut_drives_a_click_gesture_sequence_green`
`PASS [28.375s]`, `arrow_walk_keeps_focus_on_the_reference_outline_neighbour`
`PASS [57.228s]`, `benchmark_windowed_per_case_boot_cost` `PASS [68.456s]`.

Register rows still red, with messages identical to rev 1 and to the register:
rows 1, 3 and 4 panic at `crates/holon-integration-tests/src/pbt/op_write_cap.rs:381`
with "editable surface not projected by this driver"; rows 12 and 13 with
"vacuity guard: the Tab chord changed no parentage"; row 14 with "vacuity guard:
the blur dispatched NO operation (6 history rows before and after)". **16 → 12
still holds.**

None of the new bail paths fires anywhere in the run: `not in bounds after`,
`re-clicking`, `never moved focused_block` and `scroll_entity:` all grep to 0.

## New defect — register row 3 gives up rev 1's gain and now flips to TIMEOUT

`general_e2e_composed_pbt_windowed` regressed against rev 1. Three observations
at rev 2, two of them with the row running alone:

| Revision | Condition | Result |
|---|---|---|
| base (register) | serial | `FAIL 89s`; TIMEOUT under load |
| rev 1 (mine) | serial, full suite | `FAIL [55.438s]` |
| rev 2 (mine) | serial, 11-test batch | `TIMEOUT [120.042s]` |
| rev 2 (mine) | isolated run 1 | `FAIL [87.801s]` |
| rev 2 (mine) | isolated run 2 | `TIMEOUT [120.021s]` |

The cause is unchanged — the log shows the same repeated
`op_write_cap.rs:381` panic while proptest shrinks — and the new 8-cycle bail
never fires. What changed is cost per case: rev 1's 2 s wall cap could cut a
lookup short, and rev 2 removed the wall cap entirely, so a slow lookup now runs
its full cycle count however long that takes. The row sits right against
nextest's 120 s budget and crosses it about two times in three.

This is **not** a regression against base — the register records row 3 as
`FAIL 89s` serial and TIMEOUT under load, which is where rev 2 puts it. But it
forfeits rev 1's improvement, and when the row times out nextest kills it, so it
emits no failure message at all. Anyone re-deriving the register from a rev-2 run
will see row 3 as a TIMEOUT rather than as the surface-projection red it is.
The lane could not have seen this: it ran only the five green rows, and row 3 is
not among them.

Not a weave blocker. It should be recorded on register row 3 as "FAIL or TIMEOUT,
borderline against the 120 s budget", so the next lane does not read the timeout
as a new fault.

## Limitation of this pass

I re-ran eleven named rows, not the full 390-test suite, per the brief. The
global `378 passed / 12 failed` shape is therefore carried over from my rev-1
run and is **not** re-measured at `b25c76bf08b2`. Every verb in the driver
changed signature in rev 2, so any windowed test that calls one could in
principle have moved; `cargo check --all-targets` is clean, which bounds that
risk to behaviour, not compilation. If the weave wants the register numbers
restated from rev 2, one full serial run is needed.

## Weave safety — rev 2

Safe to weave, after `gpui-reds` as planned. Still test-harness-only plus docs;
no `crates/` or `frontends/gpui/src/` change, so no production behaviour moves.
All six rev-1 defects are closed. Carry one addition into the register edits:
row 3 is borderline against the nextest timeout and will present as TIMEOUT in
roughly two runs out of three.
