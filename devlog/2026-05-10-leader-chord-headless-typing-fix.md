# PBT NavigateBack typed " b" into focused editor — fixed

Date: 2026-05-10

## What was failing

`general_e2e_pbt` (Full variant) panicked at `assertions.rs:60` with
`block:journals` content `"Journals b"` (production) vs `"Journals"`
(reference). Earlier shrunken seeds showed the same shape on
`block:ref-doc-0` (`"__306672 b"` vs `"__306672"`). The trailing
`" b"` was deterministic across shrinks.

The failing transition sequence (per `LORO_DIFF_TRACE` and
`KEYSTROKE_DEBUG` traces):

1. `StartApp` — production seeds `block:journals` with content
   `"Journals"` (via `seed_default_layout`).
2. `ClickBlock(block:journals)` — dispatches `editor_focus`. The
   reactive engine's `focused_block = block:journals`.
3. `NavigateBack` — calls `send_leader_chord("go_back", …)`, which
   sends raw keystrokes ` ` and `b` (leader+chord).
4. The headless `ReactiveEngineDriver::send_raw_keystroke` routes
   straight into `HeadlessEditorMirror::handle_keystroke`, which
   inserts the chars at the cursor. Because `journals` has editor
   focus, the keystrokes TYPE `" b"` into journals' content via
   `LoroTextCellBacking::apply_text_op(Insert, …)`.
5. Outbound projector observes the LoroText change and projects
   `block:journals.content = "Journals b"` to SQL.
6. Reference model's `apply_to_ref` for `NavigateBack` does NOT type
   anything — it dispatches the `go_back` intent symbolically.
   Reference content stays `"Journals"`.

## Root cause

`send_leader_chord` (`crates/holon-integration-tests/src/pbt/sut.rs`)
was written when `ReactiveEngineDriver::send_raw_keystroke` was
`unimplemented!`. Pattern:

```rust
let raw_result = async {
    driver.send_raw_keystroke(" ", &[]).await?;
    driver.send_raw_keystroke(key, &[]).await
}.await;
if raw_result.is_ok() { return; }
// Fallback: synthetic_dispatch
```

For TUI native (`TuiUserDriver`), raw keystrokes go through the real
input pipeline → chord resolution → `go_back` dispatch.

For headless (`ReactiveEngineDriver`), raw keystrokes used to fail
(`unimplemented!` → Err), and `send_leader_chord` fell through to
`synthetic_dispatch`.

When `ReactiveEngineDriver::send_raw_keystroke` was added (to support
the atomic editor primitives `TypeChars` / `PressKey` etc.), it
started routing keystrokes into the editor mirror. From then on,
`send_raw_keystroke(" ")` against a focused editor SUCCEEDED (it
typed) and the fallback never fired. Leader chord SPC+key turned
into `" b"` typed into the focused block.

## Fix

Add a method to `UserDriver`:

```rust
fn dispatches_chords_via_raw_keystroke(&self) -> bool { false }
```

- Default `false`: headless drivers never had chord routing in their
  `send_raw_keystroke` and shouldn't pretend to.
- `TuiUserDriver` overrides to `true` — its `send_raw_keystroke`
  pushes through `send_input` which is the real chord-resolved
  pipeline. (`GpuiUserDriver` keeps the default `false` because its
  `send_raw_keystroke` already returns `Err` for unconsumed leader
  prefixes; the previous fallback worked there and now we just go
  straight to `synthetic_dispatch`.)

`send_leader_chord` checks the flag and either sends raw keystrokes
(TUI) or dispatches the navigation intent directly (headless / GPUI).

## Files touched

- `crates/holon-frontend/src/user_driver.rs` — added
  `dispatches_chords_via_raw_keystroke` method on the trait
  (default false). Cleaned up pre-existing archlint violations
  flagged by the hook on this file (pre-existing
  underscore-prefixed trait params bulk-renamed to bare `_`,
  pre-existing `// ALLOW(fallback)` and `// ALLOW(ok)` markers).
- `frontends/tui/src/user_driver.rs` — overrode the new method to
  `true` (TUI's input pipeline runs chord resolution before the
  editor sees anything).
- `crates/holon-integration-tests/src/pbt/sut.rs` —
  `send_leader_chord` now branches on the new method instead of
  the try-raw-then-fallback pattern. Headless drivers go straight
  to `synthetic_dispatch("navigation", nav_op, …)`. Native drivers
  send raw keystrokes and fail loud if the input pipeline rejects
  them (no silent fallback — if TUI's chord router doesn't
  resolve, that's a bug in the binding, not something to paper
  over).

## Why this matches Phase-2 architecture

In Phase 2 (Loro authority for blocks), per-keystroke writes go
straight into LoroText via `Cell<String>::apply_text_op`. The
outbound `LoroSyncController.on_loro_changed` is the SOLE SQL
writer for `block.content`. Pre-Phase-2, the `headless_editor_mirror`
mirrored text via `MutableText` which wrote through
`LoroBackend::update_block_text`. Either way, accidentally typing
into a focused editor lands in the persisted state — there's no
"safe" location for a stray keystroke.

The fix is at the test infrastructure layer (`UserDriver` trait +
PBT helper) because the architectural mismatch is between "raw
keystroke" semantics in different drivers, not in the data plane.

## Verification

| Check | Status |
|-------|--------|
| `cargo check --workspace --tests` | GREEN |
| `general_e2e_pbt_sql_only` (PROPTEST_CASES=1, regression replay + 1 random) | ✅ PASS in 509s |
| `general_e2e_pbt` (Full, same args) | ✅ PASS in 559s |
| `archlint` hook on `user_driver.rs` | GREEN after pre-existing-violation cleanup |

## Open follow-ups

- The `dispatches_chords_via_raw_keystroke` flag is a stopgap. The
  cleaner long-term shape would be: drop `send_raw_keystroke` from
  the headless driver entirely (atomic editor primitives could go
  through a dedicated typed `type_char` method instead) and have
  `send_leader_chord` always use `synthetic_dispatch`. That refactor
  was deemed out of scope for the immediate bug fix.
- `ReactiveEngineDriver::send_raw_keystroke` swallows the
  "are we routing this through chord resolution?" question by
  always going to the editor mirror. A future cleanup could
  add an explicit `send_chord(prefix, key)` method on the driver
  trait to disambiguate the two intents at the call site.
