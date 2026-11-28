# Keystroke pipeline model: report

Mode: SqlOnly (the write_seq echo protocol).
Cell mode (Loro) is a follow-up card (draft at the end).

## Claim

- `Keystroke.tla` models typing into a focused SqlOnly editor: keystrokes,
  dispatch, the store, the data-sync echo and its five decisions, blur, a
  structural op, a second view of the block and one external write.
- On the current design TLC finds that every keystroke reaches the store and
  every view (`NoLostKeystroke`), no write replaces a newer one
  (`NoClobber`) and no view goes back (`MonotoneViews`), while the user only
  types, blurs and keys a chord.
- TLC finds four counterexamples on the current design. Three have a red test
  on a real store and a real view model; one is suspected (model only):
  1. Lost update: an external write is reverted by the editor's next
     whole-buffer write (keystroke or blur flush) when its echo is late.
  2. A typed trailing space is lost when the editor converges to a later
     external write.
  3. Enter after a typed trailing space: the split position is past the
     trimmed content, and `split_block` refuses it.
  4. Suspected: a key typed after Backspace-at-0 and before focus moves
     writes to the deleted block.
- Replays: 8 of the 15 keystroke bugs are design variants of the spec; each
  violates a user-level property. 7 are not expressible; each names why and
  the test that covers it.
- Conformance: a PBT drives the real `EditorViewModel` and
  `evaluate_data_sync_echo` and tla-checker replays every run against the
  spec. Fixed cases first: 26 fixed runs reach all 22 reachable cells of
  decision x intent kind x external write; 23 cells are unreachable, each
  with its reason. A planted mutant of the echo rule is red.

## What is where

- Spec: `Keystroke.tla`. Configs: `Keystroke_*.cfg`.
- Rows: `run.py`, dict `KEYSTROKE` (properties and witnesses per config) and
  `RED_TEST_ORACLES` (the oracles of the red tests on the closing designs).
- Conformance PBT: `crates/holon-frontend/tests/keystroke_conformance.rs`,
  configs `Keystroke_trace_sticky.cfg` and `Keystroke_trace_fresh.cfg`.
- Run: `python3 docs/Testing/ModelChecking/run.py`.

## The spec

### State

A text is a sequence of key ids. Key k appends k; a key in `Spaces` is a
whitespace key, which the store trims when it is trailing. The external
writer appends `ExtChar`.

| Variable | Code |
|---|---|
| `buf`, `base` | `EditorViewModel::buffer`, the commit funnels' baseline |
| `last` | `EditorViewModel::last_local_seq` |
| `clock` | `write_seq::next` (the global counter) |
| `queue` | admitted, not yet applied writes (key, flush, ext, split, join, create) |
| `sc`, `ss` | `block_raw.content`, `block_raw.write_seq` |
| `ech` | row changes in transit to the editor's data subscription |
| `pch`, `pv` | the second view of the block (a backlink panel) |
| `focus`, `mounted`, `sop`, `moved` | the focused block, the editor, the structural op |

### Actions and their code

Every action carries a `Models:` anchor in the spec.

| Action | Code |
|---|---|
| `Type` (via `LocalEdit`) | `EditorViewModel::apply_local_edit`, SqlOnly arm; `EditorView` Change arm dispatches at once |
| `DeliverEcho` (via `Decide`) | `evaluate_data_sync_echo`, `converge_from_data_sync`, `set_buffer_from_authority` |
| `BlurAway` (via `FlushOf`) | `EditorViewModel::on_blur` |
| `Structural` | `dispatch_structural_as_commit_point`, `chord_commit_intent` |
| `FollowFocus` | `apply_structural_focus`, a new `EditorViewModel` seeded from the row |
| `SlashEnter`, `ApplyStrip` | `apply_popup_action` (strip of a slash command) |
| `ExternalWrite` | a non-editor `set_field` (MCP, org re-ingest, a second editor) |
| `Apply` (`ApplyWrite`, `ApplySplit`, `ApplyJoin`, `ApplyCreate`) | the `set_field` arm of `SqlOperationProvider::execute_operation_with_origin`, `split_block`, `join_block`; admission order |
| `ApplyDetachedCreate` | `birth_creation_affordance` (pre-fix shape) |
| `DeliverPanel` | a second view that renders the delivered row |

### Facts checked at the base

- Admission is synchronous at the dispatch call
  (`OperationEngine::commit_keystroke` admits when called), so writes on one
  block land in send order: `Dispatch = "fifo"`.
- A write without `write_seq` leaves the column as it is
  (the `set_field` arm of `SqlOperationProvider::execute_operation_with_origin`): an external write is "sticky".
- The store trims trailing whitespace (`canonicalize_stored_content`).
- The create of a first keystroke into a slot is admitted before the key.
- `split_block` refuses a position past the stored text.
- Not verified: a comment on `chord_commit_intent` says "a focused editor
  receives no data-sync echo". The model has a flag for it (`EditorEcho`); the
  current design is "delivered", the replay of 2026-07-21 uses "none".

### Properties (user level)

- `NoLostKeystroke == <>[]stopped => <>[]Converged`. `Converged`: every
  typed key is in the store (a trailing whitespace key may be trimmed) and in
  the editor of its block; every view shows the store; the editor's text
  trims to the store; a keyed structural op took effect.
- `ExternalWriteKept == <>[]stopped => <>[]Kept`: the external text is in the
  store and in the editor of its block, or it is disclosed. Nothing discloses
  a dropped external write today (`Disclosed == FALSE`).
- `NoClobber`: the stamp of the stored content never goes back.
- `MonotoneViews`: no view shows an older version than before, and no typed
  key leaves the editor while it stays on its block.

### Fairness

Weak fairness on `Apply` (the runtime polls every spawned dispatch task and
admission releases claims in order), `DeliverEcho` and `DeliverPanel` (the
LiveData row signal and the views' loops), `FollowFocus`, `ApplyStrip` and
`ApplyDetachedCreate` (the foreground tasks that run them). The user and the
external writer get no fairness.

### Witnesses and teeth

- Witnesses (each must be violated, on the same config as its property):
  `NotTypedAndStopped` (the user typed every key and stopped),
  `NoStaleEcho` (an echo arrives after a newer keystroke), `NoInSync`,
  `NoAdoptBaseline`, `NoConverge`, `NoDropNoSeq`, `NeverRewritten`,
  `PanelNeverMoves`, `ExternalNeverAdopted`.
- Teeth: `Keystroke_teeth` (no keystroke write lands) violates
  `NoLostKeystroke`; `Keystroke_deltateeth` (no echo converges) violates
  `ExternalWriteKept` on a design where it holds. `NoClobber` and
  `MonotoneViews` are violated by the replays of 2026-07-12 and 2026-08-12.

## Results

Log: `target/lane-1521/d1-runpy.log`, numbers `target/lane-1521/tlc/numbers.txt`.

| Config | Design | NoLost | ExtKept | NoClobber | Monotone |
|---|---|---|---|---|---|
| `main` | current, 3 keys, one whitespace key, chord, blur | holds | holds | holds | holds |
| `quiet` | current, external write after the last key | holds | holds | holds | holds |
| `split` | current, Enter at the end, no whitespace key | holds | holds | holds | holds |
| `birth` | current, the first key births the block | holds | holds | holds | holds |
| `ext`, `fresh` | current, external write at any time | holds | **violated** | holds | holds |
| `spaceext` | current, whitespace key, external write after | **violated** | **violated** | holds | **violated** |
| `splitspace` | current, Enter after a whitespace key | **violated** | holds | holds | holds |
| `join` | current, Backspace-at-0 | **violated** | holds | holds | holds |
| `delta` | keystrokes send their edit, not the buffer | holds | holds | holds | holds |

## Findings on the current design

| # | Counterexample | Property | Red test | Oracle row | Card draft |
|---|---|---|---|---|---|
| 1 | lost update (keystroke, blur flush) | ExternalWriteKept | 2 tests | `oracle_whole` violated, `oracle_delta` holds | `target/lane-1521/followups/card-lost-update.md` |
| 2 | trailing space lost on converge | NoLostKeystroke, MonotoneViews | 1 test | `oracle_spacetrim` violated, `oracle_spacekeep` holds | `target/lane-1521/followups/card-trailing-space.md` |
| 3 | Enter after a trailing space is refused | NoLostKeystroke | 1 test | `oracle_splittrim` violated, `oracle_splitkeep` holds | `target/lane-1521/followups/card-enter-after-space.md` |
| 4 | key after Backspace-at-0 goes to the deleted block (suspected) | NoLostKeystroke | none: the window is the GPUI focus hand-off | none | `target/lane-1521/followups/card-join-ahead-key.md` |

- The red tests: `target/lane-1521/redtests/keystroke_counterexamples.rs`
  (a real SqlOnly store via `create_test_engine_with_providers`, a real
  `EditorViewModel`, the echo delivered by hand in the order TLC found).
  Log: `target/lane-1521/d4-red.log`. Card drafts: `target/lane-1521/followups/`.
- All 4 red tests fail for the finding's reason:
  - the external text is gone from the store (`"ab"`, then `"a"` after the
    blur flush);
  - the typed space is gone from the editor (`"abx"`);
  - the split of `"ab "` is refused: position 3 exceeds content length 2.
- Finding 1 is the still-open class of 2026-07-21 (an external write while
  the editor stays focused): with the echo delivered, the window is the
  echo's latency.
- The closing designs in the oracle rows: `Write = "delta"` for 1, a store
  that keeps trailing whitespace (`Trim = FALSE`) for 2 and 3.

## Replays of the 15 keystroke bugs

The gap-table query of the contract sheet gives the same 15 entries at the
base. Entries after the table: one keyword hit,
2026-11-24-main-view-bump-after-last-frame-leaves-chrome-stale (navigation
chrome, out of scope). Logs: below the table.

| Bug | Variant (pre-fix shape) | Violated | Trace step that matches the cause |
|---|---|---|---|
| 2026-07-12-enter-after-typing-intermittently-creates-block | `r_enter`: `Dispatch = "unordered"`, Enter at the end | NoLostKeystroke, NoClobber, MonotoneViews | NoClobber: state 4 applies the second key ("12"), state 5 applies the first key ("1") over it: the committed content lags the buffer |
| 2026-07-20-typed-trailing-space-deleted-100ms-later | `r_trailing`: `Echo = "ge"` (no AdoptBaseline) | NoLostKeystroke, MonotoneViews | MonotoneViews: state 7 stores "12" with the seq of "12␣"; state 8 converges the buffer to "12" (dec = Converge): the typed space is deleted |
| 2026-07-21-editor-stale-buffer-clobber-new-manifestation | `r_noecho`: `EditorEcho = "none"` | NoLostKeystroke, ExternalWriteKept | ExternalWriteKept: state 10 stores the external text; the editor never receives it (no echo) and keeps "123" |
| 2026-08-11-structural-chord-flushes-focused-editor-stale | `r_chord`: `Chord = "pending"`, `SinkBaseline = FALSE`, no blur | ExternalWriteKept | state 9: the chord queues a flush of "123", which the keystroke sink already wrote (its baseline is stale); state 10 stores the external text, state 13 the flush reverts it |
| 2026-08-12-typing-through-production-fire-forget-dispatch (SqlOnly leg) | `r_unordered`: `Dispatch = "unordered"` | NoLostKeystroke, NoClobber, MonotoneViews | NoClobber: state 4 applies "12", state 5 applies the older "1" over it: concurrent in-flight keystroke writes land out of order |
| 2026-08-17-editor-drops-external-write-with-no-write-seq | `r_noseq`: `SeqColumn = FALSE` | NoLostKeystroke, ExternalWriteKept | ExternalWriteKept: state 9 stores the external text, state 12 decides DropNoSeq on its echo |
| 2026-10-01-first-keystroke-into-a-slot-races-the-detached-create | `r_detached`: `Born = "detached"` | NoLostKeystroke | state 4 applies the key to a missing row (matches no row), state 5 creates the row empty |
| 2026-10-04-slash-command-strip-drops-a-key-queued-behind-enter | `r_strip`: `Strip = "spawn"` | NoLostKeystroke, MonotoneViews | state 8 applies the strip from the text read at Enter ("1") after key 2 was typed: key 2 leaves the buffer and the store |
| 2026-07-10-editor-buffer-goes-stale-after-join | not expressible: cell-attached editor | - | covered by `cell_editor_converges_only_on_focus_gain`, `no_cell_editor_keeps_full_backstop` (`frontends/gpui/src/render/builders/editable_text.rs`) and `cell_authority_reflects_merged_content_after_external_join` (`crates/holon-frontend/src/editor_view_model.rs`); cell-mode card |
| 2026-07-13-live-edit-link-marks-wiped-blur | not expressible: inline marks are not modelled | - | `crates/holon/tests/live_edit_link_marks.rs` |
| 2026-07-13-martin-live-report-reproduced-dogfood-typed | not expressible: inline marks (the same defect) | - | `crates/holon/tests/live_edit_link_marks.rs` |
| 2026-08-08-typing-word-left-six-errors-undo | not expressible: undo coalescing (out of scope) | - | `crates/holon/tests/undo_concurrent_keystrokes.rs` |
| 2026-09-02-concurrent-keystroke-undo-step-count-diverges | not expressible: undo coalescing | - | `crates/holon/tests/undo_concurrent_keystrokes.rs`, `undo_compound_interleave.rs` |
| 2026-08-03-text-typed-while-send-flight-silently | not expressible: the chat input draft is not a block | - | no test; the entry is open |
| 2026-08-07-rapid-enter-burst-persists-empty-junk | not expressible: the entry has no established cause | - | no test; diagnosis only |

One log per row: `target/lane-1521/d2/<bug>.log` (the variant's TLC run of the
property named first) and `target/lane-1521/d2/not-expressible.log` (the
covering tests exist at the tree).

## Conformance PBT

`crates/holon-frontend/tests/keystroke_conformance.rs`.

- A run drives a real `EditorViewModel` (SqlOnly, block `block:k1`) with
  the spec's actions `Type`, `DeliverEcho`, `BlurAway`, `Structural`
  (chord), `ExternalWrite`, `Apply`, `Stop`. The decision of each echo is read
  from `evaluate_data_sync_echo`; a Converge directive is applied as the GPUI
  adapter applies it.
- A store double does what the `set_field` arm of `SqlOperationProvider::execute_operation_with_origin` does to one
  text block (the real `canonicalize_stored_content`; write_seq updated only
  when the write carries one).
- Each step records the spec state the code gives (`buf`, `last`, `clock`,
  `queue`, `sc`, `ss`, `ech`, `dec`, ...), with write_seq values renamed to
  the spec's clock; tla-checker replays the recording against `Keystroke.tla`.
- Fixed cases first: 26 fixed runs. The test asserts that they reach exactly
  the 22 reachable cells of decision x last intent kind x external write.
  Unreachable (23), asserted never reached by the random runs:
  - flush x every decision x every external (15): a blur unmounts the editor
    and drops its subscription;
  - DropNoSeq x keystroke/structural x every external (6): every row carries
    write_seq (`NOT NULL DEFAULT 0`, projected by every block source);
  - Converge x keystroke/structural x no external (2): the editor's own
    writes echo with a seq at or below `last_local_seq` and the trimmed text.
- Random cases: 256 drawn runs (1 to 39 steps) per external class, 768
  in all; they reach 20 cells, all reachable.
- Mutant: `evaluate_data_sync_echo` with `seq <= last_local_seq` as stale
  (converge iff `echo_seq > last_local_seq`) is refused by the spec: fixed
  case 2 (`Type, Type, Type, Apply, Apply, Apply, Deliver(3)`) converges at
  step 7 where the spec drops the echo as stale.

Logs: `target/lane-1521/d3-pbt.log`, `target/lane-1521/d3-mutant.log`,
`target/lane-1521/d3-revert.log`.

## Keyword search

`rg -n 'write_seq|last_local_seq|EchoDecision|pending_commit_intent|on_blur' crates frontends`
at the base: 258 hits (log `target/lane-1521/s5-keyword.log`). Every hit has a
disposition (`target/lane-1521/s5-dispositions.tsv`):

| File | Lines | Disposition |
|---|---|---|
| `frontends/gpui/src/soft_keyboard.rs` | 57 | out: GPUI input widget focus events |
| `crates/holon-pattern/src/arcs.rs` | 392 | choice: store column ss projected into every view row; initial value 0 |
| `frontends/gpui/src/views/editor_view.rs` | 46 | choice: the sequence lives in the view model (variable last) |
| `frontends/gpui/src/views/editor_view.rs` | 375 | action: BlurAway |
| `frontends/gpui/src/views/editor_view.rs` | 433, 434, 442 | action: Type (LocalEdit) then Dispatch |
| `frontends/gpui/src/views/editor_view.rs` | 525, 526, 529, 553 | action: DeliverEcho, echo carries the store write_seq; none gives DropNoSeq |
| `frontends/gpui/src/views/editor_view.rs` | 616 | out: task_state edge, no editor writes it |
| `frontends/gpui/src/views/editor_view.rs` | 978, 988 | action: DeliverEcho Converge re-seed, last unchanged |
| `frontends/gpui/src/views/editor_view.rs` | 1122, 1129, 1133 | action: Structural (chord flush before the structural op) |
| `crates/holon-pattern/src/schema.rs` | 209 | choice: store column ss projected into every view row; initial value 0 |
| `frontends/gpui/tests/editor_ime_deferred_replay.rs` | 67, 69, 72, 73, 144, 192 | out: existing test code; listed as coverage, not a pipeline step |
| `frontends/gpui/tests/undo_survives_blur_windowed.rs` | 32 | out: existing test code; listed as coverage, not a pipeline step |
| `frontends/gpui/tests/structural_chord_stale_flush_windowed.rs` | 13 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-frontend/src/operations.rs` | 136 | out: latency token read, not a write |
| `crates/holon-frontend/src/operations.rs` | 223 | action: Dispatch, intent carries write_seq |
| `crates/holon-frontend/src/operations.rs` | 580, 590 | out: existing test code; listed as coverage, not a pipeline step |
| `frontends/gpui/tests/editor_ime_commit_sequence.rs` | 49, 52, 53 | out: existing test code; listed as coverage, not a pipeline step |
| `frontends/gpui/tests/editor_trailing_space_echo.rs` | 5, 7, 9, 20, 46, 47, 48, 51, 52, 119, 138, 155, 180, 202 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-frontend/src/editor_view_model.rs` | 31, 692, 693, 699, 700, 701, 708, 713, 723, 727 | action: Decide (DeliverEcho), adopt or drop per decision |
| `crates/holon-frontend/src/editor_view_model.rs` | 229, 246, 248, 251, 313, 420, 421, 429, 447 | choice: variable last = last_local_seq, raised only by max |
| `crates/holon-frontend/src/editor_view_model.rs` | 571, 654, 655, 657, 659, 670 | action: Type (LocalEdit) stamps write_seq, sets last |
| `crates/holon-frontend/src/editor_view_model.rs` | 743, 759, 762 | out: cell mode remote directive (follow-up card) |
| `crates/holon-frontend/src/editor_view_model.rs` | 946, 962, 982, 986, 992 | action: BlurAway and Structural (FlushOf), the flush intent |
| `crates/holon-frontend/src/editor_view_model.rs` | 1678, 1692, 1714, 2013, 2031, 2303, 2313, 2321, 2333, 2342, 2354, 2365, 2380, 2385, 2393, 2404, 2837, 2860 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-integration-tests/src/pbt/ref_caps/nav.rs` | 165 | out: existing test code; listed as coverage, not a pipeline step |
| `frontends/gpui/tests/task_keyword_blur_windowed.rs` | 17 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-frontend/src/local_edit_epoch.rs` | 12 | out: local edit epoch is a separate counter, not an ordering token |
| `crates/holon-integration-tests/tests/plan_fence_wait.rs` | 93 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-frontend/src/echo.rs` | 13, 29, 35, 44, 45, 56, 57, 59, 64, 66, 67, 68, 73, 96, 103 | action: Decide (DeliverEcho), the five decisions |
| `crates/holon-frontend/src/echo.rs` | 125, 128, 130, 136, 140, 146, 154, 164, 169, 179, 185, 187, 189, 195, 204, 218, 227, 238, 246, 250, 251, 261, 269, 273, 278, 282, 290 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-integration-tests/tests/latency_slo_gate.rs` | 142, 211, 472, 756, 870, 1078 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-integration-tests/tests/span_capture_suite/interaction_trace_connectivity.rs` | 125, 126 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-integration-tests/tests/span_capture_suite/declared_column_parity.rs` | 79, 80 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-orgmode/src/block_params.rs` | 572 | choice: store column ss projected into every view row; initial value 0 |
| `frontends/mcp/src/dense_projection.rs` | 27 | out: MCP projection version guard |
| `crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl` | 66, 68 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-integration-tests/tests/loro_suite/loro_keystroke_footprint.rs` | 107 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-integration-tests/tests/editing_suite/undo_cycle_task_state_coverage.rs` | 139, 144 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-integration-tests/tests/editing_suite/undo_prod_session_wiring.rs` | 126, 128, 131, 136 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-integration-tests/tests/loro_suite/loro_block_type_on_a_legacy_schema.rs` | 35 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-integration-tests/src/pbt/reference_state.rs` | 1712, 1713, 1718 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs` | 341 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-integration-tests/src/pbt/transitions/external_write_same_block_focused.rs` | 27 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-frontend/src/headless_editor_mirror.rs` | 51, 77, 86, 88, 91, 410, 448, 462, 465, 674 | choice: headless mirror of the same actions; the conformance PBT target |
| `crates/holon-frontend/src/headless_editor_mirror.rs` | 1063, 1067 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-frontend/src/reactive.rs` | 2347 | action: FollowFocus (focus move blurs the old editor) |
| `crates/holon-frontend/src/reactive.rs` | 4353, 4540, 4631, 4721 | out: latency token read, not a write |
| `crates/holon-frontend/src/user_driver.rs` | 695 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon/sql/prql_stdlib.prql` | 7, 20 | choice: store column ss projected into every view row; initial value 0 |
| `crates/holon-turso/tests/column_added_under_dependent_matview.rs` | 77 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon/tests/shape_gate_source_channel.rs` | 196 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-turso/sql/schema/journal_day_pages_matview.sql` | 33 | choice: store column ss projected into every view row; initial value 0 |
| `crates/holon/tests/undo_compound_interleave.rs` | 172 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-turso/tests/replace_into_matview_base.rs` | 553 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-turso/sql/schema/journal_feed_matview.sql` | 33 | choice: store column ss projected into every view row; initial value 0 |
| `crates/holon-turso/sql/schema/blocks_with_paths.sql` | 16, 38 | choice: store column ss projected into every view row; initial value 0 |
| `crates/holon-turso/sql/schema/blocks.sql` | 25, 29 | choice: store column ss projected into every view row; initial value 0 |
| `crates/holon/src/api/query_engine.rs` | 349, 374 | choice: echo row carries the store write_seq (variable ss) |
| `crates/holon/src/api/operation_engine.rs` | 2970, 3093, 3117, 3136, 3138 | choice: one content write per keystroke seq (source-text write is a keystroke write) |
| `crates/holon/src/api/operation_engine.rs` | 4860, 4861 | action: Dispatch, commit_keystroke admits synchronously with write_seq |
| `crates/holon-api/src/latency_e2e.rs` | 24, 33, 154, 547, 550, 551, 554, 556, 875, 1072, 1123 | out: latency token read, not a write |
| `crates/holon/tests/decision_shape_claims.rs` | 524 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon/src/api/holon_service.rs` | 278 | out: latency token read, not a write |
| `crates/holon-api/src/operation_engine.rs` | 122 | action: Dispatch, keystroke carries write_seq |
| `crates/holon/tests/typing_channel_claims.rs` | 156, 237, 276, 291, 327, 360, 421, 528 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-turso/src/schema_modules.rs` | 1434 | choice: store column ss projected into every view row; initial value 0 |
| `crates/holon/src/api/backend_engine.rs` | 2372 | choice: echo row carries the store write_seq (variable ss) |
| `crates/holon/tests/admission_footprint_sweep.rs` | 65, 925 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon/tests/undo_concurrent_keystrokes.rs` | 111 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-api/src/lib.rs` | 100 | out: module declaration |
| `crates/holon-api/src/write_seq.rs` | 22, 38, 45, 54 | choice: global counter clock and the converge rule seq >= last |
| `crates/holon-api/src/query_engine.rs` | 78, 79 | choice: echo row carries the store write_seq (variable ss) |
| `crates/holon-app/tests/loro_ui_row_matches_sql_row.rs` | 50 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon-api/src/latency_e2e/tests.rs` | 218, 261, 289, 299 | out: existing test code; listed as coverage, not a pipeline step |
| `crates/holon/src/core/sql_operation_provider.rs` | 3723, 3732, 3733, 3739, 3740, 3741, 3743, 3751 | action: ApplyWrite, write_seq updated only when the param is present (sticky) |
| `crates/holon-net/tests/net_rev2_storage.rs` | 1639 | out: existing test code; listed as coverage, not a pipeline step |

## Limits

- One text block of one line; no inline marks, no undo, no IME composition,
  no Loro cell.
- One external write, one structural op; the bounds are 3 keys (6 in the
  conformance configs) and 2 blocks.
- The store double of the conformance PBT is not the real store; the red
  tests use the real one.
- Finding 4 has no red test.

## Follow-up card draft: cell mode

### Model the cell-mode (Loro) keystroke pipeline in TLA+

- Why: `Keystroke.tla` models SqlOnly only. Cell mode applies text deltas
  through the CRDT, with no write_seq; the remote-delta loop converges a
  focused editor through `remote_converge_directive` and the deferred
  directive check.
- Bugs of the keystroke set that need cell mode:
  - 2026-07-10-editor-buffer-goes-stale-after-join (cell-attached editor)
  - 2026-08-12-typing-through-production-fire-forget-dispatch, its Loro leg
    (its SqlOnly leg is replayed by `r_unordered`)
- Tasks:
  - [ ] Add a cell-mode module (delta apply, CRDT merge, remote converge,
    deferred directive)
  - [ ] Replay the two bugs above as design variants; each violates a
    user-level property
  - [ ] Extend the conformance PBT to a cell-attached editor
