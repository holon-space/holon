---
id: 2026-09-18-render-spec-ids-panic-the-frontend-render
date: 2026-09-18
gap: COVERAGE
secondary: ORACLE
status: PARTIAL
summary: >-
  Six render-path sites converted a vault-supplied id with
  `EntityUri::from_raw`, which PANICS on a value that forms no URI, so a value
  the vault's own query chose took down the whole frontend render instead of
  painting a visible error. The six are fixed; the same conversion survives at
  the row pipeline's centralized choke point, so the class is not closed.
---

## Bug

The MCP lane that fixed the same panic class in its own tools
(`2026-09-17-render-org-doc-id-panic-kills-the-mcp-server`) named one remaining
twin in its final paragraph: `crates/holon-frontend/src/render_interpreter.rs`
converted the render-spec `context_id` with `from_raw`. Its verifier's sweep
found the second; this lane swept the rest of `crates/holon-frontend` and
`frontends/gpui`.

Found by code audit, not by a user. Three sites, each driven red through the
real render path with the value `"my task"`:

| Site | Value's source |
|---|---|
| `crates/holon-frontend/src/render_interpreter.rs` `parse_row_source` | the `id` column of the row the query sits in |
| `crates/holon-frontend/src/shadow_builders/view_mode_switcher.rs` | the authored `entity_uri:` argument |
| `crates/holon-frontend/src/shadow_builders/prelude.rs` `virtual_child_slot_from_arg` | the authored `virtual_parent:` argument |
| `frontends/gpui/src/render/builders/live_query.rs` `render_placed` | the `query_context_id` prop the builder emitted |
| `frontends/gpui/src/render/builders/rendered_text.rs` | the node's data-row `id` column |
| `frontends/gpui/src/render/builders/editable_text.rs` | the node's data-row `id` column |

The last two were found by this entry's verifier, which refuted a first version
of this entry that declared the sweep complete: a row whose `id` column is
`"my task"` plus a vault-authored `rendered_text(col("content"))` still unwound
the render. `ReactiveViewModel::row_id()` reads the data row's `id` column, so
it is vault-supplied — the first sweep had classed it as internal.

The context-id route is NOT the `context:` argument, contrary to the MCP
lane's note. `context` is a template argument
(`crates/holon-api/src/render_eval.rs`, `is_template_arg`), so it lands in
`ba.args.templates` and `get_string("context")` returns `None` — the same
mechanism `2026-09-14-nested-live-query-context-col-silently-binds-row-id`
documents. What actually reaches the conversion is the fallback
`ba.ctx.row().get("id")`, i.e. a value the vault's own SQL chooses:

```
live_query(#{sql: "SELECT 'my task' AS id", item_template:
  list(#{item_template: live_query(#{sql: "SELECT 1"})})})
```

`entity_uri:` and `virtual_parent:` are plain named args (neither is in the
allowlist), so a vault author typing free text there reaches the panic directly.

Every panic is at the same line:

```
EntityUri::new("block", "my task") produced invalid URI: unexpected character at index 8
  at crates/holon-api/src/entity_uri.rs:134
```

Lane `fix-render-spec-panics`, base `1632076c1e5e`.

## Root cause

`EntityUri::from_raw` delegates to `EntityUri::new`, which panics on an
unparseable URI (`crates/holon-api/src/entity_uri.rs:134`). The MCP lane
introduced the fallible counterpart for its own boundary
(`EntityUri::try_from_raw`, already present in `holon-api`) but left the
render-path consumers on the panicking one. The rendered consequence is worse
than the MCP case: the panic unwinds the GPUI render pass, so the window paints
nothing at all rather than the one region failing.

Two further `from_raw` calls consume the same class of value and are NOT fixed
here, because once the sites above are fallible the value can no longer arrive
from a render expression:

- `crates/holon-frontend/src/reactive_view_model.rs:1451` — the
  `view_mode_switcher` `entity_uri` PROP. Its `from_raw("unknown")` default is a
  valid URI, so only an already-invalid prop panics, and the shadow builder no
  longer produces one.
- `frontends/gpui/src/render/builders/view_mode_switcher.rs:39` — the same prop
  on the GPUI side, same reasoning.

Swept and left alone as out of class: ids read from a matview row or an
operation response (`render_interpreter.rs` `pick_active_variant`,
`row_origin.rs`, `reactive.rs`) carry ids the database minted, and the test
fixtures in `tour.rs` / `value_fns/` are literals.

## Missing piece

**Primary (COVERAGE).** The keystone's generators mint row ids from a
well-formed alphabet and no seeded asset carries a free-text `entity_uri:` or
`virtual_parent:`, so no case can produce a render-spec id that forms no URI.
The alphabet cannot reach the state.

**Secondary (ORACLE).** No invariant asserts that a malformed render-spec id
paints a visible error node. A case that did reach the path would fail the run
by unwinding the render (`inv-no-observed-errors` captures swallowed panics,
not one that escapes a render pass), not by an invariant naming the defect.

## Remedy

PARTIAL. `render_spec_block_uri(arg, id)` in
`crates/holon-frontend/src/render_interpreter.rs` is the one fallible
conversion all render-spec id sites share; it wraps `EntityUri::try_from_raw`
and names both the argument and the offending value. The six sites above and
`frontends/mcp/src/describe_ui_expand.rs`'s `context_id` (the helper moved
there, the MCP site now imports it) all call it. Each site turns the `Err` into
the channel it already has: an `Err(String)` for `shared_live_query_build`, a
`ViewModel::error` node for the shadow builders, a painted banner for GPUI.

`EditorView` no longer converts at all: `editable_text` parses the row id once
and passes the typed `EntityUri` into `EditorView::new`, which previously
re-parsed the same string at seven sites. One conversion point per builder.

The GPUI failure banner (`builders::prelude::error_banner`, shared by
`live_query`, `rendered_text` and `editable_text`) is wrapped in
`crate::geometry::tracked(.., "error", ..)` with the message as its
`displayed_text`, so it is readable by the same windowed assertions that
already read `builders::error`'s, instead of being an untracked `div`.

Pinned red-first by `crates/holon-frontend/tests/render_spec_context_id.rs`
(5 tests: one per site plus the helper's bare/schemed resolution),
`frontends/gpui/tests/live_query_context_id_windowed.rs` and
`frontends/gpui/tests/render_spec_row_id_windowed.rs` (windowed: one test per
`rendered_text` / `editable_text`, asserting the painted banner names the value).
Red logs: `lane-logs/red-site1.log`, `lane-logs/red-site2.log`,
`lane-logs/red-rowid-sites.log`, and the per-site teeth reverts under
`lane-logs/teeth/`. Green logs: `lane-logs/green-site1.log`,
`lane-logs/green-site2.log`, `lane-logs/green-rowid-sites.log`,
`lane-logs/green-affected2.log`.

### Still open — the same conversion at the row pipeline's choke point

`holon_api::widget_spec::entity_uri_from_id_str` is the ONE place the row
pipeline turns a row's `id` column into an `EntityUri`, and it too calls
`from_raw`. It is reached during paint and navigation with a vault-supplied id:

- `ReactiveViewModel::entity_id()` (`reactive_view_model.rs:1066`), which has 49
  consumers — `focus_path.rs` (render/navigation), `user_driver.rs`,
  `builders/badge.rs:21`, `builders/table.rs:157`, the integration-test drivers.
  **Demonstrated panicking** on a row whose `id` is `"my task"`:
  `lane-logs/probe-entity-id.log`.
- `reactive.rs:1046/1060/1068` (row-set keying), `advice_weaver.rs:179/180`,
  `navigation.rs:180/183`, `focus_path.rs:1191/1193/1347`,
  `builders/drop_zone.rs:35/36` (drop gesture), `views/editor_view.rs:2391`
  (editor menu item).

Two sites convert a row column but their reachability was not demonstrated:
`render_interpreter.rs:963` (`pick_active_variant`, reached only when the row's
profile declares variants) and `row_origin.rs:168/298`.

This is not closed by patching consumers: the conversion is one function with
49 call sites, and every consumer would need its own failure channel. It is an
architecture decision, escalated rather than guessed. Candidates:

1. **Validate at the row boundary.** Refuse the row loudly where it is bound
   into the render (the `row_render_context` / `ReactiveRowSet` seam) so every
   downstream consumer sees an id that is already known-good. One conversion
   point, closes the class for the render; needs the collection drivers to have
   a refused-row arm.
2. **Make the boundary fallible and thread the failure.** `entity_uri_from_id_str`
   returns a `Result`, `RowIdentity`/`entity_id()`/navigation/focus-path carry
   it. Wider, and it reaches the keystone oracle.

Recommendation: (1) for the render path, with (2) as the end state if the
non-render consumers (drivers, advice weaving) need the failure too.

Classed out of scope, with reason: `reactive.rs:3227` (affordance id minted by
`creation_placeholder_id`), `reactive.rs:3693` (synthetic `query:<hash>` key),
`reactive.rs:5298/5428` (op-response ids the server minted), `reactive.rs:7046+
and tour.rs / value_fns/` (test literals), `row_origin.rs:374/527`
(literal / fixture), `gpui lib.rs:552` (LiveBlock node id from the render tree).

Unreachable once the six entry points refuse: `view_event_handler.rs:132`
(`edit_target_id`) and `editor_view_model.rs:364` (`note_local_edit`) both read
the editor handler's context id, and no editor is mounted for a bad row id.
`reactive_view_model.rs:1451/1453` and `gpui builders/view_mode_switcher.rs:39`
read the `entity_uri` PROP, which the shadow builder no longer writes invalid.
