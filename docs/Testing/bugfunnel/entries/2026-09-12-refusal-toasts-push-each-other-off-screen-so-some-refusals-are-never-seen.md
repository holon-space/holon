---
id: 2026-09-12-refusal-toasts-push-each-other-off-screen-so-some-refusals-are-never-seen
date: 2026-09-12
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  Seven connection-file refusals produced five visible toasts; the other two
  never appeared, with no overflow indicator, so a user cleaning up several bad
  files is silently told about only some of them.
---

## Bug

Found by the `dogfood-explorer` gate for `user-connections` (main
`f134df9ece6c`).

Six synthetic connection files, each inadmissible for a different reason, were
placed in one sandbox integrations directory and all six switched on. The boot
log carries seven disclosures — one per refused file, plus a leftover-state-file
warning.

The screen shows five toasts: `inlinesecret`, `linkedthing` (both its file
refusal and its leftover state file), `oldshape`, and `cleartexthost`. The
refusals for `futureshape` (declares `schema_version: 99`) and `foreignsecret`
(references a secret outside its own namespace) are in the log and nowhere on
screen. Screenshot `scratchpad/dogfood-uc/shots/05-refusals.png`, log
`logs/app4.log`.

The toast column fills the right edge of the window from y≈300 to the bottom.
There is no "2 more" affordance, no scroll, and no visual hint that anything was
dropped. A user who fixes the five they can see restarts and is then told about
the two they could not — or, if they instead concluded the other files were
fine, never learns.

The two that vanished are not the least important: the foreign-namespace refusal
is a security rule, and it is the one a hostile file would trip.

## Root cause

Not traced to a line by this pass. What is measured is the count: 7 disclosures
emitted, 5 rendered, 0 indication of the difference. Whether the stack caps its
length, or simply lays out more toasts than fit and clips, was not determined —
both produce this observation and the remedy differs, so the entry does not pick
one.

## Missing piece

PERCEPTION. This is a property of a rendered stack under a window height and
cannot be expressed headlessly. `frontends/gpui/tests/` has
`inert_integration_disclosure_windowed.rs` and
`integrations_row_narrow_window_windowed.rs`, so the windowed harness can both
raise disclosures and reason about constrained geometry — it has simply never
raised more of them than fit.

The natural pin is a windowed test that emits N disclosures at a fixed window
size and asserts either that all N are reachable, or that the count of hidden
ones is itself painted. The keystone PBT cannot express it.

## Remedy

FIXED. The root cause the entry declined to guess at is now measured: it was a
STATE cap, not a layout clip. `ShareUiState::push_toast`
(`frontends/gpui/src/share_ui.rs`) held `MAX_TOASTS = 5` and did
`self.toasts.remove(0)` — the two oldest refusals were dropped from the state
before any layout ran, which is why nothing on screen hinted at them.

- `push_toast` no longer evicts. The list stays bounded on its own, because
  every bus condition upserts by key, so its length is the number of DISTINCT
  conditions in effect.
- `render_toast_stack` caps what it PAINTS at five and, when there are more,
  paints a final tracked line reading "and N more not shown — open Settings ›
  Integrations for the full list". The count is a fact about the state, so it
  cannot drift from it.

A LATER ROUND replaced the fixed cap, because a fixed number is a guess about
payload length and this disclosure carries two absolute paths on cap-exempt
lines: one refusal can be 60px tall and another 700px. A verifier showed the
top toast still painting at `h=0` with an ordinary long path. The stack now
sizes itself to the viewport — it reserves the count line FIRST (so the count
can never be the clipped one), then admits toasts while their ESTIMATED height
fits the remaining budget. Zero admitted is a legal answer: a short window that
cannot hold even one of these disclosures shows the count alone, which is worse
than showing a refusal and far better than showing neither.

The estimate is measured, not derived: a box holding 1380 characters of
absolute path painted 694px, i.e. ~39 characters to a line, not the ~71 a 12px
advance predicts — long unbroken path segments do not wrap where prose does. It
rounds UP, because erring high shows one toast fewer (still counted) while
erring low paints a refusal at zero height.

The entry's judgement that a transient stack is the wrong home for a list of
files to repair still stands, and is now the measured conclusion rather than an
opinion; the overflow line points at the Settings section
rather than duplicating it. Making that section list refused files is a separate
piece of work and is NOT done here.

The PERCEPTION gap is closed by
`frontends/gpui/tests/refusal_toasts_reach_the_user_windowed.rs`, which raises
seven refusals in a real 1512x900 window and asserts each is either named on
screen or covered by a painted count. Before the fix it failed with "2 of 7
refusals are not named on screen and no painted line says \"2 more\"", listing
the five that painted — the dogfood observation reproduced exactly.

The rung now runs at TWO window heights (900 and 720) with a 565-character
directory, and judges GEOMETRY as well as text: every painted line must clear
the top inset with a full line's height, and the count line must do so too. That
last clause was added after a weaker version of it — "height > 0 and the bottom
edge is inside the window" — waved through a count line painted 2px tall at
y=0. A line clipped at the TOP reports exactly that.

## Attribution

PRE-EXISTING, not a `user-connections` regression. Verified by reading the tree
at `a5e161c0` (the commit before that lane): every line named above is already
there — `git show a5e161c0:<path>`. What the lane changed is reachability: it
made the files user-supplied, so a shape that had only ever been authored
in-tree became one a user can write.
