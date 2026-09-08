---
id: 2026-09-08-keystone-novel-reds-only-under-machine-contention
date: 2026-09-08
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Two keystone failure shapes — SutOrgRender NotFound on structural-page.org and
  novel inv-sql-budget divergences — appear only while the machine is saturated
  by cold builds, and vanish in 20 A/B runs at the same and the parent revision.
---

## Bug

Found by two smoke lanes running the composed keystone at chain tip `7a2e0147`
while the machine carried load ≈ 57 with 12 rustc processes (two cold
scratch-tree builds in flight).

Shape 1 — 4 panics in
`scratchpad/w-mcp-authority-fix-smoke.24607.log`:

    SutOrgRender: read org file: Custom { kind: NotFound, error: "No such file or directory (in-memory): /private/var/folders/hc/2q6czxpx6j9_87bq787752jw0000gn/T/.tmpv9OkMJ/structural-page.org" }

at `crates/holon-integration-tests/src/pbt/composed/harness.rs:1206`, each with a
different temp directory.

Shape 2 — 5 novel divergences in `scratchpad/smoke-x3-3-71894.log`, classified
by `scratchpad/smoke-x3-3-kr-71894.log`:

    [known-reds] FAIL: 5 novel panic(s), 40 known-red panic(s), 0 collateral (ignored).

The novel signature is one budget short of the pinned known red:

    [inv-sql-budget] 1 budget violation(s):
      OpenTabViaModifierClick.sql_reads: 23 exceeds expected 22 + tolerance 0 = 22 (watches=0, docs=4)

against the known-red `24 exceeds expected 23 + tolerance 0 = 23` seen 40 times
in the same log.

## Root cause

Not the code. An A/B population run afterwards, on an unloaded machine, puts
both shapes at zero:

- `scratchpad/pop-tip.log` — tip `7a2e0147`, 10 runs: `POP tip: green=9 red=1 of
  10`, and the single red is `run 4: RED novel=[] [known-reds]
  PASS-WITH-NOTE: 118 known-red panic(s), 0 novel`.
- `scratchpad/pop-parent.log` — parent `5c2db13e`, 10 runs: `POP parent: green=9
  red=1 of 10`, its red likewise `run 2: RED novel=[] … 49 known-red panic(s), 0
  novel`.

So the novel rate is 0/20 under normal load at both the suspect revision and its
parent, versus 2 of the 4 contended runs. The tip's own parent behaves
identically, so nothing in the tip commit is implicated; what changed between the
green and red populations is the machine, not the tree. Both shapes are
consistent with scheduler starvation: a temp-vault file not yet visible when the
render step reads it, and a read-count budget drifting by one when settling races
resolve differently under contention.

## Missing piece

The keystone harness does not disclose machine load in its verdict, and the two
affected invariants have no contention-aware budget. A run that fails because 12
rustc processes were competing for the CPU is reported in exactly the same words
as a run that fails because the product regressed, so every lane that hits it
must spend an A/B population to tell the two apart.

## Remedy

Open. Record load average and runnable-process count in the harness verdict so a
contended run is self-identifying, and decide whether `inv-sql-budget` and the
org-render read need a contention-aware tolerance or a hard refusal to run above
a load threshold. Until then, treat a novel signature seen only on a saturated
machine as unconfirmed and re-run the A/B population before filing a regression.
