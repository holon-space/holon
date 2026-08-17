---
id: 2026-08-17-embed-entity-command-has-no-reachable-completion
date: 2026-08-17
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Selecting the "Embed Entity" slash command split the block and left the
  literal "/enti" behind instead of running the command.
---

## Bug

Martin, dogfooding the GPUI desktop app (2026-08-17). Typing `/enti` narrows
the slash-command popup to the single proposal "Embed Entity" (`embed_entity`).
Both ways of invoking it fail:

1. **Enter** — instead of running the command, a new block is created below and
   the original block keeps the literal text `/enti`.
2. **Double-click** on the popup row — nothing happens.

Lane `lane-embed-entity`.

## Root cause

Two independent defects behind one symptom.

**(1) The Enter fall-through.** `embed_entity` takes `target_uri: &EntityUri`,
which the descriptor macro types as `TypeHint::String`: `parse_param_type_hint`
(`crates/holon-macros/src/operations_trait.rs:1524`) only emits
`TypeHint::EntityId` for an `#[entity_ref(..)]` attribute or a `*_id` parameter
name, and otherwise falls through to `infer_type_hint_from_rust_type`, whose
catch-all arm is `TypeHint::String`. So at selection time
`MatchedOperation::entity_params_needed()` was empty while
`is_fully_satisfied()` was false, and `CommandProvider::on_select` reached its
final `PopupResult::NotActive`. Every frontend maps `NotActive` to
`EditorAction::None`, which is indistinguishable from "no popup was open" — so
the GPUI Enter arm (`frontends/gpui/src/views/editor_view.rs:1526`) ran its
`split_block` default.

`#[entity_ref(..)]` had never been usable: the macro read the attribute but
emitted the trait verbatim, so annotating a parameter failed to compile with
`cannot find attribute 'entity_ref' in this scope`. Consequently
`TypeHint::EntityId` had **zero** production sites, and with it the entire
param-collection phase was dead — `set_search_results` /
`is_collecting_params` / `param_search_entity` had no callers outside
`command_provider.rs`'s own unit tests, and nothing ever issued the target
search.

**(2) The dead double-click.** `render_popup`
(`frontends/gpui/src/views/editor_view.rs:1917`) attaches no click handler to
any row. The popup is keyboard-only for *every* slash command; `embed_entity`
is simply where Martin noticed.

## Missing piece

The slash-menu correspondence lock
(`crates/holon-app/tests/slashmenu_correspondence.rs`) already fed the REAL
operation catalog through the production `CommandProvider` — but it only
asserted what the menu **shows** (presence, then uniqueness). No assertion
covered what happens when a row is **selected**, so a Listed command with no
reachable completion was invisible to it.

The keystone could not have covered the difference either: the headless mirror
maps every non-`Execute` popup result to `None`
(`crates/holon-frontend/src/headless_editor_mirror.rs:693`), so where prod
corrupted the block by splitting it, the mirror silently dispatched nothing —
the divergence that made this an ENVIRONMENT gap as well as an ORACLE one.

The `command_provider.rs` unit tests did exercise param collection, but against
a hand-built fixture that declared `target_uri` as `TypeHint::EntityId` — the
production descriptor never did. The fixture asserted the behaviour the
catalog did not have.

## Remedy

Fixed (Enter path):

- `crates/holon-macros/src/operations_trait.rs` — strip `#[entity_ref]` /
  `#[not_entity]` from the emitted trait so the attributes are actually usable.
- `crates/holon-core/src/traits.rs:2087` — annotate `embed_entity`'s
  `target_uri` with `#[entity_ref("block")]`.
- `crates/holon-frontend/src/command_provider.rs` — param collection now runs a
  live `search_link_candidates` search (the same capability `LinkProvider` uses
  for `[[` links) instead of reading a vector nothing populated; the dead
  `set_search_results` stub is deleted. A command with no reachable completion
  now returns `PopupResult::Failed` (toast + command text stripped) rather than
  `NotActive`, so a dead end can never again present as a block split.
- `crates/holon-frontend/src/popup_menu.rs` — `select_current` no longer
  dismisses on `PopupResult::Updated`, which had been discarding the provider
  that holds the next phase's state.

Oracle added — `crates/holon-app/tests/slashmenu_correspondence.rs`:

- `every_listed_command_has_a_reachable_completion` sweeps the whole Listed set
  from the real catalog and fails on any command whose selection is neither
  `Execute` nor `Updated`. Red before the fix with exactly
  `[("embed_entity", "NotActive")]`.
- `embed_entity_opens_the_target_picker` pins the specific dogfood witness.

Fixed (click path, lane `lane-popup-ux`, task #45):

- `crates/holon-frontend/src/popup_menu.rs` — `PopupMenu::select_index` moves
  the highlight to the clicked row and then runs the SAME `select_current` the
  Enter key runs, so a pointer pick cannot drift from a keyboard pick.
- `crates/holon-frontend/src/editor_view_model.rs` —
  `EditorViewModel::on_popup_item_clicked` maps that through the existing
  `popup_result_to_action`.
- `frontends/gpui/src/views/editor_view.rs` — the Enter arm's result handling
  is factored out into `apply_popup_action`, and each popup row now carries an
  `on_mouse_down` that calls it. Escape routes through it too, since cancelling
  a picker phase now edits the editor text.

Oracle added — `frontends/gpui/tests/popup_row_click_windowed.rs`:
`clicking_a_popup_row_runs_the_command_just_like_enter` runs `delete` from the
slash menu twice in one window, by keyboard then by mouse, and requires the two
to leave the same buffer. Red before the fix with the keyboard leg at `""` and
the mouse leg still holding `/del`.

Fixed (visible command text, lane `lane-popup-ux`, task #47, ruling D1.b):

- `PopupResult::PhaseAdvanced { hide_prefix_start }` replaces the bare
  `Updated` a provider returned when moving to a follow-up phase, carrying the
  instruction to hide the typed command.
- `EditorAction::HideCommandText` / `RestoreCommandText` — the frontend lifts
  the command text out of the visible block and hands it to the controller,
  which puts it back verbatim if the phase is cancelled. `on_text_changed`
  routes keystrokes straight to the picker's filter while the text is hidden,
  because with the `/` gone there is no trigger left to re-match.
- `CommandProvider` no longer prefix-strips the command out of the filter when
  the frontend hides it — the filter IS the search term then.

Oracle added — `frontends/gpui/tests/slash_command_text_hidden_windowed.rs`:
the phase hides `/emb`, Escape restores it verbatim, and typing a search term
leaves only the term visible while a later Escape restores `/embproj`.
