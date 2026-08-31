---
id: 2026-08-31-state-toggle-row-absent-recurs-after-its-soak
date: 2026-08-31
gap: ORACLE
secondary: COVERAGE
status: OPEN
summary: >-
  The `state-toggle-row-absent` signature, fixed 2026-08-05 and awaiting its
  soak, recurred — the keystone offered a toggle target the main panel does not
  render after a focus change, so the 2026-08-05 fix is not proven.
---

## Bug

`general_e2e_composed_pbt` failed with the signature the registry carries as
`state-toggle-row-absent` (KeystoneKnownReds.md:110), whose status was
`fixed-pending-soak`:

```
[toggle_state] click #1 failed for block:bulk-1-0: cycle_state_toggle: could not
resolve the state_toggle cycle intent for block:bulk-1-0 in region main within
2s. block:bulk-1-0 renders NO node in region main — the panel is not showing
this block. It renders 4 distinct entities: [block:368857d2-…,
block:__virtual:368857d2-…, block:default-main-panel, block:journals]
```

Found running the full workspace suite in the webpbt lane
(`lane-logs/webpbt-workspace-tests.log:18207`; minimal failing input at
:20077). Recorded here because the project's own classifier calls it novel:
`scripts/keystone-known-reds.sh` matches only `known-red` rows, and the
registry's rules (KeystoneKnownReds.md:19-24) say a `fixed-pending-soak`
recurrence reports as NOVEL precisely so the tier stops absorbing it — "the fix
is treated as believed, not proven". So this is a regression to triage, not a
pass-with-note.

## Root cause

Not fully diagnosed here; the decoded payload names the mechanism. The shrunk
transitions are:

```
[ BulkExternalAdd(doc_uri = block:368857d2-… , the journal day page)
, NavigateBack
, ToggleState(block:bulk-1-0 -> Todo) ]
```

The bulk blocks land under the journal day page; `NavigateBack` then moves focus
off it; the toggle names `block:bulk-1-0`, which the panel no longer renders.

The 2026-08-05 fix introduced `RefBlockTree::main_panel_renders`, implemented
via the panel query's own traversal, and used it for both the generator's
candidate set and `ToggleState`'s visibility precondition. That closed the
page-stop and depth-cap arms. This payload adds an arm it does not cover: the
predicate answers "does the panel render this block" **against a focus root that
`NavigateBack` has already changed**, so a target that was visible when the
candidate set was computed is gone by the time the click is dispatched.

## Missing piece

The visibility precondition is not re-evaluated against the focus in force at
dispatch time — a candidate set computed before a focus-changing transition is
stale, and nothing detects that. The soak that would have caught this never ran
to completion: the row sat at `fixed-pending-soak` for 26 days.

## Remedy

Open. Two things done here, neither a fix:

1. The registry row is reverted from `fixed-pending-soak` to `known-red` with
   this payload decoded, which is what the registry's own rules prescribe for a
   recurred fixed signature.
2. Attribution: **not caused by the webpbt lane's diff** — but not on the
   strength of an A/B. An earlier draft claimed the signature was "red with and
   without it", citing `webpbt-workspace-tests-Abase.log` vs
   `webpbt-workspace-tests.log`; counted directly, the signature appears **0**
   times in the first and **20** in the second, so that sentence misread its own
   source and is withdrawn. The two arms are not comparable anyway: the A arm ran
   with the clock change reverted, which moves the boot day and therefore the
   generated state space (see the closing note).

   The conclusion rests on the run-to-run spread instead. Six keystone runs
   across this lane and its verifier produced **five distinct signatures and at
   least one fully green run**: `state-toggle-row-absent`,
   `drawer-open-matches-ref`, `inv-editor-text/mirror`,
   `inv-main-panel-rows-match-focus` dropped-row, and a `SutOrgRender …
   structural-page.org` NotFound. A deterministic regression does not behave that
   way, and none of the five is date- or offset-shaped. Two of the six runs were
   green-or-pass-with-note, including one the classifier scored `0 novel`.

Note for whoever picks this up: the lane's `TestClock` default offset (+14h)
moves the keystone's frozen boot day from 2026-01-15 to 2026-01-16, so the
generated journal day — and the state space explored around it — is not the same
as in runs before 2026-08-31. Reference and SUT shift together, so nothing
breaks, but a signature's exact payload is not comparable across that boundary.
