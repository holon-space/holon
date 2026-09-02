---
id: 2026-09-02-a-cook-recipe-loses-its-title-and-all-its-metadata
date: 2026-09-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The document block stored for a `.cook` recipe keeps the file stem as its
  title and an empty properties bag, so `>> title:`, `servings`, `tags` and
  `source` never reach the page and `servings` is lost from the system
  entirely.
---

## Bug

Found dogfooding the kitchen feature on a copy of Martin's real vault (lane
`kitchen-dogfood`). `Resources/Rezepte/Spaghetti-Carbonara.cook` carries

```
>> title: Spaghetti Carbonara
>> servings: 4
>> course: Hauptgericht
>> tags: [italienisch, schnell, pasta]
>> source: Familienrezept
```

After a fully successful ingest the `recipe` row is right — `title` is
`Spaghetti Carbonara`, `course` is `Hauptgericht` — but the DOCUMENT BLOCK that
the page renders from is not:

```sql
SELECT id, content, properties, property_kinds FROM block_raw
WHERE id = 'block:8a802b12-4e49-4a63-3e06-c71696ebc072';
-- content = 'Spaghetti-Carbonara'   (the file stem, not the title)
-- properties = '{}'   property_kinds = NULL
```

The sidebar shows `Spaghetti-Carbonara`, and `servings`, `tags` and `source`
exist nowhere. `servings` is the sharpest case: it is deliberately not written
to the recipe row, and `crates/holon-kitchen/src/rows.rs` says why — "the
metadata reaches the recipe page through the document block's properties either
way". That premise is false in production, so a recipe's serving count is
dropped on the floor with no error.

## Root cause

The adapter builds the right document. `CookFormatAdapter::parse`
(`crates/holon-kitchen/src/file_format.rs`) ids it `EntityUri::file(rel)`, sets
`content` to the metadata title, and calls `set_property` for every non-title
metadata key — the `course` value proves the properties were populated at parse
time, because the recipe row reads `course` back out of
`document.get_property_str("course")`.

But no `file:`-schemed block exists in `block_raw` after ingest
(`SELECT id FROM block_raw WHERE id LIKE 'file:%'` returns nothing). The block
that survives is the UUID one minted by the directory walk, and only the CHILD
blocks carry the adapter's ids (`block:Resources/Rezepte/…::b::0`, with their
`step_number` properties intact). So the walk's document block wins the row and
the adapter's document — title and properties together — is discarded. Same
seam as [[2026-09-02-a-refused-cook-file-still-leaves-a-document-block]], seen
on the happy path.

## Missing piece

`crates/holon-kitchen/tests/cook_ingest.rs` asserts the adapter's RETURN VALUE
carries the metadata properties, which it does. Nothing asserts they are still
there after a real vault boot has stored them.
`crates/holon-integration-tests/tests/cook_vault_ingest.rs` checks the document
block exists and the step blocks are children, but never reads the document's
`content` or `properties` back. The gap is exactly the hop between the two
tests.

## Remedy

OPEN, and this one blocks the recipe page: the render profile
(`crates/holon-kitchen/assets/types/recipe_profile.yaml`) already avoids
`servings` and `tags`, so the page silently omits what the file says. Fix is
that the adapter's document identity and property bag survive storage. The
closing assertion belongs in `cook_vault_ingest.rs` — after a real boot, the
recipe's document block reads back `content == "Spaghetti Carbonara"` and
`properties` holding `servings`, `tags` and `source` — red before the fix.
