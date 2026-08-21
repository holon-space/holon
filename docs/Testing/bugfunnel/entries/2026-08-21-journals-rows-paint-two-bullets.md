---
id: 2026-08-21-journals-rows-paint-two-bullets
date: 2026-08-21
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Every row inside a Journals day section paints TWO bullets side by side — a
  grey tree-chrome leaf dot and the block's own blue bullet — because the two
  journals tree variants omit the `show_bullet: false` rule that every other
  outline in the app carries.
---

## Bug
Martin, dogfooding the LogSeq-look Journals feed on 2026-08-20, saw a double
bullet on each row of a day section: a small grey dot immediately left of the
block's blue bullet (screenshot shows the pair on both painted day sections).
Found outside any automated test — the windowed journals PBT was green.

## Root cause
The two dots come from two different, both-enabled bullet sources:

- Grey dot = the tree chrome's leaf bullet,
  `frontends/gpui/src/render/builders/tree_item.rs` `bullet_dot` (:100-114),
  emitted when `LeadingMarker::Bullet` is selected. `show_bullet` defaults to
  TRUE (`node.prop_bool("show_bullet").unwrap_or(true)`, :298).
- Blue dot = the block's OWN bullet, the `icon(col("bullet_shape"), …)` that
  `block_profile.yaml`'s `default` (:167) and `query_block_titled` (:160)
  variants draw for every block.

Every other outline in the app suppresses the chrome dot so only the block's
own bullet paints — `assets/default/types/collection_profile.yaml:30` and
`assets/default/index.org:39` both carry
`rules: [#{when: always(), override: #{show_bullet: false}}]` on their `tree(…)`.
The two journals tree variants in `assets/default/types/block_profile.yaml` —
`embedded_page_expanded` (:95) and `embedded_page` (:109) — had NO `rules` at
all, so the default kept the chrome dot on.

The mechanism is sound and the contract is pinned
(`tree_item.rs:437-470`): this is an ASSET-level omission, not a renderer bug.

## Missing piece
ORACLE. The interaction is fully generated and the defective state IS reached
in the harness — the windowed journals PBT
(`frontends/gpui/tests/gpui_journals_logseq_look.rs`) paints both dots in its
frame. Nothing went red because the oracle counts painted slot ENTITIES
(`slot_painted`, :174-181): it asks "did the creation slot's entity paint?",
never "how many bullet GLYPHS does a row show?". A row with one bullet and a
row with two are indistinguishable to every assertion the file had.

## Remedy
FIXED. Both journals tree variants in `block_profile.yaml` now carry the same
rule the production outline uses:
`rules: [#{when: always(), override: #{show_bullet: false}}]`.

Oracle gap closed: `gpui_journals_logseq_look.rs` REQ4 now reads bullet GLYPHS.
Tree-chrome bullets are observable in the painted tree as
`widget_type == "tree_bullet"` with el_id `tree_bullet_id_for(target)`
(`tree_item.rs:388-393`), so the test asserts that a day-content row — which by
profile always draws its own bullet — has NO chrome bullet registered for it.

The harness also needed a day with a real child row materialised in the MAIN
panel: the pre-seeded 2026-01-0x days sort to the BOTTOM of the date-DESC feed
and never reach the viewport, which is why the frame contained no double-bullet
row to see. The late day now carries a child block (`jday-zz-late-entry`).

Red with the two `rules:` removed:

```
REQ4 one bullet per row: the day-content row jday-zz-late-entry already draws
its own `bullet_shape` bullet, so the tree chrome must draw none — but
["tree_bullet::jday-zz-late-entry"] painted, giving the row TWO dots.
```

## Looked at, deliberately NOT changed: the left sidebar
The left sidebar's page rows also paint two leading glyphs per line — a chrome
`tree_bullet` and, beside it, the row's own `icon`. The mechanism is the same
omission as above: the sidebar's Pages tree (`assets/default/index.org`, the
`left_sidebar::render::0` block) is a `tree(...)` with NO `rules`, so
`show_bullet` defaults to true.

What differs is the second glyph, and it is why this is NOT the same bug. The
sidebar's item template is
`selectable(row(icon("notebook"), spacer(6), text(col("content"), ...)))`, so
the glyph beside the dot is a NOTEBOOK page icon, not the block's
`bullet_shape` bullet. This entry's defect is "the row already draws its OWN
bullet, so the chrome dot duplicates it". A sidebar row draws a page-type icon
instead, so a leaf dot followed by a page icon is a look, not a duplicate.

**RULED BY MARTIN (SB-1.a, 2026-08-21, PROVISIONAL — "for now, not a final
decision"): KEEP the leaf dot + icon.** The sidebar is unchanged and
`index.org` stays byte-identical with main. Since the ruling is explicitly
provisional, the implementation is recorded here so a reversal is cheap.

### If the ruling is revisited, this is what it takes
One line — the same clause every other tree carries — on the `left_sidebar`
tree in `index.org`:

```
rules: [#{when: always(), override: #{show_bullet: false}}]
```

Suppressing the leaf dot does not flatten the tree: `LeadingMarker::None` still
reserves the marker gutter (`tree_item.rs:398-408`) so indentation is
unchanged, and `show_chevron` is independent, so a page with children keeps its
disclosure control.

A windowed assertion in the REQ4 shape pins it — chrome bullets are observable
as `widget_type == "tree_bullet"` with el_id `tree_bullet_id_for(target)`. Two
precision notes from the verifier, worth starting from rather than rediscovering:

- SCOPE THE PREDICATE TO A PANEL. The REQ6 draft matched
  `tree_bullet::<page-id>` anywhere in the frame and inferred "sidebar" from
  x-coordinates measured in a *different* run. A correct assertion must
  establish the panel itself (parent chain, or an x-bound derived from the
  frame), or it cannot distinguish a sidebar dot from a main-panel one.
- COVER THE SIXTH ROW. The draft iterated the test's `feed` days only, missing
  `fe3f2c27-…` — the real journal page the `daily_journal` rule mints — which
  is also a sidebar row. Iterate the painted page rows, not the fixture list.
