# The Killer Demo — Sketch

*Goal: a 90-second video that makes Holon's architectural lead visible to a viewer who has never heard of CRDTs, IVM, or reactive cells.*

The viewer should walk away thinking: *"I cannot do this in Obsidian or Notion, no matter how many plugins I install."*

## The three primitives to show

1. **Typed agent provenance** — every agent-authored block knows which tool call produced it.
2. **Re-executable org source blocks** — the human's notes ARE the executable record.
3. **Reactive agent supervision** — push-updated PRQL view of agent activity.

A real demo composes these into a single believable workflow. Don't show them as three disconnected features.

---

## The scenario

**Setup**: User is a developer maintaining a side project (e.g., an open-source library). They're triaging this week's GitHub issues and writing a weekly retro post. Two parallel agents help: a "triager" categorizing issues, and a "summarizer" drafting the retro.

This scenario was chosen because:
- It's a real workflow many viewers do today.
- It has natural concurrency (two agents) — exercises the CRDT advantage.
- It has natural provenance need (which issues did the agent triage vs. me?) — exercises the taint advantage.
- The retro doc is a re-executable artifact — exercises source-blocks.

---

## Frame-by-frame storyboard

### Scene 1 — 0:00–0:10 — The org file as a living document

Camera on a single org file: `weekly-retro.org`. Three headings visible:

```org
* Weekly Retro — Week of 2026-05-11
** Issues triaged this week
   #+BEGIN_SRC mcp :server github :tool list_issues
   { "repo": "me/proj", "since": "2026-05-04", "state": "open" }
   #+END_SRC
** Notable changes
** Draft post
```

Voiceover: *"This is just an org file. But the code block isn't documentation — it's a live tool call."*

User hits a keystroke. The block expands inline with results — 14 issues appear as child blocks under the heading, each tagged `:agent-derived:` and color-tinted.

### Scene 2 — 0:10–0:25 — Provenance is visible

User clicks one of the new issue blocks. A side panel shows:

```
Provenance:
  Source: tool call → github.list_issues
  Invoked from: weekly-retro.org § Issues triaged this week
  At: 2026-05-14 10:23:04
  Inputs: { "repo": "me/proj", "since": "2026-05-04", "state": "open" }
```

User clicks "revert this block" — the issue block disappears. They click "revert the whole call" — all 14 disappear. They click undo — all 14 reappear together.

Voiceover: *"Every block knows where it came from. You can undo a single agent action, the whole call, or replay it next week with one keystroke."*

### Scene 3 — 0:25–0:45 — Two agents, one document

Split-screen briefly: left = the org file in the GPUI app; right = a terminal with two Claude Code processes running.

**Agent A** is told: "categorize the issues by type and add `:bug:` `:feature:` `:perf:` tags."

**Agent B** is told: "for each notable closed PR this week, draft a one-line summary under 'Notable changes'."

Both agents start working *simultaneously*. The user watches the org file update live — tags appearing on issue blocks (Agent A), bullet points materializing under "Notable changes" (Agent B). No flicker, no merge conflict dialog.

Voiceover: *"Two agents, same document, no merge conflicts. Loro CRDT under the hood — but you don't have to know that."*

### Scene 4 — 0:45–1:05 — Reactive supervision

User opens a new doc: `agent-watch.org`. It contains one PRQL block:

```prql
from blocks
filter source_tool_call != null
filter created_at > @2026-05-14
group { tool_call_id } (
  aggregate {
    tool = first source_tool,
    count = count this,
    last_activity = max created_at
  }
)
sort -last_activity
```

The block is rendered as a live table — and rows update *as the agents work*. Counters tick up. New tool-call rows appear when an agent calls a new tool. The user clicks a row → drills into the 8 blocks that tool call produced → reverts one bad triage → returns to the watch view, sees the counter decrement.

Voiceover: *"This is tail -f for your agents. Push-updated. No polling. No plugins."*

### Scene 5 — 1:05–1:25 — Re-executable next week

Cut to a calendar showing "Next Monday."

User opens last week's `weekly-retro.org`, scrolls to top, presses one key. Every tool-call source block re-executes against current state. The "Issues triaged" section refreshes with this-week's issues; the agent tags re-flow; the "Notable changes" rebuilds.

Voiceover: *"Your weekly retro is a program. Run it again — get this week's retro. The notes are the workflow."*

### Scene 6 — 1:25–1:30 — The close

Black screen, text:

> Holon — your notes, your agents, one source of truth.
> Open source. Local-first. Multi-agent ready.

---

## What this demo specifically shows that nothing else can

| Capability | Holon | Obsidian + MCP | Notion + MCP | Tana | Anytype |
|---|---|---|---|---|---|
| Block-level agent provenance | ✓ | ✗ (file-level only via git) | ✗ (page-history coarse) | ✗ | ✗ |
| Revert a single tool call | ✓ | ✗ | ✗ | ✗ | ✗ |
| Re-execute notes as workflow | ✓ | ✗ (no native exec) | partial (Notion AI blocks, but not user-defined MCP) | ✗ | ✗ |
| Concurrent agents, no merge | ✓ (CRDT) | ✗ (LWW files) | ✓ (cloud locks) | ✓ (cloud locks) | ✓ (CRDT) |
| Push-updated agent activity view | ✓ (IVM) | ✗ (polling plugins) | ✗ | ✗ | ✗ |

The demo is structured so each scene shows one differentiator + the prior scene's differentiator still working. By scene 5, the viewer has seen all five.

---

## What needs to be built to ship this demo

Most of the substrate exists. The gaps are the *visible* parts:

1. **Provenance property drawer** — every block created by `execute_source_block` or future MCP-proxy must auto-stamp `:source-tool:`, `:source-call-id:`, `:source-doc:`, `:created-at:` properties. Today, source blocks execute but don't tag children.
2. **Provenance side panel UI** — a panel that reads the property drawer and shows "Invoked from", "Inputs", "Revert this block / Revert the whole call". Need to land in at least one frontend (GPUI is the demo target).
3. **Color-tint on agent-derived blocks** — a renderer-level affordance that makes provenance *visible* in normal viewing.
4. **MCP-proxy MVP** — at minimum, route `github.list_issues` through Holon's proxy so the demo source block actually works. The full MCP++ design isn't required; one server proxied with provenance stamping is enough.
5. **PRQL view of provenance** — needs the property drawer schema landed first, then it's a query.
6. **Two-agent demo harness** — script that spawns two Claude Code processes with predefined prompts, used for the recording. Not a product feature — just demo infrastructure.
7. **Re-execute-all keystroke** — bound to "re-run every source block in this document, in order." `execute_source_block` exists; the document-level re-run loop doesn't.

Estimated scope: 1–2 weeks for one person who knows the codebase. Items 1, 2, 4 are the load-bearing ones; 3, 5, 6, 7 are smaller.

---

## What the demo deliberately omits

- **PRQL syntax explanation.** Viewer sees PRQL once, in scene 4. They don't need to understand it; they need to see the table update live.
- **Org-mode evangelism.** Don't argue for org. Show that it works.
- **CRDT explanation.** "No merge conflicts" is the user-visible claim; never say "Loro" in the voiceover.
- **Architecture diagrams.** Zero. The demo is workflow-first.
- **Mobile.** Holon's mobile story is weak — don't show it.
- **Setup.** Cut to a running app. Onboarding is a separate problem.

---

## Distribution

- 90-second hero video on the README and landing page.
- Twitter/Mastodon cut: scenes 2 + 3 only (30 sec). Provenance + concurrent agents are the most viral 30 seconds.
- HN post: link the video + write up scene 5 (re-executable notes) as a blog post — that's the idea that gets technical readers to dig in.
- Conference talk: the full 90-second video opens the talk; the rest is "how we built each piece."
