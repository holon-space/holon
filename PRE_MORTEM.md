# Holon: Pre-Mortem Analysis

*Last updated: 2026-02-23*

> *It's 2028. You invested two years of full-time effort into Holon. It didn't take off. What went wrong?*

This document catalogs the most probable failure modes, ranked by expected impact (probability x severity). It is intended to be revisited quarterly to check which failure modes are materializing and whether counter-measures are working.

---

## Context & Assumptions

- Holon is developed by a solo developer
- The core value proposition is **deep bidirectional integration** with external systems (Todoist, JIRA, Calendar, Mail) — not just syncing data in, but working with external items directly inside Holon with changes flowing back
- The Petri Net engine is a secondary differentiator that adds formal reasoning (WSJF, dependency tracking, waiting-for) on top of the integration layer
- The market (early 2026) is flooded with vibe-coded productivity tools; shipping fast is no longer a differentiator — shipping something *visibly impossible to replicate* is
- The MCP ecosystem is maturing and represents a potential path to reducing per-integration maintenance cost
- **AI-accelerated development changes timelines.** The Todoist integration took ~2 days to plan and build from scratch with AI agents; a second pass would take ~4 hours. Integration *creation* is no longer the bottleneck — ongoing maintenance, API changes, and edge-case handling are. This substantially reduces the solo-developer risk for initial development, but maintenance burden scales with the number of live integrations regardless of how fast they were built

---

## Failure Modes: Ranked by Expected Impact

### #1. Integration Maintenance at Scale (P: 65%, Severity: Critical)

**The story:** With AI-assisted development, building each integration took days, not months. By mid-2027 you had 5 integrations: Todoist, Calendar, JIRA, Gmail, Linear. The initial builds went fast. Then Todoist deprecated their Sync API v9. Google Calendar changed their webhook format. JIRA introduced a new permissions model. Each breaking change was individually small, but with 5 live integrations, there was always something broken. Users who depended on the Calendar integration lost trust when it silently stopped syncing for 2 weeks while you were fixing the JIRA OAuth flow. The product felt unreliable — not because any single integration was bad, but because the surface area of "things that can break" grew linearly while your maintenance bandwidth didn't.

**Why it's still the top risk (adjusted):** AI collapses the *creation* cost of integrations from months to days. But maintenance cost scales with the number of live integrations regardless of how fast they were built. A broken integration that worked yesterday damages trust more than a missing integration that was never promised. The risk isn't "can I build 5 integrations" — it's "can I keep 5 integrations working simultaneously."

**Counter-measures:**
- **MCP for writes, native for reads.** Use external MCP servers (hosted or Rust-library) for write operations (mark done, update priority, create task). Keep native sync code only for the read/change-detection direction. This reduces the per-integration surface area exposed to API changes.
- **Integration health monitoring.** Automated checks that detect when a sync provider stops producing updates, or when API calls start failing. Surface this to the user ("Calendar sync paused — Holon detected an API change") rather than silently breaking.
- **Staged rollout.** Don't go from 1 to 5 integrations in a quarter. Add one, stabilize for a month, then add the next. Each integration must prove it survives a maintenance cycle before the next one is added.
- **AI-assisted maintenance.** The same AI acceleration that builds integrations can fix API breakages — but only if the integration code is clean, well-tested, and has good error reporting. Invest in integration test infrastructure (the fake client pattern from `holon-todoist` is the right model).

**Monitoring signal:** If more than one integration is broken at the same time, this is materializing. Track "days since last integration breakage" per provider.

---

### #2. Invisible Differentiation (P: 70%, Severity: Critical)

**The story:** Holon's architecture IS genuinely different — CRDTs, deterministic ranking, bidirectional sync, local-first, Petri Net engine. None of this can be vibe-coded. But users couldn't see the difference in the first 5 minutes. The landing page said "unified task management with intelligent prioritization" — exactly what 50 other tools say. The formal model was invisible. The architectural advantages only mattered after months of use. Nobody got to months because they bounced at minute 2.

The market is drowning in half-baked tools. A minimal Todoist wrapper with WSJF sorting looks exactly like what a weekend vibe-coder would produce, regardless of the engineering underneath. "Slightly different sort order" isn't a compelling demo.

**Why it's critical:** In a saturated market, legibility of differentiation matters more than the differentiation itself. You have a real moat but no way to show it.

**Counter-measures:**
- **Lead with the waiting-for dashboard.** Connect Todoist → Holon instantly shows: "You have 7 open delegations. @Kat: Zeitplan erstellen (12 days, no response). @Tax Consultant: review documents (5 days)." This is immediately, obviously useful and no other tool does it automatically from Todoist labels.
- **Show the explanation.** When ranking tasks, show *exactly why* each task ranks where it does. "This task is #1 because: priority A (weight 100) x deadline in 2 days (urgency 180) / 30min = WSJF 9.3." No LLM-based tool can show its work like this. Users don't need to know it's a Petri Net — they see a system that explains itself and can be argued with.
- **The living org file.** For the Obsidian/LogSeq crowd: "Keep writing in your editor. Holon watches your org files. Tasks get WSJF-ranked. Delegations get tracked. Your plain text files become intelligent without locking you into any app." This is the opposite of every vibe-coded tool that wants you inside their app.
- **Demo video, not feature list.** A 60-second video showing cross-system task ranking + automatic waiting-for detection + explained prioritization is worth more than any landing page.

**Monitoring signal:** If alpha users say "neat" and go back to Todoist, this is materializing.

---

### #3. Vision Documents Replace the Product (P: 55%, Severity: High)

**The story:** You spent more time refining VISION_PETRI_NET.md, BUSINESS_ANALYSIS.md, SWOT.md, FEASIBILITY_TODOIST_WEDGE.md, and PRE_MORTEM.md than writing code. The documents became increasingly elaborate and self-referencing. The gap between vision and implementation grew. The intellectual satisfaction of the design replaced the satisfaction of shipping.

This is the most insidious failure mode because it feels like progress. The Petri Net formalism is genuinely elegant — WSJF, WIP limits, Eisenhower, Pomodoro all emerging from five primitives is intellectually thrilling. But intellectual elegance and product-market fit are orthogonal.

**Why it ranks high:** The evidence is already present. There are 7+ vision/strategy documents totaling thousands of lines. The Petri Net engine works but doesn't yet have a clear "killer feature" story beyond WSJF ranking. The Self Digital Twin (energy, focus, mental slots, flow dynamics) is designed in detail but has no implementation timeline. This pattern — deep design, deferred implementation — is the signature of this failure mode.

**Counter-measures:**
- **Freeze vision documents.** No more edits until the wedge has 10 active users. This document (PRE_MORTEM.md) is the last strategy document for now.
- **Every coding session should produce user-visible change.** Infrastructure is necessary, but if a week passes without anything a user could notice, re-evaluate.
- **Set a hard ship date for the Todoist integration wedge.** Miss it → re-evaluate everything.

**Monitoring signal:** If the ratio of strategy-doc commits to code commits exceeds 1:5 in any month, this is materializing.

---

### #4. The "Good Enough" Problem (P: 50%, Severity: High)

**The story:** You shipped deep Todoist integration. Users connected it. They saw their tasks ranked by WSJF. They said "nice" and went back to Todoist. The ranking wasn't dramatically better than their existing intuition. Todoist already sorts by priority and due date. The delta needs to be viscerally obvious in the first 5 minutes, and "slightly different sort order" isn't that.

The value of cross-system integration is **superlinear** — one integration is marginally useful, two is interesting, three is transformative ("I see my Todoist tasks, JIRA tickets, and calendar events in one ranked list — I've never had this before").

**Revised assessment:** With AI-accelerated development (days per integration, not months), reaching the 3-integration threshold is feasible within weeks rather than years. This substantially reduces the risk of being stuck in the low-value "one integration" phase. The danger shifts from "can't build enough integrations" to "built three integrations but the unified view isn't compelling enough to justify another app."

**Counter-measures:**
- Push quickly to 3 integrations (Todoist + Calendar + one work system). The product story is "unified view across systems," and three systems tells that story.
- Even with ONE integration, demonstrate cross-domain value: "Your Todoist tasks ranked alongside your org-file tasks in a single list" is something Todoist alone cannot do.
- The waiting-for tracking works with just Todoist + org files — already two systems.
- **The cross-system insight is the differentiator**, not the ranking. "Your JIRA deadline conflicts with 3 Todoist tasks and you have no calendar time free" — this is what no single-system tool can produce.

**Monitoring signal:** Alpha users use Holon for viewing but switch to native apps for acting.

---

### #5. AI Agents Make the Integration Layer Obsolete (P: 45%, Severity: High)

**The story:** By 2027, Claude/GPT agents could directly query Todoist, JIRA, Calendar, and email via tool use. Users asked "what should I work on next?" and got a synthesized answer without needing a unified data layer. Holon's architectural moat (local unified cache) became less valuable when AI could query N APIs on-demand.

The "formal model moat" is real — agents give probabilistic suggestions while Holon gives deterministic guarantees. But most users don't care about formal guarantees. They care about "does it work for me."

**Counter-measures:**
- **Become the structured backend that AI agents query.** The MCP server already exists. Position Holon as "the state layer AI agents need for reliable task management" rather than only "the UI you interact with."
- **Emphasize what agents can't do:** persistent state tracking (waiting-for across days/weeks), deterministic ranking (explainable, auditable), offline operation, privacy. An agent can answer "what should I do next?" but it can't maintain a waiting-for list across sessions or guarantee the same answer for the same inputs.
- **The question-answering loop.** `? @[[Kat]]: When should we do XYZ` → auto-detection of Kat's response via LLM across any integrated channel → suggestion to mark as answered. This combines integration depth with AI intelligence in a way that a stateless agent cannot replicate.

**Monitoring signal:** Mainstream AI tools (Claude Projects, ChatGPT) add native Todoist/JIRA integration with persistent task tracking.

---

### #6. Flutter Desktop UX Never Reaches "Good Enough" (P: 55%, Severity: High)

**The story:** Flutter desktop was... okay. But the outliner experience never matched LogSeq or Obsidian for keyboard-driven power users. Text editing felt slightly wrong. Performance with large block trees was sluggish. Users tried it, said "it's promising," and kept using their existing tool.

An outliner is one of the most interaction-intensive UIs — every keystroke matters. Building a great outliner in Flutter is fighting the framework's strengths (mobile-first widget model vs. desktop text editing).

**Counter-measures:**
- Consider whether the **MCP server + CLI** is actually the better first interface for power users (Tier 1). A `holon rank` command that reads Todoist and prints a WSJF-ranked list might validate the concept faster than a GUI.
- If Flutter desktop, invest heavily in the editing experience BEFORE any other feature. Users won't tolerate a mediocre outliner.
- Keyboard-first: the Which-Key navigation system described in VISION_UI.md is a differentiator for power users. Ship it early.

**Monitoring signal:** Users consistently report UX friction in the outliner (keyboard handling, performance, text editing feel).

---

### #7. MCP Abstraction Mismatch (P: 50%, Severity: Medium)

**The story:** You built the MCP client adapter to reduce integration maintenance. You connected to the Todoist MCP. But the Todoist MCP exposed `create_task(content, due_date)` while your `SyncableProvider` trait expected streaming change sets with sync tokens. The granularity didn't match. MCPs are designed for agent tool-use (request/response), not for continuous bidirectional sync. You ended up building a sync layer ON TOP of MCP calls, which was almost as much work as a native integration.

**Counter-measures:**
- **Separate the concerns.** MCP for write operations (mark done, update priority, create task). Native sync code for read/change-detection (ETags, sync tokens, webhooks). MCP doesn't need to replace `SyncableProvider` — it replaces `OperationProvider`.
- Validate this split with the second integration (Calendar) before committing to it architecturally.

**Monitoring signal:** Building an MCP-based integration takes >60% of the effort of a native integration.

---

### #8. Privacy Doesn't Convert Users (P: 55%, Severity: Medium)

**The story:** "Local-first + privacy" attracted interest from EU professionals and privacy advocates. But these users were a small, demanding group. They wanted iOS, Android, polished UX, team features — everything a solo dev can't provide. The mainstream didn't care about privacy enough to switch.

Privacy is a feature people claim to want but rarely pay for. Obsidian succeeded on UX + extensibility, with privacy as a bonus, not the main sell.

**Counter-measures:**
- Don't lead with privacy in marketing. Lead with capability ("see all your tasks in one place, ranked intelligently"). Privacy is a differentiator for the converted, not a conversion tool.
- Privacy becomes more valuable as a retention moat than an acquisition channel — users who've invested in local-first don't want to migrate to cloud.

**Monitoring signal:** Privacy is the #1 cited reason for interest but not for continued use.

---

### #9. Loro/Turso Dependency Risk (P: 30%, Severity: Critical)

**The story:** Loro (CRDT) and Turso (embedded SQLite with IVM) are both young projects. A critical sync bug corrupted data. Or maintainers pivoted. Or performance didn't scale. You were locked into dependencies that couldn't support the vision.

Evidence of existing friction: Turso IVM hangs on chained materialized views, context parameter preloading issues, and other workarounds already documented in project skills.

**Counter-measures:**
- Abstract CRDT and database layers behind traits (partially done via `SyncableProvider`, `OperationProvider`).
- Maintain awareness of Loro/Turso project health.
- Consider whether plain SQLite + manual change tracking might be simpler than relying on Turso's IVM for some use cases.

**Monitoring signal:** More than 2 weeks per quarter spent on Loro/Turso workarounds rather than feature development.

---

### #10. Complexity Alienates Contributors (P: 40%, Severity: Medium)

**The story:** You open-sourced Holon. Potential contributors looked at the codebase — Petri Nets, Rhai expressions, PRQL compilation, org-mode bidirectional sync, Loro CRDTs, Turso IVM, Flutter FFI bridges — and noped out. The intellectual barrier to contribution was too high. The project remained solo despite being open source.

**Counter-measures:**
- Create bounded "contribution zones" (e.g., "add a new verb to the dictionary," "add a Todoist field mapping," "write an MCP tool adapter").
- Keep the Petri Net engine encapsulated — contributors shouldn't need to understand it to add features.
- Good first issues that don't require understanding the full architecture.

**Monitoring signal:** Open-source contributions remain zero 6 months after public release.

---

## The Meta-Pattern

AI-accelerated development fundamentally changes the solo-developer equation. Building integrations in days rather than months means the creation bottleneck is largely eliminated. The failure modes shift accordingly:

- **Creation risk → Maintenance risk.** Five integrations can be built in a month. Keeping five integrations working simultaneously over years is the real challenge. Every external API is a dependency you don't control.
- **Speed risk → Legibility risk.** In a market flooded with vibe-coded tools, building fast is the commodity. The risk isn't "can't ship fast enough" — it's "shipped something that looks identical to 50 vibe-coded tools despite being fundamentally different underneath."
- **Scope risk → Focus risk.** AI makes it tempting to build everything because everything is cheap to build. The discipline shifts from "what can I build" to "what should I NOT build yet."

The three most dangerous failure modes (#1 Integration Maintenance, #2 Invisible Differentiation, #3 Vision as Product) share a root cause: **spending effort on the wrong things.** Maintaining integrations that nobody uses yet. Designing features that don't make the differentiation visible. Writing strategy documents instead of the code that makes the strategy real.

The most likely post-mortem sentence is:

> *"Holon had 5 deep integrations, a Petri Net engine, and a beautiful vision. But from the outside it looked like another task manager. The people who tried it said 'this is impressive' but couldn't explain to a friend why they should switch. The ones who stayed loved it. There just weren't enough of them."*

The antidote is not "ship faster" (the market punishes undifferentiated speed) and not "ship the full vision" (maintenance will drown you). It's **make the differentiation visible in the first 2 minutes** — then earn trust through reliability.

---

## Quarterly Review Checklist

Use this checklist every quarter to assess which failure modes are active:

- [ ] **#1 Integration maintenance:** Is more than one integration broken at the same time? Am I spending >50% of time on maintenance vs. new features?
- [ ] **#2 Invisible differentiation:** Can a new user articulate what makes Holon different within 5 minutes?
- [ ] **#3 Vision over product:** What's my strategy-doc-to-code commit ratio this quarter?
- [ ] **#4 Good enough:** Are users working IN Holon or just viewing Holon and acting elsewhere?
- [ ] **#5 AI obsolescence:** Have mainstream AI tools added persistent task state tracking?
- [ ] **#6 Flutter UX:** Are users reporting outliner friction?
- [ ] **#7 MCP mismatch:** Is MCP-based integration significantly cheaper than native?
- [ ] **#8 Privacy conversion:** Is privacy driving acquisition or just interest?
- [ ] **#9 Dependencies:** How many weeks were spent on Loro/Turso workarounds?
- [ ] **#10 Contributors:** Any external contributions in the last quarter?

---

## Related Documents

- [BUSINESS_ANALYSIS.md](BUSINESS_ANALYSIS.md) — Strategic analysis, competitive positioning, go-to-market
- [SWOT.md](SWOT.md) — SWOT matrix
- [VISION.md](VISION.md) — Technical vision and phased roadmap
- [VISION_PETRI_NET.md](VISION_PETRI_NET.md) — Petri Net + Digital Twin model
- [VISION_AI.md](VISION_AI.md) — AI roles and Trust Ladder
- [FEASIBILITY_TODOIST_WEDGE.md](FEASIBILITY_TODOIST_WEDGE.md) — Todoist browser overlay exploration
