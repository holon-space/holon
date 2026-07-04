# wip-misc-fixes

Source: copied out of the default workspace's stale WIP commit `xutmutsm`
(based on old `main` @ `1d20a720`, never touched again after 2026-07-15).
`xutmutsm` itself was left byte-identical — this is a non-destructive copy via
`jj restore --from xutmutsm <path>`.

## What's here

- `frontends/gpui/src/views/editor_view.rs` — adds `#[cfg(feature = "mobile")]`
  to the `focus_gen: Cell<u64>` field. On integration tip the field's *usage*
  (`note_focus_gained_mobile`/`note_focus_lost_mobile`/`focus_gen()`) is
  already mobile-gated, but the field declaration and its `new()` initializer
  were not — so non-mobile builds set it and never read it (dead-code
  candidate). Small, safe, no adaptation needed; applied cleanly.
- `scripts/squash-frontends.sh` — adds a `--from REV` flag so
  `jj squash --into <bookmark> --from <rev> -- <dir>` can target a specific
  source revision instead of always squashing the whole stack. Ops tooling,
  no adaptation needed; applied cleanly.

## Not carried forward (see orchestrator report for full DROP rationale)

Everything else in `xutmutsm` was either already landed on `integration`
byte-for-byte, pure import/whitespace/doc-style churn, a dependency-lockfile
hazard (iroh 0.96→1.0.2 / ed25519-dalek vendored-patch removal — the exact
churn `ed25519-lock-churn` in project memory warns never to do bare), or
actively regressive (would have deleted work `integration` already has:
LogSeq LATER/NOW dialect removal per the 2026-07-13 vault-compat ruling, the
`expand_toggle`/`block_expanded_view` view-state feature ratified
2026-07-16, and test coverage such as
`blocked_by_requires_edge_round_trips_through_store`).

No ADR 0024 Phase 2 / Pattern-AST content was found in `xutmutsm` at all —
the premise that it contained such work did not hold up under inspection.

Next step: run `cargo build -p holon-gpui --features mobile` and a workspace
build to confirm these two hunks still compile against current `integration`,
then describe/commit.
