---
id: 2026-09-02-concurrent-keystroke-undo-step-count-diverges
date: 2026-09-02
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  Concurrent typing left 7 undo steps where sequential typing leaves 3, once in
  ~4 full runs; not reproduced in 9 isolated runs and not yet root-caused.
---

## Bug

`holon::undo_concurrent_keystrokes concurrent_keystrokes_keep_every_undo_step`
failed once during the D65.a/D66.a/D64.a increment
(`lane-logs/d2-nextest.txt`):

```
panicked at crates/holon/tests/undo_concurrent_keystrokes.rs:203:5:
assertion `left == right` failed: concurrent typing must leave the same undo
history as sequential typing
  left: [Some("second l"), Some("second"), Some("secon"), Some("sec"),
         Some("seco"), Some("se"), Some("")]
 right: [Some("second l"), Some("second"), Some("")]
```

Seven undo steps against the sequential reference's three. Note the left walk is
also **out of order** — `"secon"` precedes `"sec"`, which precedes `"seco"`.

Frequency: 1 of ~4 full `-p holon -p holon-app` runs. Not reproduced in 9
isolated runs — 3 by this lane (`lane-logs/d2-undo-iso.txt`) and 5 plus one
full-run pass by the verifier (`lane-logs/verify/v3-undo.txt`, load average
5.7–6.6). Found by agent exploration (lane `reds-triage`), not by a gate — the
suite it lives in is the one no gate ran.

## Root cause

**Not yet established.** Two candidates, and the evidence does not yet separate
them:

1. A real race in the inverse-command log: concurrent keystrokes fail to
   coalesce into one undo step, so per-character inverses survive individually.
   The out-of-order walk is the stronger lead — a pure coalescing miss would
   still leave the steps monotonically ordered.
2. An over-specified oracle: exact equality with the sequential walk may be
   stricter than the property that actually matters (that undo reaches empty
   without skipping a character).

`type_word_as_the_editor_does` deliberately spawns un-awaited writes 1 ms apart
to provoke interleavings, so the failing interleaving is one the test is built
to reach — it is simply rare.

## Missing piece

The oracle asserts a **correctness equality**, not a latency budget, and its
subject *is* concurrency. So the load-sensitivity remedy used for the vault-scale
guard is wrong here: a `max-threads = 1` nextest group would delete the only
condition under which the property is exercised at all. This entry exists
because a flake with a correctness oracle must be root-caused, not pinned away.
A high-repeat run (the differing step counts are the lead) is the next step.

## Remedy

OPEN. Deliberately NOT pinned to a serial test-group, and deliberately not
reclassified as a false alarm while its mechanism is unknown.
