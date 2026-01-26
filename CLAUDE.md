# RULES
- Whenever there's a bug in Flutter, always check if the E2E test in crates/holon-integration-tests/tests/general_e2e_pbt.rs can reproduce it.
- If the E2E test doesn't reproduce the issue think about how prod and E2E test can be made more similar, so that the E2E test can reproduce it.
- Don't add new PBTs for Flutter bugs, only use the existing PBT in general_e2e_pbt.rs
- **NEVER** swallow errors!! Use `Result` and enrich the error message with information.
- **ALWAYS** `tee` before filtering output

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
See ARCHITECTURE.md

# Development
See [DEVELOPMENT.md](DEVELOPMENT.md) — testing (nextest, coverage) and log analysis scripts.
