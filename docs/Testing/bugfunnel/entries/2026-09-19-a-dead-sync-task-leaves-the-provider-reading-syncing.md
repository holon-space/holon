---
id: 2026-09-19-a-dead-sync-task-leaves-the-provider-reading-syncing
date: 2026-09-19
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  When the Todoist sync task died mid-sync the `integration_state` row stayed on
  `Syncing` indefinitely, so the sidebar shows an in-progress spinner for a
  provider whose replica will never be written again.
---

## Bug

Found by the `dogfood-integ` lane, same session as
[[2026-09-19-todoist-full-sync-mirror-divergence-kills-the-replica]]. Sandbox
MCP port 8720, real credentials.

The Todoist sync task panicked at 20:12:53.704668. Eight minutes later the
mirror still read:

```
provider_name   status
claude-history  Connected
gcal            Connected
github          Pending
gmail           Unavailable
shopping        Unavailable
todoist         Syncing
```

`Syncing` is a transient. Nothing moves it: the task that would have written
`Connected` is gone, and nothing writes `Failed` either, because the failure
took the form of a panic rather than an `Err` the registry could record. The
sidebar row therefore reads "mid-sync" forever, which is the most reassuring
thing it could possibly say about a replica that lost 83 of its 96 rows.

By contrast `gmail` and `shopping`, which failed at CONNECT time and returned
`Err`, both reach `Unavailable` and both raise a toast. The difference is not
severity — it is only where in the pipeline the failure happened.

## Root cause

The status machine has no terminal state for "the sync task stopped existing".
Statuses are written by the code paths that complete; a path that unwinds writes
nothing, and `Syncing` is whatever the last completed write left behind. There
is no watchdog, no deadline, and no `Drop`-based transition.

This is the mirror image of the escape
`2026-08-18-integrations-section-shows-one-stale-row`, which
`crates/holon-integration-tests/tests/frontend_suite/integration_state_boot_records_status.rs`
closed for `Pending`: that rung asserts every enabled provider leaves boot with
a status that is not `Pending`. `Syncing` is the same defect wearing a different
label, and the rung's filter `s == "Pending"` does not catch it.

## Missing piece

No invariant says a non-terminal integration status must be transient. The
existing boot rung names one forbidden value literally instead of asserting the
property — that after boot settles, every enabled provider rests in a TERMINAL
status (`Connected`, `Unavailable`, `Failed`), never a progress one.

## Remedy

Open. Widen the invariant in `integration_state_boot_records_status.rs` from the
literal `Pending` to the set of non-terminal statuses, and give the registry a
terminal write on the failure path so a dead sync task records `Failed` with its
cause rather than leaving the last progress value standing.
