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

OPEN. The oracle now exists —
`crates/holon-integration-tests/tests/vault_scale_interaction_latency.rs` boots
a synthesized vault of this shape (120 files / 2793 blocks) headlessly and
measures both rungs — and the projection defect named above is fixed
([[2026-09-08-the-boot-scan-projects-every-file-with-a-full-document-walk]]:
121 full walks per boot became 1, snapshot p50 258 ms → 8 ms). The interaction
SLO is still missed, so this entry stays open.

What the re-measurement changed about the diagnosis (lane `nav-latency`,
2026-09-08, headless, on a machine also running three other build lanes):

The 10.7 s / 19.9 s figures came from two samples taken while four builds
saturated the machine, as that disclosure said, and they do not reproduce. On a
synthesized vault of this shape, navigate p50 measures 278-464 ms at 309-2793
blocks, and on a COPY of Martin's own vault (121 files / 2599 blocks) 322.9 ms.
So the miss is real and consistent — roughly 1.4x to 2.3x the budget — but it is
hundreds of milliseconds, not twenty seconds.

The growth law could not be pinned down, and saying so is the honest result.
Six runs of the SAME tree at 2793 blocks produced navigate p50s of 301, 322,
333, 401, 464 and 1407 ms, with boot times from 42 s to 151 s over the same
runs. That 4.7x spread on unchanged code is larger than any difference between
scale rungs, so this machine cannot separate "navigate is O(vault)" from
"navigate has a fixed floor". A quiet-machine run of
`vault_scale_interaction_latency` at three rungs would settle it, and is the
cheapest next step.

What IS settled:

1. The projection is no longer the suspect. Its Loro-walk leg is now 8 ms of a
   201 ms pass at 2793 blocks; the remaining ~193 ms is the SQL sink write
   (`consolidator.apply`). That is where the next investigation goes.
2. `set_field` is not the problem at this scale: p50 60-163 ms across rungs,
   inside the budget. Only navigate misses it.

Boot remains the other open thread: 9x blocks cost 15.6x time before the fix,
and although the fix roughly halved it, ~50 s to first usable state on 2600
blocks is still far from acceptable.
