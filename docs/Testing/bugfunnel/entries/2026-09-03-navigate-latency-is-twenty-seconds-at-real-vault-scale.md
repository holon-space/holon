---
id: 2026-09-03-navigate-latency-is-twenty-seconds-at-real-vault-scale
date: 2026-09-03
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  On a 2257-block vault, navigate interaction-to-visible measures p50 10.7s /
  p95 19.9s and set_field p95 634ms, against a 200ms SLO.
---

## Bug

Found by exploratory dogfooding (lane `dogfood-explore`) against a copy of
Martin's real vault: 131 documents, 2257 blocks, 128 pages.

`scripts/measure_latency.py` over the session log (`RUST_LOG=…,holon_latency=debug`):

    PROD END-TO-END  interaction -> visible (stage=e2e)
    action        n     p50      p95      max
    navigate      2  10748.5  19938.0  20959.0   ms
    set_field     7    323.0    634.0    664.0   ms

    PIPELINE STAGE COST
    projection (full pass)      21   337.0  23381.0  63951.0  ms
    projection (snapshot only)  21   246.0   5162.0  22660.0  ms

    BOOT INGEST
    boot_ingest_total            1          159888 ms  (2m 40s to first usable state)
    boot_file                  131    39.0   1167.5   85466.0 ms

The SLO is p95 interaction -> projection-visible < 200ms. `set_field` misses it
by 3.2x; `navigate` misses it by 100x. Driving the app over MCP required a
three-second settle after every navigation before `describe_ui` returned the new
page, which is the same effect seen from the other side.

Disclosure: measured while four parallel Rust builds saturated the machine, so
the absolute numbers are inflated. The magnitudes are not explainable by load
alone — a single file taking 85s to ingest and a 21s full projection pass are
structural, and `navigate` sample count is only 2, so treat its p95 as
indicative rather than precise. A quiet-machine re-measure is the first
follow-up.

## Root cause

The stage attribution points at the projection: `projection (full pass)` p95 is
23.4s and the doc-size line reads `blocks p50=2276 max=2276 (full-document DFS
snapshot per commit)`. Each projection pass walks the whole document rather than
the changed subtree, so cost scales with vault size instead of edit size. Three
of 21 passes were full reseed walks attributed to `coldboot`; the remaining 18
took the incremental path and still produced a p50 of 337ms.

## Missing piece

`inv-settle-budget` and `inv-sql-budget` exist but are class-3 temporal checks
that a one-shot live sweep cannot score — `run_self_checks` on the live app
skips both, and 33 of 34 invariants in total, because the live snapshot hosts
only `SutBackend`. So no gate scores end-to-end latency at real-vault scale: the
keystone runs at fresh-boot scale where single-digit milliseconds are expected
and the budget invariants never engage against 2257 blocks.

## Remedy

Open. Re-measure on an idle machine to get defensible numbers, then give the
latency budget an oracle that runs at vault scale rather than fixture scale.
