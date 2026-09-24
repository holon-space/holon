---
id: 2026-09-24-oracle-warnings-evict-latency-violation
date: 2026-09-24
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  Five drain-estimate warnings pushed a service-time violation out of the
  oracle ledger, and the banner turned from red to amber.
---

## Bug

A verifier round on the slo-waves lane pushed one service-time violation and
then five drain-estimate warnings into `OracleStatus`. The violation was gone,
and the banner read "ORACLE WARNING (5) — a disclosure, not a failed check".
The drain warning is edge-triggered per origin, so every below/above flap of
the estimate pushes another one.

## Root cause

`OracleStatus::push_latency` (`crates/holon-oracles/src/status.rs`) kept the
newest five latency findings in one list, whatever their severity. The banner
(`frontends/gpui/src/oracles_ui.rs`, `render_banner`) is red only while a
violation is in the snapshot.

## Missing piece

No test pushed findings of both severities past the cap. The keystone never
drives the latency oracle's ledger.

## Remedy

The ledger keeps one capped list per severity, and the snapshot lists
violations before warnings. A warning can no longer evict a violation or push
it out of the banner's first lines. Test:
`status::tests::warnings_never_evict_or_bury_a_violation`, red on the single
list (`lane-logs/d1-red.log`, teeth in `lane-logs/d1-teeth/`).
