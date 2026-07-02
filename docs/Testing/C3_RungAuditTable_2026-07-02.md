# C-3 Rung Audit Table (2026-07-02) — the §8.12 gate made checkable

Phase 1 step 0 of `~/.claude/plans/streamed-shimmying-parrot.md` (lines 105–121).
Enumerates every transition class that would appear in the WINDOWED generated
alphabet, with its CURRENT driver rung (read from the transition's
`apply_to_sut` path and the cap impl the composed SUT hosts today) and its
TARGET rung per §8.12 ("when a window exists, gesture writes must ride the
window driver"). Rung ladder (Design §8.11): **Gpui** (window driver:
`GpuiUserDriver`/`SimUserDriver` via `DriverInputComponent::with_input`) ⊐
**ReactiveEngine** (headless VM rung: click-intent resolution +
`HeadlessEditorMirror` keystrokes) ⊐ **Direct-dispatch**
(`engine.execute_operation` / `OpDispatchWriter` / interior mutation).

All paths relative to `crates/holon-integration-tests/src/pbt/` unless noted.

## How the windowed SUT is wired today (the facts the table rests on)

- `compose_sut_windowed_base[_seeded]` boots the full headless stack with
  `DriverPlacement::Deferred` (`composed/builder.rs:113-121`, `131-158`): the
  gesture-driver caps (`SutDriver`/`SutBlockInteract`/`SutArrowNavigate`) are
  ABSENT from the base (`composed/builder.rs:358-371` — the
  `with_input_headless` install is skipped).
- `overlay_windowed_caps` (`window_slice/builders.rs:158-172`) then inserts
  `GpuiWindowComponent` (`SutLayout`) + `DriverInputComponent::with_input(engine,
  driver, geometry)` — the window's `GpuiUserDriver`/`SimUserDriver` backs
  `SutDriver`/`SutBlockInteract`/`SutArrowNavigate`
  (`driver_input.rs:271/343/521`, registered `driver_input.rs:543-560`).
- BUT the deferred base still registers ALL the other headless write caps via
  `HeadlessFrontendComponent::register` (`frontend_slice/components.rs:1892-1963`)
  plus builder inserts: `SutFocusWrite`, `SutEditorMirrorWrite`,
  `SutBlockTreeWrite` (`KeystrokeBlockTreeWriter` replace at
  `composed/builder.rs:324-334`), `SutViewControl`, `SutNavHistoryWrite/Drive`,
  `SutMutate`, `SutHistoryWrite`, `SutEdgeFieldWrite`
  (`composed/builder.rs:341-347`). These are the mixed-rung rows below —
  exactly §8.12's disclosed interim violation.

## The table

**Phase 1 step 1, increment 1 landed (2026-07-02):** mechanisms 1+2 re-backed the
keystroke-writer family (rows 1–4, 6) and the window-click family (rows 16–18) onto the
window driver. In `overlay_windowed_caps`
(`window_slice/builders.rs:189-195`) the deferred base's headless-driver-backed write caps
are now `CapMap::replace`d with window-driver-backed siblings: `SutBlockTreeWrite` via
`HeadlessFrontendComponent::keystroke_writer_with` (`frontend_slice/components.rs:315`);
`SutFocusWrite`/`SutEditorMirrorWrite`/`SutMutate` via `WindowFrontendWrite`
(`window_slice/components.rs:449`), which delegates to the base's driver-parameterized
`*_via` bodies (`frontend_slice/components.rs:1150-1290`). move_up/move_down (row 5,
mechanism 3) stay on the disclosed `OpDispatchWriter` fallback; the 6 EXCLUDED rows stay
excluded; headless (deferred base / `full_headless`) behavior is unchanged.

Status legend: **OK** = already at target rung in the windowed build.
**REBIND** = headless impl exists at a lower rung; a window-driver-backed
sibling is the C-3 work. **EXCLUDED** = no window-driver mechanism exists yet;
must NOT enter the windowed generated alphabet until re-backed (tracked
Phase 3 blocker). **N/A** = layer-invariant non-gesture (Design §8.11
care-point 4 / op-layer table row "External/Lifecycle") — rung rule does not
apply; included with justification.

| # | Transition class | Backing cap(s) | Composed impl (file:line) | CURRENT rung | TARGET rung (windowed) | Status | Notes |
|---|---|---|---|---|---|---|---|
| 1 | SplitBlock | `SutBlockTreeWrite` (`transitions/split_block.rs:269-273`, apply `:296-298`) | `KeystrokeBlockTreeWriter::apply_split_block` `op_write_cap.rs:268-307` (focus + home + N×right + enter over `comp.driver()` = headless `ReactiveEngineDriver`; installed via `caps.replace` `composed/builder.rs:324-334`) | ReactiveEngine | Gpui | **OK** (2026-07-02) | Windowed sibling = same keystrokes through the window driver: `keystroke_writer_with(driver)` replaced in `overlay_windowed_caps` (`window_slice/builders.rs:189`). |
| 2 | JoinBlock | `SutBlockTreeWrite` (`transitions/join_block.rs:138-141`, apply `:165-167`) | `KeystrokeBlockTreeWriter::apply_join_block` `op_write_cap.rs:309-316` (focus + home + backspace, headless driver) | ReactiveEngine | Gpui | **OK** (2026-07-02) | Same windowed `keystroke_writer_with` sibling as row 1 (`window_slice/builders.rs:189`). |
| 3 | Indent | `SutBlockTreeWrite` (`transitions/indent.rs:114-117`, apply `:141-143`) | `KeystrokeBlockTreeWriter::apply_indent` `op_write_cap.rs:319-324` (focus + tab) | ReactiveEngine | Gpui | **OK** (2026-07-02) | Same windowed `keystroke_writer_with` sibling as row 1. |
| 4 | Outdent | `SutBlockTreeWrite` (`transitions/outdent.rs:105-108`, apply `:132-134`) | `KeystrokeBlockTreeWriter::apply_outdent` `op_write_cap.rs:326-331` (focus + shift+tab) | ReactiveEngine | Gpui | **OK** (2026-07-02) | Same windowed `keystroke_writer_with` sibling as row 1. |
| 5 | MoveBlockUp / MoveBlockDown | `SutBlockTreeWrite` (`transitions/move_up.rs:116-119`/`move_down.rs:122-125`, apply `:143-145`/`:149-151`) | `KeystrokeBlockTreeWriter::apply_move_up/down` → `self.fallback` = `OpDispatchWriter::execute("move_up"/"move_down")` `op_write_cap.rs:335-343` → `:180-185` → `engine.execute_operation` `:115-121` | **Direct-dispatch** (even headless) | Gpui (chord) | REBIND (new code) | The ONLY structural ops still on the dispatch floor. No chord path exists at ANY rung yet ("until the chord-resolution rebind (`send_key_chord`) lands", `op_write_cap.rs:332-334`). This is the plan's "finish the KeystrokeBlockTreeWriter rebind" — but scoped to move_up/move_down only. |
| 6 | TypeChars / DeleteBackward / MoveCursor (editor family) | `SutEditorMirrorWrite` (`type_chars.rs:97-100`, `delete_backward.rs:143-146`, `move_cursor.rs:88-91`) | `HeadlessFrontendComponent` `frontend_slice/components.rs:1175-1237` (`send_raw_keystroke` per char / backspace through the headless `ReactiveEngineDriver` → `HeadlessEditorMirror`) | ReactiveEngine | Gpui (window `InputState` keystrokes) | **OK** (2026-07-02) | Windowed `WindowFrontendWrite` sibling routes `send_raw_keystroke` through the window driver (`apply_type_chars_via`/`apply_delete_backward_via`/`apply_move_cursor_via`, `frontend_slice/components.rs:1225-1290`). ⚠ Routing only — the rebound cap is installed + green in the initial-frame catalog; no existing windowed test yet DRIVES the editor keystrokes end-to-end (4b). §8.11 soft-spot (`InputState` vs `HeadlessEditorMirror`) is where end-to-end faithfulness will be proven. |
| 7 | PressKey | `SutBlockInteract` (`press_key.rs:32-35`, apply `:235-237`) | `DriverInputComponent` `driver_input.rs:343` — wraps whichever driver the placement installed | MIXED by `DriverPlacement`: Gpui after `overlay_windowed_caps` (`window_slice/builders.rs:158-172`); ReactiveEngine under `HeadlessReactive` (`composed/builder.rs:364-371`) | Gpui | **OK** | Already correct: windowed build gets the window driver, headless gets VM rung — the one-driver-per-run rule working as designed. |
| 8 | ArrowNavigate | `SutArrowNavigate` (`arrow_navigate.rs:34`, apply `:247-256`) | `DriverInputComponent` `driver_input.rs:521`, registered `:560` | MIXED (same as row 7) | Gpui | **OK** | E4 input family; rides the overlay driver. |
| 9 | ClickBlock | `SutBlockInteract` (`click_block.rs:71-74`, apply `:246-248`) | `DriverInputComponent` `driver_input.rs:343`; body pattern `click_block.rs:46-58` (`require_bounds` → `click_entity` → `wait_for_engine_focus`) | MIXED (same as row 7) | Gpui | **OK** | ⚠ Carries the `click_entity` nav-degradation wart: a nav-bound click can silently degrade to an in-memory `set_focus` (tracked in commit d5952940's message). Not a rung problem, but a faithfulness debt on this row. |
| 10 | ExpandToggle / set_block_expanded | `SutBlockInteract` (`expand_toggle.rs:41-44`, apply `:104-106`) | `DriverInputComponent` `driver_input.rs:343` | MIXED (same as row 7) | Gpui | **OK** | View-local; correctly bottoms out at the VM rung headless (§8.11 care-point 3). |
| 11 | ToggleCollapse | `SutBlockInteract` (`toggle_collapse.rs:34-37`, apply `:77-86`) | `DriverInputComponent` `driver_input.rs:343` | MIXED (same as row 7) | Gpui | **OK** | Same as row 10. |
| 12 | SwitchViewMode | `SutBlockInteract` (`switch_view_mode.rs:33-36`, apply `:71-80`) | `DriverInputComponent` `driver_input.rs:343` | MIXED (same as row 7) | Gpui | **OK** (rung) — ⚠ faithfulness gap | Rung is right, but the C-5 Tier 4 finding (2026-07-02) applies: the UI-adjacent SwitchViewMode click path does not move `current_view` yet — cf. row 22 (`SwitchView` interior set). Rung-OK ≠ semantics-OK here. |
| 13 | ToggleDrawer | `SutBlockInteract` (`toggle_drawer.rs:29-32`, apply `:76-85`) | `DriverInputComponent` `driver_input.rs:343` | MIXED (same as row 7) | Gpui | **OK** | |
| 14 | DragDropBlock | `SutBlockInteract` (`drag_drop_block.rs:33-36`, apply `:206-208`) | `DriverInputComponent` `driver_input.rs:343` | MIXED (same as row 7) | Gpui | **OK** | Prod drop_zone dispatches `move_block` (`drag_drop_block.rs:198`); windowed = driver drag gesture. |
| 15 | TriggerSlashCommand | `SutBlockInteract` (`trigger_slash_command.rs:98-101`, apply `:208-210`) | `DriverInputComponent` `driver_input.rs:343`; body pattern `trigger_slash_command.rs:47-87` | MIXED (same as row 7) | Gpui | **OK** | |
| 16 | NavigateFocus (sidebar click) | `SutFocusWrite` (`navigate_focus.rs:41-44`, apply `:238-247`) | `HeadlessFrontendComponent::apply_navigate_focus` `frontend_slice/components.rs:1118-1158` — clicks the LeftSidebar entry via the component's INTERNAL `ReactiveEngineDriver` (`:1122`, `:1143`) | ReactiveEngine — base; **Gpui after overlay** | Gpui (window sidebar click) | **OK** (2026-07-02) | Windowed sibling landed: `WindowFrontendWrite::apply_navigate_focus` → `apply_navigate_focus_via(window_driver)` (`window_slice/components.rs:464`; body `frontend_slice/components.rs:1150`) clicks the sidebar entry through the window driver; overlay `replace` at `window_slice/builders.rs:193`. |
| 17 | FocusEditableText | `SutFocusWrite` (`focus_editable_text.rs:52-55`, apply `:205-206`) | `HeadlessFrontendComponent::apply_focus_editable_text` `frontend_slice/components.rs:1160-1165` (`click_entity(main, id)` via internal headless driver) | ReactiveEngine | Gpui (window click on the editable) | **OK** (2026-07-02) | `WindowFrontendWrite::apply_focus_editable_text` → `apply_focus_editable_text_via(window_driver)` (`frontend_slice/components.rs:1185`); overlay `replace` at `window_slice/builders.rs:193`. |
| 18 | ToggleState / cycle_task_state | `SutMutate` (local cap, `toggle_state.rs:149-153`, apply `:310-313`) | `HeadlessFrontendComponent::toggle_state` `frontend_slice/components.rs:1545-1571` — loops `engine.execute_operation("cycle_task_state")` click_count times | **Direct-dispatch** (faithful op, but no click-intent resolution) | Gpui (click the `state_toggle` widget N times) | **OK** (2026-07-02) | `WindowFrontendWrite::toggle_state` → `toggle_state_via(window_driver)` (`frontend_slice/components.rs:1637`) clicks the `state_toggle` widget `click_count` times through the window driver (click_count math shared with the headless direct-dispatch path via `toggle_click_count`); overlay `replace` at `window_slice/builders.rs:195`. ⚠ Rebound + cap-set-present; not yet DRIVEN end-to-end by an existing windowed test (4b). |
| 19 | NavigateHome | `SutNavHistoryWrite` (`navigate_home.rs:33`, apply `:126-133`) | `HeadlessFrontendComponent` `frontend_slice/components.rs:1261` — `execute_operation("navigation", "go_home")` (per the `SutNavHistoryDrive` doc comment `:1491-1499`, same dispatch path) | Direct-dispatch | Gpui (home affordance click / chord) | EXCLUDED | No window-driver-backed impl or affordance-click path exists yet. Tracked Phase 3 blocker. |
| 20 | NavigateBack / NavigateForward | `SutNavHistoryDrive` (`navigate_back.rs:35`, `navigate_forward.rs:32`; apply `:83-84`/`:85-86`) | `HeadlessFrontendComponent::navigate_back/forward` `frontend_slice/components.rs:1500-1511` — `dispatch_navigation("go_back"/"go_forward")` | Direct-dispatch | Gpui (leader-chord via window PressKey) | EXCLUDED | Doc comment `:1494-1495` notes E2ESut reached these via GPUI driver chords — the windowed mechanism (chord → nav op) is not built for the composed SUT. Tracked Phase 3 blocker. |
| 21 | PinBlock | `SutNavHistoryDrive` (`pin_block.rs:38-46`, apply `:147-148`) | `HeadlessFrontendComponent::pin_block` `frontend_slice/components.rs:1512-1521` — `dispatch_navigation("focus_pin")` | Direct-dispatch | Gpui (shift-click, per `pin_block.rs:11`) | EXCLUDED | Needs modifier-click support through the window driver — new mechanism, not a rebind. Tracked Phase 3 blocker. |
| 22 | UnpinBlock | `SutNavHistoryDrive` (`unpin_block.rs:38-46`, apply `:115-116`) | `HeadlessFrontendComponent::unpin_block` `frontend_slice/components.rs:1523-1528` — `dispatch_navigation("close", history_id)` | Direct-dispatch | Gpui (close-affordance click) | EXCLUDED | No windowed geometry entity for the close affordance verified. Tracked Phase 3 blocker. |
| 23 | SwitchView | `SutViewControl` (`switch_view.rs:28-31`, apply `:76-78`) | `HeadlessFrontendComponent::switch_view` `frontend_slice/components.rs:1449-1457` — `*self.current_view.lock() = name` | **Direct-dispatch (interior mutation — the lowest possible)** | Gpui (view-switch click) | EXCLUDED | The known faithfulness gap (C-5 Tier 4 finding 2026-07-02): the UI-adjacent SwitchViewMode click path does not move `current_view` yet, so there is NOTHING window-faithful to rebind onto. Fixing the prod-faithful click path is a prerequisite (fix the cap, don't withhold it). Tracked Phase 3 blocker. |
| 24 | UndoLastMutation / Redo | `SutHistoryWrite` (`transitions/undo_last_mutation.rs`, `redo.rs`) | `HeadlessFrontendComponent` `frontend_slice/components.rs:1471-1487` — `engine.undo()/redo()` directly | Direct-dispatch | Gpui (Cmd+Z / Cmd+Shift+Z chord) | EXCLUDED | A user gesture in prod (chord) but driven as a raw engine call. NOT in the plan's Phase 1 step 1 list. Tracked Phase 3 blocker. |
| 25 | CreateDocument | `SutAppLifecycle` (`create_document.rs:33`, apply `:95-96`) | `HeadlessFrontendComponent` register `frontend_slice/components.rs:1957-1963` (seam-rebuild entry point; mints a doc through the session) | Direct (lifecycle) | — | N/A | Justified exclusion from the gesture set: classed as External/Lifecycle in §8.11's op-layer table (layer-invariant). If a windowed "new page" UI gesture ever enters the alphabet it becomes a REBIND row. |
| 26 | SetEdgeField | `SutEdgeFieldWrite` (`set_edge_field.rs:80-84`, `cap_transition!` `:193-195`) | `EdgeFieldWriter` `op_write_cap.rs:345+`, hosted `composed/builder.rs:341-347` (writes the Loro authority doc via prod `set_block_{tags,requires}`) | Direct (seam) | — | N/A | Non-gesture mutation (models external/API edge-field writes; H12 catch). §8.11 care-point 4: layer-invariant, rides its own cap. |
| 27 | ApplyMutation (all source arms) | `SutSeamMutate` / routed CapMap arms (`apply_mutation.rs:44-48`, apply `:762-763` → `apply_mutation_routed` `:656+`) | routed source→sub-cap `apply_mutation.rs:686-749`; `SutSeamMutate` on `HeadlessFrontendComponent` `frontend_slice/components.rs:1944-1947` | Direct (seam) | — | N/A | §8.11 care-point 4 names `ApplyMutation` explicitly layer-invariant. The E2ESut keychord path in `apply_mutation.rs:804-835` is the LEGACY monolith impl, not the composed SUT's. |
| 28 | WriteOrgFile (external seam) | `SutFixtureFs` (`write_org_file.rs:92`, apply `:327-362`) | fixture-fs write of an org file (external editor simulation) | Direct (external seam) | — | N/A | Deliberately NOT a user gesture — it models the out-of-process actor. Must never ride a driver. |
| 29 | BulkExternalAdd | `SutSeamMutate` (`bulk_external_add.rs:44`, apply `:190-191`) | `HeadlessFrontendComponent` `frontend_slice/components.rs:1944-1947` (live `FileSyncController` seam) | Direct (external seam) | — | N/A | Same justification as row 28. |
| 30 | Peer family (PeerEdit, PeerCharEdit, SyncWithPeer, MergeFromPeer, AddPeer, DeliverBlockContent) | Loro peer caps (`transitions/peer_*.rs`, `sync_with_peer.rs`, `merge_from_peer.rs`, `deliver_block_content.rs`, `add_peer.rs`) | Loro-arm components (peer docs), e.g. `apply_mutation.rs:719-732` for the routed peer arm | Direct (replication seam) | — | N/A | §8.11 care-point 4: `Peer*` layer-invariant. Peers are other processes, not this window's user. |
| 31 | Lifecycle/env family (StartApp, SimulateRestart, GitInit/JjGitInit, SetupWatch/RemoveWatch, EmitMcpData, ConcurrentSchemaInit, CreateDirectory, CreateStaleLoro, Nothing) | various lifecycle/env caps (`transitions/*.rs`); `SutMcpEmit` no-op `frontend_slice/components.rs:1459-1464` | lifecycle/seam impls | Direct (lifecycle) | — | N/A | §8.11 op-layer table: External/Lifecycle = layer-invariant. |

## (a) Size estimate of the real C-3 work

**Pure/near-pure rebinds (bodies already exist, window hosting is the work):**
- Row 17 FocusEditableText — generic `SutLayout + SutDriver` body exists
  (`focus_editable_text.rs:35-41`). Small.
- Row 18 ToggleState — generic click body exists (`toggle_state.rs:112-131`).
  Small.
- Rows 1–4 SplitBlock/JoinBlock/Indent/Outdent — `KeystrokeBlockTreeWriter`
  already speaks keystrokes; the work is a windowed sibling that sends the same
  keystrokes through the window's driver instead of the headless
  `ReactiveEngineDriver` (i.e. parameterize the writer over `UserDriver`, or a
  windowed `SutBlockTreeWrite` component). Medium — one mechanism covering 4
  classes, plus the split-position content-cell read must work against the
  windowed editor.
- Row 6 editor family — same mechanism as rows 1–4 (keystrokes via window
  driver / `InputState`). Medium; this is where §8.11's "editor logic trapped
  in the GPUI widget" soft spot bites.
- Row 16 NavigateFocus — windowed sidebar click; needs the LeftSidebar entries
  to have registered bounds in the window geometry. Medium (the plan's named
  item #1).

**New code (no mechanism exists at any rung yet):**
- Row 5 MoveUp/MoveDown — the `send_key_chord` chord-resolution rebind
  (`op_write_cap.rs:332-334`). This is the only structural op still on the
  dispatch floor and the largest single mechanism gap.

**Already done (no C-3 work):** rows 7–15 — the entire
`SutBlockInteract`/`SutArrowNavigate` gesture family rides
`overlay_windowed_caps`' window driver today.

**Not C-3 (prerequisite prod-faithfulness fixes):** row 23 SwitchView needs the
UI-adjacent click path to actually move `current_view` before any rebind is
meaningful; row 9's `click_entity` nav-degradation wart (d5952940) and row 12's
SwitchViewMode semantics gap are faithfulness debts on already-OK rungs.

Net: **9 REBIND rows ≈ 3 mechanisms** (window-driver keystroke writer covering
rows 1–4+6; window click hosting covering rows 16–18; chord dispatch for
row 5).

**Update 2026-07-02 (Phase 1 step 1, increment 1):** mechanisms 1 (rows 1–4, 6) and
2 (rows 16–18) landed — all 8 flipped to OK (see the header note + per-row Status).
Only **mechanism 3 (row 5 move_up/move_down)** remains a REBIND: the `send_key_chord`
chord-resolution dispatch, the sole structural op still on the `OpDispatchWriter` floor.
The 6 EXCLUDED rows are unchanged. Caveat carried into 4b: the rebound caps are installed
on the windowed `CapMap` and pass the initial-frame + ClickBlock catalog green, but no
existing windowed test yet DRIVES Split/Type/NavigateFocus/ToggleState end-to-end through
the window — that end-to-end drive (and any faithfulness surprises it surfaces, esp. the
§8.11 editor soft spot) is 4b's job.

## (b) EXCLUDED rows — each a tracked Phase 3 blocker

- [ ] Row 19 NavigateHome — no windowed home-affordance path.
- [ ] Row 20 NavigateBack/NavigateForward — leader-chord → nav-op path not built
      for the composed windowed SUT.
- [ ] Row 21 PinBlock — window driver has no modifier-click (shift-click) support.
- [ ] Row 22 UnpinBlock — no windowed close-affordance geometry entity.
- [ ] Row 23 SwitchView — prod-faithful UI click path does not move
      `current_view` yet (C-5 Tier 4 finding); fix the cap first.
- [ ] Row 24 UndoLastMutation/Redo — no windowed Cmd+Z/Cmd+Shift+Z chord path.

Per the plan (lines 110–112, 119–120): none of these classes may enter the
windowed generated alphabet until re-backed; the exclusion is disclosed here,
never silently driven cross-rung.

## (c) Alphabet classes the plan's Phase 1 step 1 list does not mention

The plan names only: `SutFocusWrite` sidebar-click, the E4
`PressKey`/`ArrowNavigate` input family, and the `KeystrokeBlockTreeWriter`
join/indent/outdent/move rebind. Found in the alphabet but unnamed:

1. **ToggleState** (row 18) — REBIND, and a cheap one (generic click body exists).
2. **FocusEditableText** (row 17) — a second `SutFocusWrite`-cap class, distinct
   from the sidebar click; also a near-pure rebind.
3. **UndoLastMutation/Redo** (row 24) — gesture-in-prod (chord), currently raw
   `engine.undo()`; EXCLUDED.
4. **The nav-history family** NavigateHome/Back/Forward, PinBlock, UnpinBlock
   (rows 19–22) — all Direct-dispatch, all EXCLUDED.
5. **SwitchView** (row 23) — EXCLUDED with a prod-faithfulness prerequisite.
6. **TypeChars/DeleteBackward/MoveCursor** (row 6) — arguably implied by the
   plan's "editor keystrokes via the window", but the plan names only PressKey;
   the `SutEditorMirrorWrite` family is a separate cap needing its own sibling.

**Headline staleness finding:** the plan's C-3 text "finish the
`KeystrokeBlockTreeWriter` rebind (join/indent/outdent/move off the
`OpDispatchWriter` fallback)" is stale — join (`op_write_cap.rs:309-316`),
indent (`:319-324`) and outdent (`:326-331`) are ALREADY keystroke-driven at
the VM rung; only move_up/move_down remain on the fallback (`:335-343`). The
remaining fallback scope is smaller than the plan states, but it needs new
chord machinery, not a rebind.
