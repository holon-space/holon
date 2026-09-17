---
id: 2026-09-11-drawer-open-matches-ref-fires-in-the-reverse-direction
date: 2026-09-11
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The TUI windowed PBT reds on `inv-drawer-open-matches-ref` in the direction
  the known-red registry does NOT cover — the SUT paints the left sidebar
  CLOSED while the reference says open. ROOT-CAUSED 2026-09-17: the
  provider-stability probe leaks its narrow 500 px viewport when the engine had
  none, so every later render on that shared engine takes the layout's mobile
  `if_space` branch, where both sidebars are `overlay` drawers whose mode
  default is CLOSED. The reference (desktop-first: both sidebars `shrink`) is
  right.
---

## Bug

Found by the `slot-birth` weave gate, `holon-tui::tui_ui_pbt`:

```
crates/holon-integration-tests/src/pbt/composed/harness.rs:1390
reconciled composed sequence diverged from the oracle:
[("inv-drawer-open-matches-ref",
  "[inv-drawer-open-matches-ref] drawer block:default-left-sidebar rendered
   open=false but reference says open=true")]
```

Gate log: `.jj/sw/check.log` (nextest detail in the run's
`w-slot-birth-nextest.38224.log`). The draw was a 7-transition windowed
alphabet run (`BulkExternalAdd`, `SetEdgeField`, `CreateBlockUnderFocus`,
`InstantiateTemplate`, `NavigateFocus`, `RenamePage`, `RehomeEntity`), failing
at step 0.

## Root cause

**FOUND 2026-09-17 — an ENVIRONMENT defect in the harness, not a product
defect and not an oracle defect. The reference model is RIGHT.**

`GpuiFrontendEngineComponent::provider_stability_report`
(`crates/holon-integration-tests/src/pbt/window_slice/components.rs`; registered
by `overlay_windowed_caps`, so it runs in BOTH the GPUI windowed loop and the
TUI PBT) deliberately forces a NARROW viewport to drive the `if_space`-gated
mobile action bar
(`inv-value-fn-provider-arg-variance-13`'s `PROBE_VIEWPORT = 500x800`). Its
restore was conditional on there having BEEN a viewport:

```rust
if let Some(v) = prev_viewport {
    reactive.ui_state().set_viewport(v);
}
```

and the loading/spacer early `return None` skipped the restore entirely. The
TUI and windowed composed slices never push a viewport at all
(`grep -rn set_viewport crates/holon-integration-tests/src/pbt/` returns the
probe's two call sites and nothing else), so `prev_viewport == None` and the
probe's 500 px viewport was LEFT SET on the shared engine for the rest of the
run.

The default layout's outer `if_space(600)` then selects its mobile branch —
`bottom_dock(columns(drawer(left, overlay), main, drawer(right, overlay)), …)`
(`crates/holon-api/src/perspective.rs`) — where BOTH sidebars are `overlay`
drawers, and `DrawerMode::default_open()` is FALSE for `Overlay`. The reference
bridge (`crates/holon-integration-tests/src/pbt/layout_bridge.rs`) hardcodes
both sidebars as `Shrink` (open), which is the correct modelling of "no
viewport known": `if_space` reads that state as desktop-first, and the wide
branch makes both sidebars `shrink`. So the SUT was rendering a state no real
user would ever see, and the invariant correctly reported the divergence.

This also explains the reported shape: "failing at step 0" is the first check
tick after the probe ran, and the region is `left` while the mechanism covers
both.

`gap` is re-classed from ORACLE to ENVIRONMENT. The invariant and the reference
were both right; the environment put the SUT into an unreachable render state.

## Missing piece

**Nothing structural remains open in the headless keystone** — this half is
invisible there (`inv-value-fn-provider-arg-variance-13` is deselected without
a `SutFrontendEmissions` provider), which is why the keystone could not see it.

**DISCLOSED RESIDUAL:** evidence is strong but not a controlled A/B. Six
post-fix TUI runs (`lane-logs/tui-pbt-post-fix.log`, `lane-logs/tui-pbt-soak-{1..5}.log`)
produced ZERO drawer divergences, with `inv-drawer-open-matches-ref` selected
and green on every step — and the leaking probe
(`inv-value-fn-provider-arg-variance-13`) runs on every tick, so the leak path
was exercised throughout. Two of those six failed on UNRELATED signatures
(`inv-watch-rows-match-ref`; a `[read-only homes]` panic at
`frontend_slice/components.rs:7095`), flagged for separate triage rather than
attributed here. The filed pre-fix rate was 0/3 passing at BOTH base and tip,
i.e. the signature fired every run, but those runs drew different alphabets
than mine and no base arm was rebuilt in the fixing lane. The registry row
stays `fixed-pending-soak`, so a recurrence classifies as NOVEL.

## Remedy

**FIXED 2026-09-17** in `crates/holon-frontend/src/reactive.rs` +
`crates/holon-integration-tests/src/pbt/window_slice/components.rs`:

1. `UiState::restore_viewport(Option<ViewportInfo>)` — `set_viewport` cannot
   express the "no viewport known yet" state, so a caller could not put the
   engine back where it found it.
2. The probe's body moved to the module-level free fn `probe_provider_stability`
   and `provider_stability_report` became a thin wrapper with a SINGLE restore
   point after it, so no exit path (including the loading/spacer early return)
   can skip the restore. The wrapper comment names why: leaving the narrow
   viewport in place moves every later render on that shared engine onto the
   mobile branch.

The registry row `drawer-open-matches-ref-reverse` is flipped to
`fixed-pending-soak` with this cause.
