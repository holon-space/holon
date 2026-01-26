---
name: petri-net
description: |
  Petri Net simulator for personal project management using Digital Twins.
  Delegates all deterministic operations to the holon-engine CLI binary.
  Claude handles: scenario design, net editing, agent transitions, and interpretation.
  Use /petri to interact with the simulator.
author: Claude Code
version: 0.3.0
allowed-tools:
  - Read
  - Write
  - Edit
  - Grep
  - Glob
  - Bash
  - Task
  - AskUserQuestion
  - mcp__perplexity__perplexity_ask
---

# Petri-Net Simulator (v0.3 — Flat-Net Model)

All deterministic Petri net operations are handled by the `holon-engine` binary.
Claude's role is: scenario design, net editing, interpreting results, and agent transitions.

## Engine Binary

The engine lives at `crates/holon-engine`. Build it before first use:

```bash
cd crates/holon-engine && cargo build
```

Binary path: `crates/holon-engine/target/debug/holon-engine`

All engine commands take `--dir <path>` pointing to a scenario directory.

## File Structure

A scenario lives in a directory with three files:

- `net.yaml` — Transitions, objective function, constraints
- `state.yaml` — Initial token states (flat key-value attributes)
- `history.yaml` — Append-only event log (managed by the engine)

State is ALWAYS `state.yaml + replay(history.yaml)`. Event sourcing. Never edit state.yaml after simulation starts.

### YAML Format

#### `net.yaml`

```yaml
transitions:
  collect_income_statement:
    inputs:
      - bind: person          # local name for the matched token
        token_type: person     # match tokens of this type
        precond:               # attribute constraints for matching
          energy: ">= 0.2"    # Rhai comparison
          status: "active"     # exact match
      - bind: doc
        token_type: document
        precond:
          kind: "income_statement"
          status: "missing"
    outputs:
      - from: person           # re-produce bound token
        postcond:              # Rhai expressions for new attribute values
          energy: "person.energy - 0.02"
      - from: doc
        postcond:
          status: '"found"'    # string literal postcond (note: Rhai needs inner quotes)
    duration: 15               # minutes

  sleep:
    inputs:
      - bind: person
        token_type: person
        precond:
          energy: "<= 0.3"
    outputs:
      - from: person
        postcond:
          energy: "if person.energy + 0.7 > 1.0 { 1.0 } else { person.energy + 0.7 }"
    duration: 480

objective:
  expr: |
    let tax_value = if tax_refund.status == "realized" { tax_refund.value } else { 0.0 };
    (tax_value) * discount + self.energy * 100.0
  constraints:
    - "self.energy > 0.05"
  discount_rate: 0.05
```

#### `state.yaml`

```yaml
clock: "2025-03-01T08:00:00Z"
tokens:
  self:
    token_type: person
    energy: 0.8
    health: 0.85
    status: active
  income_statement:
    token_type: document
    kind: income_statement
    status: missing
```

#### Key Concepts

- **Token types:** Each token has a fixed `token_type` (e.g., `person`, `document`, `asset`). Tokens never change type. Input arcs match by `token_type`.
- **Status attribute:** Use `status` to track lifecycle state (e.g., `missing → found → filed`). This replaces the old multi-place model. Preconditions check status via exact match; postconditions set it via Rhai string expression.
- **Binding (`bind`/`from`):** Local names within a transition. Input arcs bind tokens by type; output arcs reference them by the same name.
- **Preconditions:** Attribute constraints on input arcs. Comparison operators (`>= 0.3`), exact match (`"active"`), or placeholder bind (`"$var"`).
- **Postconditions:** Rhai expressions on output arcs computing new attribute values. Can reference any bound token by name. For string values, use Rhai string syntax: `'"value"'` (YAML single-quotes wrapping Rhai double-quoted string).
- **Placeholders (`$name`):** In precond, captures the current value. In postcond, uses it.
- **Token creation (`creates`):** Output arcs can create new tokens. `id_expr` is a Rhai expression for the new ID, `token_type` is the type, `attrs` are Rhai expressions for initial attributes.
- **Token consumption (`consume: true`):** Input arcs with `consume: true` remove the bound token after the transition fires. Consumed inputs don't need a matching output arc.
- **Materialization:** The `rank_tasks` tool auto-materializes org task blocks into a Petri Net: task blocks become transitions, `self` is always a token, `[[wiki links]]` create entity tokens, and `depends_on` creates dependency chains via completion tokens.

## Commands

When the user invokes `/petri`, parse the subcommand and delegate to the engine:

### Commands delegated to engine (use Bash tool)

For all commands below, run:
```bash
cd crates/holon-engine && cargo run -- <command> --dir <scenario_dir>
```

- `/petri state` → `cargo run -- state --dir <dir>`
- `/petri enabled` → `cargo run -- enabled --dir <dir>`
- `/petri step [transition]` → `cargo run -- step [transition] --dir <dir>`
- `/petri simulate <N>` → `cargo run -- simulate <N> --dir <dir>`
- `/petri whatif <transition>` → `cargo run -- whatif <transition> --dir <dir>`
- `/petri history` → `cargo run -- history --dir <dir>`
- `/petri validate` → `cargo run -- validate --dir <dir>`
- `/petri reset` → `cargo run -- reset --dir <dir>`
- `/petri objective` → `cargo run -- objective --dir <dir>`

### Commands handled by Claude

- `/petri load <dir>` — Set the active scenario directory for subsequent commands. Read and summarize the net.yaml and state.yaml to give the user an overview.
- `/petri edit` — Interactive net editing. Help the user add/modify transitions, tokens, or the objective function by editing the YAML files directly.
- `/petri design` — Help the user design a new scenario from scratch. Ask about their projects, tokens, and goals, then generate the YAML files.
- `/petri explain` — After running engine commands, interpret the results for the user. Explain why transitions are/aren't enabled, what the ranking means, and suggest next steps.
- `/petri rank` — Rank active task blocks from the Holon database using WSJF. Calls `rank_tasks` via MCP (when holon-live is running) or materializes from org files directly. Shows tasks ordered by value-per-minute with priorities and dependencies.

### Scenario Directory Resolution

When the user says `/petri load <dir>`, remember that directory for subsequent commands.
If no directory has been loaded, check for a scenario in the current working directory.
The examples live at: `crates/holon-engine/examples/tax-and-car/`

## Ranking: How It Works

The engine ranks enabled transitions by **Δobj / duration** — the objective function improvement per minute. This IS Weighted Shortest Job First (WSJF), derived from first principles rather than configured.

- For each enabled transition, the engine simulates firing it on a cloned marking
- Computes `obj(after) - obj(before)` = value produced
- Ranks by `value_produced / duration_minutes`
- Ties broken by lexicographic transition id

No priority weights, no effectiveness curves, no WSJF config. If something matters, it's because the objective function values it.

## Agent Transitions (Claude's Responsibility)

When a transition in `net.yaml` has `executor: agent`, Claude handles it:

1. Run `cargo run -- enabled --dir <dir>` to see if the agent transition is enabled
2. Read the transition definition to get the prompt template
3. Replace `{{bind_name.attribute}}` with current values from state
4. Report to user: "This transition requires an agent. Executing with tools: [list]"
5. Use the Task tool to spawn a subagent with the specified tools
6. Parse the agent's result
7. If successful: run `cargo run -- step <transition> --dir <dir>` with appropriate placeholder values
8. If failed: report error, transition does NOT fire

Agent transitions are the ONLY non-deterministic part. Everything else is handled by the engine.

## Important Principles

- **NEVER manually compute** state, enabled transitions, rankings, or objective function values. Always delegate to the engine.
- **NEVER modify state.yaml** after simulation starts. All state changes go through the engine via history.yaml.
- **Always run validate** after editing net.yaml to catch structural errors.
- **Flag gaps**: If the engine returns an error or unexpected result, report it as: "GAP: [description]" — this helps improve both the net definition and the engine.
