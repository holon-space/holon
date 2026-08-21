---
id: 2026-08-22-backlinks-section-not-observable-headless
date: 2026-08-22
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  The seeded main-panel "Linked references" accordion renders nothing observable
  in the composed headless slice, and the widget-snapshot translator cannot see
  a generic widget's props or an error message at all.
---

## Bug

Focusing a page whose `backlinks` matview HAS a referencing row renders the
page outline and then nothing — no section header, no backlink row. Found by
agent exploration (lane `gap-refs`) while un-`@wip`ing
`references.feature` log:13. Red log (widget dump): `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/f8e04c02-b393-426b-8bcd-e0887f66acb3/scratchpad/gap-refs-logs/run7.log`; the prod-path probe output is in
`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/f8e04c02-b393-426b-8bcd-e0887f66acb3/scratchpad/gap-refs-logs/probe-links.log`.

Production is CORRECT. Driving the real ingest path
(`TestEnvironmentBuilder::without_loro()`, two org files) shows a bare
`[[Project Alpha]]` resolving BY NAME:

```
block_links  => source_block_id=block:referencing-block, target="Project Alpha",
                kind=page, resolved_id=block:7deb3fc4-…
backlinks    => target_id=block:7deb3fc4-…, id=block:referencing-block,
                content="Link to Project Alpha"
```

So the data layer is right and the defect is in what the section renders — or
in what a test can see of it.

## Root cause

Two distinct pieces, both measured:

1. **The snapshot translator is blind to most of the tree.**
   `view_model_to_snapshot` (`crates/holon-integration-tests/src/pbt/vm_snapshot.rs:50`)
   copies `props` only for `StateToggle`, `EditableText`, `RenderedText`,
   `Text`, `Badge`, `ExpandToggle`, and `Drawer`; every other kind falls to
   `_ => {}`. A generic `ViewModel::from_widget(name, props)` — which is what
   `accordion` returns with its `title` — contributes NO props, and
   `ViewKind::Error` carries its message in `kind`, not `props`
   (`crates/holon-frontend/src/view_model.rs:1189`), so it contributes nothing
   either. `snapshot_text` reads only `entity_id` + `props.values()`
   (`pbt/fixtures/assert.rs`), so `the widget contains "Linked references"`
   cannot pass however correct the render is, and a section that degraded to a
   visible error would read as simply absent.

2. **The accordion's `live_query` yields no row in this harness.** The query
   (`assets/default/index.org:23`) joins `backlinks` to `focus_roots` and
   `navigation_cursor` for region `main`. The same query returns a row against
   the real ingest path. Not yet localized; the `focus_roots` /
   `navigation_cursor` join is the prime suspect — the sibling seed test has to
   `INSERT` `navigation_cursor` by hand to make the section resolve
   (`crates/holon-app/tests/backlinks_section_seed.rs:200`).

`inv-viewmodel-no-error-widgets` is in the composed catalog and did not fire,
so an error widget is unlikely — but per (1) no fixture assertion could have
told us either way.

## Missing piece

No test anywhere proves the backlinks section RENDERS AS A WIDGET WITH ROWS.
`crates/holon-app/tests/backlinks_section_seed.rs` covers real ground either
side of that: `render_entity_expands_collection_view_marker` (L321) calls the
production `engine.blocks().render_entity(&main_uri, …)` and asserts on the
resulting expression, and `backlinks_query_lists_incoming_links_for_focused_page`
(L360) extracts the shipped SQL from the seeded asset, runs it against a live
`SqlOnly` engine with real focus rows, and asserts exact result ids. What no
test joins up is the last hop — an interpreted render producing a populated
section. The one place that became observable, the composed headless slice
(reachable now that the reference leg keeps org-authored inline marks —
`2026-08-22-ref-seed-org-file-drops-inline-marks`), shows nothing, and per (1)
the assert vocabulary cannot distinguish "absent", "empty", and "error".

There is a second, sharper hole in the same file, and it is the one this bug
rests on. `create_linking_block` (L160) builds its mark with `id_link_marks`
(L149), an **id-form** `EntityRef::from_uri(EntityUri::block(…))` target whose
own doc comment notes it resolves trivially at the write boundary. So the
seeded test exercises only id-form resolution. **No surviving test exercises
the NAME-form resolution path** — the path a bare `[[Project Alpha]]` takes,
and exactly the path the production evidence above depends on. (The probe that
produced that evidence was a temporary fixture and is not in the tree.)

## Remedy

OPEN. Three independent pieces:

* Teach `view_model_to_snapshot` to carry generic widget props and the
  `ViewKind::Error` message, so a fixture assertion can name a section header
  and an error is loud rather than invisible. This is the enabling half.
* Then localize why the section's `live_query` returns no row headlessly and
  fix that, and un-`@wip` the `Linked references list the backlinks grouped by
  source` scenario in
  `crates/holon-integration-tests/tests/fixtures/logseq-parity/references.feature`,
  whose comment records the state of the investigation.
* Independently of both: give `backlinks_section_seed.rs` a NAME-form case
  alongside its id-form `create_linking_block`, so the resolution path a bare
  `[[Page]]` actually takes has a surviving test. This is cheap and does not
  depend on the snapshot work.
