---
id: 2026-09-13-undo-precondition-reader-finds-no-content
date: 2026-09-13
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  On a full production-wiring boot, undoing a `set_field` on `content` is
  always stale-dropped because the undo precondition reader finds the field
  absent rather than changed.
---

## Bug

Boot the shared production wiring over a temp vault
(`holon_app::new_from_config_with_di`), dispatch one ordinary user
`block/set_field` on `content`, then undo it. The undo never runs: it is
dropped as stale, with

```
state changed under undo: shutdown-probe-child.content
expected String("set by the user") but found None
```

`found None` is the point. The precondition is not failing because something
changed the value — it is failing because the live-state reader returns nothing
at all for that field.

Found by the `cell-undo` lane while trying to pin the one-stack invariant
across both undo mechanisms (D115.A increment 2), after a verifier reported
that "every journalled replay is stale-dropped even with no typing" on this
harness.

## Root cause

Not established beyond the observation above. What IS established, by a control
that isolates it:

`crates/holon-app/tests/text_undo_manager.rs`
`control_a_journalled_set_field_alone_stale_drops_on_this_harness` arms no undo
manager, writes no text-epoch marker and types nothing. It still stale-drops
with `found None`. So the cause is **not** the cell leg and **not** increment
2's markers: it is the undo precondition reader
(`crates/holon/src/api/undo_persistence.rs` `SqlUndoStateReader`, reading
`crate::storage::BLOCK_WRITE_TABLE`) on this wiring.

Two candidates worth checking first, neither confirmed:

- an id-scheme mismatch — the precondition names `shutdown-probe-child.content`
  without the `block:` scheme, while the block is addressed as
  `block:shutdown-probe-child` elsewhere;
- the `content` column not being populated in the write table for a block whose
  text lives in the CRDT, so the read is legitimately empty.

The distinction matters: the first is a bug in the undo path, the second means
the precondition is asking the wrong store.

## Missing piece

No test undoes an ordinary content `set_field` through a full production-wiring
boot and asserts the value comes back. The journal's own suites
(`crates/holon/tests/undo_*.rs`) build their substrate directly rather than
booting the shared wiring, so a reader that returns nothing there is never
exercised; the keystone's `undo_last_mutation` is gated to Turso slices and
asserts the reference model's whole-tree snapshot rather than this reader's
answer. That combination is why an undo that never applies has gone unnoticed.

## Exposure

Unknown, and deliberately not guessed. If it reproduces in the running
application, undo of a content edit does nothing on this wiring and reports
`StaleDropped` rather than failing visibly — which would be a user-facing
defect of the first order. If it is specific to how this harness projects
`content`, it is a test-parity gap. Establishing which is the first step for
whoever takes it: run the same gesture against a real vault through the MCP
surface.

## Remedy

Open. Pinned in the meantime as a tripwire: the control test asserts the
CURRENT (wrong) behaviour and its failure message says to re-enable end-state
assertions in `interleaved_typing_and_operations_walk_back_in_one_order`, which
cannot observe journalled restoration until this is fixed.
