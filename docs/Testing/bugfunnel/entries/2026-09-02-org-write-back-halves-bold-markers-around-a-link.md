---
id: 2026-09-02-org-write-back-halves-bold-markers-around-a-link
date: 2026-09-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Org write-back rewrites `**[text](url)**` as `*[text](url)*` while leaving
  `**plain text**` alone, silently changing a file the user never edited.
---

## Bug

Found dogfooding the kitchen feature on a copy of Martin's real vault (lane
`kitchen-dogfood`). The run touched only `Resources/Rezepte/`, but a
`diff -rq` of the copy against the original afterwards showed the app had also
rewritten three tracked org pages it merely re-rendered. One of them,
`Agents/cc/e89d494a-d249-46ed-b08a-3436af07240c.org`, had not been touched in
the original since 10:32, so its difference is purely the app's own write-back.

Line 11, before and after — five `**` markers become one:

```
- Both PRs are open: **[tenant-data#128](…/128)** (fixture) and **[ai-root#575](…/575)** (gate fixes). **The thing y…
+ Both PRs are open: *[tenant-data#128](…/128)* (fixture) and *[ai-root#575](…/575)* (gate fixes). **The thing y…
```

The two bold spans that WRAP A MARKDOWN LINK lose one asterisk each. The plain
bold span in the same line, same paragraph, keeps both. The file is otherwise
byte-identical and the same 11 lines long, so nothing else moved.

Evidence: `vault-diff.txt`, `bold-orig.txt` and `bold-copy.txt` in the lane
scratchpad
(`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/`).

## Root cause

Not diagnosed here. The non-uniformity is what rules out deliberate
normalisation: if the renderer were converting markdown bold to org bold it
would convert all three spans, and it converts only the two whose content is a
link. So the mark's extent is being computed differently when a link node sits
inside it, and one delimiter character is dropped on the way out.

The user-visible cost is small per occurrence and unbounded in aggregate: a
re-render nobody asked for edits prose nobody changed, and it does so on the
real vault, on every boot that re-renders these pages.

## Missing piece

The org round-trip pin
(`crates/holon-app/tests/org_store_org_round_trip.rs`) covers property drawers
and identifier stability — the hazards named in CLAUDE.md — but its fixtures do
not carry a bold mark wrapping an inline link. So the byte-stability property
it asserts is real and this content shape simply never reaches it.

## Remedy

OPEN. The closing test is a round-trip fixture whose text is
`**[label](https://example.test/1)** and **plain**`, asserting byte-stability
through store and render — red before the fix. Worth generating the mark/link
nesting rather than hand-listing it: this is the second shape of the same
family, and a hand-written third fixture will miss the fourth.
