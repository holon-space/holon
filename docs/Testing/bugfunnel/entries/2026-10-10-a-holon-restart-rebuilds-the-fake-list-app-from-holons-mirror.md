---
id: 2026-10-10-a-holon-restart-rebuilds-the-fake-list-app-from-holons-mirror
date: 2026-10-10
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  The keystone's remote-list fixture rebuilt the fake list app from Holon's
  mirror on every Holon restart, so a tick made only in Holon became an app
  tick that no pull had seen; the app's later un-tick then failed to lower
  Holon's tick. Production cannot reach that state; the app now outlives the
  restart.
---

## Bug
The verifier of lane shopping-count-pull (holon-space/holon-work#109) replayed
four steps through the hand-authored harness: the app serves `item0` active,
Holon ticks it (never pushed), Holon restarts, the app un-ticks it.
`inv-remote-list-mirror-matches-ref` went red: the mirror kept `checked = 1`
where the reference expected `0`.

## Root cause
The fixture, not the product. On reboot, `RemoteListSut::boot` rebuilt the
fake app from the mirror table, and the reference model did the same
(`reboot_reseeds_peer_from_mirror`). After a Holon-only tick that invents a
tick in the app that no pull observed, so `remote_base.checked` stays `false`.
The reconciler lowers a latch only when the merge base records the peer's own
tick (`crates/holon-connections/src/reconcile.rs:197-201`); with no such
record, keeping Holon's tick is the correct three-way result.

In production the app is a separate process that keeps its own list across
Holon's restarts, and every tick a pull sees lands in `remote_base` (insert
path `mirrored_columns`, `reconcile.rs:128,149`; live-row path
`reconcile.rs:211-218`). An app tick unseen by any pull is therefore never
followed by a lowering Holon could attribute to the app.

Red before the fixture fix: hand-authored case
`remote-list-the-app-keeps-its-list-across-a-holon-restart` red at
`inv-remote-list-peer-matches-ref` (the rebuilt app held `item0` ticked where
the app kept it active).

## Missing piece
`Reboot` is off in randomized keystone draws unless `HOLON_PBT_REBOOT=1` (and
a weight) is set, and no hand-authored case paired `Reboot` with a remote-list
step. The fixture's restart semantics were therefore never exercised, and its
"the two were in sync before the restart" assumption was false exactly after
`TickLocally`.

## Remedy
- The fake app lives on `HeadlessFrontendComponent` (`remote_list_app`), so it
  outlives every reboot of that component; `RemoteListSut::boot` takes it from
  there and the mirror-based rebuild is deleted. The reference model keeps its
  app across a reboot too.
- Two hand-authored cases pin the restart: the case above, and
  `remote-list-an-item-un-ticked-in-the-app-after-a-holon-restart-is-active-in-holon`
  (the production-reachable variant: the pull records the app's tick before the
  restart, so the app's un-tick after it lowers Holon's tick). Both green.
- Reboot stays off in default randomized draws (cost of a full re-boot per
  draw, see `transitions/reboot.rs`). The lane ran one weighted randomized pass
  with `HOLON_PBT_REBOOT=1 HOLON_PBT_REBOOT_WEIGHT=20` and RemoteListSync
  boosted to draw both together. One case drew `Add`, `Reboot`, then six
  remote-list steps with no remote-list red; the run's one red is the
  registered known red `syn-real-mint`, whose shrunk sequence has no
  remote-list step. The two cases are the standing guard.
