# now-query-mcp

- **Agent:** `claude-opus-dogfood-2026-05-08`
- **Task:** `block:now-query-mcp`
- **Completed:** 2026-05-08T11:56:42.471701+00:00
- **Commit:** `c6a82b2242d93fdca92c6bd14dae836f7312ea73`

## Summary

Shipped four MCP tools that expose the agent dogfood loop: `now_for_agent` (augments now-query::src::0 with assigned-to filter + DOING state + own-claims-first sort), `claim_task` (best-effort conditional claim with 1s re-verify against the file-watcher race window), `add_subtask` (UUID-minting wrapper over block.create), and `complete_task` itself.

Filed two follow-ups under this task via add_subtask:
- block:49696543-7ce3-403b-95be-a8682198b4e3 — wire `tags`/`requires` edge fields in add_subtask
- block:059f50f0-c8c9-4ded-86b9-412cd34fda0d — investigate `block:` prefix inconsistency on standalone org-file ingestion

Architecture decision recorded earlier in session: agents share holon-pkm/ via the file watcher (500ms debounce window) and work in separate worktrees of holon/. Per-property writes from different MCPs converge cleanly because the renderer rebases on local Turso state before serializing; same-property contention inside the debounce window is the only race, mitigated by claim_task's re-verify. Tools live in frontends/mcp/src/tools.rs; params in frontends/mcp/src/types.rs; chrono added to MCP Cargo.toml. Files commit will follow in a separate diff per user.
