---
id: 2026-10-08-gpui-drops-banner-conditions-without-a-surface
date: 2026-10-08
gap: PERCEPTION
secondary: ORACLE
status: FIXED
summary: >-
  The GPUI window painted no condition whose profile places it in a banner,
  except `PairingReimportDeferred`: a previous run's panic, a task panic, a
  stuck database or a stopped view engine reached the bus and was never drawn.
---

## Bug
Found by code audit in the boot-always A1+A2 lane (round 4), while looking
for the frame at which a previous run's panic record is first seen by the
user. There was none.

## Root cause
`ShareUiState::apply_degraded` (`frontends/gpui/src/share_ui.rs`) matched
`ConditionPlacement::Banner` and handled only `PairingReimportDeferred`; every
other Banner kind fell through without a surface. Eleven kinds carry that
placement (`crates/holon-api/src/condition_profile.rs`), among them
`PreviousRunPanicked`, `TaskPanicked`, `DatabaseStuck` and
`ViewEngineStopped`. ADR 0035 (docs/Architecture/Model.md, "Conditions")
rules that a frontend with no surface for the declared placement falls back
to a toast and logs the substitution.

## Missing piece
No windowed test raised a Banner-placed condition and read what the window
painted; `degraded_bus_bridge_windowed.rs` asserts only that the bus has a
subscriber. The keystone checks the bus, not the paint.

## Remedy
A Banner kind without its own surface is drawn as a keyed toast (one per
condition, upserted on re-raise), and its kind is logged once at WARN. Pinned
by `frontends/gpui/tests/banner_conditions_draw_as_toasts_windowed.rs`; red
in `lane-logs/a12-fix4/red-banner-fallback.log`.
