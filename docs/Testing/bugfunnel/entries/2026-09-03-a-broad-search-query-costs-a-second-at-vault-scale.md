---
id: 2026-09-03-a-broad-search-query-costs-a-second-at-vault-scale
date: 2026-09-03
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  A one- or two-character search query costs 0.8-5.3 s on a 2257-block vault
  depending on machine load, far over the 200 ms interaction SLO, because cost
  scales with rows RETURNED and the two branches return their full 20 + 30
  LIMIT.
---

## Bug

Found by measurement in lane `search-fix` while fixing
`2026-09-03-quick-open-search-returns-no-matches-for-every-query`. It is a
distinct defect from that one: that entry's `JOIN` made every query slow, this
one survives its fix and affects only broad queries.

On the 2257-block / 129-page fixture, after the `JOIN` fix
(`crates/holon-app/tests/quick_open_search_at_vault_scale.rs`):

    query "S"      50 hits (20 pages + 30 content)   806 ms
    query "Su"     50 hits                           784 ms
    query "Sup"     1 hit                            106 ms
    query "Supp"    1 hit                            107 ms
    query "Suppe"   1 hit                            106 ms

The first two characters of any word are therefore over the 200 ms SLO, and they
are exactly the characters a user types first. It is not warm-up: an untimed
search runs before the loop and the pattern is unchanged.

## Root cause

Cost scales with rows RETURNED, not rows scanned, and it is inside the SQL, not
in the Rust that follows it. Timing the content branch alone through
`DbHandle::query`, same table and same scan:

    ... LIKE '%S%'      -> 30 rows in 408 ms
    ... LIKE '%Suppe%'  ->  1 row  in  55 ms

Same predicate, same 2257-row scan, 7x the time for 29 more rows — roughly
12 ms per returned row. So the scan is cheap and per-row materialization is
expensive; two branches at 20 + 30 rows put a broad query near a second. The
label column is the suspect: the content branch selects `b.content` in full for
every hit.

## Missing piece

ENVIRONMENT: the keystone runs on a vault of a few dozen blocks, where 50 hits
are not reachable and a per-row cost of 12 ms is invisible. Nothing in the
headless suite runs search at a vault scale where returned-row count and stored
content length are realistic — the defect is structurally unreachable there,
which is why it took a hand-built 2257-block fixture to see it.

Secondary COVERAGE: search had no latency assertion at all until this lane; the
bound now in `quick_open_search_at_vault_scale.rs` is `JOIN_REGRESSION_BOUND`,
6 s, which pins the JOIN regression and does NOT hold search to the 200 ms SLO.
That gap is this entry.

The 6 s bound is set that high because the cost this entry describes is itself
load-sensitive on a shared machine. The same one-character query on the same
fixture measured 0.81 s, 1.09 s and 1.10 s on quiet runs and 5.29 s on a run
under heavy concurrent compilation — a range of 0.8-5.3 s. A bound anywhere
inside that range flaps; the JOIN regression it guards cost 6-10 s, so 6 s sits
below that floor and above every load-driven measurement seen so far.

## Remedy

Open. Do not tighten the existing budget before the cost is fixed — that would
turn a known-slow path into a red gate. Likely directions, cheapest first:
truncate the label in SQL (`substr(b.content, 1, N)`) so a hit carries a display
snippet rather than a whole block, which the modal truncates anyway; then
re-measure per-row cost before considering the LIMITs. When it lands, lower
`JOIN_REGRESSION_BOUND` to the SLO and let this entry's numbers be the red-first
proof.
