# Scale-soak results

Repeatable vault-scale latency + resource soak. See `DEVELOPMENT.md` → "Scale Soak" for
the full description and `just soak` invocation.

## What lives here

Each `soak-<size>-blocks-<timestamp>.txt` is one run's report:

- per-action-type end-to-end latency (`stage=action_total`): count / p50 / p95 / max / mean
- per-stage cost (projection full pass, projection snapshot, CDC rows) + dominator line
- projection doc size (blocks) — confirms the vault actually scaled
- the p95 < 200ms **SLO gate** verdict (`LATENCY GATE PASSED/FAILED`)
- RSS start / peak / end / growth (MB)

## How to run (nightly)

```bash
just soak            # 5000 blocks, ~320 actions
just soak 10000 480  # 10k blocks
```

Commit the produced `soak-*.txt`. To spot a regression, diff the newest against the
previous committed run of the same size — watch the worst-action p95 and the RSS growth.

## The scale GATE, as distinct from this report

`just soak` is a REPORT: it drives the random keystone, prints a table, and ends
in `|| true` so it can never fail a build. `just latency-scale-gate` is the
gate version of the same axis, and the two differ everywhere that matters to a
gate:

| | `just soak` | `just latency-scale-gate` |
|---|---|---|
| drive | random keystone draws | fixed transition list, `hand-authored-regressions/latency-scale.jsonl` |
| navigations | whatever the draw produces | 32, each to a DISTINCT never-visited page |
| judged against | one global p95 threshold | `e2e.p50.navigate` vs the 200 ms SLO, `docs/Testing/latency-scale-ceilings.txt` |
| can fail a build | no (`\|\| true`) | yes |
| seed proved | no | yes — the SUT's own live block count, asserted at boot |
| busy host | scored anyway | exit 3 UNJUDGED, never a red |

The distinct-page requirement is the whole point. Navigation mints one
materialized view per first-visited block, so a repeat visit re-uses a cached
view and costs nothing like the first one. A workload that revisits pages is
structurally blind to the cost, which is why the existing latency ratchet
(three documents, cycled) never saw it.

```bash
just latency-scale-gate              # 1600 blocks / 32 pages — the gate
just latency-scale-gate 204 6        # ~200 blocks / 34 pages — the small-corpus control
```

Both use the same default `settle_ms`, and they have to: it is the third
argument, it caps every harness convergence deadline, and two sizes taken
under different caps are not comparable. It also has to be large enough for
the seed — at 3200 blocks a 30 s cap never resolves the boot's Loro sync
handle and the run dies. It is a CAP, not a sleep, so raising it costs nothing
on a vault that settles.

Wall time on the dev host: about 2.5 minutes of test time at 1600 blocks
(35 s boot ingest), about 4 minutes at 3200.

**A busy host yields exit 3 UNJUDGED, not a red.** Every rung here is
wall-clock, and the same statistic moves several-fold with host load. The
recipe reads the load average before and after the run and refuses to score a
run above `max_load`. Note it does NOT use the sibling gate's contention
screen: that covariate is the mean `matview_ddl` duration, which at soak scale
is dominated by the very mints this rung exists to score.

**This gate is RED at scale on an unmodified tree, and GREEN on the small
corpus.** That contrast is the point — a rung red at every size would say
nothing about scale. It is the covering rung
for `docs/Testing/bugfunnel/entries/2026-09-19-navigation-costs-six-seconds-on-the-real-vault.md`,
authored before the fix so the fix has something to turn green. Do NOT add it
to `landing-gate` until that entry closes; the ceilings file says the same.

## Caveats (what the numbers do NOT include)

- final GPU paint (headless run, no window)
- real on-disk file-watcher churn (vault seeded once, not re-written mid-run)
- multi-peer CRDT sync/merge latency (single in-process peer)
- platform differences (dev host only)

## First measured run + scale findings ledger

See `SCALE_FINDINGS.md` for the first clean measured table (500 blocks, settle=60s,
CRDT on) and the classified scale-blocker ledger. TL;DR: the pipeline works
end-to-end at 500 blocks under CRDT; SplitBlock p50=132ms / p95=194ms; the
**dominator is the full-document DFS projection snapshot per commit (~95% of action
wall)**, which scales with vault size and is the real p95>200ms SLO breach cause —
a prod-bug candidate, not harness tuning.

## Reporting bug (harness)

The `just soak` recipe prints `action_total events: 0` because it greps the raw
log for the literal `stage=action_total`, but the tracing output renders it as `stage=<ANSI>"action_total"<ANSI>` (ANSI escapes around `=` plus surrounding quotes)
escapes (`stage<ESC>[2m=<ESC>[0m"action_total"`). The Python analyzer
(`measure_latency.py`) strips ANSI and parses correctly, so the table itself is
right — only the count line and the recipe's own gate short-circuit are affected.
Fix: pipe the log through an ANSI stripper before `grep -c`, or count via the
Python parser.
