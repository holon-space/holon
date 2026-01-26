# TUI PBT — make the test exercise the real input pipeline

Context: `~/.claude/plans/please-create-a-plan-harmonic-thimble.md` rewired
`tui_ui_pbt` to drive the renderer's `app_handle_input_event` for chord
ops and clicks. Investigation after that landed showed the test still
takes large API shortcuts (`apply_navigate_focus`, `apply_arrow_navigate`,
`apply_edit_via_*`) that bypass the keyboard pipeline and silently mirror
focus state into the engine. Real-world TUI bugs ("modifications don't
work") therefore stay invisible.

This file lists the ordered work to close those gaps. Tick items as
they land.

## Phase A — Remove API shortcuts in the SUT

Each item rewrites a `crates/holon-integration-tests/src/pbt/sut.rs`
`apply_*` method to drive the `UserDriver` instead of poking
`engine.execute_op` / `engine.ui_state().set_focus(...)`.

- [x] **A1. `apply_focus_editable_text` — delete the silent fallback.**
      `sut.rs:2186-2198` falls through to `synthetic_dispatch("navigation",
      "editor_focus", ...)` on `click_entity` failure with an `eprintln!`.
      Per CLAUDE.md "fail loud, never fake": let `click_entity`'s `Err`
      propagate. (Sequencing: do this first — it's small and surfaces
      whether `click_entity` is actually reliable in steady state, which
      is a prerequisite for A2-A5.)

- [x] **A2. `apply_navigate_focus` — drive via the LeftSidebar.**
      `sut.rs:660-690` currently calls `self.navigate_focus(...)`
      (which is `execute_op("navigation","focus",...)` at
      `test_environment.rs:1540`) and then mirrors `engine.ui_state().set_focus()`
      twice. Rewrite to `driver.click_entity(doc_id, "left_sidebar")` —
      the sidebar Selectable's bound action is `navigation.focus`, so
      the existing intent handler at `reactive.rs:2237` will set focus
      naturally. Delete the two `set_focus` mirrors. Delete the
      `dump_nav_tables` if it becomes redundant.

- [x] **A3. `apply_navigate_home` — drive via leader chord.**
      `sut.rs:700-712`. The TUI binding for "go home" needs identifying
      (likely a leader-Space sequence in `app_main.rs`'s leader-key
      table; if missing, add one as a tiny production change). Use
      `driver.send_raw_keystroke` for that chord. Delete the two
      `engine.ui_state().set_focus(None)` mirrors.

- [x] **A4. `apply_navigate_back` and `apply_navigate_forward` — drive
      via leader chord.** Same pattern as A3. Identify or add TUI
      bindings; remove the `execute_op` calls in `test_environment.rs:1559`
      and `:1570`.

- [x] **A5. `apply_arrow_navigate` — emit Up/Down keystrokes.**
      `sut.rs:2298-2338` currently asserts ref-state predicted focus
      against the reactive tree, then directly calls `engine.ui_state().set_focus(predicted_focus)`
      (lines 2333, 2336). Replace with a loop that emits `steps`
      Up/Down/Left/Right keystrokes via `driver.send_raw_keystroke`.
      Delete the two `set_focus` mirrors. The keyboard handler in
      `app_main.rs` (`advance_focus`/`reconcile_focus`) becomes the
      thing under test, and any divergence between predicted and actual
      focus is now a real failure rather than a forced match.

- [x] **A6. `apply_edit_via_view_model` and `apply_edit_via_display_tree`
      — collapse into `type_text` / delete.** `sut.rs:1034` and `sut.rs:1150`
      query the DB directly with `engine.execute_query(SQL)` and
      interpret the render DSL inline. That's not what a user does.
      Two paths to evaluate:
      - Replace the SUT body with `driver.type_text(block_id, new_content)`,
        which goes through `nav_to → Enter → keystroke-per-char → Enter`.
      - Or delete the corresponding transitions (`EditViaViewModel`,
        `EditViaDisplayTree`) entirely if `TypeChars` already covers
        the edit surface. Decide based on whether they exercise
        anything `TypeChars` doesn't.

- [x] **A7. Audit for residual `engine.ui_state().set_focus(...)` calls
      in test code.** After A2-A5, run
      `grep -rn 'ui_state().set_focus' crates/holon-integration-tests/`.
      Anything that remains is a shortcut. Either route through the
      driver or delete.

## Phase B — Type-system enforcement (close the door behind us)

- [x] **B1. Make `UiState::set_focus` non-public to test code.**
      `crates/holon-frontend/src/reactive.rs:844`. Move it behind
      `pub(crate)` or seal it via a trait whose only impl is the intent
      handler at `reactive.rs:2243`. Force every other call site to
      dispatch the `navigation.focus` / `navigation.editor_focus`
      intent. The eight test-side callers found in Phase A will become
      compile errors if any were missed — that's the point.

- [x] **B2. Architecture test: no direct focus mutation outside the
      intent handler.** Add a rule to
      `crates/holon-architecture-tests/tests/architecture_rules.rs`
      that flags any `set_focus(` outside `crates/holon-frontend/src/reactive.rs`
      (where `maybe_mirror_navigation_focus` lives). Sibling rule to
      the existing `no_raw_sql_in_frontends` and friends.

- [x] **B3. Architecture test: no `execute_op("navigation", ...)` from
      test code that simulates a user action.** Same file. Whitelist
      legitimate non-UI navigation (e.g. setup helpers).

## Phase C — Generic transition-weight env var

The current `chord_op_weight()` in
`crates/holon-integration-tests/src/pbt/transition_dispatch.rs:54` is
parochial and requires per-file wiring.

- [x] **C1. Replace `chord_op_weight()` with a generic
      `variant_weight_multiplier(name)` lookup.** Reads
      `HOLON_PBT_WEIGHTS=Indent:200,Move*:100,*Edit*:50` once via
      `LazyLock`. Returns `1` by default. Supports prefix/suffix glob
      patterns.

- [x] **C2. Apply the multiplier in the macro, not at each call site.**
      Edit the `aggregate_transitions` body at
      `transition_dispatch.rs:330` so each variant's weight is
      automatically multiplied:
      ```rust
      arms.push((w * variant_weight_multiplier(stringify!($variant)), ...));
      ```
      Then revert the per-file `Some((chord_op_weight(), strat))` back
      to `Some((1, strat))` in `indent.rs`, `outdent.rs`,
      `toggle_state.rs`, `move_up.rs`, `move_down.rs` and drop the
      `chord_op_weight` import. Remove `chord_op_weight()`.

- [x] **C3. Document the env var in `frontends/tui/tests/tui_ui_pbt.rs`
      doc comment** (or the closest test-readme) so the next person
      doesn't reinvent it.

## Phase D — Actually exercise edit-mode chord ops

- [x] **D1. Implement `tui_keystrokes_for_op` for `split_block`.**
      `frontends/tui/src/user_driver.rs:372` currently `bail!`s. The
      sequence is: `nav_to(block) → Enter (open edit mode) → assert
      edit_state Some → MoveCursor(position) → Ctrl+x →
      await_chord_settled`. The cursor positioning piece needs
      MoveCursor support — see plan §3 Task 4.

- [x] **D2. Implement `tui_keystrokes_for_op` for `join_block`.** Same
      pattern, but cursor at 0 + Backspace. Unblocks the JoinBlock
      transition.

- [ ] **D3. Verification rerun.** Once A-D land:
      - Plan Verification §1 (baseline regression): `cargo test -p holon-tui --test tui_ui_pbt 2>&1 | tee /tmp/baseline.log` should still pass.
      - Plan Verification §3 (synthetic break): set
        `HOLON_PBT_WEIGHTS=Indent:200,Outdent:200,ToggleState:200,Move*:100`
        plus the synthetic `false` return in `dispatch_block_op_on_focused`
        (`app_main.rs:662`); a chord transition should fire and the
        ref-state divergence should panic. Revert; passes again.
      - Plan Verification §4 (stability, 50× back-to-back): no flakes.

## Phase E — Operational polish

- [ ] **E1. `HOLON_DEBUG_CHORD=1` permanent flag.** One log line per
      `dispatch_block_op_on_focused` reached. Cheap, useful when
      investigating future regressions. Add in `app_main.rs:662`.

- [ ] **E2. Document the `enter_pressed` editor_focus dispatch fix.**
      `app_main.rs:803` was the production bug surfaced by the new
      pipeline (TUI didn't dispatch `navigation.editor_focus` on
      Enter-into-edit-mode, leaving `engine.focused_block` stale).
      Worth a unit test that asserts `engine.focused_block()` updates
      after `app_handle_input_event(Enter)` on a Block region — guards
      against regression after B1 makes `set_focus` private.

- [ ] **E3. Optional: fold `interaction_tx` into `input_tx`** (plan
      §Task 6). Out of scope until A-D land; revisit if maintaining the
      parallel channel becomes burdensome.

## Phase F — Ongoing hygiene

- [ ] **F1. Whenever a new transition is added, the `apply_*` method
      MUST drive `UserDriver`, not `execute_op` or `engine.ui_state.*`.**
      Codify in `crates/holon-integration-tests/src/pbt/CONTRIBUTING.md`
      (or wherever transition-authoring guidance lives) once Phase A is
      done. B2's arch test enforces it mechanically.

---

**Dependencies**: A1 → A2 → (A3, A4, A5 can land in parallel) → A6 → A7
→ B1 → B2/B3 → C1/C2/C3 → D1/D2 → D3 verification → E.
