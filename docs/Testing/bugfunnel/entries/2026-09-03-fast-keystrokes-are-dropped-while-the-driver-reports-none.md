---
id: 2026-09-03-fast-keystrokes-are-dropped-while-the-driver-reports-none
date: 2026-09-03
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Three characters typed in quick succession leave one character in the store,
  while the driver reports 3 sent, 3 handled, 0 dropped.
---

## Bug

Found by exploratory dogfooding (lane `dogfood-explore`) against a copy of
Martin's real vault (2257 blocks).

Typing into a focused block via one `type_text` call loses characters:

  - `type_text "XYZ"` -> reply `{"keystrokes_sent":3,"keystrokes_handled":3,"dropped":0}`;
    stored content gained `YZ`. The `X` is gone.
  - `type_text "ABC"` on a second block -> same reply shape, `0 dropped`;
    stored content gained a single `C`.
  - The same three characters sent as three separate `type_text` calls one
    second apart all landed (`CQWE` after the earlier `C`).

So the loss is a function of keystroke rate, and the driver's own accounting
denies it: `dropped` reads 0 in exactly the runs where two of three characters
never reached the store.

This is user-facing data loss for anyone typing at speed, and the false counter
means any test built on this driver can lose keystrokes and still go green.

## Root cause

Not isolated in this session. The shape — only the last keystroke of a burst
survives — is consistent with each keystroke being applied against a snapshot of
the block content taken before the previous keystroke committed, so each write
overwrites its predecessor rather than composing with it. Recorded here as an
escape with its reproduction, not as a diagnosis.

Measured under heavy machine load (four parallel Rust builds); the one-second
spacing control passing on the same machine argues the rate, not the load, is
the variable, but the load is disclosed.

## Missing piece

The `dropped` counter is the oracle that should have caught this and it is
wrong: it counts keystrokes the driver handed to the editor, not keystrokes that
reached the store. No invariant compares the characters sent in a burst against
the delta in stored content, so the keystone can drive fast typing (it does) and
never notice the loss.

## Remedy

Open. Make the counter honest — reconcile sent keystrokes against the stored
content delta and report the difference — then add an invariant that a burst of
N printable keystrokes into one focused block grows that block's content by
exactly N characters. That invariant should go red on this build before the
editor race is touched.
