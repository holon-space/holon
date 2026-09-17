# D130: Colour from a column

**Lane:** `d130-colour-from-column`
**Workspace:** `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/d130-colour-from-column`
**Base:** `edf1602fd7a1` (main; sentinel `grep -c id_like_but_undeclared crates/holon-api/src/entity_reference.rs` = 2).
**Ruling:** D130.a (Martin, 2026-09-15). Build the generic DSL colour-from-column lever, then pin the condition-row severity colour with a rung on the `conditions` source.
**Depends on:** error-remedy Inc 2 (ADR 0035). Present on integration: `condition_bus.rs`, `condition_profile.rs`, `condition_detail.rs`, `row_source.rs`, `condition_source.rs`.
**Status:** Option A approved by the senior review of 2026-09-17, amendments folded in. Inc 0a and Inc 0b are LANDED in this workspace. Inc 1 to 3 are not started.
**Caveat:** an earlier draft of this design came from a cheaper model. It was not reused. Every claim below is re-derived from the workspace at the sentinel.

## 1. First principles

### Goal

One lever. A layout-doc author binds a widget's colour to a row's column value, and no Rust changes per feature.

The lever must also be the **only** place a colour decision is expressed. Today five sites turn a colour name into a pixel, each with its own vocabulary and its own silent fallback (section 2.1). A lever that adds a sixth makes the problem worse.

### Constraints

1. **The binding is layout-doc data.** The layout doc is the org file with `render` source blocks (`assets/default/index.org`, `assets/default/types/block_profile.yaml`). The DSL is Rhai-backed, parsed by `parse_render_dsl` (`crates/holon-api/src/render_dsl.rs:70`).
2. **Theme tokens, not literal colours.** A layout-doc colour names a theme token. A literal hex cannot follow the active theme, and both a light and a dark theme ship.
3. **Refuse at the DSL build boundary, never at paint.** A resolver runs inside the frame loop, so a refusal there is a panic in the UI or a fallback. The refusal belongs where a bad `source:` is already refused (section 2.2).
4. **Red-first PBT** per `.claude/skills/holon-feature/SKILL.md`. Colour is something the user sees, so the lever needs a GPUI windowed PBT. The headless keystone cannot see paint.
5. **Severity is the first consumer, not the design.** `ConditionSeverity` is already closed and typed (`crates/holon-api/src/condition_profile.rs:23`) with a stable string contract (`crates/holon-api/src/condition_source.rs:85`). It is a good first case because it is small, and a bad thing to special-case.

### What generality buys

The shape "one column value decides one visual property" recurs: task state, priority, due proximity, sync status, conflict status. `state_accent` (section 2.1) is the shipped proof that the shape recurs and that we have been paying Rust for it each time.

## 2. Architecture

### 2.1 Render-DSL style capabilities BEFORE Inc 0b

The table under it is the state Inc 0b replaced. It is kept because it is the case for the lever: five resolvers, five vocabularies, five silent substitutions. Section 3's Inc 0b records what now stands.

`RenderExpr` (`crates/holon-api/src/render_types.rs:779`) has no style node, and its `Object` fields are untyped (`HashMap<String, RenderExpr>`). A colour is an ordinary **string argument**: `text(..., #{color: "muted"})`, `icon(..., #{color: "primary"})`, `card(#{accent: ...})`.

Five resolvers turn that string into a pixel, and they disagree:

| # | Site | Accepts | Unknown becomes |
|---|------|---------|-----------------|
| 1 | `frontends/gpui/src/render/builders/text.rs:179` `resolve_color` | `muted`, `secondary`, `warning`, `error`, `success`, `#hex` | `foreground` |
| 2 | `frontends/gpui/src/render/builders/icon.rs:146` `icon_color` | `primary`, `accent`, `muted`, `warning`, `info`, `success`, `foreground` | `muted_foreground` |
| 3 | `frontends/gpui/src/render/builders/card.rs:3` `hex_to_hsla`, `card.rs:27` `parse_hex_u32` | hex only | grey `0x888888FF`, dark `0x2A2A27FF` |
| 4 | `frontends/gpui/src/render/builders/board.rs:69` `parse_hex` | hex only | the card background |
| 5 | `crates/holon-api/src/render_eval.rs:120` `resolve_color_name` | `red`/`green`/`blue`/`yellow`/`white`/`gray`, `muted`, `#hex` | `#FFFFFF` |

Resolver 5 has exactly one consumer in the tree, `frontends/waterui/src/render/builders/text.rs:33`.

Three further style paths bypass the DSL:

- `state_accent` (`crates/holon-frontend/src/value_fns/state_accent.rs:39`) maps a `task_state` column to a literal hex, with an `unknown -> neutral` fallback and a hardcoded palette.
- `severity_color` (`frontends/gpui/src/share_ui.rs:1975`) maps `ConditionSeverity` to literal RGBA.
- `RuleSpec` (`crates/holon-api/src/render_types.rs:53`) is a real declarative per-row override lever: `rules: [#{when: eq("level", 0), override: #{role: "page_title", show_bullet: false}}]`. It is consumed only by collection builders (`crates/holon-frontend/src/reactive_view.rs:2276`), and only for their own render-context flags and chrome props.

### 2.2 Where the colour string is, and where it must be refused

Validated by reading the tree, not inferred:

- The shadow builder stores the colour as an **untyped string prop**. `crates/holon-frontend/src/shadow_builders/text.rs:66` does `__props.insert("color", Value::String(c))`, where `c: String` came from the macro's typed `color: Option<String>` parameter.
- The GPUI renderer reads it back as a string at paint time: `frontends/gpui/src/render/builders/text.rs:52` `let color = node.prop_str("color").map(|s| s.to_string())`, then line 125 calls `resolve_color`. So today **nothing** validates the name, and the only inspection happens inside the frame loop.
- There is no typed-argument parsing for named props at the DSL level. `RenderExpr::Object` values are `RenderExpr`, and the name reaches the builder as a plain `String`.

So the parse seam is the **shadow builder**, and the tree already has the exact precedent at two levels:

- Same file, same statement list: `ellipsis` is validated at the build boundary, with the comment saying why (`crates/holon-frontend/src/shadow_builders/text.rs`, "Validated here, at the build boundary, rather than at render").
- `crates/holon-api/src/icon_name.rs` is the canonical shape for a closed name vocabulary: a `&[&str]` table, a newtype with `parse(&str) -> Result<Self, Unknown>`, an error that names near misses, and a gpui-side test asserting the table and the renderer agree. Its doc comment gives the same reasoning this lane needs ("a name arriving from a config file has no such reader ... parsed here, at the config boundary, and a typo is a refusal").

Refusal shape: `ViewModel::error(...)`. That is what `live_query` returns for a bad `source:` (`crates/holon-frontend/src/shadow_builders/live_query.rs:42`) and what `accordion`, `expand_toggle` and `outline` return for their own build-time faults. It is reachable from a non-raw builder body, because such a body is inlined into `build(ba) -> ViewModel` and `text` already early-returns there (`return ViewModel::from_widget("text", __props);`). No panic, and no conversion to `raw fn`.

**The builder alone is NOT enough, and implementation found the hole.** `text`,
`icon` and `spacer` are props-only widgets (`is_props_only_widget`,
`crates/holon-frontend/src/render_interpreter.rs:383`), so a collection's
`item_template` takes the `resolve_props` fast path
(`reactive_view.rs:1306`, `:1948`), which re-derives props from the same
expression through the macro-generated `resolve_props_from_args` and **never
runs the builder**. A bad colour inside an item template would therefore reach
the frame loop anyway. So the check also runs where the doc is PARSED:
`parse_render_dsl_with_engine` (`crates/holon-api/src/render_dsl.rs:207`) walks
the finished `RenderExpr` and refuses any literal `color` / `accent` argument the
theme does not define. Both paths now agree because validation is
accept-or-refuse with no rewrite: this is why `THEME_TOKENS` lists `secondary`
separately instead of canonicalising it into `muted`, which would have had the
full build write `muted` while the fast path wrote `secondary`.

The parse walk keys on the ARGUMENT NAME, not on a per-widget schema: `holon-api`
cannot see which params a builder declares (`WIDGET_META` lives in
`holon-frontend`). It therefore also refuses a colour arg on a widget that
silently drops it, which is how the gallery's inert `badge(#{color: ...})` was
found (section 2.3).

### 2.3 Hex literals: decision

**`ThemeToken` has NO hex arm, and no CSS-name arm.** Grep results, corrected after
implementation (the first pass counted only direct `lit_str("#...")` literals and
under-reported the gallery by an order of magnitude):

| Location | Hex hits | Verdict |
|---|---|---|
| `assets/default/*.org`, `assets/default/types/*.yaml` (the shipped layout docs) | **0** | Nothing to migrate. Shipped layout content uses only `color: "muted"`, `color: "primary"` (4x), `color: "success"`, `accent: "primary"`. |
| `assets/themes/*.yaml` (11 theme files) | many | NOT colour args. These files ARE the theme definitions; hex is the correct content. Never "migrate" these. |
| `assets/icons/**/*.svg` (5 files) | many `stop-color="#..."` | NOT colour args. This is artwork. A token has no meaning inside an SVG. |
| `crates/holon-frontend/src/widget_gallery.rs` | **41** hex strings: 3 direct `lit_str` literals, ~29 accent/colour args passed through Rust helpers, 9 `color_swatch` labels | Migrated in Inc 0a. |
| `assets/` CSS colour names | **0** | The CSS-name arm of resolver 5 is dead vocabulary. Drop it. |

No shipped layout doc needs an arbitrary colour, so nothing here is a finding for
Martin. Should one arise, it is a finding, not a hex arm.

**The gallery forced two migrations that the plan first mis-scoped:**

- `state_accent` returned a literal hex from a hardcoded dark-theme palette
  (`#7D9D7D` / `#D4A373` / `#C97064` / `#5A5A55`). It is used only by the dev
  gallery, never by a shipped layout doc, so its palette moves to the tokens the
  doc comment already names (`success` / `warning` / `error` / `muted`). It is a
  producer of hex into an `accent` arg, so the refusal is not landable without it.
- The gallery's `color_swatch` passed `#{color: hex}` to `badge`. **`badge`
  declares no colour parameter**, so that argument was dropped unread by the
  builder and again by the renderer: the "swatch" has only ever shown the hex as
  a text label. A palette exhibit that genuinely PAINTS a literal colour is a
  real want that this vocabulary cannot serve, and it is recorded in section 4
  as a finding rather than answered with a hex arm.

**Two live defects** fall out of section 2.1, and they motivate the lever:

- `assets/default/types/block_profile.yaml:135` ships `card(#{accent: "primary"})` and lines 162/181/189/196 ship `icon(..., #{color: "primary"})`. Resolver 1 does not know `primary` (silent `foreground`); resolvers 3 and 4 reject it as non-hex (grey/dark). Shipped layout content is being silently degraded today.
- `severity_color` and the future conditions row would be two authorities for one fact, which is the divergence ADR 0035 exists to remove.

### 2.4 Options

**Option A. `style_from(column, map)` as a value function.**
The map is layout-doc data: `text(col("label"), #{color: style_from("severity", #{info: "muted", warning: "warning", error: "error"})})`. It returns a theme-token name, which the builder parses into a `ThemeToken`. It reuses the shipped value-fn seam (`crates/holon-frontend/src/value_fns/mod.rs`), so `state_accent` becomes a deletion rather than a precedent.
Tradeoff: the map repeats at each use site, and totality needs the column's value vocabulary declared somewhere (section 2.5).

**Option B. A row-level `class` column plus one class-to-style table per collection.**
One table, no repetition, totality checkable in one place.
Tradeoff: the `class` column must be minted by the row map, which is Rust (`crates/holon-api/src/live_data_source.rs:62`). That puts a per-feature Rust colour decision back in the data layer, which is exactly what D130.a rules out. Rejected.

**Option C. `color: col("severity")`: the column value is the token.**
Minimal, and it already parses; `card(accent: col("accent"))` is in the dev gallery (`crates/holon-frontend/src/widget_gallery.rs:1064`).
Tradeoff: it couples the data vocabulary to the theme vocabulary, it cannot translate (`ERROR`, `err`, `2` are unmappable), and an unrecognised value silently falls back. Today's defect with a shorter spelling. Rejected as the general lever, but it is the degenerate case Option A must keep working.

**Option D. Extend `rules:` with a style override.**
Reuses a shipped declarative lever.
Tradeoff: `rules:` is a collection-level argument and its merged overrides are consumed by the collection builder, not by nested widgets. Colouring a nested `text(...)` would need inherited props visible to every widget, which makes every prop name ambiguous ("does `color` come from the row override or the widget argument?"). It also duplicates one map into N rules. Rejected for now, see the risk register.

### 2.5 Recommendation

**Option A**, with three refinements that make it a single lever rather than a sixth resolver:

1. **A closed theme-token vocabulary in `holon-api`**, shaped exactly like `icon_name.rs`. `ThemeToken` parses from a string against an explicit table; an unknown token is a loud refusal naming near misses. Canonical set: `foreground`, `muted`, `primary`, `accent`, `info`, `success`, `warning`, `error`, plus `secondary` (both it and `muted` resolve to one pixel in every frontend, and both are listed rather than canonicalised, for the fast-path reason in section 2.2).
2. **The refusal happens at the DSL boundary, in two places**: the parsed expression (covers the fast path) and the shadow builders (covers Rust-built expressions). Section 2.2.
3. **A declared value vocabulary for enum-valued columns.** `NamedSourceDef` declares column *names* today (`crates/holon-api/src/row_source.rs:230`). For the `severity` column the values are also closed (`info`, `warning`, `error`). Declaring them makes the layout-doc map checkable for **totality at doc load**: a map that misses a severity is refused, rather than falling back at runtime on the one value nobody tested.

The decisive tradeoff: Option A is the only option where the mapping is layout-doc data AND applies at any widget AND keeps the value-to-token translation out of Rust. Option B buys "one table" by putting the style decision back in Rust. Option C is the current silent fallback. Option D needs an inherited-props mechanism larger than the feature.

## 3. Increments

Each increment is independently landable and carries its own red-first PBT. Inc 0 is split at the ~400-line threshold the review set: 0a is the vocabulary and the refusal (headless), 0b is the resolution and the paint (windowed).

### Inc 0a: the token vocabulary and the refusal. LANDED

**Scope as built** (wider than first planned, because the refusal is not landable without the two migrations it forces):

| # | Change | File |
|---|---|---|
| 1 | `ThemeToken`: table, `parse`, `as_str`, near-miss error, serde. Modelled on `icon_name.rs`. | `crates/holon-api/src/theme_token.rs` (new), `lib.rs` |
| 2 | The parse-boundary walk refusing a literal `color`/`accent` the theme does not define. This is the seam that closes the item-template fast path. | `crates/holon-api/src/render_dsl.rs` |
| 3 | One shared `theme_token_prop` helper plus the four colour-accepting builders (`text.color`, `icon.color`, `spacer.color`, `card.accent`) refusing an unknown name via `ViewModel::error`. Covers Rust-built expressions. | `crates/holon-frontend/src/shadow_builders/{prelude,text,icon,spacer,card}.rs` |
| 4 | `state_accent`'s palette moves from literal hex to `success` / `warning` / `error` / `muted`. Forced: it is a hex producer into `card(accent:)`. Used only by the dev gallery, never by a shipped layout doc. | `crates/holon-frontend/src/value_fns/state_accent.rs` |
| 5 | The gallery's 41 hex strings migrate: ~29 accent args, 3 direct literals, and `color_swatch`'s inert `#{color: hex}` dropped (the exhibit now lists the token table, which is what a layout may name). | `crates/holon-frontend/src/widget_gallery.rs` |

**Red-first PBTs (headless):**
- `crates/holon-api/tests/render_dsl_colour_refusal.rs` (7 cases): the parse refusal, including the item-template shape the builder cannot see, and the two no-false-refusal guards (a per-row `col()` colour, and a known token).
- `crates/holon-frontend/tests/text_color_token.rs` (4 cases): the builder refusal, the shared vocabulary across all four widgets, and the no-rewrite rule.
- Red captured before implementing and again by neutralising both gates, so the log matches the tests as they now stand: 5 + 2 failures, each quoting the offending expression rather than a build error. See `lane-logs/inc0a-red.log`.

**Gate results:** `gate-compile` pass (incl. `check-web-arm`), `gate-arch` 7/7, `fmt --check` clean, clippy `-D warnings` clean on the touched crates, `nextest -p holon-api` 586/586, `nextest -p holon-frontend` 628/628, `keystone-smoke` 4 passed 0 failed, `keystone-known-reds` GREEN. `holon-waterui` is out-of-workspace (excluded for a known wgpu/naga issue) and checks clean standalone. Logs under `lane-logs/`.

**Does NOT close the shipped `"primary"` defect.** After 0a, `primary` is a *known* token but resolvers 1, 3 and 4 still cannot paint it. Inc 0b closes that, and 0b must follow in this lane.

### Inc 0b: one resolver, the paint plumbing, the windowed rung. LANDED

**Scope as built.** The five resolvers are gone; each frontend has ONE
token-to-pixel function taking a `ThemeToken`.

| # | Change | File |
|---|---|---|
| 1 | `ThemeToken` becomes an ENUM with `ALL`, so each frontend's mapping is an exhaustive match and a missing token is a compile error rather than a catch-all. | `crates/holon-api/src/theme_token.rs` |
| 2 | The GPUI resolver: one exhaustive `theme_token_color`, plus `theme_token_from_prop` (discloses a name that reached paint unvalidated) and `optional_colour_prop` / `packed`. | `frontends/gpui/src/render/builders/theme.rs` (new) |
| 3 | `text`, `icon`, `spacer`, `card`, `board` all resolve through it. `resolve_color`, `icon_color`, `hex_to_hsla`, `parse_hex_u32` and `parse_hex` are deleted. | the same five builders |
| 4 | The GPUI `text` builder declares what it paints, via `with_painted_colors`. | `frontends/gpui/src/render/builders/text.rs` |
| 5 | waterui gets its own `theme_token_color` over holon's default dark theme, and its text builder resolves through it. | `frontends/waterui/src/render/builders/theme.rs` (new) |
| 6 | `resolve_color_name` is DELETED (its only consumer was waterui; 0 shipped uses of its CSS vocabulary). | `crates/holon-api/src/render_eval.rs` |

**Plan correction.** The plan expected `RenderedElement` to gain `painted_fg` /
`painted_bg` and two projection sites to be updated. Implementation found that
`BoundsSnapshot::entries` already carry the full `ElementInfo`
(`crates/holon-layout-testing/src/snapshot.rs:43`), which has `painted_fg` since
`frontends/gpui/src/geometry.rs:487`. No `RenderedElement` change was needed, so
that risk row is retired rather than mitigated.

**Two decisions worth recording:**

- **`accent` resolves to `ThemeColor.accent`**, not `accent_foreground` as the
  legacy icon table had it. `apply_holon_theme` sets `accent` from holon's
  `primary` and `accent_foreground` from `text_primary`, so keeping the legacy
  mapping would have made the token `accent` paint the SAME colour as
  `foreground`: the exact conflation this lane removes. No shipped layout used
  `color: "accent"` before 0a migrated the gallery to it.
- **waterui's mapping reads holon's default dark theme** (`ThemeColors::default_dark()`).
  That frontend wires no live theme, so a token cannot follow the user's choice
  there. Disclosed in its module doc.

**Red-first PBT (windowed):** `frontends/gpui/tests/text_colour_windowed.rs`, two
cases. One asserts each of the nine tokens paints the `gpui_component` slot it
names, against the ACTIVE theme (the expected mapping is written out in the test
rather than shared with the renderer, so a change on either side fails). The
other asserts `primary`, `muted` and `foreground` paint three distinct colours in
ONE frame, so a theme swap cannot make it pass or fail on its own.

**Red evidence:** `lane-logs/inc0b-red.log`, captured after the paint plumbing
and before the resolver change, so the failure is the colour conflation and not a
missing tracker:
```
`text(#{color: "accent"})` painted [10, 10, 10, 255], but that token means the
theme's Accent slot ([245, 245, 245, 255]).
`color: "primary"` and `color: "foreground"` painted the same colour ([10, 10, 10, 255]).
```
The same two cases also fail on the Inc 0a baseline with the 0b builder changes
reverted, which is the red re-confirmed on a clean tree.

**Gates:** `gate-compile` clean; `gate-arch` 7/7; `fmt --check` exit 0; clippy
`-D warnings` exit 0 on holon-api, holon-frontend and holon-gpui; `nextest`
holon-api + holon-frontend 1214/1214; `keystone-smoke` 4 passed 0 failed;
`keystone-known-reds` GREEN; blinc `cargo check --locked` exit 0.

**The GPUI suite has pre-existing reds and load-sensitive flakes.** A full
`nextest -p holon-gpui --features pbt` run fails 17 tests on MY tree and 17 on the
0a baseline. Diffing the two sets: 12 failures are common and pre-existing, 5
were load-induced flakes (each passes in isolation on my tree, and 3 baseline
failures passed in my run), and the 2 tests that differ by design are
`text_colour_windowed` itself, red on the baseline and green here. **Zero
regressions.**

**Known limit, stated rather than papered over:** only `text` declares what it
paints, so the windowed rung proves the resolver through `text`. `icon`, `card`,
`board` and `spacer` call the same exhaustive function, and no colour-name
resolver remains in any frontend to check structurally, but their painted
colours are not in the layout record. Giving `card` and `icon` the same paint
declaration would let the shipped `card(accent: "primary")` and
`icon(color: "primary")` be asserted directly; that is a follow-up, not part of
0b.

**waterui is NOT compile-verified.** Its check cannot run in this environment:
`waterkit-screen`, a transitive dependency, fails to build its Swift helper
(`'CGWindowListCreateImage' is unavailable in macOS: Please use ScreenCaptureKit
instead`), and a `--target wasm32-unknown-unknown` attempt fails earlier in
`errno`. Both failures reproduce on the 0a baseline, so they are pre-existing and
not caused by this lane. The waterui change is small and mirrors the GPUI module,
and every symbol it names was read at its definition, but it has not been through
a compiler.


### Inc 1: the generic lever `style_from(column, map)`

**Build:** `style_from` as a value function. It takes a column name and a map of value to token, and resolves per row, as `state_accent` does today. The builder parses the value the function returns into a `ThemeToken` at the same boundary as 0a, so a map VALUE that is not a token is refused at doc load.

**Two rules the review fixed, each with its own refusal test:**

1. **`default` and totality are mutually exclusive.** A `default` token is REQUIRED when the column has no declared value vocabulary, and REFUSED when it has one: a default on a closed vocabulary hides a missing arm, which is the silent fallback this lane exists to remove. Two refusal tests, one per direction.
2. **An absent (NULL) column value with a declared vocabulary is refused at build, not defaulted.** "The row has no value for a column the source declares" is a source defect, not a style choice.

**Hosted:** the map's keys are checked against the declared vocabulary; the map's values against the token table.

**Red-first PBT (windowed):** a collection row whose `kind` column is `a` must paint the token the doc's map binds to `a`, in the same frame as a row whose value the map binds elsewhere, so the two are compared against each other. Today the map is not applied and both paint the plain colour.

**Done when:** the two rows paint two tokens, the two `default` rules each have a red-then-green refusal test, and no Rust names a specific column.

### Inc 2: the severity rung on the `conditions` source

**Build:**
- `ThemeToken::for_severity(ConditionSeverity)` in `holon-api`, total, no wildcard arm.
- Declare the `severity` value vocabulary on the `conditions` source (`crates/holon-api/src/condition_source.rs:32`).
- A shipped layout-doc collection over `source: "conditions"` whose `label` is coloured by `style_from("severity", ...)`.
- The toast's `severity_color` (`frontends/gpui/src/share_ui.rs:1975`) derives from `ThemeToken::for_severity`, so the layout row and the toast cannot disagree. This is the ADR 0035 single-authority point and it stays.

**Red-first PBT, two halves:**
- **Headless (totality).** The layout-doc severity map must cover every value the `conditions` source declares for `severity`; a map missing `warning` must be refused, in the style of `crates/holon-api/tests/row_source_parse_refusals.rs`.
- **Windowed (paint).** A condition row of severity `error` and one of severity `info` must paint two different tokens in one frame, each equal to the active theme's resolved value. This half is `Skipped` headless, exactly as `inv-paint-text-styling` is (`crates/holon-integration-tests/src/pbt/invariants/bodies/paint_text_styling.rs`), because paint needs `SutLayout`.

**Gate:** as Inc 1, plus `cargo nextest run -p holon-integration-tests --features pbt`, `just keystone-smoke`, `bash scripts/keystone-known-reds.sh <log>`.

### Inc 3: retire `state_accent`

**Build:** move the board layout doc to `style_from` and delete `accent_for_state` (`crates/holon-frontend/src/value_fns/state_accent.rs:39`) and its registration. Inc 0a already moved its palette onto tokens, so what remains is the deletion: the mapping is now expressible as layout-doc data.
**Red-first PBT:** strengthen the board rung (`crates/holon-frontend/src/widget_gallery.rs:2073` is the nearest assertion today) to assert the token the doc's map binds, proving the mapping left Rust.
**Done when:** no Rust function maps a column value to a colour.

## 4. Out of scope

- **The `rules:` lever.** Option D is not built or changed here.
- **A style editor UI.** The lever is layout-doc data.
- **Non-colour style properties.** Size, weight and spacing stay as they are.
- **A palette exhibit that PAINTS a literal colour.** RULED: D136.a (Martin, 2026-09-17) drops it. The gallery shows the token vocabulary instead, with no dev-only literal-colour widget and no parser exemption. Implemented: `color_swatch` is deleted, `token_swatch` renders one badge per token, and the section generates its rows from `ThemeToken::ALL` so it cannot drift from the vocabulary. Note for the record that the old `color_swatch` never painted a colour anyway: `badge` declares no colour parameter, so its `#{color: hex}` was dropped unread by the builder and again by the renderer.
- **`assets/themes/*.yaml` and `assets/icons/**/*.svg`.** Theme definitions and artwork, not colour args (section 2.3).
- **Blinc's and waterui's rendering.** Inc 0 only removes their divergent vocabularies; their pixels are not otherwise in scope.
- **Per-instance severity.** ADR 0035 rules severity is per kind. Not revisited.
- **The `conditions` row's prose.** `condition_source.rs` deliberately ships no prose column. Inc 2 adds no message text.

## 5. Risk register

| Risk | Why it bites | Mitigation |
|---|---|---|
| The lever becomes a sixth resolver | A new colour path beside five old ones is worse than none | 0a builds the one vocabulary and the refusal; 0b deletes the five. 0b must land in this lane. |
| 0a alone leaves `primary` accepted but unpaintable | Validated: resolvers 1, 3 and 4 cannot paint `primary` today | Stated in 0a's done-criteria. 0b closes it. 0a does not regress: that fallback already exists. |
| Text foreground is not captured per element | Validated: only `search_ui.rs` calls `with_painted_colors`, so a `text` run's `painted_fg` is `None` | The plumbing is an explicit part of 0b, with its own risk row here. |
| `RenderedElement` gains fields | Built in two places (`window_slice/components.rs:65`, `frontends/gpui/tests/sticky_accordion_pbt.rs:218`) | Additive `Option` fields. Both sites updated in 0b. |
| Blinc is behind a non-canonical feature | `CANON_FEATURES` is `holon-integration-tests/pbt`, `holon-integration-tests/web-arm`, `holon-gpui/pbt`, so `just gate-compile` does not compile `--features blinc` | 0b adds the explicit `cargo check -p holon-frontend --features blinc`. Probed at this base; result recorded in the lane log. |
| Totality cannot be checked for query sources | `live_query(#{sql: ...})` columns come from SQL, so no declared vocabulary exists | Inc 1 requires an explicit `default` token where no vocabulary is declared, and refuses one where it exists. Loud either way. |
| Windowed assertions against a hardcoded hex | Light and dark both ship; a constant passes in one theme and lies in the other | Every windowed rung compares token against token in one frame, or against the theme resolved at run time. Never a literal. |
| The toast keeps an imperative path | `share_ui.rs` draws toasts outside the render DSL, so the layout-doc rung cannot reach it | Inc 2 makes both read `ThemeToken::for_severity`. If that is dropped, the second authority returns and this lane does not close ADR 0035's gap. |
| The builder gate is bypassed by the props fast path | Validated: `text` / `icon` / `spacer` are props-only, so an `item_template`'s props come from `resolve_props_from_args` and the builder never runs | Closed in 0a by the parse-boundary walk, which both paths sit downstream of. A regression test pins the item-template shape. |
| Validation keys on the arg NAME, not a per-widget schema | `holon-api` cannot see a builder's declared params (`WIDGET_META` lives in `holon-frontend`) | Accepted: it also refuses a colour on a widget that would drop it, which is how the gallery's dead `badge(#{color: ...})` surfaced. A schema-keyed check is a later refinement, not a correctness gap. |
| The validation runs on every parse | `parse_render_dsl` also serves `parse_predicate` and the profile/asset load path | The walk is a cheap recursion over the finished expression, and it refuses only a literal `color` / `accent` that the table does not hold. Verified green across the full holon-api and holon-frontend suites. |
| The map repeats per use site | Option A's known weakness | Accepted for the first consumer. A named style block in the layout doc is a later increment, not a reason to choose Option B. |

## 6. Staleness-guard greps

Run from the workspace root. Each must return the stated shape, or this plan is stale.
Values below are as MEASURED after Inc 0a, so the "before" expectations are noted
where they no longer hold.

```bash
# The lever does not exist yet.
grep -rn "style_from" crates/ frontends/ assets/ | wc -l          # expect 0 before Inc 1

# The vocabulary exists and is wired at both boundaries (Inc 0a).
grep -rn "ThemeToken" crates/holon-api/src/ | wc -l               # expect 21 after Inc 0a (0 before)
grep -n "fn validate_colour_args" crates/holon-api/src/render_dsl.rs   # expect :245
grep -n "const COLOUR_ARGS" crates/holon-api/src/render_dsl.rs         # expect :230
grep -n "fn theme_token_prop" crates/holon-frontend/src/shadow_builders/prelude.rs # expect :213
grep -n "ICON_NAMES" crates/holon-api/src/icon_name.rs            # expect the shape to mirror

# ONE resolver per frontend, and no colour-name string match anywhere else (Inc 0b landed).
grep -rn "fn resolve_color\b\|fn icon_color\|fn hex_to_hsla\|fn parse_hex" frontends/gpui/src/ | wc -l   # expect 0
grep -rn "resolve_color_name" crates/ frontends/ | wc -l           # expect 0 (deleted)
grep -n "pub(crate) fn theme_token_color" frontends/gpui/src/render/builders/theme.rs    # expect :23
grep -n "pub(crate) fn theme_token_hex" frontends/waterui/src/render/builders/theme.rs   # expect :26
grep -c "ThemeToken::" frontends/gpui/src/render/builders/theme.rs # expect 10 (9 tokens + the signature)

# The paint plumbing the windowed rung reads (section 2.2).
grep -n "with_painted_colors" frontends/gpui/src/render/builders/text.rs # expect 1
grep -n "painted_fg" frontends/gpui/src/geometry.rs                # expect :568 region
grep -rn "with_painted_colors" frontends/ | grep -v "fn with_painted_colors"  # expect text.rs + search_ui.rs

# Hex is gone from the gallery and was never in a shipped layout doc (section 2.3).
grep -rEoh '#[0-9a-fA-F]{6}' assets/default/ | wc -l               # expect 0
grep -cE '"#[0-9a-fA-F]{6}"' crates/holon-frontend/src/widget_gallery.rs # expect 0 after Inc 0a (was 41)
grep -c 'named("color", lit_str(hex))' crates/holon-frontend/src/widget_gallery.rs # expect 0
grep -c 'color: "primary"' assets/default/types/block_profile.yaml  # expect 4
grep -c 'accent: "primary"' assets/default/types/block_profile.yaml # expect 1

# The severity authority this lane must unify (Inc 2).
grep -n "fn severity_color" frontends/gpui/src/share_ui.rs        # expect :1975
grep -n "pub const fn severity_name" crates/holon-api/src/condition_source.rs # expect :85

# The per-feature colour Rust Inc 3 retires. Its palette is tokens as of Inc 0a.
grep -n "fn accent_for_state" crates/holon-frontend/src/value_fns/state_accent.rs # expect :43
grep -c '#[0-9a-fA-F]\{6\}' crates/holon-frontend/src/value_fns/state_accent.rs   # expect 0

# Text foreground is still NOT captured per element (Inc 0b plumbing).
grep -rn "with_painted_colors" frontends/ | grep -v "fn with_painted_colors" # expect search_ui.rs :474,:487 only

# The paint plumbing Inc 0b depends on.
grep -n "painted_fg" crates/holon-pbt-core/src/capabilities.rs    # expect 0 before Inc 0b
grep -n "pub struct ElementInfo" crates/holon-frontend/src/geometry.rs # expect :31

# The named-source seam this lane binds to.
grep -n "pub const CONDITIONS" crates/holon-api/src/condition_source.rs # expect :32
grep -n "pub struct NamedSourceDef" crates/holon-api/src/row_source.rs  # expect :146

# Base sentinel for this lane.
grep -c id_like_but_undeclared crates/holon-api/src/entity_reference.rs # expect 2
```
