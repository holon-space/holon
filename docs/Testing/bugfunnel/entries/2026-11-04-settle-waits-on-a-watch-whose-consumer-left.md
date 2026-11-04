---
id: 2026-11-04-settle-waits-on-a-watch-whose-consumer-left
date: 2026-11-04
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  The keystone settle waited 30 s for a view watch whose consumer had already
  gone to apply a batch routed to it, and failed on stage cdc-applied.
---

## Bug
The integrate run of the settle-target change went red in the keystone tour at
step 122 of 156 (AuthorSelfPageRule):
`stage cdc-applied at applied seq 0, target sent seq 620 to view watch_view_d754af05c2d07c6f`,
raised by the settle in
crates/holon-integration-tests/src/pbt/composed/wide_e2e.rs:298.
The lane's own checks did not run the tour.

## Root cause
A reactive watch hands its consumer a chain of forwarding tasks: the demux
sends to the snapshot forwarder, which sends to `enrich_stream`
(crates/holon/src/api/ui_watcher.rs:605), which sends to the consumer. When
`unwatch` aborts the consumer (crates/holon-frontend/src/reactive.rs:3811), the
enrich task notices only on its next send. So the next batch for the view is
still accepted by the demux and counted as routed on the watch's
`DeliveryCursor`; then the chain closes behind it. The demux drops its
subscriber only on a later batch for the same view, and it holds the cursor
until then. `MatviewManager::settled_up_to`
(crates/holon-turso/src/matview_manager.rs:825) reported that cursor as
lagging, although no consumer was left to apply the batch. The settle then
waited for a batch nobody would apply.

## Missing piece
No settle test closed a watch's consumer before a write; the lane checks
never ran a scenario that opens and drops watches between writes.

## Remedy
`settled_up_to` counts a subscription only while its stream is still read
(the delivery entry keeps a weak sender, crates/holon-turso/src/matview_manager.rs:684).
Red-first test
`pbt::convergence::tests::a_watch_whose_consumer_left_does_not_hold_the_settle`
(crates/holon-integration-tests/src/pbt/convergence.rs:610): red before the
fix with `stage cdc-applied at applied seq 0, target sent seq 1`, green after.
