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
