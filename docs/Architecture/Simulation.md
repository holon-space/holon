# Simulation & Hypothetical State

Status: design note. The catalog-substrate ruling is RATIFIED and recorded as
[ADR 0031](../adr/0031-native-transition-catalog-and-macro-reification.md); the D5
scenario store is still PENDING RATIFICATION. This file holds the durable reasoning and
the use-case gallery; as each lands, the schema specifics move to
[Schema.md](Schema.md) and this note stays as the conceptual map.

## The three regimes

Hypothetical state serves workloads with opposite optimization targets. Naming
the regime first prevents building the wrong substrate:

| Regime | N | Objective | Substrate |
|---|---|---|---|
| **Search** | 10⁴–10⁷ candidates | compiled expression, cheap per step | Digital Twins, in-memory engine (holon-engine `ObjectiveDef`/`CompiledExpr` over `TaskMarking`) |
| **Alternatives** | 3–10 candidates | LLM judge / human | agent sessions (staging) or owned block subtrees |
| **Preview** | 1 | human | scenario store: fork + firing list, review, accept = fire |

Decisive constraints behind the table:

- **Search never touches block content.** Search needs an enumerable move set
  and a cheap computable objective; prose has neither (moves are unbounded,
  scoring needs an LLM, which caps evaluations at ~10² and destroys the search
  premise). Falsifier that would reopen this: a feature with a *compiled*
  objective over content. None known.
- **Search never persists per step.** Turso is single-writer; N parallel
  chains writing per-step rows serialize on one lock. Memory holds chains and
  trajectories; Turso stores the run's identity (seed, params, catalog
  version) and the winner materialized in scenario form, so promotion to a
  reviewable changeset is a copy, not a translation.
- **Two-tier fidelity.** Search runs on the cheap in-memory marking; the
  winner is re-staged once through the faithful substrate (Loro fork for
  blocks, twin overlay for external state) and its objective re-evaluated
  there before it is shown. Divergence between the two scores is a bug
  candidate (differential oracle; the §6.4 marking-equality experiment is the
  embryo).
- **Alternatives are not quarantined.** The scenario store exists for state
  that must not become real until a decision (trust-gated external writes).
  Research drafts and candidate plans are real PKM content — kept, linked,
  searched — so they live as blocks. Mechanically: the agent-session flow
  with a trust policy of auto-accept scoped to a namespace
  (`research/<topic>/<agent>`); provenance rides `OpOrigin`/`_provenance`.
  Synthesis across candidates is ordinary block reads — which the scenario
  store forbids by design (no cross-scenario reads).

## Use-case gallery

Classify a new idea by regime before designing anything for it.

**Search (compiled objective over twin attributes):**
- Schedule / agenda optimization: assign `SCHEDULED` dates across open tasks
  under constraints (deadlines, capacity, dependencies).
- "What if I take on this project": forecast task load / completion over
  estimated durations. The LLM-at-the-boundary pattern applies — an LLM
  estimates parameters *once* (effort, durations, dependencies) into twin
  attributes; the compiled engine runs the thousands of what-ifs. The LLM is
  never in the evaluation loop.
- Digital-Twin what-ifs over connector state (Todoist load, calendar
  density) with simulated external firings scored before anything is sent.

**Alternatives (small N, judged):**
- LLM deep research: N agents think divergently in owned subtrees; pick the
  best or synthesize. The thinking transcript is content, not quarantine
  material.
- Multi-alternative replanning: 2–3 candidate reorganizations of open work;
  the user picks one; losers are archived, not reverted.
- Draft / rewrite variants of a section as sibling subtrees.

**Preview (n=1, review-then-accept):**
- Agent dry-runs: an agent's staged changeset previewed against a Loro fork
  before its writes are trusted.
- Trust-gated external effects: a held firing whose consequence is shown on
  the connector's twin ("task X will be marked done in Todoist"); accept
  fires it, reject never calls the API.
- Bulk-operation preview (archive everything done before X, mass retag).

This gallery is deliberately open — add new cases *with their regime* so the
substrate decision is made consciously.

## Relation to the PN reification (ADR 0031)

Ratified and recorded in
[ADR 0031](../adr/0031-native-transition-catalog-and-macro-reification.md): the catalog
is Holon-native and macro-reified, and this engine is fed FROM it rather than being it.

User-intent operations as PN transitions pays off as vocabulary plus a shared
catalog, under three standing guards: no PN runtime in the live dispatch path
(accept = semantic replay through the normal dispatcher); the catalog is
derived from op definitions (macro reification), never hand-maintained in
parallel; adoption is incremental — only ops appearing in scenarios need
declarations. Effects below the declaration boundary (e.g. the consolidator's
`sort_key` minting) are not derivable and stay covered by the mutation-proven
marking-equality oracle. The catalog substrate must be loadable by BOTH the
in-memory engine and the real dispatcher — one catalog, two consumers, or the
differential oracle is meaningless.
