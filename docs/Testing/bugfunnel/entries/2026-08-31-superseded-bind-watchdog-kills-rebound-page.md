---
id: 2026-08-31-superseded-bind-watchdog-kills-rebound-page
date: 2026-08-31
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A second `reset_vault` landing inside a rebind's pre-first-envelope window
  left that bind's boot watchdog armed, and ten seconds later it replaced the
  healthy rebound page with the "Boot failed" recovery card and an empty DOM.
---

## Bug

Found by adversarial verification of the page rebind-on-reset change
(2026-08-16-rebuilds-browser-worker-engine-leaves-live), not by a test. Boot the
served page, issue two `reset_vault` calls back-to-back over the MCP relay, then
idle: the page reaches `ready` on the newest engine, and ~10s later flips to
`data-boot-state="failed"` showing "root-layout watch produced no projection
within 10000ms" with zero rendered nodes. The engine is healthy throughout — it
holds all 24 blocks of the fresh vault. Only the page dies.

Reproducible 3/3 whenever a reset lands inside a rebind's pre-first-envelope
window. Negative controls stay healthy: one reset then 25s idle, and two resets
spaced 300ms apart.

## Root cause

A bind (`bind_root_view`, frontends/dioxus-web/src/main.rs) is a chain of awaits
plus two callbacks, and a reset can land anywhere inside it. Each bind armed its
own watchdog over a private `Rc<Cell<bool>>` that only that bind's own snapshot
callback could flip. Nothing made a superseded bind inert.

Reset #2 tore its engine down before the gen-2 bind's subscription ever
delivered, so that bind's flag stayed `false`. The pump then rebound to gen 3
and that bind went `Ready` — and the orphaned gen-2 watchdog fired afterwards.
`boot_state` is a `Signal`, so the last writer wins regardless of which bind it
belongs to. Timing is unambiguous in the repro log: the failure lands 9.91s after
the gen 2→3 line, earlier than the live bind's own watchdog could fire, and that
bind demonstrably delivered.

The same shape had two more instances: a superseded bind could report its own
`Failed`/`NoRootLayout` over the live page, and a swap during the BOOT bind let
two binds both write `watch_handle`, so the loser's page-side `on_snapshot`
listener became unremovable.

## Missing piece

No rung reset the vault twice with the second reset overlapping the first
rebind, and none stayed on the page long enough to outlive the 10s watchdog —
`web_arm_reset_vault_rebinds_the_live_page` resets once and asserts within ~3s.
A rung that finishes before the watchdog can fire cannot see this class of bug
at all.

## Remedy

Every bind now carries an epoch (`claim_bind_epoch` / `bind_is_current`,
frontends/dioxus-web/src/main.rs). A caller claims the next epoch before
touching shared state, which makes every bind already in flight inert; each
continuation — the watchdog, the snapshot callback, the handle publish, and
every failure report — checks its epoch first and does nothing when superseded.
That closes the family rather than the watchdog instance.

Pinned by `web_arm_superseded_bind_cannot_kill_the_rebound_page` (two
back-to-back resets, then idle past the watchdog window), with
`web_arm_spaced_resets_keep_the_page_alive` as the negative control that keeps
the rung honest about the overlap being the trigger. Red-for-the-right-reason
on the pre-epoch page: the overlap rung failed with the watchdog message and
"the engine is healthy and holds 24 blocks" while the spaced control passed in
the same run.

## Residuals

A superseded bind whose `engineWatchView` already returned exits without calling
`drop_subscription`, so it leaves one orphaned drain task in the worker. Bounded
at one per superseded bind and wiped by the next swap's `subscriptions::clear()`,
and the zero-ERROR smoke cannot see it. The cheap fix is to drop the
subscription on that exit path instead of only declining to publish its handle.

`web_arm_spaced_resets_keep_the_page_alive` failed 1-of-5 against a heavily
loaded server, with the inverse signature and unreproduced; its diagnosis was
lost because the run's output had been tailed. The rung's `bail!` prints the
engine-vs-page comparison that identifies which side failed, so a gate runner
must keep its FULL output — tailing this rung throws away the only evidence that
distinguishes a real regression from load.
