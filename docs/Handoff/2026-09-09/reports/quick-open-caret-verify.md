# quick-open-caret — adversarial verification

## Agent 5 delta

Verifier: fresh-context, read-and-run only. Nothing fixed, nothing committed,
no jj/git write ran.

`pwd` for every verdict below:
`/Users/martin/Workspaces/pkm/holon/.claude/worktrees/quick-open-caret`

Tree under verification: `@ = 73223855ffb5`, `@- = 830d794f878f` (matches the
brief). `jj diff -r 73223855ffb5 --stat` = 34 files, +1207/-207 (matches).
Tree-identity asserts both hit:
`crates/holon-integration-tests/src/pbt/transitions/jump_to_search_hit.rs` and
`crates/holon-frontend/src/view_event_handler.rs` (`fn edit_target_id`).

**Overall verdict: CONFIRMED with two defects and one materially overstated
claim.** No gate I ran contradicts the lane. The lane's own diagnosis of the
parked defect names the wrong write leg, and the headless keystone replay does
not pin production caret seating the way the bugfunnel entry implies.

---

### C1 — D97.a semantics and the single writer — CONFIRMED

- One writer. `ReactiveEngine::spawn_caret_seat`
  (`crates/holon-frontend/src/reactive.rs:3039`) is called from exactly four
  sites, all guarded by `seat_caret_for_navigation`: `reactive.rs:3938`,
  `:4080`, `:4175`, `:4249`.
- `seat_caret_for_navigation` (`reactive.rs:4830`) returns a destination only
  for `NavigationOp::Focus | OpenTab` and only for `region == "main"`
  (`reactive.rs:4870-4872`).
- Grep for a second caret writer on the navigation path found none. The other
  `ALLOW(direct_focus_mutation)` sites are unrelated: `reactive.rs:2994`
  (birth seat), `:4425` (`BuilderServices::set_focus`, the user-gesture door),
  `:4888` (blank home view clears), `:4983` (delete clear).
- The immediate-birth interception is genuinely deleted. The removed hunk in
  `BuilderServices::set_focus` is visible as deleted lines in
  `lane-logs/verify-c9-diff.patch:440-445`
  (`if let Err(e) = self.birth_creation_affordance(id.as_str())`). The
  replacement (`reactive.rs:4413-4428`) only reaps newborns and records a
  `CaretPlacement::UserPlacement`.
- `caret_block_for_edit` is on `BuilderServices`: trait declaration/default at
  `reactive.rs:434`, engine impl at `reactive.rs:4369`.
- Seat semantics match the ruling: `spawn_caret_seat` queries
  `QueryEngine::first_caret_target`, with two last-writer guards (nav
  generation, user-caret generation).

### C2 — Reference model — CONFIRMED

- Seats on `nav_focus` and `nav_open_tab` only, both through the single
  `ReferenceState::seat_caret_after_navigation`
  (`crates/holon-integration-tests/src/pbt/ref_caps/nav.rs`), which returns
  early for any region other than `Main`. Both previously did
  `focused_block = destination`.
- `caret_seat_for_navigation` (`reference_state.rs:1979`) returns the first
  sorted child, else `RowOrigin::creation_placeholder_id(destination)`. Mints
  nothing.
- `TypeChars` placeholder branch: `caret_creation_slot_parent` detects a
  `RowOrigin::CreationPlaceholder` caret and routes to
  `birth_block_via_creation_slot`, returning before the per-char loop
  (`transitions/type_chars.rs`).
- The default is **fail-loud**: `birth_block_via_creation_slot` on
  `RefBlockTreeMut` (`crates/holon-pbt-core/src/capabilities.rs`) is
  `unimplemented!("...this reference models no creation affordance...")`. It
  panics; it does not no-op.
- Note, not a defect: the method moved from `RefLayoutMutate` (where it was
  *required*, no default) to `RefBlockTreeMut` *with* a default. A new
  reference can now omit it and only discover that at runtime rather than at
  compile time. The panic keeps it loud, which is what D97.a asked for.

### C3 — `JumpToSearchHit` — CONFIRMED on registration, **REFUTED on teeth**

Registration and weight confirmed: module + re-export + enum variant +
`one!(JumpToSearchHit, c::SutSearch)` in
`transitions/mod.rs`; `weighted_generator` returns weight `6`.

Driver, not internals: `apply_to_sut` calls only
`SutSearch::jump_to_search_hit`
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:2098`),
which runs the production `quick_open_search` and dispatches through
`synthetic_dispatch("navigation", "focus", ...)`.

Page gate present in **two** places (`weighted_generator` filter and
`preconditions`, `jump_to_search_hit.rs`), disclosed in a 6-line comment at the
precondition. It is *not* in the module `//!` doc, which is what the brief
asked for — minor.

**Anti-vacuity — this is the finding.** After the jump the transition asserts:

| half | what it asserts |
|---|---|
| ref (`apply_to_ref`) | `assert_caret_seated_inside` — on the **model's own** `focused_block`, one line after `nav_focus` set it |
| SUT (`components.rs:2098-2131`) | the overlay offered the hit; `navigation.focus` succeeded; `assert_navigate_focus_landed` — the **root**, not the caret |

Nothing in the transition compares the **production** caret to the model's.
The oracle that would is `inv-focus-matches-ref`, and it is windowed-only by
construction: its wiring needs `SutDriver`, which
`crates/holon-integration-tests/src/pbt/composed/invariants/focus_matches_ref.rs:10-12`
states "storage/headless slices lack `SutDriver` and deselect". My own headless
runs confirm it empirically — `inv-focus-matches-ref=deselected` in the
engagement summary of both `lane-logs/verify-c5-handauthored.log` and
`lane-logs/verify-c6-unparked.log`.

So headlessly `JumpToSearchHit` **would pass with the production caret left on
the destination** — the exact D97.a bug. Production seating is pinned only by
the two GPUI windowed rungs. I could not run the decisive probe (revert the
four `spawn_caret_seat` calls and replay): the sandbox classifier blocked the
edit, and I am read-only by role. The analysis above stands on the wiring doc
plus the two `deselected` engagement lines.

Consequence for C7 below: the bugfunnel entry's claim that the keystone
transition "asserts the seat as its postcondition; replayed deterministically
by `a-jump-into-a-populated-page-...`" is true of the model and **not** of
production. Agent 5's report is more honest than the entry — it says the
red-for-the-right-reason log "is not runnable headlessly". The entry does not
carry that caveat. This is a disclosed, pre-existing architectural limit, not
a defect the lane introduced, but the entry overstates the coverage.

### C4 — Headless mirror arming — CONFIRMED as prod-faithful

`crates/holon-frontend/src/headless_editor_mirror.rs:271` adds
`vm.set_async_context(engine.clone())` before `apply_local_edit`. GPUI does the
same at editor mount: `frontends/gpui/src/views/editor_view.rs:152`,
`controller.set_async_context(services.clone())`, inside the editor's
constructor. This is the mirror catching up to production, not a harness
patch. Difference worth naming: GPUI arms once at mount, the mirror arms on
every keystroke. The setter is idempotent, so this is cosmetic.

### C5 — Gates, all re-run by me

| gate | log | summary line I observed |
|---|---|---|
| `cargo check --workspace --all-targets` | `verify-c5-check.log` | `Finished dev profile ... in 29.68s`, 0 errors |
| nextest `-p holon-frontend` | `verify-c5-frontend.log` | `Summary [5.844s] 590 tests run: 590 passed, 0 skipped` |
| nextest `-p holon-integration-tests` substring `navigate` | `verify-c5-integration-nav.log` | `Summary [40.421s] 11 tests run: 11 passed (1 slow), 688 skipped` |
| `just hand-authored` | `verify-c5-handauthored.log:6232` | `test result: ok. 9 passed; 0 failed` — 81 `running case` lines, 81 `PASSED case` |
| `just keystone-smoke` | `verify-c5-keystone-smoke.log` | exit 0, `test result: ok. 4 passed; 0 failed` |
| `scripts/keystone-known-reds.sh` on that log | — | `GREEN: run passed, nothing to classify`; `PASS: 1 green run(s)` |
| `just check-frontend-wasm` | `verify-c5-fewasm2.log` | `FRONTEND_WASM_EXIT=0` |
| `just check-worker-wasm` | `verify-c5-wkwasm.log` | `WORKER_WASM_EXIT=0`, `test result: ok. 5 passed` |
| `featuremap.py check` | `verify-c5-featuremap.log` | `docs/Architecture/FeatureMap.md is up to date`, exit 0 |
| `bugfunnel.py check` | `verify-c5-bugfunnel.log` | `655 entries, 0 problems`, exit 0 |
| nextest `-p holon-gpui` 3 windowed tests (extra, mine) | `verify-gpui-windowed.log` | `Summary [44.293s] 10 tests run: 10 passed (10 slow), 0 skipped` |

Named hand-authored case present in my run:
`verify-c5-handauthored.log:6214` `running case
"a-jump-into-a-populated-page-seats-the-caret-on-its-first-child"`, and
`:6229` `PASSED case` for the same name. The lane cited `a5-ha-full-2.log:6117`;
my line number differs, the case is the same and it passed.

Two deviations from the brief's gate list, both explained:

1. **The `search` and `jump` substring filters match ZERO tests.**
   `cargo nextest list -p holon-integration-tests --features pbt` produces 928
   lines; `grep -ciE 'search|jump'` on it = 0, `grep -ciE 'navigat'` = 17
   (`verify-c5-list.log`). Running `search jump` as substrings gives
   `Summary [0.003s] 0 tests run: 0 passed, 699 skipped / error: no tests to
   run` — a false-green shape if anyone reads only the exit intent. The lane's
   "11 passed, 688 skipped" is entirely the `navigate` third of its filter,
   which I reproduced exactly. The new transition has **no** test whose name
   contains `search` or `jump`; it is reached only through the composed
   keystone and the hand-authored replay.
2. **My `keystone-smoke` was fully green** — 4/4, zero reds, so the classifier
   had nothing to classify. The lane saw 23 known `editor-text-mirror` reds and
   0 novel. Different proptest draw. "0 novel" holds in my run trivially; I
   cannot corroborate the count 23.

### C6 — The PARKED empty-destination replay — CONFIRMED, and reproduced

Parked correctly: the case is `#`-commented in
`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl` with
a 7-line inline reason above it. The runner really skips it —
`load_cases` (`crates/holon-integration-tests/src/pbt/hand_authored.rs:196`)
filters `!line.is_empty() && !line.starts_with('#')`, and in my run the string
`a-jump-into-an-empty-page` appears **0 times** in
`verify-c5-handauthored.log`. It is not silently passed.

The runner does take a path: `HOLON_HAND_AUTHORED_SIDECAR` (joined to
`CARGO_MANIFEST_DIR`), plus `HOLON_HAND_AUTHORED_CASE` to select one case. I
un-parked into a copy at `lane-logs/verify-unparked.jsonl` (82 JSON lines vs 81
tracked) and ran the case alone. The tracked file was never edited:
`sha256sum crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`
= `ee71a191c41d969227a29889ce75b97ccee197f648e6a38cf79e13643d867f0d`, unchanged
across the run.

**It reds.** `lane-logs/verify-c6-unparked.log:300`:

```
thread 'hand_authored_keystone_regressions' panicked at
crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:2669:21:
[SutEditorMirrorWrite::apply_type_chars] send_raw_keystroke('z') failed:
dispatch_intent_sync: block.set_field failed: Operation 'set_field' on entity
'block' failed: set_field('content'): capture prior state:
Block not found: block:b26f233b-0891-4ddd-9c84-55701b141734
```

**DEFECT 1 — the lane names the wrong write leg.** The report says the failure
is "on the SqlOnly write leg — no Loro cell". My log shows the opposite. The
preceding line, `verify-c6-unparked.log:297`, is:

```
WARN holon_loro::loro_block_operations: [LoroBlockOperations::execute_operation]
CRUD op 'set_field' failed: set_field('content'): capture prior state:
Block not found: block:b26f233b-...
```

The case's own wiring is `storage_adapters: ["Loro", "Turso"]`, and the writer
that raised "Block not found" is `LoroBlockOperations`. The report's causal
story — "GPUI does not show it because its editor writes through the CRDT cell"
— is therefore not supported by this evidence: the CRDT leg is the one that
failed. The **race is real** and the replay is correctly parked; only the
attribution is wrong. That matters for D109, because the ordering fix has to
target the leg that actually loses the newborn.

I did confirm the GPUI half is green independently:
`verify-gpui-windowed.log`, `enter_navigating_out_of_quick_open_seats_a_caret`
and `a_jump_into_an_empty_page_left_again_births_nothing` both PASS. So the
headless/windowed divergence is genuine, whatever its mechanism.

### C7 — Bugfunnel entry — CONFIRMED on naming, PARTIAL on coverage

`docs/Testing/bugfunnel/entries/2026-09-08-quick-open-enter-navigation-leaves-no-editable-focus.md`
is `status: FIXED` and names its covering tests (entry lines 78-90). The named
tests exist and pass:

- `enter_navigating_out_of_quick_open_seats_a_caret`
  (`frontends/gpui/tests/quick_open_returns_focus_windowed.rs:485`) genuinely
  exercises the `page_title` root cause: it asserts the engine caret is the
  destination's creation slot and **not** the destination
  (`:506-511`), that the seated row holds WINDOW focus (`:512-518`), that
  `inv-window-focus-matches-engine-focus` now MEASURES rather than skips
  (`:519-524`) — which is precisely the "the title row mounts no editable_text"
  mechanism the entry describes — that navigation alone creates nothing
  (`:525-529`), and that the first keystroke births exactly one block carrying
  `"v"` (`:531-545`).
- `a_jump_into_an_empty_page_left_again_births_nothing` (`:554`) covers the
  no-leak half.

The keystone half is where the entry overstates: see C3. The replay pins the
model's seat, not production's.

### C8 — Report hygiene — one of three is durable only in the report

| divergence | recorded where | verdict |
|---|---|---|
| caret 0 vs end-of-text after clicking the seated row | tracked `keystone.jsonl`, in the live case's own `description` field | durable |
| first keystroke reads 17 vs modelled 7 | same `description` | durable |
| dioxus-web `editor.rs:347` unrouted | **nowhere but the lane report** | **DEFECT 2** |

I grepped `docs/` and the replay corpus for a dioxus record of this and found
none. The file itself (`frontends/dioxus-web/src/editor.rs:340-352`) carries a
comment about the caret seam but says nothing about `edit_target_id` or the
creation slot. Mitigating: it is fail-loud rather than silent — a slot id
reaching there trips the assertion at
`crates/holon-frontend/src/editor_view_model.rs:1307`. Item 4 of the lane's
"not done" list (`apply_mark`/`remove_mark` on the raw `context_id()`) is
likewise report-only.

The parked production defect itself IS durable — it lives in the tracked
`keystone.jsonl` comment block, which is the right place given the lane's
reasoning that a test-caught red is not a funnel escape. I agree with that
reasoning.

### C9 — Secrets and real data — CONFIRMED clean

`jj diff -r @ --git` saved to `lane-logs/verify-c9-diff.patch` and scanned for
`ANTHROPIC|API_KEY|SECRET|_TOKEN|PASSWORD|PRIVATE KEY|holon-pkm|\.env`: the
only hits are two `query_sql(ACTIVE_RIGHT_ROOT_SQL)` / `BACKGROUND_TAB_SQL`
lines matching on `_TOKEN`-adjacent text, no values. No env dumps, no vault
content, no fixture copied from `holon-pkm`.

Files changed outside `crates/ docs/ frontends/`: **none**.
`jj diff -r @ --summary | grep -vE '^(crates/|docs/|frontends/)'` is empty.
`hand-authored-regressions/` lives under `crates/holon-integration-tests/`.

### C10 — Comment hygiene — one violation

`frontends/gpui/tests/overlay_sidebar_dismisses_windowed.rs:595`:

```rust
// `Region::RightSidebar::as_str()`. The old `"right"` named no
// region at all: it navigated nothing and was visible only through
// the caret the mirror used to move for every region.
```

This is history narration ("the old", "used to move"), which
`~/.claude/skills/commenting/SKILL.md` bans. The first sentence carries all the
value. Cosmetic, but it is the one clear hit.

Everything else I read is within the two-reason rule and describes current
state. The longest new comments — `spawn_caret_seat`'s doc
(`reactive.rs:3027-3038`), `ViewEventHandler::edit_target_id`
(`view_event_handler.rs:121-127`), and the `birth_block_via_creation_slot`
default (`capabilities.rs:356-372`) — each justify one non-obvious design
choice and stay at two reasons.

Side note on the same file: that hunk changes the test's `region` param from
`"right"` to `"right_sidebar"`, so the test now exercises a real region where
it previously exercised none. It passes (`verify-gpui-windowed.log`), but the
rung's meaning changed and the commit message does not mention it.

---

## Weave recommendation

Safe to weave with the empty-destination half PARKED. Every gate is green in my
own runs, the parked case is genuinely skipped rather than silently passed, and
the production defect is reproducible on demand from a copy of the corpus.

Three things to carry into D109 and the commit:

1. Give Martin the corrected leg. The reproduction shows `LoroBlockOperations`
   raising "Block not found", not the SqlOnly leg. D109's ordering fix should
   be aimed accordingly.
2. Soften the bugfunnel entry's keystone sentence, or accept that the seat's
   production pin is windowed-only. As written it claims more coverage than
   exists.
3. Record the dioxus-web gap somewhere tracked, or state plainly that the
   fail-loud assertion is the record.

None of the three blocks a weave.

---

## Agent 6

`pwd` for every verdict: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/quick-open-caret`

Tree verified: `@ = ac4b90e3f81f` (was `73223855ffb5` at the Agent 5 delta),
`@- = 830d794f878f` unchanged. `jj diff -r @ --stat` = 35 files, +1346/-207.
One new file vs the prior rev:
`docs/Testing/bugfunnel/entries/2026-09-09-dioxus-web-editor-never-resolves-a-slot-caret.md`.

**Verdict: CONFIRMED. All three findings from the Agent 5 delta are resolved.
Safe to weave with the empty half PARKED pending D109.**

### C3 teeth — CONFIRMED, and the assertion cannot pass vacuously

`SutSearch::jump_to_search_hit` now takes `expected_first_child` and ends with
`assert_caret_seated_in`
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:1753-1780`,
called at `:2177`).

- **Reads through the component's engine handle, not internals.** The poll is
  `self.reactive.focused_block()`, where `reactive: Arc<ReactiveEngine>`
  (`components.rs:172`). The windowed invariant reads the identical call —
  `self.engine.focused_block()` at
  `crates/holon-integration-tests/src/pbt/driver_input.rs:376`. Same method,
  same type, same value. The *drive* still goes through
  `synthetic_dispatch("navigation", "focus", ...)`, so the drive-via-drivers
  rule is intact; this is an observation, not a new drive seam.
- **No vacuous branch.** `expected_first_child = None` does not skip — it
  resolves to `RowOrigin::creation_placeholder_id(root)`, a concrete id, and
  the same hard assert applies (`components.rs:1758-1764`). Both branches
  assert an exact equality against a non-null expected value. There is no
  `if let Some`, no early return, no `Skipped`.
- **Fails loud on timeout.** The 3s poll ends in `assert!`, not a break.
- The expected value comes from the reference (`state.sorted_children(&self.hit)
  .into_iter().next()` in `apply_to_sut`, `jump_to_search_hit.rs:183`), read
  pre-apply, and is resolved into SUT space by the component's shared
  `resolve_id`. Model and production are genuinely cross-checked.

Teeth log inspected and consistent with the claim.
`lane-logs/a6-teeth.log:290` carries the panic at `components.rs:1772`:
`[JumpToSearchHit] after navigating main to block:structural-page, the engine
caret is None but must be block:parent`; `:376` `test result: FAILED. 8 passed;
1 failed`; `:729` `test result: ok. 9 passed` after restore. Production is
untouched: `sha256sum crates/holon-frontend/src/reactive.rs` =
`08937105480be2b055d3769fe33ba89a57b89a94076edffaa9c7c56a4842c46d`, byte-identical
to the value I recorded in the Agent 5 delta, and
`grep -c HOLON_TEETH_NO_SEAT crates/holon-frontend/src/reactive.rs` = 0.

Minor, not a defect: the probe drove the caret to `None` rather than to the
destination id, so it demonstrates the assert fires on a missing seat rather
than on the exact D97.a wrong-seat shape. The assertion is an equality against
one expected id, so a caret left on the root fails it too.

### Defect 1 (attribution) — CONFIRMED fixed

Corrected in all three places, and the withdrawn story is gone:

- `crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl:723-729`
  now reads "Measured on the CRDT leg: with this case's `["Loro","Turso"]`
  wiring the failure is raised by `LoroBlockOperations::execute_operation`".
  The old "SqlOnly write leg" and "GPUI does not show it because its editor
  writes through the CRDT cell" sentences are removed. The mechanism is stated
  as the spawn/dispatch ordering, with D109 named.
- The 09-08 bugfunnel entry carries the same corrected leg.
- `grep 'CRDT cell' keystone.jsonl` returns nothing.

### The overstated coverage claim — CONFIRMED fixed

The 09-08 entry's Keystone bullet now states plainly that the poll, "not
`inv-focus-matches-ref`, is what pins production headlessly: the invariant needs
`SutDriver` and deselects in every headless slice", cites the teeth red, and
adds that the EMPTY half is pinned windowed only. That is accurate against
what I measured in the Agent 5 delta.

### Defect 2 (dioxus-web) — CONFIRMED fixed

New entry `2026-09-09-dioxus-web-editor-never-resolves-a-slot-caret.md`,
`status: OPEN`, `gap: COVERAGE`. It names the exact site
(`frontends/dioxus-web/src/editor.rs:347`), states the fail-loud assertion at
`crates/holon-frontend/src/editor_view_model.rs:1307` as the interim record,
gives the real reason the one-line GPUI fix does not port (no `BuilderServices`
on a wire-dispatching editor), and lists a two-step remedy. The COVERAGE
classification is right: the oracle exists and is loud, nothing can generate
the interaction. `bugfunnel.py check` counts 656 entries (was 655), consistent
with exactly one addition.

### C10 comment — CONFIRMED fixed

`frontends/gpui/tests/overlay_sidebar_dismisses_windowed.rs:595-596` now reads
"Exactly `Region::RightSidebar::as_str()`: the dispatcher matches the region
string, so any other spelling navigates no region." No history narration, one
reason.

### Gates, all re-run by me

| gate | log | summary line |
|---|---|---|
| `just hand-authored` | `verify-a6-handauthored.log:6601` | `test result: ok. 9 passed; 0 failed ... in 782.14s` — 81 `running case`, 81 `PASSED case`; jump case PASSED at `:6598` |
| `just keystone-smoke` | `verify-a6-keystone.log` | exit 0, `test result: ok. 4 passed; 0 failed` |
| `keystone-known-reds.sh` on it | — | `GREEN: run passed, nothing to classify`; `PASS: 1 green run(s)` |
| nextest `-p holon-frontend` | `verify-a6-frontend.log` | `Summary [3.775s] 590 tests run: 590 passed, 0 skipped` |
| nextest `-E 'test(navigate) or test(focus)'` | `verify-a6-filter.log` | `Summary [39.875s] 26 tests run: 26 passed (1 slow), 673 skipped` |
| `bugfunnel.py check` | — | `656 entries, 0 problems`, exit 0 |
| `featuremap.py check` | — | `FeatureMap.md is up to date`, exit 0 |

The corrected filter reproduces the claimed 26 exactly. Note it had to run from
a script file (`lane-logs/verify-a6-filter.sh`): passing the filterset inline
through zsh fails with `number expected` on the parentheses.

Secrets and paths re-scanned on this rev: no `ANTHROPIC|API_KEY|SECRET|
PASSWORD|PRIVATE KEY|holon-pkm` hits in `lane-logs/verify-a6-diff.patch`; no
tracked file outside `crates/ docs/ frontends/`; `lane-logs/` is untracked.

### Weave recommendation

Weave. Every gate is green in my own runs, the seat is now pinned headlessly by
a non-vacuous engine read with a demonstrated red, the leg attribution matches
the reproduction I ran in the Agent 5 delta, and both documentation gaps are
closed. The empty-destination replay stays PARKED for D109.
