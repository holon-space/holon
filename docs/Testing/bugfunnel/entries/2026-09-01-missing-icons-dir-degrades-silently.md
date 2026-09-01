---
id: 2026-09-01-missing-icons-dir-degrades-silently
date: 2026-09-01
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  When the resolved assets/icons directory does not exist, every SVG icon in the
  app degrades to a bullet glyph with zero log output, so the UI looks
  intentional while all iconography is dead.
---

## Bug

Found by `dogfood-explorer` pass #2 over v0.0.23 (`d49ef0316a77`).

Launching the built binary directly (`./target/debug/holon-gpui`, the shape a
shipped binary takes) rendered **every** icon in the app as a bullet `•`: the
Journals tree icon, the Integrations header's sync icon, and all five
integration row icons. The Integrations sidebar looked like a plain unstyled
list, and the D53.c icon work appeared not to have landed.

It had landed. Relaunching the identical binary with `HOLON_WORKSPACE_ROOT` set
rendered all icons correctly (robot, calendar, inbox, document, checkbox,
notebook, sync). The feature was fine; the asset lookup was not.

The defect is not the fallback itself — an unknown icon name legitimately falls
back to a bullet. The defect is that a **missing icon directory** is
indistinguishable from a correct render: the app logs nothing at all.
`grep -ci "holon.icons|icons may render|assets/icons"` over the full app log
returns **0**.

This cost real investigation time in this session and produced a false "icons
are missing" reading of a shipped feature.

## Root cause

`frontends/gpui/src/render/builders/icon.rs:14-46` resolves the icons directory
in priority order: `HOLON_WORKSPACE_ROOT`, then `CARGO_MANIFEST_DIR`, then
`current_exe().parent()/assets/icons`. It emits a `tracing::warn!` only when
`current_exe()` *fails* or has no parent — never when the directory it settled
on does not exist.

`render_icon_styled` (same file, 182-226) then does:

```rust
if let Some(svg_name) = icon_svg_name(name) {
    let path = icons_dir().join(format!("{svg_name}.svg"));
    if path.exists() { /* render SVG */ }
}
// falls through to the Unicode glyph path
```

`path.exists()` is false for every icon, so every name falls through to
`icon_char`, and the SVG-only names (`robot`, `calendar`, `inbox`,
`document_text`, `checkbox`, `notebook`, `sync`) have no entry in `ICON_CHARS`
and land on `ICON_CHAR_DEFAULT` — the bullet.

Under `cargo run` (which sets `CARGO_MANIFEST_DIR`) and in the shipped macOS
bundle (`scripts/bundle-macos.sh:40-44` stages assets into
`Contents/Resources/assets` and symlinks `Contents/MacOS/assets` to it) the
directory exists and icons render. `target/debug/assets/icons` does not exist,
so only a direct raw-binary launch is affected.

Evidence: `/tmp/dogfood2-0901/logs/app2.log` (no icon lines, silent bullets) vs
`/tmp/dogfood2-0901/logs/app3.log`; screenshots
`shots/02-integrations.png` (bullets) and `shots/03-icons.png` (correct icons).

## Missing piece

No boot-time check that the resolved icons directory exists, and no
disclosure when it does not. Every automated path that renders icons — windowed
tests, `just live-verify` — runs under `cargo run` and therefore always has
`CARGO_MANIFEST_DIR` set, so no test can reach the missing-directory state. The
failing code path does not exist in the test environment: an ENVIRONMENT escape.

Because the shipped bundle stages the assets, this is not currently a
user-facing ship defect. It is a fail-loud violation that turns any packaging
regression into a silent, plausible-looking degradation rather than an error.

## Remedy

Open. Proposed:

1. Warn once at startup, naming the resolved path, when the icons directory does
   not exist — "icons will render as fallback glyphs". Cheap, and turns a silent
   class of packaging regression into a loud one.
2. Consider failing loud instead: if a bundled build cannot find its own assets,
   that is a broken install, not a degraded mode.
3. Add a packaging smoke check asserting the staged bundle resolves at least one
   SVG icon, so a future change to `bundle-macos.sh` cannot silently ship a
   bullet-only UI.
