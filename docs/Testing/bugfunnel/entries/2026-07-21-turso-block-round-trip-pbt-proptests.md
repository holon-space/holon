---
id: 2026-07-21-turso-block-round-trip-pbt-proptests
date: 2026-07-21
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  turso_block_round_trip_pbt: 2 of 3 proptests DEAD-RED since introduction —
  harness never seeded the doc-root block generated trees parent under, so the
  first create tripped the (correct) parent-FK guard; zero signal the whole
  time
source_line: 1065
---

## Bug

turso_block_round_trip_pbt: 2 of 3 proptests DEAD-RED since introduction —
harness never seeded the doc-root block generated trees parent under, so the
first create tripped the (correct) parent-FK guard; zero signal the whole
time

## Missing piece

seed doc root (mirrors sibling tests); guard + generator both correct,
neither weakened

## Remedy

FIXED+WOVEN 2026-07-21 (cycle 2; pbt-genfix)
