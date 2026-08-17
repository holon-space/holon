---
id: 2026-07-16-claude-history-claude-code-integration-killed
date: 2026-07-16
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  claude-history (Claude Code) integration killed at connect by resource
  auto-discovery: `finish_integration` merges server-advertised resource
  templates into the sidecar, the auto-discovered 'plan' entity's URI template
  has an unexpandable param → `into_strategy` fails → the ENTIRE integration
  (3 declared, working entities) registers INERT; partial state left behind
  (`cc_plan` cache table exists, cc_session/task/message don't); error is
  terse ("Failed to build strategy for 'plan'") naming an entity the sidecar
  never declared
source_line: 822
---

## Bug

claude-history (Claude Code) integration killed at connect by resource
auto-discovery: `finish_integration` merges server-advertised resource
templates into the sidecar, the auto-discovered 'plan' entity's URI template
has an unexpandable param → `into_strategy` fails → the ENTIRE integration
(3 declared, working entities) registers INERT; partial state left behind
(`cc_plan` cache table exists, cc_session/task/message don't); error is
terse ("Failed to build strategy for 'plan'") naming an entity the sidecar
never declared

## Missing piece

no test connects a real MCP server whose resource templates exceed the
sidecar's declared entities; no invariant "declared entities survive an
undeclared auto-discovered one failing"

## Remedy

FIXED 2026-07-17 — two-part fix in holon-mcp-client. (1) Shape:
auto-discovery no longer attaches a `list_resource` sync to a PARAMETERIZED
template (`is_concrete_uri` in mcp_resource_discovery.rs) — a `{param}`
template has no parent key here, so the entity is registered schema-only
instead of given an unbuildable strategy. (2) Non-fatal: strategy build
moved to `build_entity_strategies` which collects per-entity failures and
reports them loudly (`error!`) instead of `?`-aborting the whole integration
— declared entities survive an undeclared one failing (disclosed
degradation). Tests: `integration_resilience_tests` in mcp_integration.rs
(parameterized template not listable; parameterized sync fails
into_strategy; one unbuildable entity does not sink the declared ones)
