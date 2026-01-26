---
name: turso-fix
description: "Orchestrate cross-repository bug fixes between holon and Turso. Use when you discover a Turso issue in holon, need to diagnose if it's your code or Turso's, create a reproducer and handoff prompt, and validate the fix back in holon."
---

# Turso Fix Workflow

## What This Skill Does

Orchestrates a bug-fix workflow across holon (application) and Turso (dependency):

1. **Diagnose** - Determine if the issue is in holon or Turso
2. **Reproduce** - Create a minimal SQL reproducer using `turso-sql-replay`
3. **Handoff** - Write a structured handoff document for the Turso session
4. **Validate** - Test the Turso fix back in holon

The Turso repo lives at `~/Workspaces/bigdata/turso/`. Fixes are done manually in a separate Claude session there — this skill produces the reproducer and handoff, then validates the result.

## Quick Start

```
/turso-fix "Materialized view returns stale rows after DELETE"
```

Or run individual phases:

```
/turso-fix diagnose
/turso-fix reproduce
/turso-fix handoff
/turso-fix validate
```

---

## Phase 1: Diagnose

### Goal
Determine whether the bug is in holon or Turso.

### Steps

1. **Gather evidence from holon**
   - Read error logs, stack traces, panic output
   - Identify the failing code path
   - Note Turso API calls involved (`conn.execute`, `conn.query`, `set_change_callback`, matview operations)

2. **Check the E2E PBT** (per CLAUDE.md rules)
   - Can `crates/holon-integration-tests/tests/general_e2e_pbt.rs` reproduce it?
   - If not, think about how to make prod and E2E more similar

3. **Analyze Turso code** (if issue appears to be in Turso)
   - Search `~/Workspaces/bigdata/turso/` for relevant functions
   - Compare expected vs actual behavior

4. **Make determination** and state it clearly:
   - **Holon issue** — fix in this repo
   - **Turso issue** — proceed to Phase 2
   - **Both** — fix holon part first, then proceed to Phase 2

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

## Phase 2: Reproduce

### Goal
Create a minimal, self-contained SQL reproducer using the `turso-sql-replay` tool.

### The turso-sql-replay Tool

Located at `tools/src/turso_sql_replay.rs`, built with:
```bash
cargo build -p holon-tools
```

#### Subcommands

| Command | Description |
|---------|-------------|
| `extract <logfile>` | Parse a `HOLON_TRACE_SQL=1` log into a `.sql` replay file |
| `replay <file.sql>` | Replay against Turso with CDC + matview consistency checks |
| `minimize <file.sql>` | Reduce to minimal crash reproducer (6-phase ddmin) |
| `run <logfile>` | Extract + replay in one shot |

#### Typical Reproduction Flow

1. **Capture a SQL trace** from the Flutter app:
   ```bash
   HOLON_TRACE_SQL=1 flutter run 2>&1 | tee /tmp/flutter-sql-trace.log
   ```

2. **Extract and replay** to confirm the bug reproduces:
   ```bash
   cargo run --bin turso-sql-replay -- run /tmp/flutter-sql-trace.log --check-after-each
   ```

3. **Extract to a file** for iteration:
   ```bash
   cargo run --bin turso-sql-replay -- extract /tmp/flutter-sql-trace.log -o /tmp/replay.sql
   ```

4. **Filter to relevant tables** if the trace is large:
   ```bash
   cargo run --bin turso-sql-replay -- extract /tmp/flutter-sql-trace.log \
     --include block,current_focus \
     -o /tmp/replay-filtered.sql
   ```

5. **Minimize** a crash reproducer:
   ```bash
   cargo run --bin turso-sql-replay -- minimize /tmp/replay.sql
   ```
   This uses ddmin (6 phases: prefix bisection, table groups, chunk removal, individual DML, individual DDL, final cleanup) to find the smallest set of SQL statements that still triggers the crash.

6. **Minimize** a matview inconsistency:
   ```bash
   cargo run --bin turso-sql-replay -- replay /tmp/replay.sql --check-after-each
   ```
   When `--check-after-each` detects a mismatch, it prints the diverging statement number. Use `--stop-at` in the extract phase to narrow down.

#### Replay File Format

The `.sql` files use annotated comments that the replayer understands:

```sql
-- Extracted from: /tmp/flutter-sql-trace.log
-- Statements: 42
-- Time range: 2026-03-19T10:00:00.000000Z .. 2026-03-19T10:00:05.000000Z

-- !SET_CHANGE_CALLBACK 2026-03-19T10:00:00.100000Z

-- Wait 50ms
-- [actor_ddl] 2026-03-19T10:00:00.150000Z
CREATE TABLE IF NOT EXISTS block (...);

-- [execute_sql] 2026-03-19T10:00:00.200000Z
INSERT INTO block (id, content) VALUES ('b1', 'hello');
```

- `-- !SET_CHANGE_CALLBACK` — registers CDC callback (tests IVM interaction)
- `-- Wait Nms` — timing between statements (honored with `--replay-timing`)
- `-- [tag]` — statement metadata (actor_ddl, execute_sql, etc.)
- Parameters are inlined (no `$param` or `?` placeholders in the output)

### Save the reproducer

Save the minimal reproducer SQL in `devlog/` with a descriptive name:
```
devlog/2026-03-19-turso-ivm-stale-delete.sql
```

---

## Phase 3: Handoff

### Goal
Write a clear, actionable handoff document for the Turso fixing session.

### Steps

1. Write the handoff to `devlog/YYYY-MM-DD-turso-fix-handoff.md`
2. Include the minimal reproducer SQL inline
3. Point to the Turso repo location: `~/Workspaces/bigdata/turso/`

### Handoff Template

````markdown
# Turso Bug Fix: [Brief Title]

## Bug Description
[Clear description of the bug]

## Reproduction

### Minimal SQL reproducer
```sql
[Paste the minimized .sql content here]
```

### How to run
```bash
# From ~/Workspaces/pkm/holon/
cargo run --bin turso-sql-replay -- replay devlog/YYYY-MM-DD-reproducer.sql --check-after-each
```

### Expected behavior
[What should happen]

### Actual behavior
[What actually happens — include panic message, inconsistency output, etc.]

## Analysis

### Relevant Turso code locations
- `turso/src/[relevant file]`: [what it does]
- `turso_core/src/[relevant file]`: [what it does]

### Root cause hypothesis
[Your best guess at what's wrong and why]

### Suggested fix approach
[If you have ideas]

## Acceptance Criteria
- [ ] Bug is fixed
- [ ] Existing Turso tests pass
- [ ] New test covers this case
- [ ] Holon's `turso-sql-replay` reproducer passes clean
- [ ] Changes are minimal and focused

## Turso Repo
`~/Workspaces/bigdata/turso/` (branch: `holon`)
````

### Tell the user

After writing the handoff, tell the user:
> Handoff written to `devlog/YYYY-MM-DD-turso-fix-handoff.md`.
> Open a Claude session in `~/Workspaces/bigdata/turso/` and paste the handoff content.
> When the fix is ready, come back here and run `/turso-fix validate`.

---

## Phase 4: Validate

### Goal
Test the Turso fix in holon.

### Steps

1. **Ensure Turso dependency points to the fix**
   - Check `.cargo/config.toml` for local path override:
     ```toml
     [patch.'https://github.com/nightscape/turso.git']
     turso = { path = "/Users/martin/Workspaces/bigdata/turso/turso" }
     turso_core = { path = "/Users/martin/Workspaces/bigdata/turso/turso-core" }
     ```
   - Or update the git branch in `Cargo.toml`

2. **Rebuild**
   ```bash
   cargo build -p holon-tools 2>&1 | tee /tmp/turso-fix-rebuild.log
   ```

3. **Re-run the reproducer**
   ```bash
   cargo run --bin turso-sql-replay -- replay devlog/YYYY-MM-DD-reproducer.sql --check-after-each 2>&1 | tee /tmp/turso-fix-validation.log
   ```

4. **Run the E2E PBT**
   ```bash
   cargo test -p holon-integration-tests --test general_e2e_pbt 2>&1 | tee /tmp/turso-fix-e2e.log
   ```

5. **Report results**
   - If reproducer passes clean: fix is validated
   - If still failing: report back to Turso session with new findings
   - Save a devlog entry documenting the fix

---

## Related

- `tools/src/turso_sql_replay.rs` — the extraction/replay/minimize tool
- `scripts/extract-sql-trace.py` — original Python extractor (reference only)
- `crates/holon-integration-tests/tests/general_e2e_pbt.rs` — E2E property-based test
- Turso repo: `~/Workspaces/bigdata/turso/`
- Memory: check `turso_ivm_json_set_bug.md` and `MEMORY.md` for known Turso IVM issues
