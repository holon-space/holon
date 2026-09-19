---
id: 2026-09-19-tour-anchors-strip-the-scheme-and-never-resolve
date: 2026-09-19
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Tour `Entity` and `Block` step anchors looked the bounds registry up with
  `EntityUri::id()`, which strips the scheme, while every writer records the
  full URI — so every such anchor resolved to `Missing`, masked by a mock that
  recorded bare ids.
---

## Bug

Found by the VERIFIER of lane `fix-gpui-bounds-entity-id`
(`lane-logs/fix-gpui-bounds-entity-id-verify.md`) while checking whether the
lane's "the registry is keyed by the canonical URI" claim held anywhere else
in the tree. It does not hold in `crates/holon-frontend/src/tour.rs`, which
reads the registry scheme-stripped:

```rust
Self::Block(id) => Some(format!("render-entity-{}", id.id())),   // :91
AnchorSelector::Entity(id) => geo.find_by_entity_id(id.id()),    // :226
```

`EntityUri::id()` returns the local part without the scheme, so these ask for
`render-entity-page-x` and scan for an `entity_id` of `page-x`. What the
writers actually record is the full URI: `render_entity_view.rs:210` keys the
element `render-entity-{id}` with `id` the whole `EntityUri`, and every
`entity_id` in the registry is canonical. gpui's own geometry test asserts the
schemed spelling (`geometry.rs:632`, `render-entity-block:warmup`), and
`GpuiUserDriver` builds the same key from the full string
(`user_driver.rs:219`).

Consequence: every Tour step anchored on an entity or a block resolves to
`AnchorResolution::Missing`. The tour does not crash — `Missing` is a defined
outcome — it just silently fails to point at anything, which is the whole
function of a step anchor.

**This is NOT D142.a fallout.** The readers were already wrong: writers have
recorded the full URI since well before the row-id classifier landed, and
`id()` has always stripped the scheme. D142.a is how it came to light, not
what caused it.

## Root cause

A reader and a writer disagreed about the spelling of a key, and the only
test over that seam supplied the reader's spelling. The mock at
`tour.rs:444` recorded

```rust
m.insert("render-entity-page-x".to_string(), info(Some("page-x")));
```

— bare on both the key and the `entity_id`. Against that fixture the
scheme-stripping readers resolve, so `resolves_panel_and_block_anchors_and_reports_missing`
passed continuously while the production path had never worked. The `Panel`
arm of the same test uses a literal key with no entity id, so it was
unaffected and kept the test looking healthy.

## Missing piece

**COVERAGE.** The oracle was right and the fixture was wrong: the test asks
exactly the correct question ("does this anchor resolve?") and would have
caught the defect on day one against a production-shaped registry. No
invariant was missing and no new assertion was needed — the seam was simply
never exercised with the key spelling production uses.

The generalisable lesson, and the reason this is worth a row rather than a
silent fix: a hand-built `GeometryProvider` mock is a second, unchecked
implementation of the registry's key contract, and nothing forced it to agree
with `BoundsRegistry`. Every such mock is a place a reader/writer disagreement
can hide indefinitely.

## Remedy

FIXED in lane `fix-gpui-bounds-entity-id`, red-first.

1. The mock records production-shaped keys — `render-entity-block:page-x` with
   `entity_id` `block:page-x`. Red, for the right reason:
   `tour.rs:465 assertion failed: matches!(resolve_anchor(&block, &geo),
   AnchorResolution::Resolved(_))` (`lane-logs/tour-red.log`, 4 passed /
   1 failed).
2. Both readers use the full URI (`id.as_str()`). Green:
   `lane-logs/tour-green.log`, `5 tests run: 5 passed`.

Not done, and deliberately left for the orchestrator: the mock is still a
hand-written second implementation. The durable fix is for windowed seams to
assert against a real `BoundsRegistry` rather than a `HashMap` a test author
filled in, which is a harness change well outside this lane's scope.

## Relation to other entries

Same class as
`2026-09-19-gpui-fixtures-assert-pre-d142a-raw-row-ids` — a bare-id probe
against a canonical-keyed registry — but that one is a FALSE-ALARM with no
product defect behind it, and this one is a live defect in shipped code. Read
together they make the point that the bare-vs-canonical confusion had settled
on BOTH sides of the registry boundary, and only the test side announced
itself.
