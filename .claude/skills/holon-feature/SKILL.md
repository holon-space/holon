---
name: holon-feature
description: The red-first PBT contract for building ANY new Holon feature or behavior change — extend the keystone (headless) or GPUI (windowed) PBT so it fails BECAUSE the feature is missing (red for the right reason), implement, show it green, then pass the dogfood-explorer gate. Use when implementing a new feature or changing observable behavior. NOT for pure bug fixes (use bug-gap-triage) or pure refactors.
---

# Holon Feature Contract

Ratified by Martin 2026-07-22 (rules 4 & 5). Every new feature or behavior
change earns its way in through a **red-first PBT** and survives the
**dogfood-explorer** gate. No PBT ⇒ escalate to Martin BEFORE landing.

This is the feature path. Pure **bug fixes** go through
[`bug-gap-triage`](../bug-gap-triage/SKILL.md); pure **refactors** (no behavior
change) need neither. If you're changing what the system does or shows, you're
here.

## 1. Red-first — the covering PBT fails BEFORE you implement

Before touching implementation code:

1. **Extend the keystone or GPUI PBT** — add a transition and/or an invariant
   that exercises the new behavior. The generator must be able to *reach* the
   new state (transition), and an invariant must *judge* it.
2. **Run it and capture the RED output.** The failure must be **for the right
   reason**: the invariant/assertion fires because the feature is missing — NOT
   a compile error, missing symbol, or panic in scaffolding. Quote the failing
   assertion (expected vs actual), not a build error, as your proof.
3. **Implement** the feature.
4. **Run the same test GREEN.** Same transition, same invariant, now passing.
5. **The red log is part of the PR** — paste the red assertion output and the
   subsequent green run. A feature PR with no red-then-green log is incomplete.
   A red log committed under `docs/Testing/red-logs/` starts with a `BASE:` and
   a `TREE:` line and cites the seed and the command that reproduce it, never a
   `target/` path (ignored, purged with the workspace). Stamp the TREE id after a
   plain `jj status` snapshot, never with `--ignore-working-copy` inside the lane:
   that names the commit before the last edit. `just gate-arch` checks both
   (`scripts/check-red-logs.py`).

Model FIRST, then red-for-the-right-reason, then green (the standing
`pbt-model-first-red-green` directive). A property that *cannot* be made to go
red for the intended reason is itself a reportable gap — say so, don't fake a
red.

**A boundary parser is a feature too.** For each field that references another
field or section, list the legality rule and its rejection test. A round trip
only proves consistency: derive the expected result from an independent
`legal()` predicate, draw legal AND illegal inputs, and let every rejection
rule have drawn inputs that must fail by name
(`crates/holon-mcp-client/tests/transition_sidecar.rs`).

### After green — the testing loop on the real code

Red-first proves the test can see the feature. The testing loop of the
`property-based-testing` skill ("The testing loop") then hunts for the defects
the test cannot see yet. Every lane runs it after step 4:

1. **Widen the generators.** Draw all public operations and all input classes,
   failures and concurrency included, with no or very few restrictions.
2. **Run them on the real code** until a test fails or the code crashes.
3. **Analyse each failure.** A state that cannot happen in production: narrow
   the generator and write the reason down (`narrowed`). A realistic edge case
   the code misses: harden the code (`hardened`).

A test written before the code exists is not frozen. The lane runs it again on
the real code and analyses its failures; a test that ran only on stubs proves
nothing about the code.

The change summary lists each generator restriction with its reason, and each
failure with its verdict, under `Generators:` (the summary template of the
`verify-lane` skill). A restriction without a reason is a verifier finding.

## 2. Which tier — headless keystone vs GPUI windowed

| The feature is about… | Tier | Where |
|---|---|---|
| Data / state / behavior semantics (ops, projection, edges, task-state, org round-trip, query results) | **Headless keystone** | `crates/holon-integration-tests/` (transition + invariant into the composed PBT) |
| Anything the user **sees or touches** — layout, scroll, hover, caret, focus, selection, theme, drawer, keybinding | **GPUI windowed** | the windowed T3 PBTs (drive via GPUI, assert on rendered/layout state) |

A feature that spans both — a new op AND its on-screen affordance — gets **both**
a headless transition/invariant AND a windowed check. Don't settle for the
headless half when the user-visible half is the point.

## 3. Exception path — implementing without a covering PBT

Landing a feature with **no** headless or GPUI PBT covering its functionality is
allowed only in **rare** cases and **MUST be escalated to Martin BEFORE it
lands** — it is never your call to make silently.

- Escalate with: what the feature is, why a covering PBT is genuinely
  impractical (not merely inconvenient), and what you propose instead.
- Record the exception **and Martin's ruling** as a ledgered note:
  **prominently in the PR body** (there is currently no dedicated
  conventions-sibling file under `docs/Testing/` — BugFunnel.md is the escape
  ledger, not the exception ledger; if one is later added, mirror the note
  there). State it loudly enough that a reviewer cannot miss it.
- No ruling recorded ⇒ the exception is not granted. Do not land.

## 4. Dogfood-explorer — the LAST gate

Once the PBTs are green, a [`dogfood-explorer`](../dogfood-explorer/SKILL.md)
pass is the **final quality gate**. It drives the real GPUI app through the
embedded `holon` MCP and should discover **~90% of the bugs before Martin
does**. A feature is not done until it has been through this gate.

When dogfood finds a bug, the feature goes **BACK** — in this order:

1. **Enhance the PBTs to catch it FIRST** — add/strengthen the transition or
   invariant so a test goes **red for the right reason**. That red run is the
   proof the gap is now covered (the same red-first discipline as §1).
2. **Then fix** the bug.
3. **Then re-run dogfood** to confirm it's gone and nothing else regressed.

Every dogfood finding is also triaged with
[`bug-gap-triage`](../bug-gap-triage/SKILL.md) and appended to
`docs/Testing/BugFunnel.md` — that ledger is how the funnel stays honest.

## North star

- **Architecture:** [docs/Architecture/Model.md](../../../docs/Architecture/Model.md)
  (five layers, invariants) and [docs/Architecture.md](../../../docs/Architecture.md).
- **The ONE composed keystone:**
  `crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs` — one
  env-selected PBT; slices are scaffolding to delete.
- **PBT design guidance:** the `property-based-testing` skill — choosing tiers,
  oracles (metamorphic/differential), generators, and how to slice a property
  without trivializing it.
