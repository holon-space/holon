---
id: 2026-09-18-toggle-landing-loop-assumes-an-idempotent-click-verb
date: 2026-09-18
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The shared `ToggleState` landing loop re-clicks while the projection lags on
  the stated premise that stale re-clicks are no-ops, which is only true for a
  click verb that computes its target from the view — not for the TUI's
  authority-relative `cycle_task_state` chord, whose every re-click is a real
  extra cycle.
---

## Bug

While fixing the TUI's missing state-toggle gesture (`2026-09-18-tui-state-toggle-
drove-nav-enter-not-the-cycle-chord`), the first fix routed the gesture through
the TUI's `cycle_task_state` chord (Ctrl+T). The verifier then found 10
state-toggle-family failures in 52 biased post-fix runs, including a silent
oracle divergence with no panic at all:

```
[inv-task-state-storage-coherence] 1 block(s) have task_state diverging from the
reference across block_raw SQL / Loro projection.
  block:c1: ref=Some("DONE") sql=Some("") loro=Some("")
[inv-viewmodel-state-toggle-correct] StateToggle current '' != reference 'DONE'
  — lane-logs/verify-v-post-toggle-r1.log, and 5 more in the same batch
```

Instrumented (temporary probes in `components.rs::toggle_state_via` and
`app_main.rs::dispatch_block_op_on_focused`, reverted with sha256 proof):
over 112 probed runs / 278 traced toggles `reclicks=0` in 110, and **the only
2 runs with a re-click are exactly the 2 runs that diverged** (2/2 vs 0/110) (`lane-logs/probeP-r3j5.log`, `-r3j6.log`).
The trace of `r3j5` is the mechanism in one line:

```
[probe-toggle] id=block:parent iter=0 RE-CLICK #1 block_raw_still="DONE"
[probe-toggle] id=block:parent iter=0 LANDED block_raw_state=""
...
[probe-toggle] id=block:parent DONE target="DOING" final_block_raw="DOING" reclicks=1
```

## Root cause

`HeadlessFrontendComponent::toggle_state_via`
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:3765-3802`)
polls `block_task_state` for up to 500 ms after each click and RE-CLICKS if the
projection has not moved. Its comment states the premise: "Stale re-clicks are
idempotent (same value), and the first click after the view refreshes advances
exactly one step".

That premise holds only for the verb the other drivers use — a `set_field` whose
target is computed from a resolved view, so a re-click against a stale view
writes the keyword already stored. It does not hold for `cycle_task_state`,
which reads the prior keyword off the AUTHORITY and advances one step in the
ring (`crates/holon/src/api/operation_engine.rs:1917-1929`, ring `["", "TODO",
"DOING", "DONE"]`). Every re-click is therefore a real extra cycle the
reference never applied, and the SUT lands `#re-clicks` steps past it — which is
why the symptom is load-correlated (only a >500 ms projection lag triggers a
re-click) and why the loop can report success while the store is already ahead.

The loop is also not self-checking. Everything it reads — `block_task_state`,
and the view the click's target is computed from — comes from the SUT, and it
compares each reading only against the SUT's OWN previous reading, never against
the reference. It therefore breaks as soon as `block_raw` moves and can never
observe an over-advance: in the `r3j5` trace the loop reports `final_block_raw=
"DOING"` (= target, success) while the store is already one cycle past it. The
reference model has no way to express "the SUT advanced more than asked" either,
so only a later cross-layer reconcile can catch this — which is how it was
caught.

Gap class: COVERAGE. ORACLE does not fit, and the earlier label was wrong — the
invariants DID go red (`inv-task-state-storage-coherence`,
`inv-task-state-matches-ref`, `inv-viewmodel-state-toggle-correct`, and all four
`inv-blocks-match-ref` arms), so nothing was missing on the oracle side. What
the generator cannot do is REQUEST the trigger: the re-click path opens only
when the projection lags more than the loop's hardcoded 500 ms window, and no
transition, precondition or weight can ask for that.

## Missing piece

Any declaration of what a driver's toggle verb IS. `UserDriver::cycle_state_toggle`
documents the glyph-click contract and the geometry precondition, but nothing
states whether the verb is view-computed (idempotent under a stale view) or
authority-relative (one real step per call) — so a driver cannot be judged for
choosing a verb the shared loop cannot safely re-issue, and the loop cannot
adapt to one that does.

## Remedy

FIXED by ruling **D143.a** (Martin, 2026-09-18). The declaration the missing
piece asked for now exists, and the loop reads it.

**The nature is a type.** `holon_frontend::StateToggleVerb` (`Idempotent` /
`Advancing`, `crates/holon-frontend/src/user_driver.rs`) is RETURNED by
`UserDriver::cycle_state_toggle` rather than declared by a second method, so an
override cannot silently inherit a nature that contradicts the verb it issues —
the compiler demands the declaration at every impl site. The trait default and
`ReactiveEngineDriver` / `SimUserDriver` (all resolved-tree `set_field` intents)
return `Idempotent`; `TuiUserDriver` returns `Advancing`.

**The loop branches on it, exhaustively.** `toggle_state_via`'s landing wait
moved into `await_toggle_landed`
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs`), whose
`match verb` re-issues the click only for `Idempotent`; for `Advancing` it waits
the remaining attempts out on the projection and then fails loud naming the click
that never landed.

**The TUI went back to the real Ctrl+T.** `TuiUserDriver::cycle_state_toggle`
again drives the `cycle_task_state` chord through the input pipeline (the round-1
shape): it looks the op up in `engine.key_bindings()`, presses that op's keystroke
from `tui_keystrokes_for_op` — the single per-frontend translation — and awaits
`await_chord_settled`. The `advancing` declaration is what makes that safe. That
lookup is a presence check against a table hardcoded in
`crates/holon-frontend/src/reactive.rs`, not against
`assets/default/keybindings.yaml`, and the chord it returns is used only in the
error text.

### Red, then green, then teeth

The RED is a biased TUI sweep (`HOLON_PBT_WEIGHTS=ToggleState:400`, 72 runs, load
23→160) on the round-1 shape with the OLD re-clicking loop: **4 runs diverged**,
each with a re-click, and the mechanism in one trace:

```
[probe-toggle] id=block:c2 target=Doing clicks=2 start=""
[probe-toggle] id=block:c2 click#1 RE-CLICK #1 block_raw_still=""
[probe-toggle] id=block:c2 DONE target=Doing final_block_raw="DOING" reclicks=1
→ block:c2: ref=Some("DOING") sql=Some("DONE") loro=Some("DONE")
```

The same sweep also produced the barrier classes the re-click caused: **46 of the
72 runs** carry a `re-click #N failed` message, one each, split **29 `engine did
not quiesce` / 17 `nav_to: walked past target`** with no run showing both (the
second Down walk of a re-click racing the first walk's queued presses). A further
10 runs failed on a first `click #N`, not a re-click.

TEETH: with the shipped tree and only the TUI's declaration flipped to
`Idempotent`, the divergence returns. Measured as a PAIRED sweep (both binaries in
the same round, so the load is shared; three batches, load 8→285):

| arm | runs | toggle-family failures | barrier timeouts | other |
|---|---|---|---|---|
| shipped (`Advancing`) | 82 | **0** | 4 | 8 |
| teeth (`Idempotent`) | 82 | **2** (`block:c1: ref=Some("DONE") sql=Some("")`; `block:parent: ref=Some("DOING") sql=Some("DONE")`) | 5 | 3 |

Both teeth divergences landed in the load-200-285 batch (2/30 there, 0/52 at
calm): the silent over-cycle needs a >500 ms projection lag, and where the lag
does not come the mis-declaration fails loud instead (4/28 and 1/24 calm, as a
re-click's `click #N failed … engine did not quiesce`). The `other` column is
sequence noise between independent draws (a boot failure, `inv-inline-row-mount-
present`, `apply_navigate_focus`, the `cooklang is a read-only format` known-red),
so `toggle-family` is the discriminating column.

The `ref=DONE vs SUT=""` teeth signature is the entry's own counterexample
reproduced by one line of declaration.

### Not closed by this entry

`cycle_state_toggle`'s chord→op resolution (`resolve_key_chord` /
`send_key_chord`) is NOT usable for TUI-painted rows: Cmd+Enter's
`capture_action` wiring is GPUI-only, so bubbling yields `Focus`, not
`ExecuteOperation` — 41 of 41 `ToggleState` draws failed with "binds
cycle_task_state but does not resolve" (`lane-logs/redA-*`). The TUI therefore
resolves the op→chord direction against the hardcoded registry and presses its
own key. Only removing `cycle_task_state` from that hardcoded table fails loud:
a binding dropped or rebound in `assets/default/keybindings.yaml` — the file the
TUI really reads (`frontends/tui/src/keybindings.rs` `include_str!`) — is not
caught, and neither is a rebind of the registry chord itself. Measured: renaming
`action: cycle_task_state` in that yaml left 3 of 3 ToggleState-biased runs
passing.

Evidence (all `lane-logs/`): `red-evidence.txt` (the red),
`teeth-diverge-evidence.txt` (the teeth), `pair*classify.txt` +
`combined-*.txt` (the paired totals), `diagN-*`/`diagS-*` (the nav_to probe that
found the barrier defect, now its own entry), re-derivation script
`classify.py`.
