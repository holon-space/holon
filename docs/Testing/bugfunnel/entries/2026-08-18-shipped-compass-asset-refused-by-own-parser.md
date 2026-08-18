---
id: 2026-08-18-shipped-compass-asset-refused-by-own-parser
date: 2026-08-18
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  The org parser refuses `{{var}}` in a typed-edge property for EVERY block,
  including blocks marked `:TEMPLATE:` whose slots are meant to stay
  unsubstituted until instantiation — so the shipped
  `assets/default/Compass.org` template is quarantined on ingest.
source_line: null
---

## Bug

Martin dogfooding the GPUI desktop app (cold boot, 2026-08-18): a red banner
`File sync degraded (bad org file) — OrgMode initial scan failed for 1 file`.
`/private/tmp/holon-cold.log:1666` names it:

```
[FileSyncController] ingest FAILED partway — QUARANTINING this file from
write-back … path=/Users/martin/Workspaces/pkm/holon-pkm/Templates/Compass.org
error=block compass-problem-tpl-0: :contributes-to: takes bare block IDs, got
"{{mission}}" (docs/Reference/CompassConventions.md): Invalid URI
"block:{{mission}}": unexpected character at index 6
```

Holon SHIPS the offending file: `assets/default/Compass.org:25`, `:44`
(`:contributes-to: {{mission}}`) and `:77` (`{{goal}}`). Martin's vault copy is
that seeded asset. Any vault seeded from Holon's own defaults quarantines a
file and shows a permanent red banner.

## Root cause

Two subsystems disagree about who owns a `{{…}}` property value, and both are
individually right.

**The template feature says a slot is legal.** Holon has a full template
concept: `crates/holon-api/src/template_instantiation.rs` (926 lines) parses
`:TEMPLATE_VARS:` into `TemplateVars`, scans `{{name}}` slots
(`referenced_vars`), and substitutes them (`substitute`, `:486-520`) — and
critically it substitutes inside **every string property value**
(`:365-371`), not just content. So `:contributes-to: {{mission}}` becomes a
real block id, and a real typed edge, at instantiation.
`docs/Reference/CompassConventions.md` §Templates documents exactly this for
this asset: "`assets/default/Compass.org` ships one template per item type…
Each declares its slots in `:TEMPLATE_VARS:`". The asset's root blocks carry
`:TEMPLATE:` and declare `mission` / `goal` in `:TEMPLATE_VARS:`
(`Compass.org:22-23`, `:41-42`, `:72-73`). Design: `docs/Proposals/Templating-2026-07-12.md`.

**The org parser says it is not.** `parse_edge_slug`
(`crates/holon-org-format/src/parser.rs:1355-1369`) promotes any
`:REQUIRES:` / `:BLOCKED-BY:` / `:contributes-to:` slug through
`EntityUri::try_from_raw` and returns `Err` when it is not a usable block id —
with no notion of a template block. Its own doc comment names the case ("an
unfilled template placeholder … names no block id, so this rejects") and
`edge_typed_drawer_keys_refuse_a_value_that_is_not_a_usable_block_id`
(`parser.rs:1503-1531`) pins `{{mission}}` as refused across all six authoring
surfaces.

Templates must survive ingest: they live in the store as ordinary blocks,
because instantiation reads them from storage rows via `TemplateSource`. A
template that cannot be ingested can never be instantiated, so the parser's
blanket refusal makes the shipped template unreachable by the feature that
owns it.

Quarantine-on-partial-ingest itself is deliberate and behaves as designed: the
file is excluded from write-back rather than half-ingested, and disclosed as
`DegradedKind::OrgIngestFailed` (`frontends/gpui/src/share_ui.rs:1692-1696`)
instead of swallowed. Nothing about the failure handling is wrong.

CORRECTION: an earlier revision of this entry claimed "there is no template
concept in code" and concluded the asset was simply mis-authored. That was
wrong — the investigating grep filtered out `format!` lines and hid
`template_instantiation.rs`. The remedy that followed from it (rewrite the
asset to the `none` sentinel) would have stripped the `contributes-to` edge
from every instantiated Compass item, which is the edge the whole convention
exists to produce (CompassConventions.md: the agenda query is a reverse
closure over `block_contributes_to.target_id`).

## Missing piece

No gate parses the files in `assets/default/` with the production org parser,
so an asset the parser refuses ships green. Underneath that: no test
instantiates a shipped template end to end (ingest → `plan_instantiation` →
edge), which is the path that would have forced the two subsystems to agree.

## Remedy

FIXED (Martin's ruling D5B-9.a, 2026-08-18). The edge parser now PARSES rather
than validates: `EdgeTarget::{Block, None, Slot}`
(`crates/holon-org-format/src/parser.rs`) classifies each authored slug.

- `Slot` is accepted ONLY inside a template subtree — the block itself or an
  ancestor in the same file carrying the `:TEMPLATE:` marker — and only for a
  variable the enclosing `:TEMPLATE_VARS:` declares. The scope is threaded
  through `process_headlines` / `emit_section_children`, so a descendant of a
  template root is in scope without repeating the marker.
- An UNDECLARED `{{x}}` inside a template is a loud refusal naming the
  variable and the missing declaration — a template that would only fail at
  instantiation time is refused at ingest instead.
- Outside a template subtree `{{…}}` is refused exactly as before; the
  pre-existing `parser.rs` refusal test is unchanged and still green.
- A slot-bearing value contributes NO junction row, so the agenda's reverse
  closure over `block_contributes_to.target_id` never sees a template. It is
  carried verbatim as a plain drawer property, so it round-trips to disk and
  through the store and `template_instantiation.rs:365-371` still has
  `{{mission}}` to substitute.

The missing piece is closed by
`every_shipped_default_asset_ingests_with_the_production_parser`
(`crates/holon-org-format/tests/template_slot_edges.rs`), which parses every
shipped `assets/default/**/*.org` with the production parser. Red before the
fix with Martin's exact production error:

```
shipped asset .../assets/default/Compass.org does not ingest with the
production parser — a vault seeded from defaults would QUARANTINE it and show
a permanent degraded banner: block compass-problem-tpl-0: :contributes-to:
takes bare block IDs, got "{{mission}}"
(docs/Reference/CompassConventions.md): Invalid URI "block:{{mission}}":
unexpected character at index 6
```

NOT changed: `assets/default/Compass.org` is correct as authored and was left
alone. An earlier ruling to rewrite its slots to the `none` sentinel was
withdrawn — it would have stripped the contribution edge from every
instantiated Compass item.

Residual, unrelated to this fix: `:contributes-to: none` still parses to the
empty set and the renderer omits the key, so the authored `none` does not
round-trip. Pre-existing, not introduced here, and not exercised while the
file was quarantined.
