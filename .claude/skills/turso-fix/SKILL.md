---
name: turso-fix
description: "Orchestrate cross-repository bug fixes between holon and Turso. Use when you discover a Turso issue in holon, need to diagnose if it's your code or Turso's, create a handoff prompt, launch an agent to fix Turso, and test the fix back in holon."
---

# Turso Fix Workflow

## What This Skill Does

Orchestrates a complete bug-fix workflow across holon (your application) and Turso (the dependency):

1. **Diagnose** - Determine if the issue is in holon or Turso
2. **Document** - Create a structured handoff prompt with reproduction steps
3. **Dispatch** - Launch a background agent to fix Turso
4. **Validate** - Test the fix back in holon

## Prerequisites

- Turso repository cloned locally (or accessible path)
- Claude Flow installed (`npx @claude-flow/cli@latest`)

## Quick Start

Invoke the skill with a description of the issue:

```
/turso-fix "Sync fails when writing multiple rows with same timestamp"
```

Or manually trigger each phase:

```
/turso-fix diagnose   # Phase 1: Investigate the issue
/turso-fix handoff    # Phase 2: Create handoff prompt
/turso-fix dispatch   # Phase 3: Launch Turso agent
/turso-fix validate   # Phase 4: Test the fix
```

---

## Phase 1: Diagnose

### Goal
Determine whether the bug is in holon or Turso.

### Steps

1. **Gather evidence from holon**
   - Read error logs, stack traces
   - Identify the failing code path
   - Note Turso API calls involved

2. **Store findings in memory**
   ```
   Use mcp__claude-flow__memory_store to save:
   - key: "turso-fix/current/symptoms"
   - value: { error_message, stack_trace, reproduction_steps }
   ```

3. **Analyze Turso code** (if issue appears to be in Turso)
   - Search Turso codebase for relevant functions
   - Compare expected vs actual behavior

4. **Make determination**
   - Store verdict: `turso-fix/current/verdict` = "holon" | "turso" | "both"

### Diagnosis Template

```markdown
## Issue Diagnosis

**Observed behavior**: [What happens]
**Expected behavior**: [What should happen]
**Reproduction steps**:
1. ...
2. ...

**Evidence**:
- Error: `[error message]`
- Stack trace points to: `[file:line]`
- Turso API call: `[function/method]`

**Verdict**: [ ] Holon issue  [ ] Turso issue  [ ] Both
**Confidence**: [Low/Medium/High]
**Reasoning**: [Why you think it's this]
```

---

## Phase 2: Handoff

### Goal
Create a clear, actionable prompt for the Turso-fixing agent.

### Handoff Prompt Template

```markdown
## Turso Bug Fix Request

### Context
I'm using Turso in my holon application. I discovered the following issue:

### Bug Description
[Clear description of the bug]

### Reproduction Steps
1. [Step 1]
2. [Step 2]
3. [Step 3]

### Expected Behavior
[What should happen]

### Actual Behavior
[What actually happens]

### Relevant Code Locations (Turso)
- Likely location: `[file path]`
- Related functions: `[function names]`

### Evidence from holon
```
[Error message / stack trace / logs]
```

### Suggested Fix Approach
[If you have ideas about what might need to change]

### Acceptance Criteria
- [ ] Bug is fixed
- [ ] Existing tests pass
- [ ] New test covers this case
- [ ] Changes are minimal and focused
```

### Store handoff prompt
```
mcp__claude-flow__memory_store:
  key: "turso-fix/current/handoff-prompt"
  value: { prompt: "[the handoff prompt]", created_at: timestamp }
```

---

## Phase 3: Dispatch

### Goal
Launch a background agent to work on the Turso fix.

### Option A: Task Tool (Same Machine)

If Turso is cloned locally:

```javascript
Task({
  subagent_type: "coder",
  description: "Fix Turso bug",
  prompt: `
    Working directory: /path/to/turso

    ${handoff_prompt}

    After fixing:
    1. Run tests to verify the fix
    2. Summarize what you changed and why
    3. Store summary in memory: mcp__claude-flow__memory_store
       key: "turso-fix/current/fix-summary"
  `,
  run_in_background: true
})
```

### Option B: Separate Claude Code Session

If Turso is in a different workspace:

1. Copy handoff prompt to clipboard or file
2. Open new Claude Code session in Turso directory
3. Paste handoff prompt
4. When done, copy fix summary back

### Track dispatch
```
mcp__claude-flow__memory_store:
  key: "turso-fix/current/status"
  value: { phase: "dispatched", agent_started: timestamp }
```

---

## Phase 4: Validate

### Goal
Test the Turso fix in holon.

### Steps

1. **Retrieve fix summary**
   ```
   mcp__claude-flow__memory_retrieve:
     key: "turso-fix/current/fix-summary"
   ```

2. **Update Turso dependency**
   - If using git dependency: update to fix branch
   - If local path: ensure changes are available
   - Run `cargo update` or equivalent

3. **Run holon tests**
   ```bash
   cargo test --package holon-integration-tests 2>&1 | tee /tmp/validation.log
   ```

4. **Verify the specific issue**
   - Reproduce the original steps
   - Confirm the bug is fixed

5. **Record outcome**
   ```
   mcp__claude-flow__memory_store:
     key: "turso-fix/current/validation"
     value: {
       passed: true/false,
       test_output: "...",
       notes: "..."
     }
   ```

6. **Learn from success** (if fixed)
   ```
   mcp__claude-flow__hooks_intelligence_pattern-store:
     pattern: "Turso fix: [brief description]"
     type: "bug-fix"
     metadata: {
       symptoms: "...",
       root_cause: "...",
       fix_approach: "..."
     }
   ```

---

## Memory Keys Reference

| Key | Purpose |
|-----|---------|
| `turso-fix/current/symptoms` | Initial bug symptoms and evidence |
| `turso-fix/current/verdict` | Diagnosis result (holon/turso/both) |
| `turso-fix/current/handoff-prompt` | The prompt sent to Turso agent |
| `turso-fix/current/status` | Current workflow phase |
| `turso-fix/current/fix-summary` | What the Turso agent changed |
| `turso-fix/current/validation` | Test results after fix |
| `turso-fix/history/{id}` | Archived completed fixes |

---

## Configuration

Set your Turso repository path:

```bash
npx @claude-flow/cli@latest memory store \
  --key "turso-fix/config/turso-path" \
  --value "/path/to/your/turso/clone"
```

---

## Example Session

```
User: /turso-fix "Getting 'database locked' errors when concurrent writes happen"

Claude: Starting Turso fix workflow...

**Phase 1: Diagnose**
Let me investigate the issue in holon first.

[Reads error logs, traces the code path, searches Turso for locking behavior]

Diagnosis: This appears to be a Turso issue. The locking logic in
`turso/src/connection.rs` doesn't handle concurrent transaction commits correctly.

**Phase 2: Handoff**
I've created a handoff prompt and stored it in memory. Here's the summary:
- Bug: Database locked on concurrent writes
- Location: connection.rs:245
- Suggested fix: Add proper transaction queuing

**Phase 3: Dispatch**
Launching background agent to work on Turso fix...
Agent spawned. Working in background.

[Later, after agent completes]

**Phase 4: Validate**
The Turso agent completed with this fix:
- Added TransactionQueue in connection.rs
- Updated commit() to use queue
- Added test for concurrent commits

Running holon tests to validate...
✓ All tests pass. The database locked error is resolved.

Storing this pattern for future reference.
```

---

## Troubleshooting

### Agent can't access Turso repo
- Verify the path in `turso-fix/config/turso-path`
- Ensure the directory exists and is readable

### Fix summary not found
- Check if background agent completed: `mcp__claude-flow__memory_retrieve`
- Agent may still be working; check with TaskOutput

### Validation fails after fix
- Read fix summary to understand what changed
- May need to iterate: go back to Phase 2 with additional context

---

## Related

- `/github:multi-repo-swarm` - For more complex multi-repo coordination
- `/sparc:debug` - For deeper debugging sessions
- Memory commands: `npx @claude-flow/cli@latest memory --help`
