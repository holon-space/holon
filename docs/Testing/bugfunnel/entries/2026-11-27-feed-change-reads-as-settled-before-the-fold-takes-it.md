---
id: 2026-11-27-feed-change-reads-as-settled-before-the-fold-takes-it
date: 2026-11-27
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  The settle verdict skipped the block feed's delivery cursor unless a
  subscription had taken it, so a change routed to an untaken cursor and not
  yet rendered read as settled.
---

## Bug
- Found by TLC in the TLA+ model of the org write-back and settle
  (docs/Testing/ModelChecking/SpikeReport.md, section "The current design"),
  outside the PBT suite.
- Between a block-feed change and the `home_by` fold taking it, the fold flag
  and its consumer channel both read idle. Only the feed's delivery cursor
  shows the change as owed.
- The settle verdict (`settle_verdict`, crates/holon-api/src/delivery_ledger.rs)
  read that cursor only when a subscription had taken it. A change routed to
  an untaken cursor read as settled.
  - Red: `a_quiet_settle_read_of_an_untaken_feed_sees_the_newest_documents`
    (crates/holon-api/src/live_data/home_by/tests/supervision/settle_spec.rs),
    on its static example "a feed change the fold has not taken, then a
    settle read".
- In the production wiring a cursor is routed to only after it is taken, so
  the session settle did not reach the window there. The decision was still
  wrong for an input it accepts.

## Root cause
- The decision used "taken" as a stand-in for "something can be owed". The
  ledger lookup for a feed's view needs the take; the owed batches do not:
  every demux records what it routed, and the cursor records what was routed
  to it and what its consumer applied.

## Missing piece
- No property read the settle verdict against the newest documents while
  the test, not tokio, chose when the fold takes a change.
- The settle spec PBT now does both and replays every run against the TLA+
  spec.

## Remedy
- One rule for every cursor in `settle_verdict`: the feed is behind while a
  demux has not routed through the settle start, or the cursor lags and has
  not applied through the start. Unchanged for a taken cursor; an untaken
  cursor waits until the demux routes through start.
