# Use Cases: Cross-Source Integration

**Related design**: [External Element Embedding](../DESIGN_EXTERNAL_ELEMENT_EMBEDDING.md)

These use cases motivate the unified matview approach (Option 4) for embedding external elements into Holon's block tree.

## UC-1: Rich Annotations on Todoist Tasks

**Problem**: Todoist comments are limited to a few lines of text. Research notes, cross-references to emails, and structured sub-information don't fit.

**Solution**: Attach Holon sub-blocks to a Todoist task. These blocks support full content (org-mode markup, links, embedded references) and stay local — they never sync back to Todoist.

**Example**: A task "Research EU AI Act implications" has Todoist sub-tasks for deadlines, plus Holon annotation blocks containing a multi-page summary, links to relevant articles, and a cross-reference to an email thread.

## UC-2: Fine-Grained Task Tracking on JIRA Tickets

**Problem**: JIRA tickets serve the team. Personal micro-tasks, investigation notes, and implementation checklists are information overload for others.

**Solution**: A JIRA ticket appears in Holon as an external item. You attach private sub-tasks, debugging notes, and code snippets as child blocks. Your team sees the ticket; you see your private workspace around it.

**Example**: JIRA ticket "Implement OAuth flow" has three JIRA sub-tasks visible to the team. In Holon, you have 15 granular steps, a list of edge cases to test, and a link to the relevant RFC section.

## UC-3: Multi-Source Project Aggregation

**Problem**: A project like "Prepare income tax 2025" spans emails (accountant correspondence), documents (tax forms on GDrive), tasks (Todoist deadlines), and personal notes. These live in separate tools with no shared structure.

**Solution**: One Holon project node with children from every source. Emails, documents, tasks, and notes are all visible in a single tree. Cross-link any item to items from previous years' tax preparation.

**Example tree**:
```
Prepare income tax 2025
├── todoist:deadline-march-15     (Todoist: "Submit tax return")
├── gmail:msg-abc123              (Email: "Tax documents from accountant")
├── gdrive:doc-xyz789             (GDrive: "2025 Tax Summary.xlsx")
├── block:personal-notes          (Holon: deductions research)
└── block:cross-ref-2024          (Holon: link to last year's project)
```

## UC-4: Meeting Preparation & Follow-Up

**Problem**: Meeting context is scattered: the calendar event is in Google Calendar, relevant tickets are in JIRA, background emails are in Gmail, your agenda is in your head.

**Solution**: A calendar event appears as an external item. Attach agenda items, link relevant JIRA tickets and emails as children. After the meeting, attach action items that reference or create Todoist tasks.

**Example**: "Q2 Planning Meeting" node contains the calendar event, three JIRA epics for discussion, two emails with budget context, your agenda notes, and post-meeting action items linked to Todoist.

## UC-5: Learning & Reading Pipeline

**Problem**: Highlights and notes from articles/books (Readwise, Pocket, Kindle) are trapped in reading tools. Connecting them to your existing knowledge requires manual copy-paste.

**Solution**: A book or article appears as an external item. Attach highlights, connect them to existing Holon notes, link to related concepts. The external item is the anchor; your knowledge graph grows around it.

**Example**: A Readwise article "Distributed Systems Patterns" has child blocks with your highlights, each cross-referenced to relevant sections in your architecture notes.

## UC-6: Weekly Review Dashboard

**Problem**: Understanding "what did I actually do this week" requires opening 5+ tools and mentally stitching together the timeline.

**Solution**: A single query across all sources: "show me everything I touched this week" — completed Todoist tasks, updated JIRA tickets, modified org files, sent emails — all in one tree, sortable by time, groupable by project.

**Example query**: `from unified_tree | filter updated_at > @last_monday | sort updated_at desc` returns a mixed list of Todoist completions, JIRA status changes, and edited Holon blocks.

## UC-7: Decision Log with Evidence Trail

**Problem**: Important decisions (purchasing, hiring, architectural) involve evidence from multiple sources. The rationale is lost because the evidence is scattered.

**Solution**: A decision block has children from multiple sources: vendor emails, comparison spreadsheets, budget approvals, implementation tasks. Each linked item is live — changes in the source update here automatically via CDC.

**Example**: "Choose CI/CD platform" has children: three vendor evaluation emails (Gmail), a feature comparison doc (GDrive), the budget approval ticket (JIRA), and implementation tasks (Todoist). Six months later, the full decision context is still navigable.

## UC-8: Client/Project Dossier

**Problem**: Freelancers and consultants have per-client information scattered across invoicing tools, email, file storage, task managers, and time trackers.

**Solution**: One tree per client aggregating everything: invoices (accounting tool), communication (email), deliverables (GDrive), tasks (Todoist), time entries (Toggl). Open the client node, see everything.

**Example**: "Client: Acme Corp" contains all project deliverables, the full email thread history, outstanding invoices, and active tasks — queryable and annotatable.

## UC-9: Incident Response Log

**Problem**: Incident response generates artifacts across PagerDuty, Slack, GitHub, JIRA, and internal docs. Post-mortems require manually gathering these.

**Solution**: A PagerDuty alert appears as an external item. Attach investigation notes, link the relevant deploy (GitHub), reference the JIRA ticket, summarize the Slack thread. The post-mortem tree writes itself.

**Example**: "Incident: API latency spike 2026-02-28" contains the PagerDuty alert, the GitHub commit that caused it, the rollback PR, the JIRA post-mortem ticket, and your investigation notes with timestamps.

## UC-10: Cross-Source Linking ("Relate Anything to Anything")

**Problem**: Every tool lets you link within its own system. No tool lets you link *across* systems with navigable structure.

**Solution**: Holon's unified tree + GQL graph edges allow arbitrary cross-references between items from any source. "This email relates to that JIRA ticket relates to that Todoist task" — not via an integration platform that just triggers actions, but as actual queryable, annotatable structure.

**Example**: An email from a client mentions a feature request. You link it to the JIRA epic, the Todoist research task, and three related Holon design notes. Querying "what's connected to this feature request" traverses all sources.

## Demo-Worthy Moments

These are the strongest visual demonstrations for marketing:

1. **"One tree, every source"** — Expand a project node; children are a mix of Todoist tasks, JIRA tickets, emails, and handwritten notes. No context switching. The "holy shit" screenshot for PKM enthusiasts.

2. **"Query across your entire work life"** — Type a query that returns results spanning Todoist, JIRA, and local blocks in one table, sortable and filterable. Power-user magnet.

3. **"Annotate what you can't control"** — A read-only JIRA ticket with rich local sub-tasks and notes attached. "Your team sees the ticket. You see your private workspace around it." Resonates with anyone frustrated by tool limitations.

4. **"Time-travel across systems"** — Because Holon has Loro CRDT history on annotation blocks and CDC from external sources, show "what did my project look like last Tuesday" with mixed-source children. Nobody else does this.

## Design Implications

These use cases surface requirements beyond parent-child nesting:

- **Cross-source lateral links** (UC-3, UC-7, UC-10): Not just tree nesting but arbitrary edges between external items. The GQL EAV graph schema can handle this (`todoist:123` → `gmail:abc` edge), but the current design doc focuses on tree structure. This may be a separate feature.
- **Annotation block document ownership** (UC-1, UC-2): Blocks parented under external items have no document in their ancestor chain. They need a home for OrgSync and document-scoped queries.
- **Multi-source queries** (UC-6): The unified matview must support time-range filtering and cross-source sorting to enable review workflows.
