# Holon: Business Analysis

*Last updated: 2026-02-21*

---

## Core Value Proposition

Holon is an intelligence layer for personal and professional task management. It deeply integrates with existing tools (Todoist, JIRA, Calendar) and adds formal reasoning — dependency tracking, WSJF-based prioritization, delegation management, and Digital Twin synchronization — on top.

Users keep their existing tools. Holon makes them smarter.

---

## Competitive Positioning

### What Exists Today

| Tool | Strength | Limitation |
|------|----------|------------|
| **Todoist** | Quick capture, great mobile, ubiquitous | Flat lists, no dependencies, no intelligent prioritization |
| **JIRA** | Enterprise standard, rich workflow | Universally disliked UX, no personal task support, cloud-only |
| **Notion** | Flexible databases, team wikis | No computation on tasks, slow, cloud-only, privacy concerns |
| **Obsidian** | Local-first, extensible, bidirectional links | No task intelligence, plugins are fragmented, no external sync |
| **Logseq** | Outline-based, local-first, queries | Limited third-party integration, stagnating development |
| **Linear** | Beautiful UX, fast, dependency tracking | Engineering-only, no personal tasks, cloud-only |
| **Motion** | AI-powered auto-scheduling | Black-box AI, no explainability, no formal model, cloud-only |
| **Reclaim.ai** | AI calendar blocking | Calendar-only, no task graph, no WSJF, cloud-only |

### Holon's Position

Holon occupies an uncontested space: **local-first + integration-first + formally intelligent**.

No existing tool combines:
1. Deep integration with external task systems (Todoist, JIRA)
2. A formal computational model for prioritization and dependencies (WSJF, Petri Nets)
3. Local-first architecture with privacy guarantees
4. Progressive complexity that serves both simple and advanced use cases

The closest competitors are either cloud-only (Motion, Reclaim), engineering-only (Linear), or lack computation (Obsidian, Todoist).

---

## Target Users

### Tier 1: Prosumers (1,000-10,000 users)

- Obsidian/Logseq power users who want task intelligence
- Freelancers juggling multiple clients and tools
- GTD practitioners who manually maintain "waiting for" and "next actions" lists
- Knowledge workers who use Todoist for personal + JIRA for work and want a unified view

**Why they switch:** Immediate value from WSJF ranking and unified view across tools. Zero migration cost.

**What they need:** Integration setup in minutes. Noticeably better prioritization within the first week.

### Tier 2: Privacy-Conscious Professionals (10,000-50,000)

- EU-based professionals (GDPR-aware)
- People handling sensitive information (legal, medical, financial)
- Users who distrust cloud-based productivity tools

**Why they switch:** "Your tasks never leave your device" + sync across personal devices via CRDT.

**What they need:** Mobile experience competitive with Todoist. Clear privacy story.

### Tier 3: Mainstream Knowledge Workers (100,000+)

- Anyone overwhelmed by too many tasks across too many tools
- People who want "just tell me what to do next"

**Why they switch:** AI-powered task management that's reliable and explainable.

**What they need:** AI enrichment mature enough for zero-syntax input. Natural language in, intelligent prioritization out. This tier requires the AI layer to work reliably.

---

## Go-to-Market: Wedge Strategy

### Phase 1 — Wedge: "Your tools, smarter" (Tier 1)

**Offer:** Connect Todoist and/or JIRA. Get unified view + WSJF ranking + automatic waiting_for tracking.

**Technical requirements:**
- Todoist sync (exists)
- JIRA sync (planned)
- WSJF computation from priority + due date + duration
- Basic `@Person` delegation detection from Todoist labels

**Value delivered in:** Minutes (connect API → see ranked tasks)

**Monetization:** Free tier (1 integration). Pro tier (multiple integrations, WSJF tuning).

### Phase 2 — Expansion: "Tasks that think" (Tier 1-2)

**Offer:** Native task creation with `@`, `?`, `>` syntax. PN features: dependency enforcement, question tracking, composite transitions (projects as sub-nets).

**Technical requirements:**
- Task syntax parser (`@`, `?`, `>`, `[[links]]`)
- Petri Net materialization from block tree
- Dependency-aware WSJF (blocked tasks ranked separately)
- Native mobile capture

**Value delivered in:** Days (learn syntax → create first structured project)

**Monetization:** Pro tier (native tasks, advanced PN features).

### Phase 3 — Platform: "Your digital twin" (Tier 2-3)

**Offer:** Self DT with energy/focus modeling. AI enrichment agents. Browser automation. Calendar DT. Multi-device sync.

**Technical requirements:**
- Self DT with configurable parameters
- Local LLM inference for task classification and link suggestion
- Browser plugin for interaction tracking
- Calendar and bank API integrations

**Value delivered in:** Weeks (system learns your patterns → personalized suggestions)

**Monetization:** Premium tier (AI agents, DT integrations, automation).

---

## Risk Assessment

### Critical Risks

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| Inference gap erodes trust | Medium | High | Safe defaults (links = context). Explicit syntax available. Never block tasks based on inference alone. |
| Solo developer bottleneck | High | High | Focus on wedge first. Use AI for development acceleration. Open-source community contributions. |
| Incumbents add WSJF-like features | Low-Medium | High | The formal model + local-first + integration-first combination is hard to replicate incrementally. Todoist adding WSJF would require rearchitecting their backend. |

### Manageable Risks

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| Integration API changes | High | Medium | Abstract integration layer. Todoist API is stable. JIRA API changes are rare. |
| Local LLM quality insufficient | Medium | Medium | System works without AI. AI layer is additive, not required. |
| Mobile UX inferior to Todoist | Medium | Medium | Mobile capture stays in Todoist (integration-first). Native mobile is Phase 3. |
| Multi-source deduplication fails | Medium | Low-Medium | Start with manual linking. AI-assisted dedup in later phases. |

### Acceptable Risks

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| "Petri Net" scares users | Low | Low | Never mention it in marketing. Users see "smart task manager," not "formal verification engine." |
| Self DT calibration takes too long | Medium | Low | Defaults work. Calibration is optional optimization, not required for core value. |

---

## Key Metrics

### Wedge Phase

- **Activation:** % of signups who connect at least one integration within 24h
- **Aha moment:** First time WSJF ranking surfaces a task the user wouldn't have prioritized manually
- **Retention:** Weekly active users after 4 weeks
- **Engagement:** % of tasks completed that were top-3 in WSJF ranking (measures ranking quality)

### Expansion Phase

- **Syntax adoption:** % of tasks using `@`, `?`, or `>` markers
- **Native task creation:** % of tasks created in Holon vs. synced from external tools
- **Dependency accuracy:** % of auto-inferred dependencies confirmed vs. dismissed by users

### Platform Phase

- **AI trust:** Average trust level of AI-fired transitions (trending from Level 1 toward Level 2-3)
- **Automation rate:** % of transitions that fire without human intervention
- **DT coverage:** Number of Digital Twin sources connected per user

---

## Differentiation Moats

### 1. Formal Model Moat

The Petri Net engine computes real properties: reachability, enabled transitions, WSJF optimization, dependency analysis, simulation. This is implemented as a deterministic algorithm, not LLM inference. It's explainable ("this task is blocked because X isn't done") and auditable. An incumbent adding "AI prioritization" doesn't match this — they'd get probabilistic suggestions, not formal guarantees.

### 2. Integration Depth Moat

Shallow integrations (Zapier-style) move data. Deep integrations (Holon-style) understand semantics: a Todoist task becomes a transition, a JIRA ticket's status maps to a transition state, a Calendar event blocks a time slot. Building this semantic mapping for each integration takes significant effort that compounds over time.

### 3. Privacy Moat

Local-first + CRDT sync is an architectural choice that's extremely hard to retrofit into cloud-first tools. Todoist cannot become local-first without rewriting their entire stack. This is a permanent structural advantage in privacy-conscious markets.

### 4. Progressive Complexity Moat

The Level 0-5 graduation is a design philosophy, not a feature. It requires the entire architecture to support multiple interaction depths simultaneously. Adding this to an existing tool would mean redesigning the data model, UI, and API surface — effectively a rewrite.

---

## Related Documents

- [SWOT.md](SWOT.md) - SWOT matrix (concise reference)
- [docs/Vision/PetriNet.md](docs/Vision/PetriNet.md) - The formal Petri Net + Digital Twin model
- [docs/Vision.md](docs/Vision.md) - Technical vision and phased roadmap
- [docs/Vision/AI.md](docs/Vision/AI.md) - AI roles and Trust Ladder
- [docs/Vision/UI.md](docs/Vision/UI.md) - UX philosophy and three modes
