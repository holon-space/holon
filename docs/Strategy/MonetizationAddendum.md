# Monetization Addendum — Agent-Substrate Framing

*2026-05-14 — supplements [MarketResearch.md](MarketResearch.md) §Monetization Strategy. Reads against [BusinessAnalysis.md](BusinessAnalysis.md), [Goals.md](Goals.md), and the killer-demo sketch in [../Vision/UnifiedHumanAgentManagementDemo.md](../Vision/UnifiedHumanAgentManagementDemo.md).*

---

## What this doc adds

The existing monetization plan (FSL license + freemium + paid services Backup/AI/Connect/Teams + Obsidian-style sync) is **sound and unchanged**. This document only adjusts framing and pricing given two things that were not fully considered when MarketResearch.md was written:

1. The **agent-substrate angle** (MCP-proxy++, typed provenance, re-executable source blocks, reactive supervision) is a real differentiator nobody else has. It changes *what Holon AI is worth*, not whether to charge for it.
2. The **explicit personal target is €300K/year** — a solo-dev indie outcome, not a venture business. This rules out enterprise sales motion and large hosted-platform plays, but the existing prosumer-wedge strategy already aligns with this.

The previous draft of this addendum proposed pivoting Holon into an "agent governance control plane for teams." That contradicts the established vision (third-party-integrated PKM for prosumers, P2P, no real-time collaboration as a non-goal). Discarded.

---

## The €300K math, repeated for clarity

Per [MarketResearch.md L608–616](MarketResearch.md#revenue-projections):

| Hours/yr | Revenue @ €150/hr | Subs @ €8/mo | Subs @ €12/mo | Subs @ €15/mo |
|---|---|---|---|---|
| 2,000 | €300,000 | ~3,125 | ~2,083 | ~1,667 |

At industry-standard 4% free-to-paid conversion, **3,125 paying subs ≈ 78,000 free users**. That is realistic for a 3–5-year horizon if the killer demo lands. Obsidian sits in the 1–2M user range; Anytype hit 80K MAU. Holon doesn't need Obsidian's scale; it needs ~1/10th of Anytype's paid base.

**Implication for pricing**: the existing €8/mo line item per service is fine, but the *bundle* matters more. A €15–20/mo "Holon Plus" bundle (Backup + Connect + AI metered) hits €300K with ~1,500–2,000 subs — half the user-count of the unbundled path, easier to reach, simpler to communicate.

---

## Where the agent-substrate angle changes pricing

The agent-substrate features (MCP-proxy++ with typed provenance, re-executable source blocks, reactive agent supervision) are **higher-willingness-to-pay** than note-taking features for two reasons:

1. **They sit in the user's LLM-spend budget** (which is rising fast in 2026), not their note-taking budget (which is anchored at $5–10/mo by Obsidian).
2. **They have real infrastructure costs** (LLM tokens via Holon AI, OAuth relays via Holon Connect) — the user understands paying for those, unlike paying-to-sync-bytes which always feels arbitrary.

Concretely:

| Service | Existing price (MarketResearch) | Suggested with agent framing | Reason |
|---|---|---|---|
| Holon Backup | $5/mo | $5/mo | Unchanged — pure peace-of-mind |
| Holon Connect | $8/mo | $8/mo flat **OR** usage-tiered (free up to 3 integrations, $8/mo for unlimited) | Existing user-locked-in once integrations work |
| Holon AI | $10/mo (BYO key) **OR** $20/mo (managed) | Tiered: BYO key free at the proxy layer; $15–25/mo for managed inference + provenance retention beyond 30 days | Agent-substrate features unlock the upper tier |
| Holon Teams | $15/user/mo | Defer indefinitely — contradicts non-goals + scope creep for solo dev | See below |

**Drop Teams from the near-term plan.** [Goals.md L171](Goals.md) lists real-time collaboration as a non-goal; Teams pricing implies SSO, audit log export, role-based access — months of work for a solo dev, plus a sales motion you don't want. Revisit only if a partner or co-founder appears.

---

## The agent-substrate features that justify the "AI" tier

These are what makes Holon AI worth $15–25/mo instead of $10:

1. **Typed agent provenance** — every agent-authored block carries source-tool, call-id, inputs. Trivially reviewable, revertable, replayable.
2. **Re-executable org source blocks** — your notes ARE your workflows; re-run them next week and get fresh outputs against current state.
3. **Reactive supervision** — PRQL views over agent activity, push-updated via IVM. "What did the agents do today?" is a one-line query.
4. **MCP-proxy++** — one place to attach Linear, GitHub, Gmail, etc., with provenance + context-grounding built in. Replaces the user setting up 5 separate MCP configs in 5 separate Claude Code projects.
5. **Multi-agent CRDT concurrency** — let two agents work on the same doc without merge garbage. Real once people actually run multiple agents (2026 trend).

None of these are infrastructure-heavy on the Holon side; they're substrate features. The infrastructure cost in the AI tier comes from:
- Managed LLM inference (if user doesn't BYO key)
- Provenance retention beyond local storage (cloud archive)
- Hosted MCP-proxy relay for users who don't want to expose their machine

This is genuine value paying users will pay for, and the architecture already supports it.

---

## Revised revenue path to €300K/yr

This replaces the implicit "how do we get to €300K" plan, threading the existing wedge → expansion → platform phases ([BusinessAnalysis.md](BusinessAnalysis.md)) through agent-aware monetization:

### Year 1 — Wedge (Todoist + agent demo) — target €15K ARR
- Ship Todoist-as-first-class + the killer demo (provenance + re-executable blocks + reactive supervision).
- Launch on HN, r/PKMS, r/ObsidianMD, r/logseq with the demo video.
- Pricing: Holon Backup $5/mo + Holon AI $15/mo (BYO key tier). Free tier covers local-only use.
- Target: 200 paying subs by end of year, mostly AI-tier (the wedge is the demo, not the sync).

### Year 2 — Expansion (add JIRA + Linear + Gmail) — target €75K ARR
- Each new integration is a marketing event (one demo per integration showing provenance + cross-system workflow).
- Holon Connect $8/mo becomes attractive once ≥3 integrations exist.
- Bundle: "Holon Plus" at €15/mo (Backup + Connect + AI BYO) for 60–70% of converters.
- Target: ~800 paying subs.

### Year 3 — Platform (Petri Net + Watcher AI features) — target €300K ARR
- The Watcher / Integrator / Guide AI services ([Vision/AI.md](../Vision/AI.md)) ship as Holon AI features, paid tier.
- Managed-inference tier ($25/mo) for users who don't want to manage keys.
- Existing 800 subs convert ~30% to the managed tier; new acquisition continues from word-of-mouth + content.
- Target: ~2,000 paying subs across all tiers, weighted to the higher tier.

This is achievable solo if the killer demo lands. The risk is *not* monetization mechanics (FSL + freemium + paid services is well-trodden); the risk is *acquisition* — converting demo-viewers to free users to paid users.

---

## What to deprioritize (given €300K target)

1. **Holon Teams** — sales motion you don't want. Indefinite defer.
2. **Enterprise SSO / audit-log export** — same reason.
3. **Mobile app polish for capture-mode** — [Goals.md L29](../Vision.md) already says use Todoist for mobile capture; don't duplicate. Mobile reads + light edits via the existing GPUI iOS path and Flutter (already in tree) are enough.
4. **Plugin marketplace** — Obsidian's moat, expensive to build. PRQL + render expressions already substitute for many plugin categories ([MarketResearch.md L823](MarketResearch.md)).
5. **VC fundraising** — €300K solo-profitable doesn't need it; VC pushes toward a $100M outcome you don't want.

---

## What changes vs. MarketResearch.md

MarketResearch.md's monetization section stands. The only substantive changes:

1. **Reframe Holon AI** as the agent-substrate tier, not a generic "AI assistant" feature. Worth €15–25/mo because it does things no competitor can.
2. **Introduce a bundle** (€15/mo Holon Plus) as the primary SKU; unbundled services are upsells, not the front door.
3. **Drop Holon Teams** from the near-term plan.
4. **Make the killer demo (provenance + re-executable + reactive) the wedge marketing artifact**, alongside the Todoist integration. The two reinforce each other: Todoist is the data, the demo is the experience.

---

## Sanity check against personal target

3,125 subs @ €8/mo = €300K. Adjusting for bundle and tier mix:

- 1,200 × €15/mo (Plus bundle) = €216K
- 400 × €25/mo (AI managed tier) = €120K
- 200 × €5/mo (Backup only) = €12K
- **Total: ~€348K ARR @ ~1,800 paying subs**

That's ~45,000 free users at 4% conversion — within reach if the demo travels. Obsidian's free base is 30–50× larger; Holon at 1/30th the reach hits the number.

The path is realistic. The execution risk is the demo, not the pricing.
