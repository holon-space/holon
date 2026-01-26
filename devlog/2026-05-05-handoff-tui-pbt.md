# Handoff — TUI PBT shortcut removal (2026-05-05)

## Where we are

Picking up `~/.claude/plans/please-create-a-plan-harmonic-thimble.md` →
`frontends/tui/TODO.md`. The plan rewired the TUI PBT to drive
`app_handle_input_event` for chord ops and clicks. Investigation showed
the test still bypassed the keyboard pipeline in many places. This
session tore out the bypasses, encoded the rule in the type system,
made the env-var weight knob generic, and implemented split/join
keystroke sequences.

Status of the TODO checklist (`frontends/tui/TODO.md`):

| Phase | Item | State |
|---|---|---|
| A | A1 — `apply_focus_editable_text` silent fallback | ✅ |
| A | A2 — `apply_navigate_focus` via LeftSidebar | ✅ |
| A | A3 — `apply_navigate_home` via leader chord | ✅ |
| A | A4 — `apply_navigate_back/forward` via leader chord | ✅ |
| A | A5 — `apply_arrow_navigate` keystrokes | ✅ |
| A | A6 — `EditViaViewModel/DisplayTree` gated, atomic editor on | ✅ |
| A | A7 — audit residual `set_focus` callers | ✅ |
| B | B1 — `UiState::set_focus` is `pub(crate)` | ✅ |
| B | B2 — arch test `no_direct_focus_mutation` | ✅ |
| B | B3 — arch test `no_navigation_execute_op_in_tests` | ✅ |
| C | C1 — generic `variant_weight_multiplier` | ✅ |
| C | C2 — multiplier applied in `declare_e2e_transitions!` macro | ✅ |
| C | C3 — `HOLON_PBT_WEIGHTS` documented in `tui_ui_pbt.rs` | ✅ |
| D | D1 — `split_block` keystroke sequence | ✅ (compiles, untested) |
| D | D2 — `join_block` keystroke sequence | ✅ (compiles, untested) |
| D | D3 — verification rerun (Verif §1, §3, §4) | ⏳ next |
| E | E1 — `HOLON_DEBUG_CHORD` permanent flag | ⏳ |
| E | E2 — unit test for `enter_pressed` editor_focus dispatch | ⏳ |
| E | E3 — fold `interaction_tx` into `input_tx` (optional) | ⏳ |
| F | F1 — codify "drive via UserDriver" guidance | ⏳ |

## What landed in code

- **`frontends/tui/src/keybindings.rs` (new)** — yaml-driven binding
  table, `BindingMode {Navigation, Editing}`, `KeyMatch`, glob
  helpers. Loaded once at startup via `OnceLock` from
  `include_str!("../config/keybindings.yaml")`.

- **`frontends/tui/src/app_main.rs`** — leader-active dispatch is
  now yaml-driven. Added `key_match_from_input`, `run_navigation_action`,
  `dispatch_navigation_op`. New leader chords:
  - leader+h → `navigation.go_home`
  - leader+b → `navigation.go_back`
  - leader+f → `navigation.go_forward`
  Plus the production fix from yesterday: Block-region Enter
  dispatches `navigation.editor_focus` so the engine's `focused_block`
  tracks edit-mode entry.

- **`frontends/tui/config/keybindings.yaml`** — three new entries
  for the nav-history chords above.

- **`crates/holon-frontend/src/reactive.rs:844`** — `UiState::set_focus`
  is now `pub(crate)`. The compiler enforces "go through dispatch_intent".

- **`crates/holon-integration-tests/src/pbt/sut.rs`** — ALL
  `engine.ui_state().set_focus(...)` calls and all
  `execute_op("navigation", ...)` calls removed:
  - `apply_navigate_focus` → `driver.click_entity(doc, "left_sidebar")`
  - `navigate_back` (SutHandle method) → `send_leader_chord("b", ...)`
  - `apply_navigate_forward` → `send_leader_chord("f", ...)`
  - `apply_navigate_home` → `send_leader_chord("h", ...)`
  - `apply_arrow_navigate` → `send_raw_keystroke("up"/"down"/...)`
  - `apply_focus_editable_text` → click_entity, no fallback
  New helper: `send_leader_chord(key, label)` — sends Space then key.

- **`crates/holon-integration-tests/src/test_environment.rs:1540-1592`**
  — the four `navigate_focus`/`navigate_home`/`navigate_back`/`navigate_forward`
  helpers deleted (replaced with a comment pointing to the new
  keyboard path).

- **`crates/holon-integration-tests/src/pbt/transitions/`**
  - `navigate_focus.rs` — restricted to `Region::Main` and to
    `focusable_rendered_block_ids(LeftSidebar)`.
  - `navigate_home.rs` / `navigate_back.rs` / `navigate_forward.rs`
    — restricted to `Region::Main`.
  - `edit_via_view_model.rs` / `edit_via_display_tree.rs` — gated
    off when `atomic_editor_enabled()`.

- **`crates/holon-integration-tests/src/pbt/ui_harness.rs`** — added
  `enable_atomic_editor_if_unset()` (sets `PBT_ATOMIC_EDITOR=1`).
  Called from `tui_ui_pbt::main`.

- **`crates/holon-integration-tests/src/pbt/transition_dispatch.rs`**
  - Replaced `chord_op_weight()` with generic
    `variant_weight_multiplier(name)` reading `HOLON_PBT_WEIGHTS`.
  - Macro `declare_e2e_transitions!` now multiplies every variant's
    base weight by the env-var multiplier automatically. Per-variant
    wiring removed.
  - Pattern syntax: comma-separated `pattern:multiplier`. Patterns
    are case-insensitive; one `*` glob = prefix/suffix/contains/all.
    Multiplier `0` removes the variant entirely.

- **`crates/holon-architecture-tests/tests/architecture_rules.rs`** —
  two new tests:
  - `no_direct_focus_mutation` — flags `ui_state.set_focus(` outside
    `holon-frontend/src/reactive.rs`.
  - `no_navigation_execute_op_in_tests` — flags
    `execute_op("navigation"` outside the navigation provider.

- **`frontends/tui/src/user_driver.rs:338`** — `tui_keystrokes_for_op`
  now accepts `extra_params`. `split_block` emits Enter→Home→Right×N→Ctrl+x;
  `join_block` emits Enter→Home→Backspace. Removed the old
  unconditional `bail!` on non-empty extras.

## What's left for tomorrow

### Immediate (D3)

Run the verification suite from the harmonic-thimble plan now that
A–D are coded:

1. Baseline regression — fresh seed, `cargo test -p holon-tui --test tui_ui_pbt | tee /tmp/baseline.log` should pass.
2. Synthetic break — apply the `dispatch_block_op_on_focused → false` patch
   plus `HOLON_PBT_WEIGHTS=Indent:200,Outdent:200,ToggleState:200,Move*:100`.
   With chord ops now structurally reachable (A/D), a chord transition
   should fire and the ref-state should diverge → panic with a
   useful diff. Revert; passes again.
3. Stability — 50× back-to-back. Catches the spawn-vs-render race
   (INV-DISPATCH-COMPLETED) and the lost-wakeup race (INV-NO-LOST-WAKEUP).

A real run during this session under `HOLON_PBT_WEIGHTS=Indent:50,Outdent:50`
hit `[inv-loro-no-errors] LoroSyncController logged 2 error(s)` —
"Cannot resolve parent URI to TreeID for outdent/indent/split where
the new parent isn't yet a TreeID in the Loro tree". This panic
reproduced ON BASELINE without any env var (different seed). It is
a pre-existing intermittent failure in the SQL→Loro mirror, not a
regression introduced this session, but it is a real bug and worth
filing. Look for `[LoroSyncController] Failed to apply` lines in
captured logs.

### Phase E — operational polish

- **E1** `HOLON_DEBUG_CHORD=1` — emit one log line per
  `dispatch_block_op_on_focused` (`app_main.rs:618`). Cheap, useful
  for future regressions.
- **E2** Unit test for `enter_pressed` editor_focus dispatch
  (the production bug fixed yesterday). After B1 made `set_focus`
  `pub(crate)`, the test will need to assert on `engine.focused_block()`
  rather than poking ui_state directly.
- **E3** Fold `interaction_tx` into `input_tx` — optional, plan §Task 6.

### Phase F — ongoing hygiene

- **F1** Document the "always drive `UserDriver`, never `execute_op`
  / `set_focus`" rule in
  `crates/holon-integration-tests/src/pbt/CONTRIBUTING.md` (or
  wherever transition-authoring guidance lives). The arch tests B2/B3
  enforce it mechanically; F1 is the human-readable companion.

## Known intermittent panic

`crates/holon-integration-tests/src/pbt/sut.rs:3317`
(`inv-loro-no-errors`). Pre-existing in the codebase. Reproduced on
both baseline and chord-weighted runs in this session. Likely real
SQL→Loro mirror bug. Not blocking the TODO progression but worth
filing as a separate issue:

```
[inv-loro-no-errors] LoroSyncController logged N error(s).
Search captured logs for `[LoroSyncController] Failed to apply` to find
which event(s) the SQL→Loro mirror dropped (e.g.
`Cannot resolve parent URI to TreeID: block:UUID` for outdent/indent/split
where the new parent isn't yet a TreeID in the Loro tree).
```

## Useful one-liners for resumption

```bash
# Run the PBT.
cargo test -p holon-tui --test tui_ui_pbt 2>&1 | tee /tmp/tui.log

# Bias toward chord ops (D3 §3).
HOLON_PBT_WEIGHTS=Indent:200,Outdent:200,ToggleState:200,Move*:100 \
  PROPTEST_SEED=42 cargo test -p holon-tui --test tui_ui_pbt 2>&1 | tee /tmp/tui_chord.log

# Re-run the new arch tests.
cd crates/holon-architecture-tests && cargo test --test architecture_rules \
  no_direct_focus_mutation no_navigation_execute_op_in_tests

# Re-run the keybindings parser tests.
cargo test -p holon-tui --lib keybindings
```

## Branch state

Working copy `nsmqruyk` at start of session; many modified files +
new `frontends/tui/src/keybindings.rs`, `frontends/tui/TODO.md`,
plus seven `devlog/2026-05-04-*.md` and this handoff. User mentioned
they'll handle VCS — no commits or branch operations performed.

## Heads-up

- The `archlint` hook flags any `_<name>` parameter (use bare `_` or
  `// ALLOW(unused_param): <reason>`). Hit it once at `click_entity(_, region: &str)`.
- `serde_yaml` is already a workspace dep; no Cargo.toml edits.
- `OnceLock` for the parsed `HOLON_PBT_WEIGHTS` table — if you mutate
  the env at runtime it won't pick up. Set before `main()` returns.
