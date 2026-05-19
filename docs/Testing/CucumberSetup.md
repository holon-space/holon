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

Steps are matched in `src/pbt/fixtures/matchers.rs`. Unknown steps are a **hard
error** — add a matcher (see *Extending*) rather than letting one slip.

### Actions (`Given` / `When`)

| Step | Transition |
|---|---|
| `the app is started` (optionally `… with loro`) | `StartApp` |
| `an org file "<name>":` + docstring | `WriteOrgFile` |
| `I focus block "<id>" in region "<region>"` | `NavigateFocus` |
| `I click block "<id>"` (optionally `in region "<region>"`) | `ClickBlock` |
| `I focus the editor of block "<id>"` | `FocusEditableText` |
| `I type "<text>"` | `TypeChars` |
| `I split block "<id>" at position <n>` | `SplitBlock` |
| `I indent block "<id>"` | `Indent` |
| `I outdent block "<id>"` | `Outdent` |
| `I press backspace` (optionally `<n> times`) | `DeleteBackward` |

`<region>` is `main` / `left` / `right`.

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
GPUI window** with a real `GpuiUserDriver` — keystrokes/clicks go through the
actual input pipeline, and it drives `E2ESut<Full>` (full suite + budgets):

```bash
cargo test -p holon-gpui --features pbt --test gpui_gherkin_replay
GHERKIN_FEATURE=/abs/path.feature cargo test -p holon-gpui --features pbt --test gpui_gherkin_replay
```

Default feature: `frontends/gpui/tests/features/ordinary_block_interaction.feature`.
Env: `GHERKIN_FEATURE` (path), `PBT_KEEP_WINDOW=1` (leave window open),
`PBT_MCP_PORT=<port>` (live MCP inspection).

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

- **New action verb**: add a matcher to `match_action` in `matchers.rs`,
  constructing the typed `E2ETransition` struct.
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
