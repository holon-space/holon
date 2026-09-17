---
id: 2026-09-17-waterui-silently-drops-an-undefined-colour-value
date: 2026-09-17
gap: ENVIRONMENT
status: PARTIAL
summary: >-
  A colour value the theme does not define reached the waterui frontend's paint
  and was dropped in release builds with no log and no visible marker, so the
  widget silently painted its default colour.
---

## Bug

`render_dsl` admits a column-bound colour (`text(col("content"), #{color:
col("kind")})`), because which name the value holds is a property of the ROW and
no parse-time check can judge it. waterui's resolution of that value returned
`None` on a name the theme does not define, after a `debug_assert!(false, ...)`
that is compiled out of release builds, and its `text` builder then rendered
with no foreground override at all — the widget's own default.

So a row whose `kind` held `corrigendum` painted as though no colour had been
asked for, with nothing on screen, in a log, or in a test to say so. GPUI's twin
of the same branch logs the value before painting its default; waterui declares
no logging crate, so its twin was silent. The module doc claimed the branch was
"unreachable by construction", which the reachability above contradicts.

Found 2026-09-17 by the adversarial verifier on lane `d130-colour-from-column`,
as finding 2 of `lane-logs/d130-inc0-verify.md`.

## Root cause

The refusal happened at the wrong layer, and only one of the two frontends that
needed it had a way to disclose.

`frontends/waterui/src/render/builders/theme.rs` returned `Option<Color>` and
absorbed the parse failure; `frontends/waterui/src/render/builders/text.rs:19`
turned `None` into "paint the default". The parse itself was already correct and
shared in spirit — `holon_api::theme_token::ThemeToken::parse` — but each
frontend owned its own copy of the decision about what to do when it failed, and
waterui had no channel to disclose through.

Evidence: `frontends/waterui/src/render/builders/theme.rs` (before), and the
verifier's reproduction path in `lane-logs/d130-inc0-verify.md` finding 2.

## Missing piece

**ENVIRONMENT.** The failing code path does not exist in any test environment
that runs.

waterui is not referenced anywhere in `crates/holon-integration-tests/`: the
keystone is headless and frontend-agnostic, and waterui is an out-of-workspace
crate (`frontends/waterui` sits in the root `Cargo.toml`'s `exclude` list). So
no PBT, no keystone rung and no windowed test ever executes waterui's paint
path, and nothing could have gone red.

Compounding it: waterui does not compile in this environment at all. Its
transitive dependency `waterkit-screen` fails to build a Swift helper
(`'CGWindowListCreateImage' is unavailable in macOS: Please use ScreenCaptureKit
instead`), and a `--target wasm32-unknown-unknown` attempt fails earlier in
`errno`. Both reproduce on the lane's base commit with none of its changes, so
neither is caused by this work — but the effect is that waterui's behaviour
cannot be exercised or verified here at all.

## Remedy

PARTIAL. The silent branch is gone; the environment gap that hid it is not.

Fixed in the same change that surfaced it, on the same lane:

- The refusal moved to ONE frontend-neutral place,
  `holon_frontend::theme_arg::resolve_colour_arg`
  (`crates/holon-frontend/src/theme_arg.rs`), which the shared shadow builders,
  GPUI and waterui all call. They can no longer disagree about which names are
  colours.
- waterui's `optional_colour_prop` returns `Result`, and `text::build` renders a
  refusal through that frontend's own convention for a refused widget: the
  message in red, the shape `frontends/waterui/src/render/builders/mod.rs`
  already uses for a failed `live_query`/`live_block`. An undefined colour value
  is now VISIBLE rather than dropped.
- Unit tests for the contract, including the row-value case
  (`a_value_the_theme_does_not_define_is_refused`), live in
  `frontends/waterui/src/render/builders/theme.rs`.

What remains OPEN, and why the status is PARTIAL rather than FIXED:

1. Those waterui tests have never been executed, because the crate does not
   compile here. There is therefore no red-for-the-right-reason log for this
   fix, and none is claimed.
2. No test environment runs waterui's paint path, so nothing stops the next
   silent branch. Closing that is parity work: a waterui rung, or a build
   environment where `waterkit-screen` compiles.
3. The per-row colour still reaches the frontends as a `&str`. The props-only
   fast path (`text`/`icon`/`spacer`) re-derives props without running the
   builder, and neither `resolve_props` nor `InterpretFn` carries an error
   channel, so a refusal there is still not representable. The three ways to
   close it are set out in `lane-logs/lane-report-d130-inc0b.md` under "Item 2";
   the recommendation is to accept the disclosure (GPUI logs, waterui renders,
   nothing is silent) until Inc 1's `style_from` makes per-row colours common.
