---
id: 2026-08-31-operations-dropped-for-entities-without-a-render-profile
date: 2026-08-31
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An entity with registered operations but no render profile had its whole
  operation set discarded at resolution, making those ops both unlistable
  and undispatchable everywhere.
---

## Bug

Found by code audit while wiring the action bar's global tier (2026-08-30),
outside any automated test.

`navigation` is a dispatcher-registered operations entity — `go_back`,
`go_forward`, `go_home` and the focus/pin/tab ops all carry
`entity_name: "navigation"` (`crates/holon/src/navigation/provider.rs:25`)
and land in the profile resolver's `entity_operations` map, which
`crates/holon/src/di/registration.rs:412-418` builds from
`dispatcher.operations()` keyed on exactly that name. But `navigation` is
not a stored TYPE, so no `EntityProfile` is derived for it
(`type_profiles_from_registry` walks the type registry, and there is no
`assets/default/types/navigation.yaml` — nor should there be, it has no
table).

`ProfileResolving::resolve_with_computed` treated "no render profile" as
"no operations": the no-profile branch returned a default `RenderProfile`
with `operations: vec![]` and returned early, in both it and
`resolve_with_variants`. Every surface that asks "what can I do to this
entity?" goes through that call — `ops_of` for listing, and
`op_button`'s tap handler, which looks the op up in
`resolve_profile(&{id: target_id}).operations` before dispatching. So the
navigation ops were simultaneously invisible and undispatchable, while
being perfectly well registered.

## Root cause

Having no RENDER profile and having no OPERATIONS are different facts, and
one branch conflated them. A render profile answers "how is this entity
drawn"; an operations set answers "what can be done to it". An entity that
is never drawn — navigation is pure app state — legitimately has the second
without the first.

## Missing piece

Nothing asked for the operations of an entity that had none registered as a
render profile, so the conflation never produced a visible symptom. The
resolver's own tests cover profile-backed entities and value rows; the
operations-only entity was an unrepresented case rather than a wrong
assertion.

## Remedy

Both no-profile branches now return `self.lookup_operations(entity_name_str)`
instead of an empty vec — the same map `materialize` reads for
profile-backed entities, so the two paths agree. The rest of the branch is
unchanged: such an entity still renders as the empty default, which is
correct, because it is genuinely not drawn.

Covered by `frontends/gpui/tests/action_bar_windowed.rs`, whose global-tier
rungs went red for the right reason before the fix (the bar painted no
`go_back` / `go_forward` / `go_home`) and now assert those ops both render
after the entity tier and dispatch on a tap.

Two collaborators surfaced alongside and are fixed with it: `op_button`
ignored the descriptor's `bound_params` (so a navigation op's pre-bound
`region` went unseen and a tap would have opened a region picker instead of
navigating), and `ops_of` returned one row per catalog entry rather than per
operation NAME, so the knowingly double-advertised structural block ops
(`SqlBlockOperations` + `LoroBlockOperations`) painted twice — the same
presentation dedup the slash menu already applies in `build_command_items`,
now applied where the rows are built so every op-row surface gets it.
