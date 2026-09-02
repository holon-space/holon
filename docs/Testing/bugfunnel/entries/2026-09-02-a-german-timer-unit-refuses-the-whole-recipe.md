---
id: 2026-09-02-a-german-timer-unit-refuses-the-whole-recipe
date: 2026-09-02
gap: COVERAGE
secondary: null
status: OPEN
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

`crates/holon-kitchen/src/cook.rs::parse_recipe` propagates every cooklang
diagnostic as a hard failure. cooklang 0.18.7 validates timer units against a
built-in English table and reports an unknown one as an ERROR, not a warning,
so one German word fails the file.

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

OPEN. Two candidate fixes, needing a ruling:

1. Configure cooklang with a units file carrying the German unit names, so
   `Minuten`/`Minute`/`Stunden` parse as real timers. Keeps the fail-loud
   contract and makes the vault's language a first-class input.
2. Demote an unknown timer unit to a warning and carry the timer text through
   verbatim, refusing only structural parse errors.

(1) is the better shape: it keeps every existing refusal and adds vocabulary
rather than weakening the boundary. Either way the closing test is a
`.cook` fixture with a non-English timer unit in `cook_ingest.rs`, red before
the fix.
