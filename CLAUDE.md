# RULES
- Whenever there's a bug in Flutter, always check if the E2E test in crates/holon-integration-tests/tests/general_e2e_pbt.rs can reproduce it.
- If the E2E test doesn't reproduce the issue think about how prod and E2E test can be made more similar, so that the E2E test can reproduce it.
- Don't add new PBTs for Flutter bugs, only use the existing PBT in general_e2e_pbt.rs
- **NEVER** swallow errors!! Use `Result` and enrich the error message with information.

## Parse, Don't Validate (Type-Driven Design)
Based on: https://www.harudagondi.space/blog/parse-dont-validate-and-type-driven-design-in-rust

**Core principle**: Make illegal states unrepresentable. When data enters the system (from org files, Loro, Turso, MCP), parse it into types that encode invariants — don't pass raw strings around and re-validate them at every call site.

**Concretely**:
- Prefer newtypes and enums over `String` for domain concepts with a fixed set of valid values (e.g., `ContentType`, `TaskState`, `QueryLanguage`, `ParentRef`)
- Parse at the boundary: convert raw data into typed representations at the point of entry (org parser, SQL row deserialization, Loro extraction), not at every usage point
- A function that takes `NonZeroF32` instead of `f32` and checking `b != 0` moves the proof obligation to the caller and eliminates an entire class of bugs
- Be suspicious of `match str.as_str() { ... }` scattered across multiple files — it's a sign that a string should be an enum
- Be suspicious of `.ok()` or `_ => default` on parse results — this silently swallows invalid data. Fail loudly at the boundary instead.

**Known violations to fix** (see PARSE_DONT_VALIDATE_AUDIT.md for details):
1. `parent_id: String` — stringly-typed sum type checked via `is_document_uri()` in 29+ files
2. `content_type: String` — always "text" or "source", should be an enum
3. `task_state: Option<String>` — done-ness checked in 4 different places with inconsistent keyword lists (latent bug: `holon-core` misses "CANCELLED"/"CLOSED")
4. `source_language: Option<String>` — query language dispatch duplicated in 3+ places
5. Petri net prototype values: `BTreeMap<String, String>` mixes literal f64s and Rhai expressions, distinguished only by `=` prefix
6. `deadline`/`scheduled`: raw strings never parsed into date types
7. `tags`: comma-separated string repeatedly split/joined

# `holon-live` MCP

Every frontend automatically launches an MCP server which is available to you as `holon-live`.
You can live-inspect the DB, inspect what the UI should render, etc.
Use it whenever you have a running application and you want to look under the hood to investigate.

# Peekaboo (Flutter UI Interaction)

The Flutter app runs as `space.holon`.

**Screenshots** — use `image` to capture, then `Read` the PNG:
```
mcp__peekaboo__image(app_target: "space.holon", path: "/tmp/holon-screenshot.png")
```
This captures multiple windows. The main UI window is the one named "Holon" (e.g. `/tmp/holon-screenshot-holon-Holon-4.png`).

**UI element discovery** — `see` works on the Flutter app and returns element IDs:
```
mcp__peekaboo__see(app_target: "space.holon", path: "/tmp/holon-see.png")
```
Returns elements like `elem_7` ("Sync"), `elem_8` ("Full Sync"), etc.

**Clicking UI elements** — use `click` with element IDs from `see`:
```
mcp__peekaboo__click(on: "elem_8")  // clicks "Full Sync" button
```
Must run `see` first to create a snapshot before `click` will work.

To find window details: `mcp__peekaboo__list(item_type: "application_windows", app: "holon", include_window_details: ["ids", "bounds"])`

# Org File Conventions
See [docs/ORG_SYNTAX.md](docs/ORG_SYNTAX.md) — org files store **bare IDs** without `block:`/`doc:` scheme prefixes. The parser adds schemes at the boundary, the renderer strips them.

# Architecture
See ARCHITECTURE.md
