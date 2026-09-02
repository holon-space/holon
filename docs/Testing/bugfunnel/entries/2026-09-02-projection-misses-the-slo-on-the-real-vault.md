---
id: 2026-09-02-projection-misses-the-slo-on-the-real-vault
date: 2026-09-02
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  On a copy of Martin's real 6.5 GB vault the app's own latency oracle fires 61
  times; a single-file recipe re-ingest projects in 295–335 ms against the
  200 ms budget and boot projection reaches 12.4 s.
---

## Bug

Found dogfooding the kitchen feature on a copy of Martin's real vault (lane
`kitchen-dogfood`), launched with `holon_latency=debug` so the stage events
emit. The app's own SLO oracle raised 61 diagnostics in one session.

Post-boot, after editing one ingredient quantity in one `.cook` file:

| Stage | Measured | Budget |
|---|---|---|
| projection (recipe re-ingest, 2270 blocks) | 295 ms | 200 ms |
| projection (recipe re-ingest, 2275 blocks) | 335 ms | 200 ms |
| projection (`.cook` entity_keyed) | 304 ms | 200 ms |
| boot_parse (one recipe file) | 1227 ms | 200 ms |

At boot the same stage is far worse — one `projection` of 12371 ms and another
of 5050 ms, plus `matview_ddl` at 4683 ms. Those are boot-scoped and less
directly user-facing, but the 295–335 ms figures are not: they are the delay
between changing a recipe on disk and the change being visible, on the vault
Martin actually uses.

Evidence: `kd-logs/app.log` in the lane scratchpad
(`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/kd-logs/app.log`),
`grep 'latency-slo diagnostic'`.

## Root cause

Not diagnosed here; this entry records the measurement, not the mechanism. The
shape of the numbers points at projection cost scaling with the vault's block
count rather than with the delta: the two post-boot breaches report 2270 and
2275 blocks for an edit that touched one ingredient in one file, and they sit
just over budget at a scale where the test vaults sit far under it.

That is the ENVIRONMENT signature exactly — the budget holds at test scale and
fails at vault scale, so no amount of work on the test vault would have found
it. Compare [[latency-next-dominator-org-writeback]], which named the O(N)
shape in the neighbouring write leg.

## Missing piece

No gate measures latency against a vault of Martin's size. The oracle exists
and fires correctly — it is the only reason this was seen — but it only fires
where someone runs the app on real data, which is the dogfood channel, by hand.
`scripts/measure_latency.py` recipes run against seeded vaults orders of
magnitude smaller.

## Remedy

OPEN. Two separable pieces: find whether projection is O(vault) rather than
O(delta) on this path, and give the funnel a standing large-vault latency
measurement so the number is tracked rather than rediscovered. The second is
the cheaper and more durable of the two, and should come first — without it the
next measurement is another accident.
