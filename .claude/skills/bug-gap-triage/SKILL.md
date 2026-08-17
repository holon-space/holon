---
name: bug-gap-triage
description: Classify every manually- or exploratorily-discovered bug into one of four escape gaps (coverage/oracle/environment/perception) and record it in the bug-funnel ledger, so QA investment is steered by data. Use whenever a bug is found OUTSIDE an automated test — by Martin dogfooding, by an agent driving the app, or reported by a user.
---

# Bug-Gap Triage

Every bug found outside an automated test is an **escape**: some automated layer
should have caught it and didn't. Before (or alongside) fixing it, spend ~2
minutes classifying *how* it escaped. The distribution of escapes — not
intuition — decides where test investment goes.

## The four gaps

Classify the bug into exactly ONE primary gap (note a secondary if genuinely
dual):

| Gap | Definition | Litmus question | Natural remedy |
|---|---|---|---|
| **COVERAGE** | The keystone PBT could not have *generated* the triggering interaction (missing transition, narrowed alphabet, precondition never satisfiable, driver rung not exercised) | "Is there a transition sequence in the current catalog+wiring that reaches this state?" | Extend the generator: un-narrow the alphabet, add the transition, satisfy the precondition |
| **ORACLE** | The PBT can generate the interaction, but no invariant would have flagged the defect | "If a case had hit this state, would any invariant have gone red?" | Add/strengthen an invariant in `crates/holon-integration-tests/src/pbt/composed/invariants/` |
| **ENVIRONMENT** | Prod wiring/timing/platform differs from the test's (platform-only code paths, embedder wiring divergence, boot/DDL ordering, real-vault scale, async races the settle masks) | "Does the failing code path even run in the keystone's wiring?" | Make test and prod more similar (CLAUDE.md rule): draw the wiring, add the platform rung (McpUserDriver), fail-loud boot guards |
| **PERCEPTION** | The defect is visual/UX with no formal invariant possible in the current harness (layout overflow, touch ergonomics, theme) | "Could any headless assertion express this?" | Windowed T3 PBTs, layout snapshots, agent exploratory dogfooding |

**Latency is NOT a perception gap.** Interaction→visible latency above budget
is a formalizable bug: the SLO is **p95 interaction→projection-visible
< 200ms** (measured by the `holon_latency` stages; see
`scripts/measure_latency.py`). Classify latency escapes as ORACLE (the budget
invariant doesn't exist/fire) or ENVIRONMENT (budget holds in test scale but
not vault scale).

## Procedure

1. **Classify** using the litmus questions above. If torn between COVERAGE and
   ENVIRONMENT: COVERAGE = the *interaction* is ungeneratable; ENVIRONMENT =
   the interaction is generatable but the *failing code path/wiring/timing*
   doesn't exist in the test environment.
2. **Write one file** under
   [docs/Testing/bugfunnel/entries/](../../../docs/Testing/bugfunnel/), named
   `YYYY-MM-DD-short-slug.md`. One escape, one file — never append to a shared
   file, so two lanes recording an escape from the same base cannot conflict.

   ```markdown
   ---
   id: 2026-08-16-page-switch-rendered-accordion-must-direct   # = the filename stem
   date: 2026-08-16
   gap: PERCEPTION              # ENVIRONMENT | COVERAGE | PERCEPTION | ORACLE
   secondary: ENVIRONMENT       # null when the bug is not genuinely dual
   status: FIXED                # FIXED | PARTIAL | MITIGATED | OPEN | NOTED
   summary: >-
     One sentence naming the observable defect.
   ---

   ## Bug
   What was seen, and how it was found (dogfooding, agent exploration, user
   report, verifier, code audit) — plus the task/lane it came from.

   ## Root cause
   The mechanism, with `file.rs:line` citations and the evidence (log paths,
   test names, measured numbers).

   ## Missing piece
   The concrete thing whose absence let it escape — "no page-delete
   transition", "iOS Focus/Blur never fire", "no gate executes windowed tests".

   ## Remedy
   What was done, or what remains open.
   ```

   Cite the entry by its `id` from code and docs. Do NOT cite it by position:
   pre-2026-08-17 comments saying "BugFunnel row 144" refer to an ordinal in a
   table that was reordered as it grew, and those numbers are not recoverable.
3. **Attempt keystone repro** (existing CLAUDE.md rule): check whether
   `crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs` can
   reproduce it. For COVERAGE/ORACLE gaps the fix INCLUDES closing the gap
   (add the transition/invariant) so the keystone goes red before the prod fix
   lands. For ENVIRONMENT gaps, note what prod/test parity work would be
   needed. For PERCEPTION, pin with a windowed/layout test or a gherkin
   `.feature` replay (`src/pbt/fixtures/gherkin.rs`).
4. **Validate — there is no counter to update.** The gap distribution is
   derived from the entry files, never stored, so a hand-maintained total
   cannot drift or merge wrongly:

   ```
   python3 scripts/bugfunnel.py check     # schema: gap, status, id vs filename
   python3 scripts/bugfunnel.py counts    # the distribution
   python3 scripts/bugfunnel.py list --gap ORACLE --status OPEN --since 2026-08-01
   python3 scripts/bugfunnel.py index     # regenerate INDEX.md (gitignored)
   ```

   `check` must pass before you land. Read `INDEX.md` or a filtered `list` to
   scan the funnel — never read all of `entries/` to answer a question about
   it.

## Why this matters

The 2026-07 baseline audit of ~21 documented escapes:
**ENVIRONMENT 12 · COVERAGE 7 · PERCEPTION 2 · ORACLE 0** — the invariant
catalog was never the weakness; generation and environment parity were.
Investment decisions (wiring draw, dogfood-explorer agent, alphabet
un-narrowing) were ranked directly from this distribution. Keep the dataset
alive so the ranking stays honest.
