# Contribution & Revenue-Sharing Strategy

This document captures the thinking around how external contributions to Holon
should be incentivized, given that Holon is a B2C product that the maintainer
needs to earn a living from.

## Problem Statement

- Holon is intended to be the maintainer's primary income source.
- If revenue flows only to the maintainer, external contributors have weak
  motivation to help.
- The codebase architecture deliberately minimizes the need for plugins; the
  main extension point is **MCP sidecars** (data-source connectors).
- The maintainer cannot personally write sidecars for every system because
  that requires access to each target system.
- Question: how to fairly compensate (or otherwise motivate) contributors
  without creating disproportionate overhead or undermining the business?

## The Slicing Pie Model (Reference)

[Slicing Pie](https://slicingpie.com/) (Mike Moyer) is a dynamic equity-split
framework where each contributor's share is recalculated continuously based
on the **fair market value of what they put at risk**.

### Mechanics
- Every contribution (time, money, supplies, IP, facilities) is converted into
  **slices** via a normalized multiplier:
  - **Cash**: 4× the dollar amount (highest risk).
  - **Time**: unpaid fair-market salary × 2.
  - **Ideas / IP / equipment**: 2× fair market value.
  - **Paid contributions**: 1× (lower risk because reimbursed).
- Each person's % = their slices ÷ total slices, recomputed on every
  contribution.
- The split stays dynamic until a **break-even event** (profitability,
  funding round, sale), then freezes.

### Recovery on Departure
- Fired for cause / resigns without good reason → loses non-cash slices.
- Fired without cause / resigns for good reason → keeps time slices, loses
  unvested.
- Cash contributions are never clawed back.

### Why It's Interesting Here
- Solves the upfront-fairness guess problem.
- Aligns reward with risk borne.

### Why It Doesn't Map Cleanly to Holon
- Original model freezes at break-even; an ongoing-revenue product needs
  decay or a rolling window.
- Bookkeeping burden falls on the maintainer (the exact thing contributors
  are supposed to relieve).
- "Continuous dynamic split" is unproven for OSS revenue share.

## Adapted Slicing Pie for Software Revenue (Sketch)

If applied at all, the shape that could work:
1. **Pie = a fixed % of net revenue** (e.g. 30–50%), not equity. The
   maintainer keeps the rest as the "operator share" covering fixed costs,
   support, infra, legal, sales.
2. **Rolling window**: contribution slices decay over 24–36 months, so
   incentives track *current* contribution, not contributions made years ago.
3. **Multipliers (simplified)**:
   - Code merged: hours × fair hourly rate × 2.
   - Cash / infra: reimbursement first, then 4× into slices.
   - Maintainer's own time counts at the same rate as everyone else's; the
     operator share covers the founder asymmetry.
4. **Lightweight intake**: PR merged → log hours → maintainer confirms →
   slices issued. Anything heavier kills participation.

### Failure Modes to Design Against
- **Gaming via trivial PRs** — mitigation: hours per merge are subject to
  approval; trivial work earns trivial slices.
- **Bus factor of fairness** — if the maintainer is sole arbiter, distrust
  builds. Publish the ledger.

## Alternatives Considered

### 1. Bounties (Gitcoin, Algora, Replit Bounties)

**Pros**
- Dead simple; no ongoing accounting.
- Natural fit for well-scoped tasks; market-clears the price.
- Contributor risk is bounded.
- Tax/legal is just contractor 1099-style.

**Cons**
- Transactional, not aligned with project success.
- Bad fit for fuzzy work (architecture, refactors, design, support, docs).
- Maintainer bears all upfront capital risk.
- Tends to attract one-and-done contributors.

**Fits Holon when** revenue is steady and specific features need outsourcing.

### 2. Retroactive Public Goods Funding (Optimism RetroPGF, drips.network)

**Pros**
- Rewards real impact, not promises or hours.
- Avoids hour-padding gaming; only outcomes count.
- Works for fuzzy contributions.
- Naturally decays — old contributions get smaller share of newer rounds.

**Cons**
- Needs a funding pool big enough to matter.
- Subjective scoring requires a trusted council; hard with <5 contributors.
- Contributors don't know what they'll earn until the round closes.
- Per-round overhead is significant.

**Fits Holon when** there are ~10+ active contributors and pooled revenue.
Premature now.

### 3. Dual-License + Revenue-Share Contracts (Sentry, MariaDB, Sidekiq)

**Pros**
- Predictable revenue (license sales) is easier to split.
- Contributors get ongoing royalties — strong alignment.
- Legal precedent exists.
- CLA gives legal control to enforce splits.

**Cons**
- Only works with a commercial-license business model.
- CLAs are politically fraught in OSS.
- Per-contributor negotiation creates resentment and doesn't scale.
- Maintainer becomes an employer-ish entity with payroll/tax obligations.

**Fits Holon if** monetization is B2B (it isn't — Holon is B2C).

### 4. Sponsorship Pools / Open Collective / drips.network splits

**Pros**
- Fully transparent, low-overhead, no contracts.
- Splits can change over time without renegotiation.
- Works for any revenue source.

**Cons**
- Total income is usually small.
- Splits are still subjective.
- No retention mechanism — departing contributors keep their split.
- Doesn't solve "I need to earn a living."

**Fits Holon as** a secondary channel, not primary income strategy.

### Comparison

| Model                     | Predictable income? | Long-term alignment? | Overhead | Fits early-stage solo? |
|---------------------------|---------------------|----------------------|----------|------------------------|
| Bounties                  | Per task            | Low                  | Low      | Yes                    |
| Retro PGF                 | Per round           | High                 | High     | No                     |
| Dual-license + rev-share  | Yes                 | High                 | Medium-High | Only if B2B          |
| Sponsor pools             | No                  | Medium               | Low      | Yes (small $)          |
| Slicing-Pie-adapted       | Maybe               | Highest              | High     | Risky                  |

## MCP Sidecars

Holon's main extension point is MCP sidecars — connectors to external
systems (Todoist, JIRA, Linear, Gmail, Calendar, Drive, …). Sidecars are
**low-effort to write** but require access to the target system, which the
maintainer can't always provide.

### "Free Months" as Currency

Pattern in the wild: Dropbox referrals, Notion credits, 1Password family
plan tricks.

**Reactions cluster into three groups**
- **Loved by existing happy users** — face value ≈ 100%; writing a sidecar
  for a system they already use feels like a hobby that pays for itself.
- **Tolerated by prospective users on the fence** — "I'll write the sidecar
  I need; free months let me try the paid tier risk-free." Best acquisition
  funnel: ship a sidecar, become a paying user, evangelize.
- **Rejected by professional OSS contributors** and people in regions where
  the subscription price isn't meaningful — they prefer cash.

The honest critique: *"You're paying me in your own scrip."* Only works if
the product is something they'd buy at full price.

### Proportional-to-Paying-Users Mechanic

Solves the gaming problem (a sidecar nobody uses earns nothing) and aligns
incentives toward sidecar quality.

**Concrete shape**
- Each paying user with sidecar X *enabled and used in last 30 days*
  contributes 1 unit toward sidecar X's author.
- Author earns `floor(active_users / N)` free months per month, where N is
  tuned (10? 20?) so popular sidecars earn meaningfully and niche ones earn
  occasionally.
- Cap stockpile at ~24 months; beyond that, switch to cash or plateau.
- Multiple sidecars stack.

**Why this fits B2C MCP sidecars specifically**
- Contributor is likely already a paying user; free months are real money
  to them.
- Bounded work (days, not months) — modest reward is appropriate.
- Measurable without subjective scoring; telemetry suffices.
- No CLA, no equity, no payroll, no tax forms.

### Pitfalls to Design Around

1. **Telemetry trust** — track sidecar usage without being creepy. Aggregate
   counts only, opt-out, document clearly.
2. **Quality vs. quantity** — someone might publish 20 mediocre sidecars to
   farm months. Mitigations: lightweight pre-publish review; "official" tier
   the maintainer curates.
3. **Forks and competing sidecars** — split rewards by which one users
   actually enable, not first-merged-wins.
4. **Abandonment** — original author disappears, fixer should inherit
   share. Track current maintainer of record; transfer rewards on handoff.
5. **Bootstrap problem** — when there are 50 paying users total, proportional
   rewards are tiny. Seed with flat bounties for the first ~10 sidecars,
   then transition to proportional.

## Do Monetary Incentives Even Make Sense for Low-Effort Contributions?

For genuinely low-effort, scratch-your-own-itch contributions, monetary
incentives are often **counterproductive**. There's research on this
("crowding-out effect" — Deci, Lepper, Ariely).

### The Crowding-Out Effect
- Daycare fines for late pickup → late pickups *increased* (parents
  reframed it as a paid service).
- Paid 1% commission collected *less* than unpaid volunteers; paid 10%
  beat unpaid; paid 0% beat both.
- Swiss nuclear-waste site survey: 51% accepted compensation-free; 25%
  accepted with compensation.

Mechanism: small payments signal "this is a market transaction." If the
payment is below labor-market rate, people compare and decline. Without
payment, they compare to "is this worth doing?" and many say yes.

### When Monetary Incentives Do Help
- When the task is **boring** (docs translation, accessibility audits,
  license header cleanup).
- When the contributor doesn't *use* the thing they're contributing to.
- When there's a **status-conferring** dimension to the payment.

### Empirical Pattern of OSS Contributions
1. **"Scratch your own itch" majority (60–80%)** — money irrelevant,
   recognition mildly nice.
2. **"Portfolio builders" (10–20%)** — want their name visible. Money
   distracts from the signal.
3. **"It's my hobby" tail (5–10%)** — often most prolific. Thanks and a
   t-shirt go further than a check.
4. **"Professional contractor" segment (<5%)** — only show up for paid
   work. Useful for boring stuff, won't form a community.

### Math Check
A sidecar takes 1–2 days for someone who knows the target system; fair-market
rate is $500–$2000. Holon can't pay that. Anything less feels insulting.
*Not* paying sidesteps the comparison entirely.

## Recommendation

Given Holon is B2C, low-effort plugins, scratch-your-own-itch contribution
profile:

1. **Skip monetary rewards at launch.** Spend energy on making the
   contribution path frictionless and recognition prominent.
2. **Make publishing trivial.** `holon sidecar publish` as one command.
   Friction kills more contributions than missing rewards.
3. **Visible attribution.** Author name on the sidecar listing in-app.
4. **Fast, friendly review.** Most OSS contributors quit because the
   maintainer ghosted their PR for 3 months, not because they weren't paid.
5. **A public "sidecars I'd love" wanted-list.** Tells people what to
   build; pre-validates demand.
6. **Maintainer status / commit access** for prolific contributors. Costs
   nothing; means a lot.
7. **Genuine thanks.** A personal email when someone publishes.

### When to Revisit

If after a year there is: significant user base, demand for sidecars to
systems nobody on the contributor base uses (enterprise CRMs, niche
scientific tools), *then* introduce a small targeted bounty program for
those specific gaps. Not a general reward system — a "we'll pay $500 for a
working Salesforce sidecar" wanted-poster. That's understood as
commissioning specific work, not motivating volunteers.

The "free months" + proportional-to-paying-users model is a fine fallback
if demand outpaces supply, but introducing it preemptively will likely
*reduce* contributions, not increase them.

## Other Contribution Surfaces in the Vision

A scan of [Vision.md](../Vision.md) and children ([AI.md](../Vision/AI.md),
[LongTerm.md](../Vision/LongTerm.md), [PetriNet.md](../Vision/PetriNet.md),
[UI.md](../Vision/UI.md)) surfaces additional areas where external
contributions might matter. Most are *possible* contribution surfaces; the
question for each is whether they're worth designing incentives for, or
whether the contribution model should be "frictionless path + recognition,
no rewards."

### Likely Worth Discussing

These are areas where the maintainer probably can't or shouldn't do
everything alone, and where the access / domain-knowledge constraint
mirrors the MCP sidecar pattern.

#### 1. Item-Type Extensions for Third-Party Systems
[Vision.md §1.0](../Vision.md), [Challenge 3](../Vision.md). JIRA sprints,
Todoist sections, Linear cycles, Asana custom fields. These are
trait-impl-shaped contributions tied to specific systems — same access
constraint as sidecars, often written by the same person. **Open
question**: should this just fold into the sidecar reward bucket?

#### 2. Custom Visualizations / Widget Library
[Vision.md §2](../Vision.md). Tables, kanban, calendars are core; but
Gantt charts, mind-maps, timeline views, parametric dashboards, gallery
grids, etc. are open-ended. The current widget-registry seam (see
`render_dsl_widget_registry_seam` memory) is explicitly designed for this.
**Open question**: should there be a "widget gallery" with author
attribution, and does usage-based reward make sense (e.g., adoption of a
widget by templates)?

#### 3. UI Themes
[Vision.md §7](../Vision.md). Classic OSS contribution surface; users
build for self-expression. Almost certainly **no rewards** needed —
recognition + a discoverable gallery is enough. The pattern is well-known
(VS Code themes, Obsidian themes).

#### 4. Local AI Backends / Model Adapters
See deep-dive in [Deep Dive: AI Backends](#deep-dive-ai-backends-4) below.
**Conclusion**: don't apply the sidecar reward model. The adapter layer is
maintainer-owned; the contribution surface is community-tuned **model
recipes** (which model + prompt + params for which task / language /
hardware), with attribution only.

#### 5. Plain-Text File Format Adapters
See deep-dive in [Deep Dive: File Format Adapters](#deep-dive-file-format-adapters-5)
below. **Conclusion**: split into one-shot migration adapters (treat as an
**affiliate program** with per-conversion rewards, curated tier only) and
bidirectional sync adapters (fold into the sidecar usage-based bucket).

#### 6. Petri-Net Token Type Extensions
[PetriNet.md §Five Primitives](../Vision/PetriNet.md), and the
[LongTerm.md](../Vision/LongTerm.md) "grow into a life-OS" framing. The
canonical token types (Person, Organization, Document, Monetary,
Knowledge, Resource) are extensible. Domain-specific tokens — Health,
Fitness, Recipe, Media, FinancialTransaction, Learning — are natural
contribution surfaces. **Open question**: do these need a structured
review since they affect the entity graph's semantics?

### Probably Not Worth Designing Incentives For

These are real contribution opportunities, but the standard OSS pattern
(frictionless contribution + recognition) is sufficient.

- **Automation rule templates / PRQL recipes** ([Vision.md §3](../Vision.md))
  — community-contributed "when X, do Y" templates. Zapier-template-shape.
- **Documentation, tutorials, video walkthroughs** — standard OSS.
- **Localization / translations** — not in Vision but inevitable for B2C.
  Standard OSS (Crowdin, Weblate, etc.).
- **Conflict resolution heuristics** ([AI.md §Smart Resolution](../Vision/AI.md))
  — domain-specific rules. Better as a structured rule format users can
  share, like userscripts.
- **Cross-platform polish** ([Vision.md §8](../Vision.md)) — bug reports
  and patches for Linux distros, accessibility, mobile gestures. Standard
  OSS.

### To Decide Before Designing

1. **Does the rewards model unify across all surfaces, or is it
   per-surface?** Sidecars (data-access constrained) feel different from
   themes (taste-driven) feel different from token types (semantic).
   *(Provisional answer: no, three tracks — see
   [Three Reward Tracks](#three-reward-tracks) below.)*
2. **Is there a "Holon Marketplace" concept** — one app-internal registry
   with author attribution, install counts, ratings — or are these
   separate channels (sidecar registry, theme gallery, widget library)?
3. **Does the "official tier" curation idea generalize?** A blessed set
   the maintainer reviews vs. a community tier anyone can publish to.

## Deep Dive: AI Backends (#4)

### The Universe Is Small
Realistically, the backends that matter are: llama.cpp (GGUF), Ollama,
vLLM, OpenAI-compatible HTTP, and the hosted APIs (Anthropic, OpenAI,
Gemini). That's ~6 adapters. After that you're in the long tail of niche
backends with maybe 100 users each.

### Backends Are Mostly Fungible
A user needs *one* working LLM connection. Once Ollama is supported, the
Ollama users are covered — there's no "I want both Ollama AND vLLM"
demand. Compare to sidecars, where a power user might have 8 sidecars
active simultaneously.

This kills the sidecar reward model: there's no "active users with
backend X enabled" gradient because users only enable one. Whoever lands
the Ollama adapter first captures all Ollama users — a winner-take-all
dynamic that incentivizes racing, not quality.

### Quality Stakes Are High
A buggy sidecar means one data source is flaky. A buggy LLM adapter means
The Watcher / Integrator / Guide are flaky — the entire core
differentiator. The maintainer probably can't accept a contributor's
adapter sight-unseen.

### The Right Split
Separate the surface into two layers:

- **Adapter layer** (the API call shell): tiny surface (one trait impl,
  ~200 LOC). **Maintainer-owned.** Targeted bounties for niche backends
  if demand emerges.
- **Model recipe layer** (which model + prompt + params for a given
  task / language / hardware profile): genuinely valuable contribution
  surface. Examples:
  - "Best embedding model for German notes (multilingual-e5-base, X tokens, Y dims)"
  - "Best small LLM for entity resolution at <8GB VRAM (Phi-3.5-mini, prompt template Z)"
  - "Cheapest Anthropic config for daily synthesis (Haiku, prompt cache enabled, Z context)"

Recipes are scratch-your-own-itch (people tune for their hardware /
language / workload), highly reusable, and don't have the
winner-take-all problem.

### Recommendation for #4
- Maintainer owns the adapter trait and ~6 core backend implementations.
- Community surface is **model recipes**, presented as a registry/index
  (like model cards). Attribution only, no monetary rewards.
- If a niche backend has a real user base nobody on the contributor base
  uses (e.g., a corporate GPU cluster's proprietary inference API),
  use a targeted bounty.

## Deep Dive: File Format Adapters (#5)

This is the more strategically interesting surface because **format
adapters are an acquisition channel, not a retention channel**. Every
adapter = one less reason for someone to stay locked in elsewhere.

The codebase already has the seam: `MarkdownFormatAdapter` (memory: F1
Obsidian wedge, landed Apr 2026) was explicitly built as the second
`FileFormatAdapter` to validate it.

### Two Sub-Types Need Different Reward Shapes

#### 5a. One-Shot Migration Adapters
Examples: Roam → Holon, Notion export → Holon, OPML import.

- **Per-user value**: high but one-shot. User imports once, may never
  touch the adapter again.
- **Maintenance burden**: low. Source format barely changes; failures
  surface only when someone tries to import.
- **Strategic value**: very high. A working Roam importer might
  single-handedly bring 100 paying users from a community looking for
  alternatives.
- **Reward-shape problem**: "active users using the adapter" doesn't
  apply — it fires once. The right metric is **imports that converted
  to paying users**.
- **Trust issue**: importers must be *correct*. A subtle bug that loses
  0.1% of blocks is catastrophic. Likely needs a curated/blessed tier
  with maintainer review, not anyone-can-publish.

**Reward shape**: closer to an **affiliate / referral structure** than a
sidecar reward. E.g., $X cash *or* N free months per qualified
conversion (user imported >K blocks AND became paying within 30 days).
Cash matters more here than for sidecars because (a) the work is bigger,
and (b) the contributor may not yet be a Holon user themselves.

#### 5b. Bidirectional Sync Adapters
Examples: Markdown (already), Org Mode (already), Logseq-flavored
Markdown, Obsidian-flavored Markdown variants.

- **Per-user value**: ongoing. Many users use Holon *and* their text
  editor.
- **Maintenance burden**: medium. Edge cases surface over time.
- **Strategic value**: medium. These are retention features ("I can
  keep editing in Vim").

**Reward shape**: this one *does* fit the sidecar model — active users
syncing through the adapter is a clean usage signal. Fold into the
sidecar usage-based bucket.

### Migration-Target Prioritization (Strategic, not Contribution Design)

| Target           | User Pool           | Effort | Strategic Value                                |
|------------------|---------------------|--------|------------------------------------------------|
| **Logseq**       | Stable, outliner    | Low    | Direct competitor; easy migration; clear win  |
| **Notion**       | Huge, mostly trapped | High   | Messy export (heterogeneous, lossy), but huge market |
| **Roam**         | Declining, shopping | Medium | Active migrations underway; smaller pool      |
| **Workflowy / Dynalist** | Outliner audience | Low | Perfect fit, smaller market                   |
| **OPML**         | Universal           | Very low | Cheap to support, opens many doors          |

### Recommendation for #5
- **Migrations (5a)**: affiliate program. Per-conversion bounty (cash or
  months, contributor's choice). Curated/blessed tier — maintainer
  reviews each importer for correctness before listing. Acquisition
  attribution: track which import the user came in through.
- **Bidirectional sync (5b)**: same as sidecars. Usage-proportional free
  months.

## Three Reward Tracks

Per-surface analysis converges on three distinct tracks rather than one
unified scheme:

| Track                  | Surfaces                                                  | Reward Shape                                          | Why                                                                  |
|------------------------|-----------------------------------------------------------|-------------------------------------------------------|----------------------------------------------------------------------|
| **Active-usage**       | Sidecars, item-type extensions, bidirectional file formats, *maybe* widgets | Free months proportional to active paying users using the contribution | Ongoing value, measurable usage gradient, contributor likely uses it themselves |
| **Conversion**         | Migration importers (Roam, Notion, OPML, Workflowy)       | Per-qualified-conversion bounty (cash or months, contributor's choice) | One-shot, acquisition-driving, work is bigger, contributor may not be a Holon user |
| **Recognition-only**   | Themes, AI model recipes, automation templates, token type extensions, docs, l10n | Attribution + discoverable gallery, no money          | Scratch-your-own-itch, taste-driven, or low-effort; rewards would crowd out intrinsic motivation |

This is more complex than the original "free months for sidecars" idea,
but it matches the actual economics of each surface:
- **Active-usage** rewards align with ongoing-value contributions.
- **Conversion** rewards align with discrete acquisition events.
- **Recognition-only** stays out of the way of intrinsic motivation
  where money would harm participation.

### Operational Implications
- Telemetry must distinguish "active in last 30d" (track 1), "import
  completed and converted" (track 2), and "exists / installed" (track 3,
  none of which feeds rewards).
- Three contribution gateways: sidecar registry, importer registry
  (curated), gallery (themes/recipes/templates — anyone-publishes).
- Bookkeeping for track 2 has the heaviest legal weight (cash = tax
  forms, possible 1099 obligations). Months-only as default removes
  this for most contributors.

## Cross-Reference: BusinessAnalysis.md

Correlating with [BusinessAnalysis.md](BusinessAnalysis.md) — the wedge,
user tiers, pricing, and risk profile — produces several non-obvious
adjustments to the contribution strategy.

### The Wedge Is "Your Stack, Unified" — Not Specific Integrations

BA.md uses Todoist + JIRA as examples of the Phase 1 wedge ("Your tools,
smarter"), but the actual wedge is *stack-coverage*: Holon adapts to
whatever combination of tools the user already runs. Every user has a
different stack. This reframes the contribution landscape:

- **First sidecars are reference implementations, not the wedge.** The
  maintainer writes a handful (Todoist, JIRA, etc.) for: (a) own
  dogfooding, (b) reference quality bar, (c) bootstrap demos. They do
  not *define* the wedge — each user's set defines theirs.
- **Coverage breadth > coverage depth.** A user whose primary tool isn't
  supported bounces during activation (BA.md: "% of signups who connect
  at least one integration within 24h"). The long tail of sidecars is
  existentially important, not just nice-to-have.
- **Earlier "first sidecars are product, not extension" claim revised:**
  reference sidecars are core product; tail sidecars are the wedge
  delivery mechanism, contributor-driven.

### Tier 1 Has Direct Self-Interest in Writing Missing Sidecars

BA.md's Tier 1 ("Obsidian/Logseq power users, freelancers, GTD
practitioners") is exactly the scratch-your-own-itch demographic. With
the stack-diversity framing, this is sharper than "they like writing
plugins": **a Tier 1 user whose tool isn't supported can't adopt Holon
at all.** Writing the sidecar is the only path to using Holon.

This solves the chicken-and-egg problem: the first user of an
unsupported tool is *exactly* the right person to write the sidecar,
because their alternative is bouncing. No incentive engineering needed
beyond making the path frictionless.

**Implication for the funnel**: when an onboarding user picks a tool we
don't support, the response shouldn't be "sorry, we don't support that
yet" — it should be "we don't have it yet; here's how to add it in 30
minutes (and earn N free months when others use it)." Contribution
becomes part of the new-user flow, not a separate channel.

### "Free Months" Maps Concretely to Pro Tier

BA.md defines the pricing tiers: **Free** (1 integration) → **Pro**
(multiple integrations + WSJF tuning + native tasks) → **Premium** (AI
agents + DT integrations + automation).

Concrete mapping for the reward currency:
- 1 contribution-month = 1 month of **Pro**.
- Free is already free; rewarding it is meaningless.
- Premium is too valuable to give away as adapter rewards.
- Free-tier contributors bank Pro months; Pro contributors extend; Premium
  contributors stack Pro on top of their Premium subscription.

### Migration Importer Bounty Pricing — Flatter Than Originally Proposed

Earlier proposed: high bounties for big-name targets (Logseq, Notion,
Roam) because they unlock many users. With stack-diversity framing, this
gets weaker — every user has a different prior tool, and a niche
importer (e.g., from a specific legal-practice tool) might still drive
meaningful conversions per importer-author.

Revised: **flat per-conversion rate** ($X per qualified migration
regardless of source) — fairer, avoids underpricing niche sources, and
matches the "your stack" framing. Bigger pools still earn more in
aggregate (more conversions), without the per-conversion math
discriminating.

### Tier 2 Privacy Concern Sharpens the Telemetry Pitfall

BA.md's Tier 2 (privacy-conscious, ~10× larger than Tier 1) chose Holon
*because* of the local-first guarantee. Usage-counting telemetry to feed
contributor rewards directly conflicts with this audience.

Tier 2 is the larger paying segment; alienating it to incentivize Tier 1
contributions is the wrong trade. Hard requirements:
- Telemetry opt-in, not opt-out.
- Aggregate counts only, no per-user identifiers, no per-sidecar audit
  trail beyond raw active-count.
- Documented prominently on the privacy page (not buried in ToS).
- Consider contributor self-reporting from their own users
  (bug-tracker activity, voluntary opt-in surveys) as a partial
  substitute when telemetry is too sensitive.

### Quality Control Doesn't Scale via Maintainer Review

The "blessed tier" idea (maintainer reviews each sidecar) breaks at the
long tail. The maintainer can't competently review a sidecar for a
niche legal-CRM they've never used.

Alternative quality signals at the long-tail:
- Active-user counts (already part of the reward signal).
- Bug-report rate per active user.
- Community ratings.
- **Maintainer review reserved for**: (a) the "official" reference
  sidecars, and (b) migration-importer track (data-integrity stakes).

### Risk Cross-Link

BA.md "Critical Risks" lists *"Solo developer bottleneck"* as **High
probability / High impact** with mitigation including *"Open-source
community contributions."* That mitigation line is hand-waved in BA.md;
this document is its elaboration. BA.md should reference Contributions.md
from that risk row.

### Revised Phase Ordering

With tail-coverage as the real challenge, Phase 1 needs **contribution
infrastructure already in place at launch**, not built later. Without
it, every Tier 1 user with an unsupported tool bounces.

| Phase | Contribution priority |
|-------|----------------------|
| **Phase 1 (Wedge)** | Sidecar SDK + registry + active-usage rewards must exist at launch. Reference sidecars (Todoist/JIRA/etc.) shipped with maintainer. Migration importer track active with curated review. Onboarding flow handles unsupported-tool case as contribution funnel. |
| **Phase 2 (Expansion)** | Theme/widget gallery (recognition track). |
| **Phase 3 (Platform)** | AI recipes, token type extensions, automation templates (mostly recognition; revisit if Premium-tier value depends on a thriving DT ecosystem). |

This is a stricter ordering than the original Contributions.md
recommendation ("skip rewards at launch, revisit after a year"). The
correlation insight: with stack-diversity, *not* having contribution
infra at launch directly damages the wedge's activation metric.

## Open Questions

- What's the right telemetry boundary so usage-based rewards don't feel
  like surveillance? (Sharpened by Tier 2 privacy concern above.)
- How should quality signals work at the long tail when maintainer
  review doesn't scale?
- "Onboarding-as-contribution" UX: what does the in-app prompt look
  like when a user picks an unsupported tool? Friction here directly
  determines tail coverage.
- Where does the "scratch your own itch" assumption break down? (E.g.
  enterprise CRM sidecars, accessibility audits — where it might fail.)
