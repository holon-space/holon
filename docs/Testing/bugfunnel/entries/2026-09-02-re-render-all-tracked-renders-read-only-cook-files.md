---
id: 2026-09-02-re-render-all-tracked-renders-read-only-cook-files
date: 2026-09-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Every successful `.cook` ingest is followed by `re_render_all_tracked` trying
  to render the read-only file, tripping the code's own "a write path skipped
  the controller's write-tier gate" error.
---

## Bug

Found dogfooding the kitchen feature on a copy of Martin's real vault (lane
`kitchen-dogfood`). Immediately after a recipe ingests cleanly — the same log
line that reports `write-back quarantine CLEARED … (ingest fully succeeded)` —
the debounced re-render fires against the recipe:

```
ERROR holon_orgmode::di: re_render_all_tracked (debounced) error:
write-back render REFUSED for .../Resources/Rezepte/Linsensuppe.cook: its
format is read-only (authoritative input only) and ships no renderer, so
writing a reconstructed file over it would be loss. Reaching this render means
a write path skipped the controller's write-tier gate.
```

The last sentence is the code's own diagnosis: by its author's reading, this is
a wiring bug, not bad input. It fires on the ordinary success path, once per
recipe, on every ingest.

Evidence: `kd-logs/app.log` line 1293 in the lane scratchpad
(`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/kd-logs/app.log`).

## Root cause

`re_render_all_tracked` in `crates/holon-orgmode/src/di.rs` re-renders every
TRACKED document without consulting the format registry's write tier first. A
`.cook` file is tracked like any other vault document, so the read-only tier is
only discovered inside the renderer, where `CookFormatAdapter::render_document`
and the controller's tier gate both refuse.

Nothing is lost — the refusal is exactly the guard working — but the guard is
being asked a question the caller should never have posed, and it answers at
ERROR level on a healthy path. That trains the reader to ignore an error whose
whole purpose is to be believed.

## Missing piece

No test drives the debounced re-render with a read-only format in the tracked
set. `crates/holon-integration-tests/tests/cook_vault_ingest.rs` boots a real
vault with a `.cook` file and checks the ingest, but asserts nothing about what
the app logs afterwards, and there is no captured-ERROR assertion around the
re-render path.

## Remedy

OPEN. Fix is that `re_render_all_tracked` filters by write tier before
rendering, so a `WriteTier::ReadOnly` document is never a re-render candidate.
The closing test is an assertion in `cook_vault_ingest.rs` that a successful
`.cook` ingest emits no captured ERROR — note the harness gotcha recorded in
[[error-capture-layer-install-gotcha]]: the test must touch
`SpanCollector::global()` before the SUT runs or the capture comes back empty.
