---
id: 2026-09-08-a-drawer-authored-priority-is-erased-from-the-org-file-on-write-back
date: 2026-09-08
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A priority the author spelled in the drawer as `:priority: A` was destroyed at
  parse by the generic drawer loop, after which the renderer emitted neither the
  `[#A]` cookie nor the drawer line — so the next write-back deleted the
  authored priority from the org file on disk.
---

## Bug

Found by the `org-priority` lane (2026-09-10) while fixing the sort inversion in
`2026-09-08-org-priority-ranks-c-first-and-the-drawer-spelling-stores-a-string`
— read off the parser and renderer, then reproduced. It is a distinct defect
from that one: the inversion sorts rows wrongly, this one loses vault bytes.

The vault authors priority in the drawer under its lowercase name — e.g.
`Projects/Holon/Display Placement & Resurfacing.org:61`, `:priority: A` — and
some headlines carry BOTH spellings (`** BLOCKED [#A] … :priority: A`). For
every one of those blocks the authored priority left the file on the next
write-back.

Reproduced against a build, with the fix's drawer-emission surgically disabled
(`lane-logs/app-redproof-11229.log`):

```
[drawer/loro] control: the format-only leg must already reproduce the authored bytes
  left:  "…* TODO Drawer carries it\n:PROPERTIES:\n:ID: prio-drawer\n:END:\n"
  right: "…* TODO Drawer carries it\n:PROPERTIES:\n:ID: prio-drawer\n:priority: A\n:END:\n"
```

## Root cause

Three functions, none of them wrong on its own:

1. `crates/holon-org-format/src/parser.rs` set the typed priority from the
   headline cookie, and the generic drawer loop's catch-all then ran
   `block.set_property(key, Value::String(value))` for the colliding lowercase
   `priority` key — OVERWRITING the typed integer with the raw letter.
2. `OrgBlockExt::priority()` (`crates/holon-org-format/src/models.rs`) reads that
   property through `as_i64()`, so it answered `None`: the typed value was
   destroyed, not shadowed.
3. With `priority()` = `None` the renderer's cookie branch (`models.rs`, the
   `[#{}]` push) emitted nothing, and `drawer_properties()` lists both
   `priority` and `PRIORITY` in `INTERNAL_KEYS`, so the authored drawer line was
   not re-emitted either.

Both carriers were therefore dropped, and the file was rewritten without the
priority the author had written.

## Missing piece

**COVERAGE.** The interaction was ungeneratable, so no oracle ever had a chance.
The org round-trip PBT authors priority exclusively through the headline cookie
(`crates/holon-orgmode/tests/round_trip_pbt.rs`, `set_priority_on_headline`), and
its drawer-property generator never emits a `priority` key — the one spelling the
real vault uses on nearly every headline. `INTERNAL_KEYS` was itself the reason
the shape looked untestable: any generated `priority` drawer key would have been
swallowed rather than round-tripped, so the gap was self-concealing.

## Remedy

FIXED in the `org-priority` lane, by resolving the collision at the parse
boundary instead of downstream:

- the parser now resolves `:priority:` / `:PRIORITY:` into the typed `Priority`
  before the generic drawer loop, and `continue`s past the key so the catch-all
  can never overwrite a typed value with a string again;
- a `_priority_drawer_only` carrier (same underscore-carrier discipline as
  `_drawer_order`) records the one exceptional case — the drawer carried the
  priority and the headline had no cookie — so write-back replays exactly the
  carrier(s) the author used and never invents a cookie;
- `drawer_properties()` reconstructs the drawer line from the typed field under
  the authored key's own spelling, the way `:COLLAPSED:` is reconstructed;
- `build_block_params` refuses the drawer spelling case-insensitively
  (`is_typed_field_drawer_key`), so the ingest leg cannot re-park the letter as
  a string over the typed rank.

Covering test (red first, above; green in `lane-logs/green1-58233.log` and
`lane-logs/app1-82641.log`):
`holon-app::org_store_org_round_trip::every_authored_priority_carrier_survives_the_store_byte_identical`
— all three authored carriers (cookie, drawer, both) survive org → store → org
byte-identical, on BOTH production write legs (the Loro projection writer and
the org-ingest param builder), so neither leg can drift alone.

Related: `2026-09-08-org-priority-ranks-c-first-and-the-drawer-spelling-stores-a-string`
(the sort inversion, fixed by the same lane).
