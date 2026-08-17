---
id: 2026-07-30-new-pbt-structurally-blind-production-sidebar
date: 2026-07-30
gap: ORACLE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  The new `frontends/gpui/tests/sidebar_disclosure_affordance.rs` PBT is
  structurally blind to the production sidebar it claims to certify. Both
  observables — `expand_toggle_id_for(target_id)` and
  `disclosure_halo_id_for(target_id)` — are registered ONLY when the tree_item
  VM carries an explicit `target_id` prop: in `tree_item.rs::render` the halo
  id is `explicit_target.as_deref().map(disclosure_halo_id_for)` and the
  `TransparentTracker` wrap sits inside `if let Some(target_id) =
  explicit_target`. Production sidebar rows are built by the org-tree path and
  carry ONLY `depth` and `has_children` — verified live on all 27 rows via
  `describe_ui format:"json"`, `target_id` absent on every one — so in prod
  NEITHER the chevron NOR the halo is in the bounds registry. The test is
  green solely because its synthetic rows stamp `target_id` themselves;
  removing the halo from the production path would leave it green. The
  `view_mode_switcher` root the test replicates IS faithful (prod reaches it
  via `wrap_in_query_source_switcher`, result+source modes, because
  `left_sidebar::src::0` is a query source) — the divergence is at the ROW
  level, not the root.
source_line: 1121
---

## Bug

The new `frontends/gpui/tests/sidebar_disclosure_affordance.rs` PBT is
structurally blind to the production sidebar it claims to certify. Both
observables — `expand_toggle_id_for(target_id)` and
`disclosure_halo_id_for(target_id)` — are registered ONLY when the tree_item
VM carries an explicit `target_id` prop: in `tree_item.rs::render` the halo
id is `explicit_target.as_deref().map(disclosure_halo_id_for)` and the
`TransparentTracker` wrap sits inside `if let Some(target_id) =
explicit_target`. Production sidebar rows are built by the org-tree path and
carry ONLY `depth` and `has_children` — verified live on all 27 rows via
`describe_ui format:"json"`, `target_id` absent on every one — so in prod
NEITHER the chevron NOR the halo is in the bounds registry. The test is
green solely because its synthetic rows stamp `target_id` themselves;
removing the halo from the production path would leave it green. The
`view_mode_switcher` root the test replicates IS faithful (prod reaches it
via `wrap_in_query_source_switcher`, result+source modes, because
`left_sidebar::src::0` is a query source) — the divergence is at the ROW
level, not the root.

## Root cause

the new `sidebar_disclosure_affordance.rs` GPUI PBT cannot see the
production sidebar it certifies. Both of its observables —
`expand_toggle_id_for(target_id)` and `disclosure_halo_id_for(target_id)` —
are registered in the bounds registry ONLY when the tree_item VM carries an
explicit `target_id` prop (`tree_item.rs`: `halo_id` is
`explicit_target.as_deref().map(disclosure_halo_id_for)`, and the
`TransparentTracker` wrap is inside `if let Some(target_id) =
explicit_target`). Production sidebar rows carry ONLY `depth` and
`has_children` — verified live on all 27 rows via `describe_ui format:json`
— so in prod neither the chevron nor the halo is registered at all, and the
test is green solely because its synthetic rows stamp `target_id`
themselves. Deleting the halo from the production path would keep the PBT
green. The `view_mode_switcher` root the test replicates IS the real shape
(prod wraps the sidebar via `wrap_in_query_source_switcher`, result+source
modes, because `left_sidebar::src::0` is a query source) — the divergence is
at the ROW level, not the root. Remedy: stamp `target_id` on the org-tree
tree_item path so prod and test share one observable, or register the
chevron/halo under the row's entity id when no explicit target exists.)

## Missing piece

One shared observable across test and prod: either stamp `target_id` on the
org-tree tree_item path, or fall back to registering the chevron/halo under
the row's entity id when no explicit target exists (the `id` fallback the
same function already computes for collapse persistence).

## Remedy

FIXED-in-same-land 2026-07-30 — gate-integrity finding from the dogfood
pass; per the dogfood-explorer contract it sent the feature back, and the
shared observable lands in the SAME change as the affordance. The affordance
itself renders correctly in prod (screenshot-verified), so this is a
test-blindness defect, not a user-visible one.
