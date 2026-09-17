---
id: 2026-09-17-render-org-doc-id-panic-kills-the-mcp-server
date: 2026-09-17
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  `render_org` minted the document URI with `EntityUri::from_raw` on an
  agent-supplied `doc_id`, which PANICS when the string forms no URI — and a
  vault path with a space does not — so the call was never answered at all and
  the client saw only a bare transport error.
---

## Bug

Reported by the vault-spine-dedupe lane (queue 2026-09-17, item 53a) against
the live vault: `render_org` "cannot resolve vault docs by UUID or path
(relative/encoded rejected, absolute transport-errors) — read_org_file works".
The lane's own record
(`/Users/martin/.claude/handoffs/holon-2026-09-15/vault-task-audit-2026-09-17.md:309-315`)
gives the inputs: `Projects/Holon/Plain-Text Layer.org`, its URL-encoded
spelling and the bare document UUID were rejected with `Cannot resolve … to a
file path`; the absolute path returned `Transport error: Receive failed: no
pending response` on 3 attempts.

Lane: `fix-mcp-tool-defects` (executor), base `bd8b719a103b`.

## Root cause

Two mechanisms under one report, both pinned by tests.

**1. The panic (the transport error).** `render_org` resolves the file path
through `resolve_to_file_path` and then derived the document URI separately
with `EntityUri::from_raw(&params.doc_id)` (`frontends/mcp/src/tools.rs`,
in the `render_org` body). `from_raw` delegates to `EntityUri::new`, which
panics on an unparseable URI
(`crates/holon-api/src/entity_uri.rs:134`). Every vault path carries a space,
so the minted `block:<path>` is invalid and the call panics:

```
EntityUri::new("block", "…/Plain-Text Layer.org") produced invalid URI: unexpected character at index 76
```

The blast radius depends on the build profile, and both cases present to the
caller as the same symptom: the panic escapes the handler, so **the response
for that request is never written**, and the client reports
`Receive failed: no pending response`.

- **Debug (the dev frontend, `target/debug/holon-mcp`).** rmcp spawns each
  request handler into its own task
  (`crates/rmcp/src/service.rs` `tokio::spawn(async move { service.handle_request(...) })`),
  and neither rmcp nor holon contains that panic, so it unwinds THAT task only:
  the id is never answered while the serve loop and the process survive.
  Measured: an unanswered call, then two later calls answered normally by the
  same live process (`lane-logs/verify-10-process-death.log`).
- **Release.** `Cargo.toml` sets `panic = "abort"` under `[profile.release]`,
  so the same panic ends the process and every later call in the session fails
  the same way.

No `catch_unwind` exists in `frontends/mcp/src/server.rs` or `main.rs`, so
nothing on holon's side contains a handler panic in either profile. The
earlier revision of this entry claimed the fatal-to-the-session case
unconditionally; a verifier refuted that for the debug build.

Red log: `lane-logs/04-red-A.log` (3 failures, the panic above).
Green log: `lane-logs/11-nextest-full.log`.

**2. The bare UUID (the `Cannot resolve`).** `resolve_to_file_path` asked the
alias registry for the RAW argument, but ingest registers the alias key as
the SCHEMED document URI (`EntityUri::block(bare).to_string()`, i.e.
`block:<uuid>`; `crates/holon-filesystem/src/file_sync_controller.rs:3549`).
A bare uuid therefore matched nothing and the tool refused an id that IS
registered:

```
Cannot resolve 'c0450284-7413-44e6-a5dd-4680d09ad9f8' to a file path.
```

Red log: `lane-logs/05-red-A2-C.log`.

**3. One more route, correctly refused.** For a path that DOES form a URI
(`Projects/Holon/Plain.org`, no space) the old code silently minted
`block:<path>` — a document keyed nowhere — and rendered an empty body rather
than reporting that nothing was found. Fixed by deriving the id the way ingest
does: the file's own declared id (`FileFormatAdapter::doc_id_from_content`,
reached through the new `WritebackRenderer::declared_doc_id`).

## Missing piece

**COVERAGE.** `crates/holon-integration-tests/src/pbt/composed/live_mcp.rs`
drives real MCP tools over a real vault, but no transition hands a tool a
FILE PATH or a malformed id: every generated `doc_id` is a well-formed block
id. A path argument is the one input shape the alphabet never produces, and
the panic lives exactly there.

**Secondary ENVIRONMENT.** The keystone runs the tool in-process, where a
panic unwinds one future. Production serves the same handler inline in the
stdio loop, where the same panic ends the process. So the harness can
generate the input and still never see the symptom that matters.

## Remedy

FIXED, `frontends/mcp/src/tools.rs`:

- `doc_uri_from_arg` is fallible (`EntityUri::try_from_raw`) and returns
  `invalid_params` naming the argument. `render_org` derives the document URI
  through a new `resolve_document_uri`, which uses the file's own declared id
  for path arguments and fails loud when a file declares none.
- `resolve_to_file_path` retries the alias lookup with the normalized
  `block:<uuid>` spelling.
- `ensure_block_prefix` — the same panic on the agent-facing task tools
  (`claim_task`, `complete_task`, `add_subtask`, `execute_source_block`, and
  the `requires` target parser) — is fallible too.
- `describe_ui_expand`'s `context_id` — a `context:` argument authored in a
  vault render expression, so it reaches the resolver as untrusted text — goes
  through a fallible conversion instead of `EntityUri::from_raw`.

Pinned by `mod render_org_doc_id_tests` (temp-vault behavioural tests plus
pure ones) and `mod query_survival_tests` in `frontends/mcp/src/tools.rs`, and
by `mod context_id_tests` in `frontends/mcp/src/describe_ui_expand.rs`, whose
`expand_live_query_refuses_a_context_id_that_forms_no_uri` drives the real
resolver over a real engine. Red logs: `lane-logs/04-red-A.log`,
`lane-logs/05-red-A2-C.log`, `lane-logs/22-red-round2.log`,
`lane-logs/24-red-round2-site.log`.

Not closed: a panic in any OTHER handler still costs that request its reply
(and, under `panic = "abort"`, the process). A `catch_unwind` at the serve
boundary would turn every remaining one into a loud error; that is a harness
change beyond this fix. One same-class twin is left outside this lane's crates:
`crates/holon-frontend/src/render_interpreter.rs:740` converts the same
render-spec `context_id` with `from_raw`.
