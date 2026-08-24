---
id: 2026-08-24-split-block-indent-no-previous-sibling
date: 2026-08-24
gap: ENVIRONMENT
secondary: COVERAGE
retriaged_from: COVERAGE
status: FIXED
summary: >-
  Enter on a block whose rendered surface carries an org italic delimiter fired
  the slash-command menu instead of splitting, because the headless editor
  mirror re-derived the menu from the text at the caret on every Enter.
---

## Bug

`SplitBlock`'s Enter keystroke refused, loudly:

```
panicked at crates/holon-integration-tests/src/pbt/op_write_cap.rs:299:17:
[SplitBlock/keystroke] enter [] failed: dispatch_intent_sync: block.indent failed:
Operation 'indent' on entity 'block' failed: Cannot indent: no previous sibling to become parent
```

Found by the keystone PBT under forced weights, during 2b.4 I2b verification
(lane `inc2b-i1`, verifier round 2). The `indent` in the message is not part of
the split: it is the first item of the slash-command menu, dispatched in place
of the split.

## Root cause

`HeadlessEditorMirror::handle_keystroke`'s Enter arm ran `check_triggers`
against the block's current surface text on every press, and executed the
first matching command instead of splitting whenever a `/` sat at a word
boundary before the caret
(`crates/holon-frontend/src/headless_editor_mirror.rs`). Vault syntax renders
an `Italic` mark as `/…/`, so the instantiated template child `see krtvhl now`
(Bold 0..3, Italic 4..14) has the surface `*see* /krtvhl now/`. Splitting it at
content byte 4 puts the caret immediately after that delimiter, giving an empty
filter — every block operation matches and `items.first()` is `indent`, which
refuses because the block is a first sibling.

Production reaches the menu only through
`EditorViewModel::on_text_changed` (`check_triggers` on an `InputEvent::Change`,
`crates/holon-frontend/src/editor_view_model.rs:849`), and `on_key(Enter)`
consults the live overlay (`:1003`) rather than the text. No prod gesture opens
the menu on markup the user never typed; the mirror was a second, divergent
derivation of a decision prod makes once.

Retriaged from COVERAGE: the escape is test-environment divergence from prod,
not a generator that could not reach the state. The reachability observation
below still holds and is why the divergence surfaced when it did.

## Missing piece

Prod/test parity on the slash-menu lifecycle: the mirror carried no overlay
state, so it had to guess from the buffer.

REACHABILITY is the reason it surfaced on 2026-08-24 and not earlier. The
transition and the invariants were always there; what changed was the draw
distribution. The `RehomeEntity` weight raised from 2 to 10 in 2b.4 I2b
(verifier finding D6, "the default keystone never draws `RehomeEntity`")
shifted the global distribution enough to reach it — the transition being
weighted is unrelated to the bug, but re-weighting one member re-weights every
draw.

Measured novelty at the time of filing:

- `grep -c "no previous sibling to become parent" docs/Testing/KeystoneKnownReds.md` → 0
- present in exactly one lane log, `.lane-logs/d6-w10b-34636-20260824-054320.log`, a weight
  experiment from the same round
- absent from every pre-weight-change lane log in that workspace

## Remedy

The mirror now tracks the slash menu the way prod does: `note_text_changed`
opens, refilters or dismisses it on each text-mutating keystroke, and
`slash_command_selection` consumes it on Enter. With no menu open Enter always
splits, so the Enter path is total over blocks whose surface merely contains a
`/`.

Pinned by the hand-authored keystone regression
`enter-at-an-italic-delimiter-splits-instead-of-firing-the-slash-menu`
(`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`),
which reproduces the panic above in two transitions.
