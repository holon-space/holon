---
id: 2026-08-14-flake-test-real-filesystem-event-delivered
date: 2026-08-14
gap: ENVIRONMENT
secondary: FALSE-ALARM
status: MITIGATED
summary: >-
  The real-watcher tests of holon-filesystem and holon-orgmode go red when
  fseventsd delivers nothing for a while (host-wide stall episodes), and the
  orgmode tests also went red on any delivery slower than their fixed sleeps.
source_line: 707
---

## Bug

Lanes and integrate gates that ran `holon-filesystem` `change_source::tests`
and `holon-orgmode` `file_watcher::tests` saw the real-watcher tests red,
first read as "a process an agent launches gets no events". That reading is
refuted: agent-launched lane runs pass the rename test in most runs, and the
integrate gates pass all four tests in most gates.

## Root cause

- Stall episodes: the lane reds fall in one window, across five lanes at
  once, and an integrate gate failed all four tests in one run. In such a
  window `fseventsd` delivers no event to any watcher process for seconds.
- Fixed sleeps: `test_file_watcher_detects_changes` slept 100 ms, wrote,
  slept 500 ms and then drained the channel, so every delivery slower than
  the sleeps was a red. The three negative tests passed vacuously on a late
  event.
- A Rescan (`MUST_SCAN_SUBDIRS`, dropped events) was ignored without a log
  in `notify_event_to_signals` (`crates/holon-filesystem/src/change_source.rs`).

## Missing piece

No live wait in the orgmode tests, no name for a stall red, and no
disclosure of a dropped-events Rescan.

## Remedy

- The watcher tests wait live (arm until an event arrives, nudge after each
  quiet spell, 30 s liveness budget); the negative tests end on a marker
  written after the forbidden write, with a control file that proves a late
  event is seen. A delayed relay of the real watcher draws the delivery
  delay as a property.
- A stall reads "the watch stalled" and is the known-red row
  `fs-watcher-stall` (docs/Testing/KeystoneKnownReds.md).
- A Rescan is logged at warn.
- Open: the stall episodes themselves (environment), and the product path
  in which a poll sees a rename source gone before discovery ingests the
  destination (a cascade delete, holon-work#1117).
