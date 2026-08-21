---
id: 2026-08-22-ref-seed-org-file-drops-inline-marks
date: 2026-08-22
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A hand-authored `Given an org file:` docstring containing inline markup reds
  inv-blocks-match-ref on `marks`, because the reference leg re-derives marks
  from already-stripped content and drops every link the file authored.
---

## Bug

Replaying a LogSeq-parity fixture whose org docstring carries wiki-links —

```
** Link to [[block:project-alpha][Project Alpha]]
** Mentions [[Project Alpha]] by name
```

— panics before any `Then` step runs:

```
block:idform-ref:  marks: sut=Some([MarkSpan { .. Link { target: Scheme { raw: "block:project-alpha" }, label: "Project Alpha" } }]) ref=None
block:nameform-ref: marks: sut=Some([MarkSpan { .. Link { target: Name { name: "Project Alpha" }, label: "Project Alpha" } }]) ref=None
```

Found by agent exploration (lane `gap-refs`, LogSeq-parity gap corpus,
`references.feature`) while probing what the composed headless SUT renders for
a page reference.

Red→green demonstrated on the SHIPPED scenario (not on the throwaway probe
that first surfaced it): with the `seed_org_file` call site stub-reverted to
the unseeded normalizer, `A page reference is stored as a reference, not
literal text` replays and reds on exactly this divergence —

```
[fixtures:gherkin] replaying "A page reference is stored as a reference, not literal text   # log:12" (4 steps)
inv-blocks-match-ref/block_raw: block block:referencing-block diverges from reference on Marks
  sut=Some([MarkSpan { start: 8, end: 21, Link { target: Name { name: "Project Alpha" }, label: "Project Alpha" } }])
  ref=None
```

Logs, all under
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/f8e04c02-b393-426b-8bcd-e0887f66acb3/scratchpad/gap-refs-logs/`:
`red-shipped-scenario.log` (the red above) and `green-shipped-scenario.log`
(`1 replayed, 7 skipped` from `references.feature`, scenario `OK`) with the fix
restored. `red-marks-dropped-ref-leg.log` is the original discovery red from
the temporary probe fixture, kept for provenance.

Production is CORRECT on both legs — it extracts the id-form link as
`Scheme` and the bare wiki-name as `Name`. Only the reference model is wrong.

## Root cause

`WriteOrgFile::from_org_text` (`crates/holon-integration-tests/src/pbt/transitions/write_org_file.rs:67`)
parses the docstring with the PRODUCTION parser, so the blocks it hands the
reference already have their delimiters stripped into `content` and their
spans in `marks`.

`seed_org_file` (`crates/holon-integration-tests/src/pbt/ref_caps/docs.rs:258`)
then overwrote both fields from `content` alone:

```rust
let (content, marks) = normalize_content_for_org_roundtrip(&block.content, block.content_type);
block.marks = marks;
```

`normalize_content_for_org_roundtrip` (`crates/holon-integration-tests/src/pbt/types.rs:156`)
is a render→re-ingest fixed point that starts with an EMPTY mark set. Given
already-stripped content (`"Link to Project Alpha"`) the first `render_inline_marks`
is the identity, extraction finds no delimiters, and the loop settles at
`marks = None` — silently discarding the parsed marks.

The SUT leg does not lose them: it re-renders the SAME blocks through
`OrgRenderer::render_entitys`, which replays `block.marks` back into
`[[…]]` org text. Hence the one-sided divergence.

Generator-produced blocks are unaffected: they carry raw org text with
`marks = None`, so the unseeded fixed point derives their marks correctly and
both legs agree.

## Missing piece

No route to a link-bearing block that the ORACLE accepts, via org-file ingest.
The keystone mints `Link` marks only through the editor-typed path
(`typing_text_strategy`'s `[[w]]` arms, `generators.rs:269`) — deliberately
added after the 2026-07-19 undo-drops-marks escape. The org-FILE route to the
same state existed for the SUT but was unrepresentable for the reference, so
every `.feature` that seeds a link from disk was structurally red. That is
exactly the shape every LogSeq-parity references scenario needs, which is why
the whole references cluster was unreachable.

## Remedy

`normalize_parsed_block_for_org_roundtrip` (`pbt/types.rs`) seeds the fixed
point with the block's existing marks, so both legs start from the same on-disk
org text; `seed_org_file` calls it with `block.marks`. Unseeded callers are
unchanged (`&[]`), so generator draws keep their current behaviour.

Pinned by the un-`@wip`ed scenario `A page reference is stored as a reference,
not literal text` in
`crates/holon-integration-tests/tests/fixtures/logseq-parity/references.feature`,
which replays a link-bearing org file through `ComposedSut<WideE2E>`. (The
sibling `Linked references` scenario is still `@wip` — see
`2026-08-22-backlinks-section-not-observable-headless`.)

Scope of that pin, stated precisely: it proves the MARK ROUND-TRIP — parser →
reference leg → store → renderer — because the rendered label carries the
`[[ ]]` delimiters stripped, which is false if the marks are dropped. It does
NOT prove reference RESOLUTION: nothing asserts `block_links.resolved_id`, and
no composed-catalog invariant covers link resolution, so a dangling
`[[Project Alpha]]` would produce the same mark and the same rendered label.
Closing that needs a `resolved_id` assertion vocabulary.
