---
id: 2026-08-11-keystone-asserts-cursor-position-against-editor
date: 2026-08-11
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  The keystone asserts a cursor position against an editor the SUT never
  opened.
source_line: 738
---

## Bug

(task #78 arm-(d) lane, found by an unattributable `just keystone-smoke` red
and then classified — DISCOVERED, NOT CAUSED by this lane) **The keystone
asserts a cursor position against an editor the SUT never opened.**
`apply_move_cursor` (`frontend_slice/components.rs:2273`) falls back from
`editor_live_text` to `editable_text`, which is `Err` in a Turso-only drawn
wiring, and then to `unwrap_or_default()` — so it asserts the ref-generated
`byte_position` against `""`: `[apply_move_cursor] byte_position 1 not a
char boundary of ""`. The file's own comment two lines up warns about this
very fallback; the guard covers the `editor_live_text` miss but not the
`Err` after it. Frequency 1 in 5 smoke runs (draw-dependent).

## Root cause

task #78 arm-(d) lane, found by a KEYSTONE-SMOKE RED the lane could not
attribute and then classified — DISCOVERED, NOT CAUSED by this lane: **the
keystone asserts a cursor position against an editor the SUT never opened,
so the harness reds on a state production cannot be in.**
`apply_move_cursor`
(`crates/holon-integration-tests/src/pbt/frontend_slice/components.rs:2273`)
reads `editor_live_text`, falls back to `editable_text(block, "content")`,
and — in a Turso-only (SqlOnly) drawn wiring, where `editable_text` is `Err`
because there is no Loro cell — falls further to `unwrap_or_default()`, i.e.
`""`. It then asserts the REF-generated `byte_position` is a char boundary
of that empty string: `[apply_move_cursor] byte_position 1 not a char
boundary of ""`. The file's own comment two lines above warns about exactly
this ("a blanket `unwrap_or_default` would convert every position against
`\"\"`") — the guard it describes covers the `editor_live_text` miss but not
the `editable_text` `Err` that follows it. ENVIRONMENT primary: this is the
HARNESS diverging from production, not a product defect — in prod a
MoveCursor gesture presupposes a mounted editor, whereas the generator
admits it for any FOCUSED block, so the reference models an editor the SUT
was never asked to open. ORACLE secondary: the reference's `active_editor` +
cursor model represents a state the SUT has no counterpart for, so the two
sides are not comparable at that point rather than disagreeing about a fact.
Frequency 1 red in 5 `just keystone-smoke` runs (draw-dependent). NOT CAUSED
BY THIS LANE, proven two ways rather than asserted: (1) DETERMINISTIC REPRO
— the shrunk case replays as a hand-authored case, `[Indent(block:c1),
MoveCursor(1)]` under a Turso-only wiring, and reds identically every time
(`lane-logs/task78/r8-replay-*.txt`); the keystone sets
`failure_persistence: None` and prints no seed, so this replay had to be
AUTHORED, and it is the artifact the registry asks low-frequency residuals
to carry; (2) ORDERING A/B — the lane's own suspect (round 6 moved a
vocabulary `await` between the mirror's content snapshot and its VM insert)
was ELIMINATED by re-snapshotting the content after the await and replaying:
the probe reds identically with all 8 other cases green
(`r8-ab-ordering-*.txt`); `headless_editor_mirror.rs` sha256-restored
`e227a7cf…`. The window hypothesis is refuted structurally too — the
assertion runs BEFORE `send_raw_keystroke`, the only call on that path that
can create an editor VM, so no ordering inside `seeded_editor` is reachable
and the failure is unconditional for this draw shape, not a race. It is
therefore NOT widened by this lane either. NOT SILENCED and NOT
self-registered: `docs/Testing/KeystoneKnownReds.md` requires Martin's
ratification before a signature is added, so this row is the triage and the
registry entry is owed. The repro line is recorded here so anyone can re-arm
it in one paste (it is deliberately NOT left in `keystone.jsonl`, which
would make `just hand-authored` permanently red): `{\"name\":
\"probe-movecursor-into-unopened-editor\", \"initial_state\": {\"wiring\":
{\"storage_adapters\": [\"Turso\"], \"sync_adapters\": [], \"actors\": []}},
\"transitions\": [{\"Indent\": {\"block_id\": \"block:c1\"}},
{\"MoveCursor\": {\"byte_position\": 1}}]}`. Suggested remedy for whoever
owns it: either make `apply_move_cursor` SKIP when no editor is open on the
focused block (matching prod, where the gesture presupposes one), or make
MoveCursor's precondition require an editor the SUT has actually opened —
the assertion should never be reached with a fabricated `""`.)

## Missing piece

ENVIRONMENT: the HARNESS diverges from production — a MoveCursor gesture
presupposes a mounted editor in prod, while the generator admits it for any
FOCUSED block, so the reference models an editor the SUT was never asked to
open. ORACLE: the ref's active-editor + cursor model represents a state the
SUT has no counterpart for, so the sides are incomparable there rather than
disagreeing.

## Remedy

OPEN — discovered, triaged, NOT fixed and NOT self-registered
(`KeystoneKnownReds.md` requires Martin's ratification). Not caused by this
lane, proven twice: a DETERMINISTIC hand-authored replay of the shrunk case
(`[Indent(block:c1), MoveCursor(1)]`, Turso-only wiring) reds every run —
the keystone sets `failure_persistence: None` and prints no seed, so the
repro had to be authored — and the lane's own suspect (round 6's vocabulary
`await` between content snapshot and VM insert) was ELIMINATED by
re-snapshotting after the await: identical red, 8 other cases green,
`headless_editor_mirror.rs` sha256-restored. Structurally the assertion also
runs BEFORE the only call that can create an editor VM, so no ordering is
reachable and the failure is unconditional for the draw shape — not widened
by this lane either. Repro line recorded in the increment-log entry;
deliberately not left armed in `keystone.jsonl`. Remedy for the owner: skip
`apply_move_cursor` when no editor is open on the focused block, or require
an SUT-opened editor in MoveCursor's precondition.
