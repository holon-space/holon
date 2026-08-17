---
id: 2026-08-17-disclosure-tested-before-rendering-truncated-the-remedy
date: 2026-08-17
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  The not-enabled integration toast lost its state path and remedy to an 80-byte
  cut, advised a state file the parser rejects, and panicked the render when the
  cut landed inside a multi-byte character.
---

## Bug

Found by the fresh-context verifier on the D4.b enablement-cutover lane (task
#50), before landing. Three defects in one disclosure:

- The toast a user actually sees is truncated at 80 BYTES. For Martin's own
  paths that leaves
  `⚠  Integration is not switched on — gcal: not enabled, so /Users/martin/.config/holon/integrations/gcal.yaml runs no…`
  — no state file, no remedy, nothing to act on.
- The advice, when visible, was `write \`enabled = true\` to <path>`. A file
  containing only that is REJECTED by the strict `StateFile` parser (no
  `schema_version`, no `configuration`), and because the store loads every
  provider up front, following it takes down the WHOLE integrations load.
- Byte-slicing `detail` panics when the cut lands mid-character. Reproduced at
  installed-path length 43: `end byte index 80 is not a char boundary; it is
  inside '—' (bytes 78..81)`.

## Root cause

`detail` was truncated with `&toast.detail[..80]` inside `render_toast_stack`
(`frontends/gpui/src/share_ui.rs`), so it was neither char-safe nor wide enough
for a disclosure carrying two absolute paths plus a remedy. The remedy text was
composed inline in the frontend, with no connection to
`integration_state::enabling_state_file()` — the one place that knows what the
parser accepts.

The reason all three survived a green test run: both toast tests asserted on
`DegradedToast.detail`, the PRE-render payload. Nothing in the suite ever built
the string the user reads, so the cap, its byte-slicing, and the truncated
remedy were all invisible to the tests that existed to cover exactly this.

## Missing piece

No test rendered a toast. The truncation lived inside a `for` loop in the render
function, unreachable without a window, so "what the user sees" was not a value
any assertion could name.

## Remedy

Extracted `toast_style(kind)` and `toast_message(&DegradedToast)` out of
`render_toast_stack`, making the rendered line an ordinary testable value. Three
red-first tests (`/tmp/lane-enablement-red2.log`): the rendered toast carries
the state path; it names the command rather than a TOML fragment; and no path
length in `1..120` can panic the render (a sweep, so the property survives a
format change).

Fixes: the cap counts characters and is 240 wide, so a two-path disclosure plus
its remedy survives; the remedy is composed once in the loader
(`IgnoredReason::NotEnabled { remedy }`, from `ENABLE_COMMAND`) and carried on
the degraded bus, so the UI cannot invent its own; and the detail leads with the
remedy, which is the only clause the reader acts on.

Not a keystone case: the composed E2E PBT is headless and has no toast surface.
The pin is the three windowed-free unit tests above.
