# PBT empirically verifies "click on sidebar blurs editor"; targeted fix replaces speculation

**Date**: 2026-05-08
**Continues / corrects**: `devlog/2026-05-08-133241-blur-was-fake-transition-removed.md`

## What the prior devlog got wrong

The prior devlog kept `commit_active_editor_if_changed() + state.active_editor = None;` in all four nav transitions on the assumption that production blurs the editor on a sidebar click. That assumption was unverified — the PBT's whole value is that ref-state and prod *lift each other*; pre-baking model side-effects silences the very signals PBTs exist to surface.

## What the experiment showed

Reverted the cleanup. Ran 8 seeds. Got three distinct panics:

| Seed | Panic | Class |
|---|---|---|
| 5 | `sut.rs:3923` Navigation focus mismatch (`current_focus` matview row disagrees with `navigation_history.current_focus()`) | Independent — ClickBlock or matview drift |
| 6 | `sut.rs:1486` `SCHEDULER BUG: query_and_watch timed out … mark_available() never called for 'blocks'` | Independent — Turso/scheduler |
| 8 | `sut.rs:2295` `TypeChars: send_raw_keystroke failed: GPUI keystroke not consumed: keystroke="d"` after `FocusEditableText → AddPeer → SetupWatch → SyncWithPeer → ConcurrentSchemaInit → NavigateFocus → TypeChars` | **The blur-on-nav signal** |

Seed 8 is the empirical answer: production *does* blur the editor on the sidebar click in `NavigateFocus` (otherwise the 'd' keystroke would have been consumed by the `Input` context). The earlier "model" was right in effect — but unprincipled in derivation.

## Targeted fix (verified subset only)

Re-added **only** what seed 8 verified: `state.active_editor = None;` in the four nav transitions' `apply_to_ref`. Deliberately did *not* re-add `commit_active_editor_if_changed()` — whether prod's `on_blur` actually dispatches `set_field` is a separate assumption that downstream content invariants (e.g. `inv-displayed-text`) should verify, not the navigation transitions themselves.

## Verification

Ran the same 8 seeds post-fix:

```
Seed 1: pass 50/50
Seed 2: pass 46/50
Seed 3: pass 45/50
Seed 4: pass 44/50
Seed 5: panic (Navigation focus mismatch — same independent bug as before)
Seed 6: pass 47/50
Seed 7: pass 45/50
Seed 8: panic (Navigation focus mismatch — was blur-related TypeChars panic before)
```

The blur-related TypeChars panic on seed 8 is gone. Seeds 5 and 8 now hit the same independent `current_focus`-vs-`navigation_history` invariant — that's a real bug the PBT was previously hiding behind the TypeChars panic. Tracking it is out of scope for this thread.

## What this teaches about working on PBTs

**Model assertion = production hypothesis.** Every line in `apply_to_ref` that mutates state is asserting something about how prod behaves. If the assertion is unverified, deleting it (and letting the test panic) is more valuable than guessing. The panic tells you *which* prod assumption is real.

**Workflow:**
1. Strip ref-state down to facts you can prove.
2. Run the PBT.
3. Read each panic as "production says X here, your model said Y."
4. Add the *minimum* model update that aligns with the verified production behavior.
5. Repeat.

Anything else is the test rubber-stamping the model instead of testing prod.

## Files

- `crates/holon-integration-tests/src/pbt/transitions/navigate_focus.rs` — `state.active_editor = None;` (verified by seed 8); commit removed
- `crates/holon-integration-tests/src/pbt/transitions/navigate_home.rs` — same
- `crates/holon-integration-tests/src/pbt/transitions/navigate_back.rs` — same
- `crates/holon-integration-tests/src/pbt/transitions/navigate_forward.rs` — same

## Open follow-ups (not addressed here)

1. `current_focus` matview vs `navigation_history` divergence — seeds 5 + 8 both hit the same `sut.rs:3923` assertion. Either ClickBlock's apply_to_ref doesn't push to navigation_history, or the matview lags. Worth tracing.
2. Seed 6 scheduler timeout — `mark_available()` never called for 'blocks' table. Different bug.
3. `commit_active_editor_if_changed` on nav is *not* in the model. If production does dispatch `set_field` on blur, content invariants should eventually catch that the model's `block.content` is stale post-nav. If they don't catch it, either prod doesn't commit (real bug) or no invariant covers this — also worth knowing.
