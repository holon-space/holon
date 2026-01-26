# RULES
- Whenever there's a bug in the UI, always check if the E2E test in crates/holon-integration-tests/tests/general_e2e_pbt.rs can reproduce it.
- If the E2E test doesn't reproduce the issue think about how prod and E2E test can be made more similar, so that the E2E test can reproduce it.
- Don't add new PBT tests, only use the existing PBT in general_e2e_pbt.rs
- **NEVER** swallow errors!! Use `Result` and enrich the error message with information.
- **ALWAYS** `tee` before filtering output.
  Advantages:
  1. You can get all information you need by filtering the `tee`ed output without having to run again.
  2. In non-deterministic tests like PBTs you don't need to cross-reference things that e.g. got different UUIDs.
- **ALWAYS** try `debugger-mcp` before falling back to adding `eprintln`/...
  Advantages:
  1. You get both more information out of one debug session.
  2. You spend less wall-time because you don't have to re-compile and re-start when you need the value for another variable.
  3. You don't have to remove any `eprintln`/... after the session.
- Even if you add debug output, do this only for things that you can't get via debugger and still run the app/test via debugger so you can react to the output immediately and inspect additional variables, set additional breakpoints, etc.

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

# `holon` MCP

Every frontend automatically launches an MCP server which is available to you as `holon`.
You can live-inspect the DB, inspect what the UI should render, etc.
Use it whenever you have a running application and you want to look under the hood to investigate.

# Org File Conventions
See [docs/ORG_SYNTAX.md](docs/ORG_SYNTAX.md) — org files store **bare IDs** without `block:`/`doc:` scheme prefixes. The parser adds schemes at the boundary, the renderer strips them.

# Architecture
See docs/Architecture.md

# Development
See [DEVELOPMENT.md](DEVELOPMENT.md) — testing (nextest, coverage) and log analysis scripts.

# Wiki
See [wiki/index.md](wiki/index.md) — living codebase documentation (Karpathy-wiki pattern). Covers all crates, frontends, and architectural concepts with source file references. Start with [wiki/overview.md](wiki/overview.md) for the big picture.
