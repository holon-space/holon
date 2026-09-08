---
id: 2026-09-09-content-fidelity-exempts-a-shell-with-no-descendants
date: 2026-09-09
gap: ORACLE
status: OPEN
summary: >-
  `assert_content_fidelity` fails a `reactive_shell` that has descendants but no
  visible leaves, and EXEMPTS one with no descendants at all — so the worst case
  of the defect it exists to catch, a full-height panel that painted literally
  nothing, is the one shape it lets through.
---

## Bug

Found 2026-09-09 by the `gpui-driver` lane, outside any test, while looking for
the invariant that should have caught a main panel painting zero rows.

`crates/holon-layout-testing/src/invariants.rs:238`:

```rust
if total_descendants > 0 && visible_leaves == 0 {
```

The measured frame that motivated this is a `reactive_shell` at **912×1018** —
the whole main panel — with `total_descendants == 0`
(`lane-logs/run-full-census.out:322-395`: `reactive_shell#42` appears in the
element table and nothing lists it as `parent`). `total_descendants > 0` is
false, so no violation is recorded, so the run is green.

## Root cause

The guard is doing double duty and the two duties disagree.

`total_descendants > 0` is there to skip shells that legitimately render
nothing — an empty query result, a collapsed section. That intent is sound. But
it is implemented as "has any descendant at all", which also skips the shell
that rendered nothing **because its content never mounted** — a strictly worse
failure than the one the invariant does catch (descendants present, none
visible).

The invariant cannot currently tell "correctly empty" from "should have had
rows and has none", because it reasons only over the painted tree. The
distinguishing fact — whether the shell's data source has rows — lives in the
view model, not in the bounds snapshot the function takes.

## Missing piece

**ORACLE.** The interaction is generatable and IS generated — every windowed
test that navigates the main panel produces these frames. Nothing goes red.
This is the pure oracle case from the skill's litmus: *"If a case had hit this
state, would any invariant have gone red?"* — no.

The companion hole: `inv-main-panel-rows-match-focus`
(`crates/holon-integration-tests/src/pbt/composed/invariants/main_panel_rows_match_focus.rs`)
reads the SUT's `SutRenderer` view model, not the window's painted bounds, so it
was green throughout the frames above while the window painted nothing. Between
the two, a windowed panel can paint zero rows for a focus root that has rows and
the whole suite stays green.

## Keystone repro

Not a keystone concern — the headless keystone paints no frames. This is a
windowed-layout oracle, and the fixture that would exercise it is any windowed
test that navigates to a page with rows.

## Remedy

NOT FIXED. Two candidate shapes, for whoever takes it:

1. **Narrow the exemption.** Fail when a shell is laid out at a non-trivial size
   with no descendants at all, and let genuinely-empty views opt out by painting
   an explicit empty-state widget. This makes "I rendered nothing on purpose"
   visible in the tree instead of inferred from its absence — and an explicit
   empty state is better UX than a blank box regardless.
2. **Join against the view model.** Give the assertion the row count the shell's
   data source claims, and fail when `rows > 0 && visible_leaves == 0`. Stronger,
   but couples a layout assertion to the view model.

Recommendation: (1). It keeps the invariant a pure function of the layout
record, and the product change it forces — an explicit empty state — is one we
want anyway.

Blocks: `2026-09-09-main-panel-collection-shell-is-rebuilt-empty-each-projection`
has no oracle without this. Related:
`2026-09-09-windowed-driver-reads-one-frame-for-entity-bounds` (FIXED) is what
surfaced both.
