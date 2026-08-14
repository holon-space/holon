---
id: 2026-08-14-soak-boot-poll-budget-does-not-scale-with-seed
date: 2026-08-14
gap: ENVIRONMENT
secondary: null
status: PARTIAL
summary: >-
  The composed harness polls for the Loro controller for a flat 2000ms whatever
  the seed size, while `soak_nav_latency` refuses to run below 2000 seed blocks —
  so any soak invocation omitting `HOLON_SOAK_SETTLE_MS` dies at boot and reads
  as a product failure.
---

## Bug
A hand-built soak invocation panics at boot with

```
LoroSyncControllerHandle never resolved within the boot poll budget
```

a message naming neither the knob to raise nor the seed in play. Task-#19
soak-retry lane; found by RUNNING the soak rung, where the failure was already
sitting in the tree recorded as a product "boot defect".

Measured: the §5b "corrected" command (`lane-logs/research-otel-perf.md`) omits
the variable and reproduced the panic in 6.17s
(`lane-logs/soak-t19-run2.log`); adding `HOLON_SOAK_SETTLE_MS=30000` — the value
the rung's OWN doc comment prescribes at `general_e2e_composed_pbt.rs:479` —
booted 2037 live blocks in 19.8s and ran the probe to completion
(`lane-logs/soak-t19-run3-settle30k.log`).

## Root cause
Two numbers in the same test that must agree and are set independently.
`soak_nav_latency` refuses to run below `HOLON_SOAK_SEED_BLOCKS=2000`
(`general_e2e_composed_pbt.rs:533`), while `compose_sut` polls for the
`LoroSyncControllerHandle` for `HOLON_SOAK_SETTLE_MS.max(2000)` ms and then
asserts (`composed/builder.rs:553-575`). A 2000-block seed cannot resolve in 2s,
so the default budget guarantees a boot panic for the smallest seed the rung
accepts.

`just soak` and the nav recipes pass the variable (`justfile:236,431`), so only
hand-built invocations hit it — and one did: `research-otel-perf.md` §P5
recorded the omission as a soak-scale boot failure and left H3 "NOT TESTED".

## Missing piece
ENVIRONMENT by the triage litmus: the interaction is generatable and the product
code is fine; what differs is a TEST-side timing budget that does not scale with
the seed the same test demands. Not an oracle gap — the assert fired loudly and
correctly; it simply could not be acted on, because it named neither variable.

## Remedy
PARTIALLY FIXED. The assert now reports the actual budget in ms, names
`HOLON_SOAK_SETTLE_MS` as the knob, and prints the `HOLON_SOAK_SEED_BLOCKS` the
boot ran with, so a reader of the panic can act on it without reading the
harness.

OPEN: the structural remedy — deriving the default budget from the seed size so
the two numbers cannot disagree — is deliberately NOT done here and remains the
follow-up.
