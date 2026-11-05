---
id: 2026-11-05-row-update-lost-after-first-delivery
date: 2026-11-05
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A row value set from another thread right after a `Mutable` subscription
  delivered its first value never reaches the subscriber, so a row's text
  stops updating until the next change.
---

## Bug
- `slot_reconcile::s1_b_a_row_update_still_reaches_the_slot_root_after_a_rebuild`
  failed 2 of 20 runs under host load with `left: Some("first") right: Some("before-rebuild")`.
  It was filed as a fixed-sleep flake.
- Replacing the fixed sleep with an event wait (5 s hang deadline) still failed 2 of 50 runs:
  the prop stays `"first"` for 5 s, so the update is lost, not late.
- Found by the lane that replaced the fixed-sleep settles in `crates/holon-frontend/tests/`.

## Root cause
- futures-signals 0.3.34, `MutableSignalState::poll_change` (lines 102-116 of its signal/mutable.rs):
  it checks the changed flag before it stores the waker.
  - A `set` on another thread between those two steps raises the flag and
    takes the waker slot.
  - Right after a `Ready`, the slot holds no waker, so nobody is woken.
  - The poll then stores its waker and returns `Pending` with the flag still raised.
- Holon reaches this from the text builder's subscription
  (`crates/holon-frontend/src/shadow_builders/text.rs:102`), polled on a runtime
  worker while row updates are set from other threads.
- Evidence, in `target/slot-reconcile-settle-poll/` of the lane workspace:
  - `diag-lost-update-per-process-history.log`: 31 of 300 fresh processes lost the
    update.
    - The content history was `["first"]` with one live task.
    - The next `set` revived it.
  - `repro-sweep-published.log` and `repro-sweep-published-again.log`: the standalone
    futures-signals + tokio repro lost 26 and 19 of 9300 trials.
    - The losses concentrate where the `set` lands about 10 µs after the spawn,
      during the poll after the first `Ready`.
  - `repro-sweep-patched.log`: the same sweep, with the waker stored before a final
    check of the flag, lost 0 of 9300.

## Missing piece
- No test sets a row from another thread inside the microsecond window after a
  subscription's first delivery.
- The keystone's settle and the old fixed sleeps let a later change revive the
  subscription, and that later change hides the lost update.

## Remedy
- `.cargo/patches/futures-signals/` vendors 0.3.34 with upstream's elided-lifetime
  fix and checks the changed flag again after `poll_change` stores its waker;
  the root `Cargo.toml` points `[patch.crates-io]` at it.
- Regression test:
  `mutable_cross_thread_set::a_set_during_the_subscribers_poll_is_not_lost`
  (`crates/holon-frontend/tests/mutable_cross_thread_set.rs`).
  - Its waker's clone hook calls `set`, so the set lands in the window on every run.
  - It fails with the published crate and passes with the patch.
- Upstream fix and the same test: https://github.com/Pauan/rust-signals/pull/95.
  Drop the `[patch]` entry once a release contains it.
