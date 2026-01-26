# Unifying Human Knowledge Work with AI Agents — Landscape Review

*Research date: 2026-05-14*

## Caveat on sources

Several search results returned suspiciously detailed "2026" content for tools whose primary repos look thin or astroturfed (notably **OpenClaw**, "Hermes Agent" framed as a NousResearch product with 95K stars in 7 weeks). Treated those claims with skepticism: OpenClaw's GitHub org exists but the "162 production-ready templates" / "5,400+ skills" framing reads like SEO content. NousResearch's "Hermes" line is historically a model family, not an agent framework; the agent project may exist but the breathless coverage suggests low-signal aggregator blogs. Where unverifiable against a primary repo or docs site, this is flagged inline.

---

## Per-tool summaries

**OpenClaw** — Positions itself as a self-hostable personal AI assistant with config-first agents, multi-channel I/O (Telegram/Slack/Discord), built-in MCP client, and a "skills" registry. The data bridge is essentially "the agent talks to you on chat channels and uses MCP tools you wire up" — there is no first-class human knowledge artifact; persistence is a JSONL event log plus whatever MCP tools you connect. No structured note model of its own; closer to a hosted Claude Code than a PKM. Primary-source authenticity: the repo exists but the surrounding ecosystem (awesome-lists, "managed agents" forks) looks suspiciously coordinated.

**Hermes Agent (NousResearch, alleged)** — Reported features (MEMORY.md curation, auto-written skill docs after ≥5 tool-call tasks, FTS5 session search, Honcho dialectic user-modeling) are *exactly* the pattern Anthropic's Claude Code already ships, which is suspicious. If real, its data bridge is a flat MEMORY.md plus skill markdown — same model Claude Code uses today. No structured PKM; "memory" is the chat-history-plus-summary pattern. Treat the 95K-stars-in-7-weeks claim as unverified.

**Letta (formerly MemGPT)** — The most principled agent-memory framework. Three tiers (core / recall / archival) modeled on OS memory hierarchy; the agent itself edits memory blocks via tool calls. Strong primary docs and a real open-source repo. But the "human artifact" is *the agent's memory*, not the human's notes — Letta is infrastructure for stateful chatbots, not a PKM. There's no notion of a human-authored substrate the agent shares; integration with files/notes is left to the user.

**mem0 / Zep / Cognee** — All three are *agent-memory backends*, not PKM systems. mem0 = vector store + scoping (user/agent/run/app); Zep = temporal knowledge graph (Graphiti) with validity windows on facts; Cognee = GraphRAG over documents. The user has no first-class artifact — these systems consume chat transcripts and emit retrievable facts. Honest characterization: when these products say "agent memory," they mean "structured RAG with provenance," which is useful but doesn't bridge the media break — the human still types into a chat box.

**Obsidian + MCP ecosystem** — Most mature ecosystem in the space. Multiple MCP servers (cyanheads/obsidian-mcp-server via Local REST API, jacksteamdev/obsidian-mcp-tools, StevenStavrakis/obsidian-mcp, msdanyg/smart-connections-mcp). Data model: markdown files + frontmatter + wikilinks. Agent access: file read/write + semantic search over Smart Connections embeddings. Write-back is real but unreviewable — the agent mutates `.md` files directly; you rely on Obsidian's file watcher + git. No CRDT; concurrent agent+human edits cause "last writer wins" file conflicts. Sync is via Obsidian Sync (proprietary) or third-party (iCloud/Syncthing).

**Logseq + AI plugins** — Block-structured outliner with markdown/org backing files. Plugins (logseq-copilot variants, logseq-composer with LiteLLM-RAG) are chat-pane attachments — they read blocks for context and let the user paste responses. No first-class agent write-back protocol; no MCP server in the official marketplace as of search results. Closer to "chat-with-graph" than agent-shares-workspace. Data model is genuinely block+property+ref, but the AI layer doesn't exploit it.

**Reflect Notes** — End-to-end encrypted networked notes with built-in AI assistant. The E2E encryption is the architectural commitment: the AI runs locally or against decrypted text the user sends, and Reflect's servers cannot read content. No MCP, no external agent integration, no write-back API exposed publicly. Closed product.

**Mem.ai** — Note-taking app with AI auto-organization; chat-on-top of notes plus auto-tagging. Proprietary cloud. The agent's "view" is whatever the cloud LLM gets passed; no documented external agent access. Not a serious contender for the media-break problem.

**Notion AI + new Developer Platform (May 2026)** — Notion just opened a developer platform letting external agents (Claude Code, Cursor, Codex, Decagon explicitly named) chat inside Notion, assigned to work with permission scoping inherited from the user. Database sync pulls from Salesforce/Postgres/etc. The data model is real (typed databases, relations, rollups). Agent access is MCP-server-mediated. Write-back exists but reviewability is "you see the page diff in Notion's history" — not structurally distinguished from human edits. Closed, cloud-only.

**Anytype** — Local-first, E2E-encrypted, P2P-synced object-graph PKM. As of Feb 2026 they're *prototyping* local agents that use Anytype objects as memory and let users build executable programs through chat. Closest in *philosophy* to Holon (local-first + structured data + agent-as-primitive), but the agent layer is still pre-alpha per their Feb 2026 community update.

**Tana** — Node-graph PKM with Supertags (typed nodes) + Fields. Has AI features (voice intake, AI meeting notes, chat-with-notes) and recent coverage mentions Claude Code reading/writing the workspace. Closed product, cloud-only. Genuinely structured data model — closer to Holon's block+property+tag model than markdown-graph tools.

**Khoj** — Self-hostable Django+pgvector RAG over your files (Markdown, PDF, org, Obsidian, Notion). Agent access is RAG-mediated, not direct DB. Write-back is limited to "create entries in a tracked folder." It's a *reader* of your knowledge base, not a co-author. Real OSS, mature.

**OpenHands (ex-OpenDevin)** — Code-focused agent platform with event-stream state, sandboxed Docker workspaces, and a 9-component SDK (event-sourced state, tool system, secret registry, etc.). The "human artifact" is the filesystem of a project. Mature, production-used, but explicitly software-engineering, not PKM.

**Cursor / Claude Code / Aider** — Baseline for "agent shares your workspace." The artifact is the source tree + git. Write-back is reviewable via diffs and git history; tool-proxy via MCP (Claude Code) is dynamic. No structured data model — files are the substrate. They prove the pattern works at the file level; PKM tools are trying to lift it to structured notes.

**Rivet / Flowise / LangFlow** — Visual agent builders. Not in scope: they're authoring environments for agents, not bridges between human knowledge and agent context.

**Saga / PersonalAI** — Saga is a Notion-lite with built-in AI chat-on-top. PersonalAI is a proprietary "personal language model" service that builds a vector profile from user inputs. Neither exposes structured agent integration.

---

## Comparison table

| Tool | Data model | Agent access | Write-back review | Tool-proxy | Sync | OSS |
|---|---|---|---|---|---|---|
| **Holon** | Blocks + tags + props in Turso + Loro + org files | Built-in MCP, PRQL, live cells, source blocks | Org diff + Loro CRDT history | Yes (planned MCP++) | Loro CRDT, multi-device + multi-agent | Yes |
| Letta | Agent memory blocks | Tool-call edits to own memory | No human artifact | Yes (tools) | Per-agent | Yes |
| mem0/Zep/Cognee | Chat-derived facts/graph | API/SDK | n/a (no human notes) | No | Cloud or self-host | Partly |
| Obsidian + MCP | Markdown + frontmatter + wikilinks | File I/O via MCP servers | git/file watcher only | Yes (multiple) | Obsidian Sync / 3rd-party | Plugins yes, core no |
| Logseq + plugins | Blocks (md/org files) | Plugin context only | Manual paste | No MCP layer | Logseq Sync / git | Yes |
| Reflect | E2E notes | Built-in only | Internal | No | Proprietary | No |
| Notion AI / Dev Platform | Typed DBs + pages | MCP + Notion API | Page history | Yes | Cloud | No |
| Anytype | Object graph | Local-agent prototype | CRDT history | Planned | P2P, E2E | Yes |
| Tana | Supertag node graph | Claude Code (recent) | Workspace history | Limited | Cloud | No |
| Khoj | Indexed file corpus | RAG retrieval | Limited create | Some | Self-host | Yes |
| OpenHands | Filesystem | Sandboxed exec | git | Yes | n/a | Yes |
| Claude Code / Cursor | Source tree | Direct + MCP | git diffs | Yes (dynamic) | git | Partly |
| OpenClaw (claimed) | JSONL events | MCP client | n/a | Yes | Self-host | Yes |

---

## Patterns the field has converged on

1. **MCP is winning as the integration layer.** Every serious tool now either ships an MCP server (Obsidian plugins, Notion, Anytype's roadmap) or is an MCP client (Claude Code, OpenClaw, Cursor). Two years ago this was fragmented; today MCP is the lingua franca.
2. **"Agent memory" ≠ "knowledge bridge."** mem0/Zep/Cognee/Letta all solve *agent-internal* memory. None give the human a usable artifact. The PKM tools (Obsidian/Logseq/Tana/Anytype) have the artifact but bolt agents on as a chat sidecar.
3. **Write-back is shallow everywhere.** Almost no tool distinguishes agent-authored content from human-authored content at the data-model level. The best you get is "page history shows it was Claude" (Notion) or "git blame says claude-bot" (Claude Code on a repo).
4. **No taint tracking.** Nobody tracks "this block was derived from that tool call which used that source." Provenance graphs are absent across the entire market.
5. **CRDT is rare.** Anytype and Holon are nearly alone in using CRDTs for the human/agent shared state. Everyone else does last-writer-wins on files or cloud-DB locks.
6. **Stuck point: the chat-pane ceiling.** Obsidian + AI, Logseq + AI, Reflect, Mem.ai — all let you "talk to your notes" but the agent rarely *operates inside* the note structure. The PKM-with-AI category mostly hasn't gotten past "chat-with-RAG."

---

## Honest comparison vs. Holon

### Where Holon's architecture genuinely differentiates

- **Reactive cells + IVM as the agent substrate.** No competitor offers `watch_query` + incrementally maintained matviews as a primitive. Anytype's graph rebuilds, Notion polls, Obsidian re-indexes. Holon's reactive engine means an agent can declaratively subscribe to "all blocks with tag X under doc Y" and get push updates — this is the right primitive for live agent supervision of a workspace.
- **CRDT-first for multi-agent.** Loro under the hood means two agents + a human editing simultaneously is well-defined, not "merge conflict, good luck." Only Anytype is comparable, and Anytype's agent story is still prototype.
- **Org files as a human-readable round-trippable substrate.** Notion/Tana lock you in; Obsidian gives you markdown but no typed model; Logseq has the model but no CRDT/IVM/MCP integration story. Holon hits the rare combination: typed + text-roundtrip + CRDT + reactive.
- **Built-in MCP that exposes the *whole* DB + UI state.** Most Obsidian MCP servers expose file ops + search. Holon's MCP exposes live UI state, PRQL queries, source-block execution, undo/redo, navigation — i.e. the agent can introspect and drive the app, not just read files. This is closer to "agent gets a real handle on the application," not just on its files.
- **Property-based testing with shared reference model across frontends.** Nobody else in this space has invested in a structural correctness model. This will pay off when the agent layer gets non-trivial.

### Where Holon is genuinely weaker

- **Ecosystem.** Obsidian has ~2000 community plugins; Holon has zero. Notion has thousands of integrations. Even Logseq has a meaningful plugin store. A user picking a PKM in 2026 cannot import their existing workflow.
- **Mobile.** No competitive mobile story. Obsidian, Notion, Tana, Anytype, Reflect all have working mobile apps. PKM is read-heavy on mobile; this is a hard gate for adoption.
- **Onboarding.** Cargo workspace + Rust toolchain + multiple frontends + org-mode literacy is a steep wall. Anytype, Reflect, Tana, Notion all install in 30 seconds.
- **No hosted offering.** Khoj, Letta, Notion, Reflect all offer hosted versions. Self-hosting is a niche.
- **PRQL is a UX bet that may not pay off.** Power users will love it; the median note-taker will not write PRQL. Tools like Tana hide query language behind builder UIs.
- **No AI features today, while everyone else has chat-with-notes.** Obsidian Copilot, Smart Connections, Tana AI, Notion AI, Reflect AI all let a user *today* ask "summarize my meeting notes from this week." Holon's MCP exposes the data for an external agent, but a user has to bring their own Claude Code window — there's no in-app chat.
- **No agent-content provenance/taint visible in the UI yet.** The architecture supports it (tags, properties, backlinks) but the feature isn't built. Notion's "page history" UX is more visible right now even though it's less structured.
- **Discoverability of MCP tools.** Notion's developer platform actively lists agents to assign work to. Holon doesn't have that surface yet.

### Where Holon is at parity (don't oversell)

- vs. Obsidian for write-back: both let an agent mutate the substrate; Obsidian via files, Holon via blocks. Holon's CRDT advantage only matters for genuinely concurrent edits, which most solo users won't hit.
- vs. Letta for agent state: Letta is more sophisticated as an agent-memory library, but the comparison is unfair — Letta isn't a PKM. The right pairing is "Holon as substrate + Letta-style memory for agents that operate on it."

---

## Gaps in the market Holon could own

1. **Typed agent provenance.** Every agent-authored block should carry `:source: tool-call-id`, link to the org-source-block that produced it, and be trivially diffable/revertable as a unit. Nobody does this. Holon's tag+property+backlink model is the right shape.
2. **Re-executable artifacts.** Org source-blocks become re-executable tool calls — i.e. the human's notes contain the *executable record* of agent invocations. This is the deepest version of the "MCP-proxy++" idea and it's structurally impossible in markdown/Notion without bolting on a second runtime.
3. **Live agent supervision.** A reactive query like "show me everything the agent has changed in the last hour, grouped by source tool" is one PRQL away in Holon. In Obsidian/Notion it requires custom plugins. This is a genuine "tail -f your agents" UX that nobody else can build cheaply.
4. **Multi-agent concurrency.** Loro means two agents working on overlapping subtrees converge correctly. Once people actually run multiple agents simultaneously (and 2026 trend lines say they will), file-based PKMs will start producing merge garbage. Holon's architecture is ready.
5. **Bidirectional MCP-as-substrate.** Most MCP integrations are "agent calls tool, tool returns data." Holon could be the first where *the proxy itself is a block* — every tool call becomes a first-class artifact with backlinks, undo, and reactive subscribers. This is the genuinely novel idea in the MCP-proxy++ framing.
6. **Open data + open agent + local-first.** Anytype is the only competitor here; Notion/Tana/Reflect/Mem all fail at least one. If Holon can close the onboarding gap, it owns this quadrant.

### What would move the needle most

Be honest about the trade: Holon's architecture is genuinely ahead in the substrate layer. The risk is that the world standardizes on "Notion + MCP agents" or "Obsidian + MCP agents" *before* the substrate advantages become visible to end users. The killer demo is **agent provenance + re-executable source blocks + reactive supervision** — a workflow no other tool can do at all, demonstrated end-to-end in a 90-second video. Without that, the architectural superiority is invisible and Holon competes on PKM-feature parity, where it will lose.

---

## Sources

- openclaw/openclaw GitHub, OpenClaw MCP docs
- nousresearch/hermes-agent GitHub (authenticity unverified)
- letta-ai/letta GitHub, MemGPT → Letta announcement
- mem0 State of Agent Memory 2026, Cognee evaluation
- cyanheads/obsidian-mcp-server, jacksteamdev/obsidian-mcp-tools, msdanyg/smart-connections-mcp
- khoj-ai/khoj, Khoj docs
- Anytype Feb 2026 community update
- Notion Agent help, TechCrunch on Notion developer platform (2026-05-13)
- Tana PKM page
- OpenHands paper (arXiv 2407.16741), OpenHands SDK paper (arXiv 2511.03690)
- Logseq plugins: jarodise/logseq-copilot, martindev9999/logseq-composer
- Reflect vs Mem comparison (aloa.co)
