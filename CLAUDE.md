# RULES
- Whenever there's a bug in the UI, always check if the E2E test in crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs (the ONE composed keystone PBT) can reproduce it.
- If the E2E test doesn't reproduce the issue think about how prod and E2E test can be made more similar, so that the E2E test can reproduce it.
- Every bug discovered OUTSIDE an automated test (dogfooding, agent exploration, user report) MUST be triaged with the `bug-gap-triage` skill (.claude/skills/bug-gap-triage/SKILL.md) and recorded as ONE new file under docs/Testing/bugfunnel/entries/ before/alongside the fix. Latency above the SLO (p95 interaction→projection-visible < 200ms) counts as such a bug. Totals are derived (`/usr/bin/python3 scripts/bugfunnel.py counts`) — never hand-maintained.
- Every NEW feature or behavior change follows the `holon-feature` skill (.claude/skills/holon-feature/SKILL.md): a red-for-the-right-reason keystone (headless) or GPUI (windowed) PBT BEFORE implementation, green after, the red log in the PR. Implementing without a covering PBT is a rare exception that MUST be escalated to Martin BEFORE landing.
- `dogfood-explorer` is the FINAL quality gate — it should catch ~90% of bugs before Martin does. A dogfood-found bug sends the feature BACK: first enhance the PBTs to catch it (red-for-the-right-reason as proof), then fix, then re-run dogfood. See the `holon-feature` skill.
- **NEVER** swallow errors!! Use `Result` and enrich the error message with information.
- **ALWAYS** `tee` before filtering output
- Before writing ANY org blocks into the vault (/Users/martin/Workspaces/pkm/holon-pkm) — task tracking, progress notes, handoffs — load the `holon-handoff` skill first and follow its structuring rules (imperative titles, details as child blocks, parent state derives from children). Never write vault org structure from memory.

## Error Handling Philosophy: Fail Loud, Never Fake

Prefer a visible failure over a silent fallback.

- Never silently swallow errors to keep things "working."
  Surface the error. Don't substitute placeholder data.
- Fallbacks are acceptable only when disclosed. Show a
  banner, log a warning, annotate the output.
- Design for debuggability, not cosmetic stability.

Priority order:
1. Works correctly with real data
2. Falls back visibly — clearly signals degraded mode
3. Fails with a clear error message
4. Silently degrades to look "fine" — never do this

## Parse, Don't Validate (Type-Driven Design)
Based on: https://www.harudagondi.space/blog/parse-dont-validate-and-type-driven-design-in-rust

**Core principle**: Make illegal states unrepresentable. When data enters the system (from org files, Loro, Turso, MCP), parse it into types that encode invariants — don't pass raw strings around and re-validate them at every call site.

**Concretely**:
- Prefer newtypes and enums over `String` for domain concepts with a fixed set of valid values (e.g., `ContentType`, `TaskState`, `QueryLanguage`, `ParentRef`)
- Parse at the boundary: convert raw data into typed representations at the point of entry (org parser, SQL row deserialization, Loro extraction), not at every usage point
- A function that takes `NonZeroF32` instead of `f32` and checking `b != 0` moves the proof obligation to the caller and eliminates an entire class of bugs
- Be suspicious of `match str.as_str() { ... }` scattered across multiple files — it's a sign that a string should be an enum
- Be suspicious of `.ok()` or `_ => default` on parse results — this silently swallows invalid data. Fail loudly at the boundary instead.

# VCS: how PRs get "merged" (linear history, no GitHub merges)

We never merge PRs through the GitHub UI (no merge commits, no squash-merges).
Instead:
1. Incorporate the PR branch's changes into the linear integration chain
   (stacked-workstreams weave onto `integration`, gates green), then land so
   `main` advances along the straight line.
2. Re-point the PR's bookmark to the lane tip IN the woven `integration`
   chain (`jj bookmark set <name> -r <rev-in-chain> --allow-backwards` if
   needed).
3. Push in TWO steps, in this order:
   a. `jj git push --bookmark <lane>` ALONE.
   b. Wait until `gh pr view <n> --json headRefOid` equals that tip.
   c. `jj git push --bookmark integration`.
   GitHub then marks the PR MERGED into `integration`. Pushing both bookmarks
   in one push CLOSES the PR instead. After the lane lands, `main` advances
   along the same line.

# `holon` MCP

Every frontend automatically launches an MCP server which is available to you as `holon`.
You can live-inspect the DB, inspect what the UI should render, etc.
Use it whenever you have a running application and you want to look under the hood to investigate.

# Org File Conventions
See [docs/Reference/ORG_SYNTAX.md](docs/Reference/ORG_SYNTAX.md) — org files store **bare IDs** without `block:`/`doc:` scheme prefixes. The parser adds schemes at the boundary, the renderer strips them.

# Architecture
Mental model (load first): [docs/Architecture/Model.md](docs/Architecture/Model.md) — five layers, mode axes, invariants.
See [docs/Architecture.md](docs/Architecture.md) (details in docs/Architecture/)

# Project tracking (10,000 & 50,000-foot view)
The **birds-eye view** — strategy, roadmap, gates, and the parking lot of
deferred / cross-session open topics — lives in the Holon PKM vault, NOT in this
repo:
`/Users/martin/Workspaces/pkm/holon-pkm/Projects/Holon/` (org files, one per
topic; `README.org` indexes them; `Now.org` is the G1 critical path).
This repo's `docs/` holds the **ground-level detail** (ADRs, architecture,
plans); the vault holds the altitude view and points back to those docs.
When you defer a strategy-level topic, record it in the vault as a topic-doc
headline with a slug `:ID:` (see `Display Placement & Resurfacing.org` for the
pattern) — not only in-repo. Lane state and follow-up tasks go to
`holon-space/holon-work` issues (see "Task tracking").
Note (measured 2026-08-11, ratified by Martin): underscored identifiers
round-trip byte-stable — the old "mangles underscored identifiers" claim is
refuted. The REAL round-trip hazards: `_`-prefixed property KEYS are silently
erased from disk on write-back (crates/holon-org-format/src/models.rs:886,908).
A value org cannot hold raw (a line break, surrounding space, an empty value)
is written as a JSON string literal and reads back byte-equal
(docs/Reference/ORG_SYNTAX.md). Authored drawer order
SURVIVES the store on BOTH production write legs — the Loro projection writer
and the org-ingest param builder: the `_drawer_order` carrier persists in the
stored properties bag and the renderer replays it. Pinned by
crates/holon-app/tests/org_store_org_round_trip.rs. See
docs/Reference/CompassConventions.md.

# Task tracking (lane state in GitHub issues)
Work-in-progress and lane context live in GitHub issues of the PRIVATE repo `holon-space/holon-work`. Vision and strategy stay in the vault. A fresh agent can continue any lane from its issue.
- **One issue per lane.** The orchestrator state is the state comment of issue #26.
- **State comment.** Each issue has ONE state comment, edited in place. It starts with the marker `<!-- lane-state -->`. Content: done / in progress (file:line) / next / evidence. Agents update it at each milestone.
  - Write: `~/.claude/skills/orchestrator/scripts/lane-state.sh write <issue> <markdown-file>`
  - Read: `~/.claude/skills/orchestrator/scripts/lane-state.sh read <issue>`
- **Decisions.** Ask in the decision inbox. When Martin rules, post a comment on the issue with the ruling, the direction, and possibly a (partial) plan.
- **Draft PR per lane.** Open a PUBLIC draft PR on `holon-space/holon` with base `integration` when the lane bookmark is first pushed. The PR body links the private issue by number only. No private details in the PR: no real-DB measurements, no vault content, no unfixed security defects. These stay in the private issue.
- **Landing.** Merge the PR as in the VCS section above. On landing, close the issue with a comment that names the main commit.
- **Writing style.** Load the `holon-handoff` skill before writing an issue, PR body or state comment and follow its "GitHub issues and PRs" section: actionable imperative tasks as `- [ ]` / `- [x]`, parent state derived from children, one fact per line with details in nested lists, never long lines that cram several facts.
- **Speaking names.** Every issue and PR is understandable from GitHub alone. Use speaking names and `#N` links, never aliases such as `M5`, `Inc 0` or `G1`. A milestone is an epic: a parent issue with sub-issues. Local paths, vault pages and lane logs may support a statement but never carry its meaning.
- **Secrets.** Never put secrets, tokens, passwords or env dumps into an issue or PR.

# Development
See [DEVELOPMENT.md](DEVELOPMENT.md) — testing (nextest, coverage) and log analysis scripts.

# Debugging
Prefer using the `debugger-mcp` skill over adding debug logging.
Compile code with the `debugger` cargo profile so you can inspect variables.

<!-- ast-outline:begin v=1.0.1 -->
## Prefer `ast-outline` over full reads

Usage: ast-outline <COMMAND> [OPTIONS]

Commands:
  outline       Outline given files or directories (signatures with line ranges)
  digest        One-page module map
  show          Extract source of a symbol
  implements    Find subclasses / implementations
  surface       True public API surface (resolves `pub use` / `__all__`)
  deps          Forward import-graph traversal: what a file imports
  reverse-deps  Backward import-graph: who imports a file
  cycles        Find import cycles via Tarjan SCC
  graph         Emit the dep graph (text / JSON / DOT / DSM)
  search        Hybrid BM25 + dense semantic search over the repo
  find-related  Find chunks semantically similar to a given file:line
  index         Build, refresh, or inspect the per-repo search index
  prompt        Print this agent prompt snippet
  install       Install ast-outline into a coding-agent CLI
  uninstall     Remove ast-outline from a coding-agent CLI
  status        Report what's installed where
  mcp           Run as an MCP (Model Context Protocol) server over stdio

Each command has `--json` for stable schemas and `--compact` for single-line JSON. Pass an unknown flag or no command and the help text prints automatically — there's no "default" command, every operation is explicit.

Read structure with `ast-outline` before opening full contents. Pull method bodies only once you know which ones you need.

Stop at the step that answers the question:

1. **Unfamiliar directory** — `ast-outline digest <dir>`: one-page map of every file's types and public methods.

2. **One file's shape** — `ast-outline outline <file>`: signatures with line ranges, no bodies (5–10× smaller than a full read).

3. **One method, class, or markdown section** — `ast-outline show <file> <Symbol>`. Suffix matching: `TakeDamage`, or `Player.TakeDamage` when ambiguous. Multiple at once: `ast-outline show Player TakeDamage Heal Die`. For markdown, the symbol is the heading text.

4. **Who implements/extends a type** — `ast-outline implements <Type> <dir>`: AST-accurate (skip `grep`), transitive by default with `[via Parent]` tags on indirect matches. Add `--direct` for level-1 only.

5. **You don't know the file or symbol name** — `ast-outline search "<query>"`: hybrid BM25 + dense semantic search over the repo. Use bare identifiers for symbol lookup (`HandlerStack`, `Sinatra::Base` — auto-leans BM25), full sentences for behaviour search ("how does login work" — auto-balances semantic + BM25). First call builds an index at `.ast-outline/index/` (~seconds for typical repos); subsequent calls reuse it and refresh incrementally.

6. **Find code similar to a chunk you already have** — `ast-outline find-related <file>:<line>`: returns chunks semantically similar to the one containing that line. Useful for "what else looks like this?" or finding alternative implementations. Pastes directly from `search` output (which prints results as `path:start-end`).

7. **The actual published API of a package** — `ast-outline surface <dir>`: resolves `pub use` re-exports (Rust) and `__all__` (Python) so you see exactly what a downstream user can reach, not the union of every `pub`/non-underscore item. Falls back to visibility-filtered output for Java/C#/Go/Kotlin (no real re-export concept). Use `--tree` for hierarchy, `--include-chain` to see the re-export path each entry took.

8. **What does this file pull in / who depends on it / are there cycles?** — file-level dep-graph commands. First call builds a graph at `.ast-outline/deps/graph.bin` (~hundreds of ms for typical repos); subsequent calls reuse it.
   - `ast-outline deps <file> [--depth N]`: forward — what `<file>` imports (transitively).
   - `ast-outline reverse-deps <file> [--depth N]`: backward — who imports `<file>`. Use before refactoring to know the blast radius.
   - `ast-outline cycles [<dir>]`: import cycles via Tarjan SCC. Exits non-zero when cycles exist (CI gate).
   - `ast-outline graph [<dir>] --format text|json|dot|dsm`: emit the full graph. `dsm` is a Design Structure Matrix sorted by Lakos level — visual cycle/inversion spotter.

Fall back to a full read only when you need context beyond the body `show` returned. If the outline header contains `# WARNING: N parse errors`, the outline for that file is partial — read the source directly for the affected region.
<!-- ast-outline:end -->
