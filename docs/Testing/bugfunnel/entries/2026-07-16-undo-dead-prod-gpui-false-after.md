---
id: 2026-07-16-undo-dead-prod-gpui-false-after
date: 2026-07-16
gap: PERCEPTION
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Undo dead in prod GPUI: `can_undo`=false after BOTH an agent set_field AND
  real focused-editor typing (12 keystrokes landed); `cmd+z` still reports "No
  handler matched the key chord" (historically-known unbound); word-boundary
  grouping untestable while the stack collects nothing
source_line: 828
---

## Bug

Undo dead in prod GPUI: `can_undo`=false after BOTH an agent set_field AND
real focused-editor typing (12 keystrokes landed); `cmd+z` still reports "No
handler matched the key chord" (historically-known unbound); word-boundary
grouping untestable while the stack collects nothing

## Missing piece

INVESTIGATED 2026-07-17 (base 32a1513d): the premise ("gpui doesn't wire the
stack / MCP reads a different one / cmd+z unbound") does NOT hold on this
base — the wiring and the keybinding were ALREADY correct (undo v1 landed
2026-07-10 row 86 + write_seq echo-fix row 40). PROVEN by new prod-path test
`crates/holon-integration-tests/tests/undo_prod_session_wiring.rs`
(`TestEnvironment::start_app` = same DI wiring GPUI resolves): a User-origin
`set_field` (the SqlOnly editor commit) pushes an undo entry →
`session.can_undo()` TRUE; a `HolonService` over the SAME DI `BackendEngine`
singleton (exactly what the MCP `service()` builds) ALSO reads TRUE (editor
+ MCP share ONE stack — refutes "different stack"); `undo()` reverts. The
row's THREE observations are each explained WITHOUT a broken stack: (1)
"agent set_field → can_undo false" is BY DESIGN — MCP `service()` stamps
`OpOrigin::Agent`, and undo Ruling #1 only journals `OpOrigin::User` (agents
revert via the supervision surface, ADR 0024 C2a); asserted in-test. (2)
"cmd+z → No handler matched the key chord" is NOT the GPUI keymap — cmd-z IS
bound (`frontends/gpui/src/lib.rs:1304` `KeyBinding::new("cmd-z",
TriggerUndo, None)` + ctrl-z/ctrl-y, context:None → capture_action /
app-level on_action → `share_ui::dispatch_undo_redo` → `session.undo()`;
existing test `frontends/gpui/tests/undo_redo_keybinding.rs` proves the
chord resolves). That exact string is emitted by the MCP `send_key_chord`
tool's None arm (`frontends/mcp/src/tools.rs` ~L2571) when the
`holon_frontend::input::InputRouter` — a DIFFERENT dispatch layer from the
GPUI keymap — has no binding for the chord; global GPUI actions are
unreachable via `send_key_chord`. Agents must use the dedicated `undo` MCP
tool (or `type_text`, which routes through the GPUI keymap). (3) "typing →
can_undo false" is not reproduced by the SqlOnly editor commit (which
dispatches `set_field(content)` per `InputEvent::Change` → User origin →
push). Residual real candidates, both mode/context-specific (NOT a broken
stack): (a) **Loro mode** (crdt enabled; NON-default — `crdt_enabled()` =
false) — `editor_view.rs` `InputEvent::Change` commits typed content via
`vm.apply_local` (Loro CRDT), which bypasses the OperationEngine undo push
entirely, so typed content in Loro mode never enters the undo stack; (b)
**creation-slot typing** — `editor_view.rs` deliberately does NOT dispatch
`set_field` for a `block:__virtual:<parent>` placeholder (content commits
via `create` on Enter), so keystrokes before Enter push nothing. REMEDY:
env-gap regression test added (drives the prod session path + the MCP read
path). ESCALATED forks (need a ruling before code): Loro-mode undo
(conflicts with "one shared stack" vs Loro's native undo); whether
`send_key_chord` should reach global GPUI actions; creation-slot pre-Enter
undo semantics

## Remedy

ROOT-CAUSED — wiring+keybinding correct on base; test-only closure landed;
three forks escalated
