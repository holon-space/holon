> **⚠️ SUPERSEDED / PARTIALLY STALE (2026-07-05).** This document predates the completion
> of the γ-composition PBT endgame. The `E2ESut` monolith, the `declare_pbt_slice!` /
> `component_pbt!` macros, the standalone slice binaries, and the deleted `Sut*` capability
> twins referenced below were REMOVED on the `w1-pbt-endgame` branch. The live mechanism is
> the ONE composed keystone
> [`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`](../../crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs)
> plus the cfg(test) lib slice tests (`just pbt-lib-slices`). For the current architecture see
> [`docs/Architecture/Model.md`](../Architecture/Model.md). Kept for historical context.

---

# Gherkin (`.feature`) tests as PBT fixtures

Hand-authored Gherkin scenarios are replayed through the **same** system-under-test
(SUT), reference-state machine, and invariants as the property-based tests (see
[PBT.md](PBT.md), [PbtSlicing.md](PbtSlicing.md)). A `.feature` is just another
**fixture source** — a literal, human-readable sibling of the machine-captured
`*.json` regression fixtures.

> The old `tests/cucumber.rs` (Cucumber `World` + step definitions calling
> business logic directly) has been **removed**, along with the `cucumber`
> dependency. It never went through the SUT machine, so it could drift silently
> from production. The approach below replaces it.

## Mental model

A scenario is a captured regression you wrote by hand:

| Gherkin | Maps to | Effect |
|---|---|---|
| `Given` / `When` step | one `E2ETransition` (`FixtureStep::Action`) | applied to ref model + SUT; **invariants run after every step** |
| `Then` step | one `Assertion` (`FixtureStep::Assert`) | a positional check against the live SUT |
| `Background` | steps prepended to every scenario | re-run per `Scenario Outline` row |
| `Scenario Outline` + `Examples` | one fixture per data row | `<placeholder>` substituted in step text, docstrings, and table cells |
| `And` / `But` | inherit the previous step's type | `Then … / And …` → both asserts |

**Strict semantics, always fail loud:** a failed precondition, a `Then` before
the app starts (vacuous), or a failed assertion is a **hard panic** — never a
silent skip. A feature encoding a stale assumption breaks the build.

## Writing a feature

```gherkin
Feature: Splitting a block routes prefix to original and suffix to new

  Scenario: Split a parser-created block at a byte offset
    Given an org file "demo.org":
      """
      * HelloWorldFooBar
      :PROPERTIES:
      :ID: target-block
      :END:
      """
    And the app is started
    When I focus block "block:ref-doc-0" in region "main"
    And I split block "block:target-block" at position 10
    Then within 5 seconds block "block:target-block" contains "HelloWorld"
    And within 5 seconds block "block::split-0" contains "FooBar"
```

### Addressing blocks

- **Documents**: `block:ref-doc-N`, where `N` is creation order from `0`
  (`WriteOrgFile`/`CreateDocument` bump the counter; `StartApp` does not). The
  first authored doc is `block:ref-doc-0`.
- **Content blocks**: the verbatim org `:ID:` (e.g. `block:target-block`).
- **Split-created blocks**: `block::split-N` (N = the ref model's block counter
  at split time, `0` for the first split in a fresh fixture). Resolved to the
  real backend id via `resolve_ref_block_id`, so both focus and content can be
  asserted on the new block.

## Step vocabulary

Action steps (`Given` / `When`) are read by the **generated step registry**:
each transition declares ONE phrasing next to its own struct, and
`declare_e2e_transitions!` generates the renderer, the parser, and the
registration checks. `Then` steps still go through `match_assertion` in
`src/pbt/fixtures/matchers.rs` (Increment 4 will move them too). Unknown steps
are a **hard error** in both halves.

### Actions (`Given` / `When`) — one template per transition

Authoring a step is one attribute on the transition struct:

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I click block {block_id} in region {region}")]
pub struct ClickBlock {
    pub block_id: EntityUri,
    pub region: Region,
}
```

The derive REFUSES AT COMPILE TIME, at the offending span: a placeholder that
names no field (the error lists the real fields), a repeated placeholder, a
malformed template, a missing `#[step_template]`, and — the coverage rule — any
field the template does not name.

**Every field must be covered.** A field the step cannot carry must say so
explicitly with `#[step_default]` (`Default::default()`) or
`#[step_default(expr)]` (a constant the step implies, e.g. `StartApp`'s
`wait_for_ready: true`). A defaulted field is pinned: rendering a value that
differs from the declared constant fails LOUD, because `render_step` re-parses
its own output and compares serde values before returning it.

**Quoting is decided by the field's TYPE, never by the phrasing.**
`StepField::QUOTED` says whether a type renders inside `"…"` (with `\"` / `\\`
escapes) or bare. Strings, `EntityUri`, `Region`, and the JSON-carried payload
types are quoted; counters and booleans are bare. Parsing is anchored: literal
segments must match exactly and a quoted field consumes one balanced quoted run,
so a `content` field containing `" in region "` cannot confuse the parser.
Adding a new field type means implementing `StepField` for it once (in
`holon-pbt-core/src/step_vocabulary.rs`, or in the type's own crate when the
orphan rule requires it) — including `step_field_examples()`, which is where the
catalog-wide property test gets its values (`E2ETransition::step_catalog_examples()`
is their union).

**Docstrings.** A derived step takes NO docstring; attaching one is a refusal,
not a silently dropped payload. The one transition whose payload IS a document,
`WriteOrgFile`, hand-implements `StepVocabulary` and REQUIRES its docstring
(`an org file "<name>":` + org text). Data-table-carried fields do not exist yet;
until they do, a field is either in the template, defaulted, or a compile error.

Registration-time checks run in `E2ETransition::check_step_vocabulary()` (asserted
by `tests/step_vocabulary_laws.rs`): structurally ambiguous templates are refused,
and each struct's serde key set must match its declared fields. The same test file
holds the round-trip law `parse(render(t)) == t` over the whole catalog.

The phrasings the shipped `.feature` files use:

| Step | Transition |
|---|---|
| `the app is started` | `StartApp` |
| `an org file "<name>":` + docstring | `WriteOrgFile` |
| `I focus block "<id>" in region "<region>"` | `NavigateFocus` |
| `I click block "<id>" in region "<region>"` | `ClickBlock` |
| `I focus the editor of block "<id>"` | `FocusEditableText` |
| `I type "<text>"` | `TypeChars` |
| `I split block "<id>" at position <n>` | `SplitBlock` |
| `I indent block "<id>"` | `Indent` |
| `I outdent block "<id>"` | `Outdent` |
| `I press backspace <n> times` | `DeleteBackward` |

Every other transition in the catalog has a template too — read it in the
transition's own file, or dump the whole vocabulary with
`E2ETransition::step_catalog()`.

`<region>` renders as `main` / `left_sidebar` / `right_sidebar`; `left` and
`right` are still accepted on parse.

**Forms the registry REFUSES** (the regex matchers accepted them; a template has
no optional segments):

- `the app is started with loro` — the step cannot carry `enable_loro`, which is
  therefore pinned to `false`. Recording a loro-enabled boot needs optional
  template segments; that is specced with the recorder increments, not here.
- `I click block "<id>"` without `in region "…"` — the region is a template
  field, so it is always written.
- `I press backspace` without a count, and the singular `1 time` — the count is a
  template field and the literal is ` times`.

### Assertions (`Then`)

Each may be prefixed with `within <N> seconds ` to retry until it holds or the
budget elapses (use it for anything that depends on a render/CDC pass).

| Step | Assertion |
|---|---|
| `the widget contains "<text>"` / `the widget shows "<text>"` | root widget contains substring |
| `the widget shows exactly "<text>"` | root widget equals (trimmed) |
| `block "<id>" contains "<text>"` / `… shows "<text>"` | that block's rendered subtree contains substring |
| `focus is on block "<id>"` / `block "<id>" is focused` | SUT focus resolves to `<id>` |

Note: the root layout references leaf blocks as `live_block` nodes, so block
**content text** lives in the block's own subtree — use `block "<id>" contains …`,
not `the widget contains …`, to assert content.

## Where features live & how to run

### Headless (fast) — alongside JSON fixtures in a slice

A slice declared with `declare_pbt_slice! { …, fixtures_dir: "tests/fixtures/<slice>" }`
auto-generates a `<slice>_fixtures` test that replays **every `*.json` and
`*.feature`** in that dir before the proptest sweep:

```bash
cargo test -p holon-integration-tests --features pbt \
  --test <slice> <slice>_fixtures
```

Headless replay runs the **slice's declared invariants** (a deliberately narrow
subset for speed) and **no** non-functional budgets — identical to that slice's
random sweep.

### Headless with full rigor

Because `E2ESut<V>` is itself a `StateMachineTest`, you can replay a single
feature through the **full** invariant suite **+ SQL/memory/runtime budgets**,
still headless, by choosing the SUT type args:

```rust
// narrow (slice wrapper): just the slice's invariant
run_feature_strict::<SplitBlockContentPbtMachine, SplitBlockContentPbtSut, SqlOnly>(path);
// full suite + non-functional budgets:
run_feature_strict::<VariantRef<SqlOnly>, E2ESut<SqlOnly>, SqlOnly>(path);
```

### Real GPUI window

`frontends/gpui/tests/gpui_gherkin_replay.rs` replays a feature through a **real
GPUI window** via the composed windowed path (increment 4c): gestures go through
the window's `SimUserDriver`, and it drives `ComposedSut<WideE2E>` with the full
composed invariant catalog checked every tick. Features must be **post-boot** and
authored against the wide seed (`structural-page` → `parent`/`c1`/`c2`) — no
`Given an org file` / `app is started` ceremony (the wide seed IS the boot org,
the same convention as the headless `composed_split_gherkin` fixture):

```bash
cargo test -p holon-gpui --features pbt --test gpui_gherkin_replay
GHERKIN_FEATURE=/abs/path.feature cargo test -p holon-gpui --features pbt --test gpui_gherkin_replay
```

Default feature: `frontends/gpui/tests/features/ordinary_block_interaction.feature`.
Env: `GHERKIN_FEATURE` (path).

> Only the GPUI input pipeline can catch focus/keystroke-routing bugs (e.g. a
> page-level editor swallowing a leaf's keystrokes) — those are invisible to the
> headless renderer.

## How it works

```
*.feature ─┐                              ┌─ headless slice SUT (narrow invariants)
*.json  ───┼─ FixtureSource ─ replay_steps ┤
           │   (one NamedFixture           └─ E2ESut<V> (full suite + budgets)
           │    per scenario / file)
           └─ per step: precondition (hard-panic on skip)
                → M::apply → S::apply → S::check_invariants
                → Assert: S::evaluate_assert
```

Key pieces (`crates/holon-integration-tests/src/pbt/fixtures/`):

- `gherkin.rs` — parse `.feature` → `Vec<NamedFixture>` (Background prepend,
  Outline/Examples expansion, `<placeholder>` substitution); `GherkinFixtureSource`.
- `json.rs` — `JsonFixtureSource` over captured `*.json`.
- `matchers.rs` — step text → `E2ETransition` / `Assertion`.
- `assert.rs` — the `Assertion` enum + capability-bounded `evaluate_assertion`.
- `mod.rs` — `FixtureSource` / `FixtureStep` / `FixtureAssertable`, and the
  medium-agnostic core **`replay_steps`** shared by headless + GPUI. The
  `after_start_app` hook is the only seam (no-op headlessly; window + driver
  injection for GPUI).

The shared GPUI window plumbing lives once in
`frontends/gpui/tests/pbt_harness/mod.rs::run_in_gpui_window`; the random PBT
(`gpui_ui_pbt`) and the replay (`gpui_gherkin_replay`) differ only in the runner
closure they pass it.

## Extending

- **New action verb**: add `holon_macros::StepVocabulary` + `#[step_template("…")]`
  to the transition's struct (see *Step vocabulary*). Nothing central to edit —
  the registry, the ambiguity check and the parser are generated from the one
  variant list in `declare_e2e_transitions!`.
- **New assertion**: add a variant to `Assertion` (`assert.rs`), handle it in
  `evaluate_assertion` (bounded on the capability traits it needs), and add a
  matcher to `match_assertion`.

## Why not run features through proptest's loop directly?

proptest-state-machine is a **generator + shrinker**: it has no literal-replay
entry point, and its persisted regressions are **RNG seeds** (which break when a
strategy changes), not transition values. The project already chose literal
fixtures for this reason; `.feature` files reuse that. The replay does, however,
reuse proptest-state-machine's **per-step primitives** (`M::preconditions`,
`M::apply`, `S::apply`, `S::check_invariants`) — only the outer loop differs.
A `Then` is modeled as a distinct `FixtureStep::Assert` (not a transition)
because assertions are parameterized, positional, and non-mutating, and must not
pollute the random transition space.
