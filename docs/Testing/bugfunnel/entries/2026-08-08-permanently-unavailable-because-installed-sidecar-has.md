---
id: 2026-08-08-permanently-unavailable-because-installed-sidecar-has
date: 2026-08-08
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  `claude-history` is permanently unavailable because the INSTALLED sidecar
  has drifted from the repo's, and the error never says so.
source_line: 1189
---

## Bug

(Martin dogfooding his live instance; root-caused by A/B against a throwaway
vault) **`claude-history` is permanently unavailable because the INSTALLED
sidecar has drifted from the repo's, and the error never says so.** Not a
missing binary and not a failed handshake — the child process started and
five entities were auto-discovered immediately before the rejection `entity
'session' render 'default' queries foreign table 'cc_message_fdw', whose
entity declares no vtable.fetch_contract`. The loader reads
`{config_dir}/integrations/*.yaml`
(`wiring.rs::resolve_mcp_integrations_dir`), never the repo path, and
nothing syncs them: the installed copy is Jul 28, 10433 bytes,
`fetch_contract` count **0**; `docs/integrations/claude-history.yaml` is
26250 bytes, count **5**. A/B PROOF: copying the REPO sidecar into a sandbox
config dir made the same binary connect cleanly. A schema requirement landed
repo-side after Jul 28 and silently broke every installed instance — no
version check, no migration, and an error that blames the file's content
without saying it is stale or where the current one lives. Permanent for the
session: inert provider registered (`mcp_integrations.rs:344-356`), and the
only all-clear is "the provider connecting" (`degraded_signal_bus.rs:108`).
Aggravator, not cause, for the link row: with the provider inert every
`cc-session:` link targets an entity with no row.

## Root cause

Martin dogfooding his live instance; root-caused by an A/B against a
throwaway vault — **`claude-history` is permanently unavailable because the
INSTALLED sidecar has drifted from the repo's, and the error never says
so**. The red toast Martin read as an ERROR is a toast label
(`frontends/gpui/src/share_ui.rs:1641-1645`, detail built at `:318-325`);
the log carries two WARNs. The cause is NOT a missing binary and NOT a
failed handshake — the child process started and five entities were
auto-discovered immediately before the rejection: `Failed to connect
provider 'claude-history': entity 'session' render 'default' queries foreign
table 'cc_message_fdw', whose entity declares no vtable.fetch_contract … Set
fetch_contract to snapshot, scoped_snapshot, or delta.` PROVEN BY A/B: the
loader reads `{config_dir}/integrations/*.yaml`
(`crates/holon-app/src/wiring.rs::resolve_mcp_integrations_dir`), NOT the
repo path, and nothing syncs the two — the installed copy is dated Jul 28,
10433 bytes, with `fetch_contract` occurrences **0**, while
`docs/integrations/claude-history.yaml` is 26250 bytes with **5** (counts
via `grep -c`; contents not read out). Copying the REPO sidecar into a
sandbox config dir made the same binary CONNECT CLEANLY (`Service
initialized as client`, entities auto-discovered, `Subscribing to
'claude-history://projects'`). So a schema requirement landed repo-side
after Jul 28 and silently broke every already-installed instance, with no
version check, no migration, and an error that blames the file's content
without saying it is stale or where the current one lives. Permanent for the
session: an inert provider is registered
(`crates/holon-app/src/mcp_integrations.rs:344-356`) and the condition's
only all-clear is "the provider connecting", which nothing can trigger
post-boot (`crates/holon-loro/src/degraded_signal_bus.rs:108`). AGGRAVATOR,
not cause, for the link row above: with the provider inert every
`cc-session:` link points at an entity with no row — but that row reproduces
independently (in the sandbox claude-history CONNECTED and `cc-session:`
links still blanked the panel), so they are two defects that compound.
ENVIRONMENT: no test wiring covers "installed sidecar older than the repo's
schema requirement"; the headless keystone loads no sidecar at all.
Evidence:
`docs/Testing/fixture-logs-2026-08-08/triage5-log-signatures-integration-and-share.txt`
§4a)

## Missing piece

nothing syncs `docs/integrations/*.yaml` into `{config_dir}/integrations`,
and no test loads a sidecar older than the repo's schema requirement

## Remedy

FIXED 2026-08-08 (task #18) — STRUCTURALLY, by splitting the two jobs the
installed file was doing at once. ROOT CAUSE, stated exactly: the presence
of `{config_dir}/integrations/<p>.yaml` both ENABLED a provider and SUPPLIED
its content, so every installed copy was a private fork of a schema that
keeps moving in the repo, and no code path could ever notice. Fix: the four
repo sidecars are now COMPILED INTO the binary
(`crates/holon-mcp-client/src/bundled_sidecars.rs`, `include_str!` of
`docs/integrations/*.yaml`) and carry `schema_version: 1`; an installed file
still ENABLES its provider (so bundling switched nothing on —
`gcal`/`todoist`/`jsonplaceholder` stay off without a file), but for a
provider this build ships it supplies CONTENT only when it declares this
build's `SIDECAR_SCHEMA_VERSION`. Anything else — an older/absent
`schema_version`, or a file that no longer parses — is SUPERSEDED by the
bundled copy and DISCLOSED, never silently: `load_integration_configs`
returns `LoadedIntegrations { configs, superseded }`, and
`McpIntegrationsModule` turns each `SupersededSidecar` into a WARN plus a
new sticky `ShareDegradedReason::IntegrationSidecarSuperseded` toast naming
the provider, the installed path, the bundled source path, and the
incompatibility. Martin's Jul-28 file lands in exactly that arm: it declares
no `schema_version`, so the app runs the bundled sidecar (which HAS the five
`fetch_contract`s) and `claude-history` connects, while the toast says which
file was ignored and why. A byte-identical installed copy is silent (it is
the same file, not an override); a copy declaring the current version is
honored verbatim (INFO log). HONEST SCOPE — what is NOT fixed: (a)
`schema_version` is a DECLARED generation, not a computed compatibility
proof, so an override that declares `1` and is nonetheless wrong still fails
at connect exactly as before (the guard catches stale files, not lying
ones); (b) nothing installs or updates files in `{config_dir}/integrations`
— enablement stays a manual copy, deliberately, since installing copies is
the defect; (c) the connect-time `reject_unmaintainable_fdw_queries` check
is unchanged and still runs after auto-discovery, so a genuinely
incompatible OVERRIDE is still reported by that error rather than by the
drift disclosure; (d) `~/.config/holon` on Martin's machine was NOT touched
— his stale file will produce the new toast on the next build and can then
be deleted; (e) `SIDECAR_SCHEMA_VERSION` is GLOBAL, not per-provider, so one
bump supersedes every user override of every provider at once — acceptable
only while all sidecars live in one repo directory and move together; (f)
the disclosure has two legs with different guarantees — the `warn!` is
emitted eagerly in `McpIntegrationsModule::from_dir` and always fires, but
the TOAST rides the `McpIntegrationRegistry` singleton, which fluxdi
resolves LAZILY, so a container that never touches an integration gets the
log and no UI (the eager `warn!` is the stated mitigation, not a fix).
COVERAGE for the named gap: `crates/holon-mcp-client/tests/sidecar_drift.rs`
(8 tests) boots the loader with a faithfully-shaped pre-`fetch_contract`
installed `claude-history.yaml` beside the bundled one and asserts the
supersede, the disclosure's contents, the honored-override escape hatch, the
unbundled-provider path, the not-enabled-without-a-file rule, the
byte-identical silence, and both unreadable-override arms (unparseable and
zero-byte: bundled content runs, boot survives, incompatibility says no
`schema_version` could be established). The UI leg is pinned by
`apply_degraded_routes_sidecar_superseded_to_toast`
(`frontends/gpui/src/share_ui.rs`), asserting the toast detail names
provider, installed path, incompatibility, and bundled source. RED log
preserved at
`docs/Testing/fixture-logs-2026-08-08/task18-sidecar-drift-red.txt` (base
`holon-mcp-client` sources restored under the lane via `git show HEAD:…`,
then restored): the probe panics with "the app must run the sidecar it
ships, not a copy installed before `fetch_contract` was required". Evidence
`docs/Testing/fixture-logs-2026-08-08/triage5-log-signatures-integration-and-share.txt`
§4a
