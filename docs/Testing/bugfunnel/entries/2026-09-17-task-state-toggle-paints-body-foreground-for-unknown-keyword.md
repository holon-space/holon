---
id: 2026-09-17-task-state-toggle-paints-body-foreground-for-unknown-keyword
date: 2026-09-17
gap: PERCEPTION
status: FIXED
summary: >-
  A task state outside the built-in keyword list painted the state toggle's
  glyph in body foreground — the vocabulary gave it a colour, and the frontend's
  resolver had no arm for that colour's name.
---

## Bug

The state toggle draws a task's glyph in the colour the state vocabulary names
for it. The vocabulary emits `error` for `CANCELLED` and `primary` for any
keyword outside `""`/`TODO`/`DOING`/`DONE`/`CANCELLED`/`LATER`/`NOW`, and the
GPUI toggle's resolver had arms for `muted`, `warning`, `info` and `success`
with a catch-all of `foreground`. So a CANCELLED task's glyph, and every
foreign-vault keyword such as `WAITING`, `BLOCKED` or `PAUSED`, painted as
ordinary body text.

The headline, the label and the click behaviour were all correct, which is why
this survived: only the glyph's colour was wrong, and a glyph drawn in body
foreground looks like a deliberate choice rather than a defect.

Found 2026-09-17 by the adversarial verifier on lane `d130-colour-from-column`,
as finding 1 of `lane-logs/d130-inc0-verify.md`. It came in as a refutation of
that lane's claim to have left ONE colour resolver in the GPUI frontend; the
survivor was `semantic_color`, and the reachable conflation was underneath it.

## Root cause

Two vocabularies with nothing checking that one contained the other.

`crates/holon-api/src/render_eval.rs` `state_display` returned the colour as a
`&str`, ending `_ => (state, "primary")` and mapping `CANCELLED => "error"`.
`frontends/gpui/src/render/builders/state_toggle.rs`'s `semantic_color` matched
that name against four arms and fell through to `foreground` for everything
else. Neither side was wrong on its own; the pair was, and nothing compared
them.

The same resolver also mapped `"info" => t.accent`, while the shared vocabulary
maps `Info` to `t.info` — one name with two meanings inside one frontend. Latent
only because `state_display` never emits `info`.

Evidence: `lane-logs/inc0-delta-item1-red.log` —

```
state "CANCELLED" is drawn in "error", which this builder has no arm for, so it
falls to the catch-all and paints body foreground.
```

## Missing piece

**PERCEPTION.** The property is a property of the PAINT: which colour lands on
the glyph. Nothing headless can express it.

The interaction was already generatable, so this is not a coverage hole:
`crates/holon-integration-tests/src/pbt/generators.rs:92` puts `WAITING` into the
task-state alphabet. And the keystone's nearest invariant,
`inv-viewmodel-state-toggle-correct`, already judges
`props["label"] == state_display(current).0` — it pins the DISPLAY half of the
same vocabulary. It judges `field`, `current`, `label` and the bound ops: all
props. It cannot judge colour, because colour is computed at paint from
`current` and never reaches a prop.

So the vocabulary's label half was pinned headless and its colour half was
pinned nowhere. A state could be generated, the invariant could run, and the
glyph could still be painted the wrong colour with every layer green.

## Remedy

Fixed in the same change that surfaced it, on the same lane.

`state_display` now returns `(&str, ThemeToken)`
(`crates/holon-api/src/render_eval.rs:223`), and `semantic_color` is deleted:
the toggle resolves through `theme::theme_token_color`
(`frontends/gpui/src/render/builders/state_toggle.rs:57`), whose match is
exhaustive over `ThemeToken`. The illegal state is now unrepresentable — a
consumer cannot receive a colour name its resolver has no arm for, because it
receives a type instead of a name. The out-of-list keyword takes
`ThemeToken::Primary`, which is the default the vocabulary already declared.

Gap-closing rung: `state_toggle::tests::the_declared_colour_is_the_painted_colour`
(`frontends/gpui/src/render/builders/state_toggle.rs:146`). Its red form checked
containment between the two vocabularies and quoted the CANCELLED case above;
because the fix removed the failure mode, its green form asserts the declared
mapping per state, which is the same property expressed at the new type.
Paint-level token→pixel is pinned by the windowed rung
`text_colour_windowed::a_token_paints_the_theme_slot_it_names`.

Residual, stated rather than hidden: the toggle does not declare its painted
colour, so its glyph is not directly readable from the layout record the way
`text`'s is. The class is closed by typing the boundary, but a future
regression in the toggle's own paint plumbing would still need a windowed
assertion that does not exist yet. The same limit applies to `icon`, `card` and
`board`, and is recorded in `lane-logs/lane-report-d130-inc0b.md`.
