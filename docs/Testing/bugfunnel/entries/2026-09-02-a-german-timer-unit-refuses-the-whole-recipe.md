---
id: 2026-09-02-a-german-timer-unit-refuses-the-whole-recipe
date: 2026-09-02
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A cooklang timer written in the vault's own language (`~{9%Minuten}`) fails
  the whole recipe parse, so a German recipe file never reaches the database at
  all.
---

## Bug

Dogfooding the kitchen feature end to end on a copy of Martin's real vault
(lane `kitchen-dogfood`). Three genuine German recipes were authored under
`Resources/Rezepte/` using ordinary German timer units — `~{9%Minuten}`,
`~{1%Minute}`. All three were refused at ingest:

```
cooklang source is not a valid recipe: cooklang parse failed:
  Unknown timer unit: Minuten; Unknown timer unit: Minuten
```

Every one of the three files was quarantined, no `recipe` row and no
`ingredient_use` row existed, and the app raised `OrgMode initial scan
degraded`. Rewriting the same three files with `~{9%min}` made all three ingest
cleanly (3 recipe rows, 27 ingredient_use rows), so the timer unit is the only
cause.

Evidence: `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/kd-logs/app.log`
lines 831–841 (refusal) and 1283–1290 (the same file green after the unit swap).

## Root cause

`parse_recipe` propagates every cooklang diagnostic as a hard failure (then in
`crates/holon-kitchen/src/cook.rs`, now in the wasm guest
`guests/cooklang/src/lib.rs`). cooklang 0.18.7 validates timer units against the
converter's unit table and reports an unknown one as an ERROR, not a warning,
so one German word fails the file. The table came from the bundled ENGLISH
units file alone, so it held no German word at all.

The refusal itself follows the fail-loud policy and is right in kind. What is
wrong is the blast radius: the recipe's ingredients, steps and metadata are all
parseable and independently useful, and a timer unit the parser cannot classify
costs the user the entire recipe. Holon's target vault is German
(`Resources/Rezepte`, `Journals`, `Areas` are all German-authored), so this is
the default authoring experience, not an edge case.

## Missing piece

No test generates a recipe whose timer unit is outside cooklang's English unit
table. `crates/holon-kitchen/tests/cook_ingest.rs` fixtures are all
English-unit (`min`, `minutes`), so the keystone and the kitchen suite alike
cannot reach the state. The keystone PBT
(`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`) has no
`.cook` authoring transition at all, so it cannot reproduce this.

## Remedy

FIXED by remedy (1), ruled D100.a (2026-09-10, superseding D91.a):
`ADVANCED_UNITS` stays ON and the guest gains a German units file.

`guests/cooklang/src/german.toml` is a cooklang units file layered over the
bundled English one through `ConverterBuilder`; the guest has no filesystem, so
it is compiled in with `include_str!` and the parser is built as
`CooklangParser::new(Extensions::all(), converter)` in place of
`cooklang::parse`. It lives under `src/` because `guests/build.sh` keys its
restaging on that directory.

The file adds ALIASES only, never `names` or `symbols` — those two decide how
cooklang FORMATS a unit — so German becomes readable without moving any existing
recipe's projection.

Closing tests, both in `crates/holon-plugin-host/tests/cook_ingest.rs`:

* `a_german_recipe_parses_with_its_own_timer_and_quantity_units` over the
  `kartoffelsuppe.cook` fixture (`~{20%Minuten}`, `~{1%Stunde}`, `@Mehl{200%g}`).
  Red before the fix with exactly this bug's message:
  `cooklang source is not a valid recipe: cooklang parse failed: Unknown timer
  unit: Minuten; Unknown timer unit: Stunde`.
* `the_english_projection_is_unchanged_by_the_german_units` — the whole
  `pancakes.cook` projection against a golden captured BEFORE the units file
  existed, so a regression on English recipes cannot pass unseen.

Measured cost of the layer, A/B on the 200-step recipe
(`vault_scan_latency::fuel_and_memory_headroom`): 150 550 597 → 153 378 084 fuel
(+1.9%) and 2 228 224 → 2 293 760 bytes of guest memory (+2.9%); the wasm grows
516 852 → 731 936 bytes, all of it the `toml` decoder the units file is read
with.
