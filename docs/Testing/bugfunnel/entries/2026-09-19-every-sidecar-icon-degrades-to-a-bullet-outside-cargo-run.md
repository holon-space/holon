---
id: 2026-09-19-every-sidecar-icon-degrades-to-a-bullet-outside-cargo-run
date: 2026-09-19
gap: ENVIRONMENT
secondary: PERCEPTION
status: OPEN
summary: >-
  A binary launched without `CARGO_MANIFEST_DIR` resolves its icons directory to
  a path that does not exist, so every SVG-backed icon silently falls back to
  the unknown-name bullet with no warning — the exact silent bullet the
  `IconName` parse gate was built to prevent.
---

## Bug

Found by the `dogfood-integ` lane. Screenshot:
`lane-logs/dogfood-integ-evidence/shots/03-integrations-crop.png`.

The sidebar's Integrations section paints six rows. Five of the six leading
glyphs are an identical small bullet, although the sidecars declare distinct
icons:

| Row | Declared icon | Painted |
|---|---|---|
| Claude History | `robot` | bullet |
| Google Calendar | `calendar` | bullet |
| Github | `link` | bullet |
| Gmail | `inbox` | bullet |
| Shopping list | `list` | hamburger (correct) |
| Todoist | `checkbox` | bullet |

The section header's own `sync` icon is a bullet too, as are the `notebook`
icons on every page row above it. Only `list` survives.

Nothing in the log mentions icons: `grep -c 'holon.icons' app.log` → `0`.

## Root cause

`frontends/gpui/src/render/builders/icon.rs:14-45`, `icons_dir()` resolves, in
order: `HOLON_WORKSPACE_ROOT`, then `CARGO_MANIFEST_DIR`, then
`current_exe()/../assets/icons`. Neither env var is set when the built binary is
launched directly, so it lands on `target/debug/assets/icons`, which does not
exist in this tree.

`render_icon_styled` (line 168-211) then takes the Unicode arm:

```rust
if let Some(svg_name) = icon_svg_name(name) {
    let path = icons_dir().join(format!("{svg_name}.svg"));
    if path.exists() { ...return the image... }
}
// Unicode fallback
.child(icon_char(name).to_string())
```

`icon_char` (line 135-142) looks the name up in the 28-entry `ICON_CHARS` table
and returns `ICON_CHAR_DEFAULT` (`•`) for anything absent. `robot`, `calendar`,
`checkbox`, `inbox`, `link`, `sync` and `notebook` are all absent, so all seven
become the same bullet.

Two disclosure failures compound it. `icons_dir()` warns only when
`current_exe()` fails or has no parent — never when the directory it chose is
simply not there. And the fallback carries an `// ALLOW(fallback)` marker that
excuses the branch as a default path, when for an SVG-only name it is in fact
the undisclosed degradation CLAUDE.md ranks last ("silently degrades to look
fine").

The parse gate does not help: `holon_api::icon_name::IconName` documents
membership in `ICON_NAMES` as "proof that the renderer draws it", and every name
above IS a member. The proof is false whenever the SVG directory is unreachable.

## Missing piece

`every_shared_name_resolves_to_an_svg_or_a_glyph`
(`frontends/gpui/src/render/builders/icon.rs:243-252`) checks only that
`icon_svg_name(name)` returns `Some` — a pure table lookup. No test asserts the
SVG FILE is reachable from the running binary, and no windowed rung compares two
differently-named icons and fails when they paint the same glyph.

## Remedy

Open. Three things, in order of value:

1. `icons_dir()` must fail loud (or warn once, prominently) when the directory
   it resolved does not exist — a whole icon set silently missing is not a
   default path.
2. Ship `assets/icons` next to the binary, or resolve it from a location that
   exists for a launched binary, so a non-`cargo run` launch is not degraded.
3. A rung that asserts the resolved icons directory exists and holds an SVG for
   every `ICON_NAMES` entry that `icon_svg_name` maps.
