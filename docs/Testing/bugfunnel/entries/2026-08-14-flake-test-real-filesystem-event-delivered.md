---
id: 2026-08-14-flake-test-real-filesystem-event-delivered
date: 2026-08-14
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Not a flake and not one test — no real filesystem event is delivered to any
  test process this agent session launches, so the whole real-watcher rung is
  dead for agents while it passes for Martin.
source_line: 707
---

## Bug

(task-#27 org-origin lane; reported by the instrumentation lane as a 3/3
"FSEvents-timeout flake") **Not a flake and not one test — no real
filesystem event is delivered to any test process this agent session
launches, so the whole real-watcher rung is dead for agents while it passes
for Martin.** 10/10 failures, unchanged with the harness sandbox disabled;
one layer down, `holon-filesystem` `change_source::tests` fails exactly the
three tests needing a delivered event and passes all 11 pure-classification
ones; re-running with `TMPDIR` inside the repo fails identically, so it is
the process that receives nothing, not the watched location. This lane
touches neither file.

## Root cause

task-#27 org-origin lane, found by the instrumentation lane reporting
`file_watcher::tests::test_file_watcher_detects_changes` as a 3/3
"FSEvents-timeout flake" (`lane-logs/instr-holes-unit2-46650.log`): **it is
not a flake and not scoped to that test — NO real filesystem event is
delivered to any test process this agent session launches, so the entire
real-watcher rung is dead here while it passes for Martin.** Measured: 10/10
failures of the org-level test (never 1-in-N); still fails with the harness
sandbox disabled; and the failure is present one layer DOWN, at the source,
where `cargo nextest run -p holon-filesystem --lib -E 'test(change_source)'`
fails exactly the three tests that need a delivered event —
`notify_watcher_delivers_events_after_arm`,
`a_live_in_vault_rename_still_arrives_as_one_atomic_rename`,
`a_live_write_back_after_a_move_out_does_not_rehome_the_moved_page` — while
all 11 pure-classification tests in the same module pass. Not path-dependent
either: re-running with `TMPDIR` inside the repo instead of `/var/folders`
fails identically, so it is the PROCESS that receives nothing, not the
location it watches (macOS FSEvents delivery to the process tree an agent
spawns; Darwin 25.6). REFUTES the flake framing and REFUTES any suspicion of
this lane — `crates/holon-orgmode/src/file_watcher.rs` and
`crates/holon-filesystem/src/change_source.rs` are untouched by it.
ENVIRONMENT because the interaction is generatable and the code is fine:
what differs is the TEST's platform between runners. The escape that matters
is not the watcher — it is that these three tests are a verdict-by-runner
signal, so every agent lane reads a false red and learns to discount it. NOT
FIXED: the remedy is a fail-loud availability probe that
SKIPS-with-disclosure when the process demonstrably receives no FSEvents,
rather than asserting into a timeout — filed as the open follow-up,
deliberately not written blind in this lane.)

## Missing piece

No availability probe distinguishes "this process receives no FSEvents" from
"the watcher is broken", so the tests assert into a timeout and produce a
verdict-by-runner that trains every agent lane to discount a red.

## Remedy

NOT FIXED — remedy is a fail-loud probe that skips WITH DISCLOSURE when the
process demonstrably gets no events; deliberately not written blind here.
Flake framing REFUTED.
