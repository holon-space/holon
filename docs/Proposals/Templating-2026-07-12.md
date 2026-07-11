# Templating — templates as block subtrees, instantiation as an operation

**Status:** Proposed 2026-07-12, v1 implemented alongside (see §9).
**Relates to:** ADR 0024 (unified action execution — deterministic effect IDs,
provenance, rules-as-blocks), ADR 0015 (canonical vs display placement),
`crates/holon-petri` prototype machinery, links-as-marks ruling
(content-with-marks truth + `block_links` junction), C2a provenance stamping,
C2b history relation, C6 clock relation, C7 `ParsedTask` boundary parser.

## 1. First principles

- **Everything is data on the block substrate.** A template is a block
  subtree — no new file format, no hardcoded registry, ordinary org
  round-trip. Whatever a template can contain is exactly what a block can
  contain (content, marks, properties, children, task syntax).
- **Instantiation is an operation at the intent boundary** (Model.md
  invariant 3/4): one `instantiate_template` operation, callable manually
  (UI/MCP via `available_operations`) and by rules (an ADR 0024 effect).
  Both paths execute the *same* code.
- **Idempotent under rule re-fire** (ADR 0024 P4): every created block's id is
  a deterministic name-based UUID of `(template id, context key, source node)`.
  Two replicas firing the same rule for the same binding — or one replica
  re-firing on boot re-registration / day rollover — produce the *same* block
  ids; the merge/upsert collapses them. At-most-once is a naming discipline.
- **Fail loud, never fake:** a `{{var}}` without a binding and without a
  declared default is an error that aborts the whole instantiation — never a
  silent empty substitution. Unknown binding keys (typos) are equally errors.

## 2. Template representation

A template is an ordinary block subtree whose **root** carries two properties:

| Property | Meaning |
|---|---|
| `template` | Marker + human-readable template name (e.g. `daily-journal`). Presence makes the subtree a template. |
| `template_vars` | Declared variables: comma-separated `name` or `name=default` entries, e.g. `date, mood=neutral`. Parsed at the boundary into a typed `TemplateVars` (parse-don't-validate; duplicate names, empty names, and malformed entries are parse errors). |

Variable slots appear in **content** and in **property values** of the root
and every descendant as `{{name}}`.

In an org file this is just a normal drawer — no parser special-casing:

```org
* {{date}}
:PROPERTIES:
:ID: journal-day-template
:TEMPLATE: daily-journal
:TEMPLATE_VARS: date, mood=neutral
:END:
** Agenda for [[block:journals][{{date}}]]
** Mood: {{mood}}
** TODO review inbox
```

**Why `{{var}}` and not `{var}`:** ADR 0024 already assigns single-brace
`{today}` / `{clock.today}` to *rule-environment interpolation* inside guard
and effect strings — those are resolved by the rule compiler/evaluator at
firing time, before the operation ever runs. Template substitution is a
second, later phase resolved from the operation's `bindings` param. Two
phases, two syntaxes: a rule effect can say `bindings: {date: "{day.today}"}`
— the rule layer interpolates `{day.today}` into a concrete value, the
operation substitutes `{{date}}` inside the template. Double-brace is also
the established convention (Mustache/Jinja/LogSeq), and it is statistically
absent from prose while single braces are not. No escape syntax in v1
(disclosed; a literal `{{` in template content is not representable —
revisit if it ever bites).

**WYSIWYG root:** the instance root *is* a copy of the template root
(markers stripped) — the template looks exactly like an instance with slots,
which is also what makes the org representation self-explanatory.

## 3. Prototypes vs templates: LAYER, do not unify (verdict)

`crates/holon-petri`'s prototype machinery (`PrototypeValue`,
`prototype_for`, `resolve_prototype`) is **read-time per-property value
inheritance**: an instance block exists independently, and at *evaluation*
time its numeric/`=`-computed properties are resolved by merging
prototype → instance → context and evaluating Rhai expressions. Nothing is
ever copied; changing the prototype changes every instance's resolved values
retroactively.

Templating is **write-time subtree instantiation**: new blocks are minted
once, then live their own lives; changing the template does not touch
existing instances.

These are complementary, not the same mechanism at different sizes:

- Unifying (template instantiation = creating a thin block that *inherits*
  content and children live from the template) would require live
  inheritance semantics for content, marks, and child subtrees across the
  CRDT and the org round-trip — a substrate-level feature with heavy
  invariant surface (what does editing an inherited child mean? what does
  the org file show?). Rejected for v1, and probably forever for *content*.
- Layering is free: an instantiated block can carry `prototype_for`-style
  per-property inheritance afterwards; and the instance root's
  `instance_of` provenance property (§4) is exactly the edge a later
  increment could use to add *live property* inheritance from template
  root to instances — which would then literally be the prototype
  mechanism generalized beyond f64. That is the honest relationship:
  prototypes are per-property templates *for values, resolved late*;
  templates are subtree stamps *for structure, resolved early*.

## 4. Instantiation semantics

`instantiate_template { template_id, target_parent, context_key, bindings }`:

1. **Load** the template subtree (root + all descendants, parent-before-child,
   siblings in `sort_key` order) from the SQL projection (total by
   invariant). Missing template, or `template_id` lacking the `template`
   marker property → loud error.
2. **Parse** `template_vars` into declarations; merge `bindings` over
   declared defaults. Unknown binding key → error. Any `{{name}}` occurring
   in the subtree that is not a declared variable → error (template
   inconsistency surfaces at instantiation, the earliest point it can).
   A declared variable with neither binding nor default that is referenced
   anywhere → error listing all missing names at once.
3. **Substitute** `{{name}}` in every node's content and every string
   property value. Mark spans are offset-mapped across substitutions
   (Unicode-scalar arithmetic; a span strictly containing a slot stretches
   over the substituted text, spans after a slot shift). Marks therefore
   survive as *real marks*, and the create path's existing
   `block_link_statements` derivation turns `[[ref]]` marks inside templates
   into real `block_links` rows — links-as-marks holds with zero extra code.
4. **Mint ids** deterministically:
   `UUIDv5(HOLON_TEMPLATE_NS, template_id ‖ context_key ‖ source_node_id)`
   per node (`block:<uuid>`). The `context_key` plays the role of ADR 0024's
   firing key: a rule passes its binding (e.g. the day), so re-fires
   converge; a manual invocation passes a fresh key (UI/MCP mints a UUID),
   so each manual instantiation is a new instance.
5. **Create** one block per node through the ordinary operation path
   (`create` per node, parent before child, template `sort_key`s copied for
   children so sibling order survives; the root's `sort_key` is left to the
   provider like any rule-created block). Each create flows through
   `DispatchingOperationEngine::execute_operation`, so C2a `_provenance`
   stamping, the C2b history relation, and undo classification all apply
   unchanged. The instance root gets `instance_of: <template_id>`
   (provenance edge); the `template` / `template_vars` markers are stripped;
   all other fields (`content_type`, `source_language`, `collapsed`,
   `completed`, `block_type`, properties) copy through.
6. **Return** the instance root id.

**Deep copy, per property-kind note:** v1 copies property values verbatim
(post-substitution). `=`-computed prototype expressions are strings and copy
as strings — an instantiated petri task with `=`-props behaves exactly like a
hand-written one. Task syntax in content flows through the C7 `ParsedTask`
boundary parser downstream like any authored content; substitution happens
strictly before any boundary parsing.

**Undo:** the child creates are individually classified by their provider
(create → delete inverse). A User-origin instantiation therefore pushes one
undo entry per created block; entries pop leaf-first (LIFO), which is
FK-safe. A single compound entry is deferred to the undo-grouping track
(same gap class as split/join grouping — provider coverage, not plumbing).

**Nested templates** copy verbatim (the copy is itself a template — the
`template` marker is only stripped from the *instantiated root*). Recursion
is impossible: the subtree walk is over a tree snapshot.

## 5. The manual action

`instantiate_template` is intercepted at the engine level
(`DispatchingOperationEngine`), not per-provider: it *expands into* `create`
ops that route through whatever provider owns `block` creation in the
session's wiring — one implementation for Turso, Loro+Turso, and future
wirings. It is advertised via a synthetic `OperationDescriptor` in
`available_operations("block")`, so MCP (and later UI affordances) discover
it like any other operation.

Reading the template needs a read capability the bare dispatcher does not
have, so the engine carries an optional `TemplateSource` (Turso-backed in
`BackendEngine`; a session without one fails loud:
"instantiate_template requires a template source — not wired in this
session"). Falls back visibly, never silently.

## 6. The rule form (ADR 0024, integration point)

In the ratified YAML rule grammar (being implemented by the yaml-rule
stream), rule-driven instantiation is an output arc whose effect is the
operation — the journal-from-template example end-to-end:

```yaml
#+begin_src holon_rule
name: daily-journal-from-template
input:
- bind: day
  type: clock              # read arc on the clock relation (C6)
  consume: false
- absent: true             # inhibitor arc: no journal for today yet
  type: block
  when: parent_is("journals") and name == day.today
output:
- effect: block.instantiate_template
  template_id: "block:journal-day-template"
  target_parent: "block:journals"
  context_key: "{day.today}"          # firing key → idempotent re-fire
  bindings:
    date: "{day.today}"
#+end_src
```

Single-brace `{day.today}` is rule-environment interpolation (resolved by
the rule evaluator per ADR 0024 "Guard surface vs compilation");
double-brace slots live only inside the template. **Integration point for
the yaml-rule stream:** compile an `effect:` output arc to
`execute_operation("block", "instantiate_template", params,
OpOrigin::Rule { transition_id })` — nothing else; the operation owns
deterministic ids and fail-loud binding checks.

Until the YAML grammar lands, the *existing* watcher machinery can already
drive it — the current Rhai action DSL dispatches arbitrary operations:

```rhai
block.instantiate_template(#{
  template_id: "block:journal-day-template",
  target_parent: "block:journals",
  context_key: col("name"),
  bindings: #{ date: col("name") }
})
```

paired with the clock trigger (`SELECT today AS name FROM clock WHERE
grain = 'day'`). This path is proven by an integration test (§9); the
shipped `assets/default/Journals.org` rule is deliberately **not** switched
over here (the yaml-rule stream owns that file's migration).

## 7. Org round-trip

Nothing to special-case: templates are blocks with two extra properties and
`{{...}}` in content. Properties round-trip through the existing drawer
path; `{{date}}` contains no org markup. One watch-out inherited from the
known `_`-subscript mangling class: variable names with underscores in
*content* would hit the (already fixed) subscript rule — covered by using
the existing content pipeline, no template-specific handling.

## 8. What v1 does NOT do (disclosed)

- No escape syntax for literal `{{` in template content.
- No live inheritance from template to instances (see §3 — deliberate).
- No compound undo entry (one entry per created block for User origin).
- No `after`/position hint for the instance root among its new siblings
  (provider default, same as journal-rule creates today).
- Loro-only (no-Turso) sessions have no `TemplateSource` wired and fail
  loud; wiring a Loro subtree reader is a follow-up.
- Substitution only in content and string property values — not in
  `source_name`, tags, or edge fields.

## 9. v1 implementation map

- `crates/holon-api/src/effect_id.rs` — `HOLON_TEMPLATE_NAMESPACE` +
  `deterministic_instance_id(template_id, context_key, source_node_id)`.
- `crates/holon/src/core/template_instantiation.rs` — pure planning core:
  `TemplateVars::parse`, `InstantiateRequest::from_params` (typed boundary),
  `TemplateNode`, `plan_instantiation(...) -> Result<InstantiationPlan>`
  (ordered create-param maps; all fail-loud rules from §4), mark-span offset
  mapping. Unit-tested without any backend.
- `crates/holon/src/api/operation_engine.rs` — `TemplateSource` trait,
  interception in `execute_operation`, synthetic descriptor in
  `available_operations` / `has_operation`.
- `crates/holon/src/api/template_source.rs` — `TemplateSource` trait +
  `TursoTemplateSource` (BFS over `block_raw`). Note: `DbHandle::query`
  deserializes JSON TEXT columns (`properties`, `marks`) into structured
  `Value::Object`/`Value::Array`, so the row reader re-serializes them to the
  JSON string the planner re-parses (`json_column_to_string`).
- `crates/holon/src/api/backend_engine.rs` — `TursoTemplateSource` wired at
  both engine constructions.
- Tests: planning unit tests (bindings, defaults, missing/unknown bindings
  fail loud, deterministic ids, mark offsets); engine integration tests
  (instantiate twice same `context_key` → converged, different key → second
  instance; marks land in the created rows); rule-driven integration test
  via the existing watcher machinery (clock row → rule fires →
  instance exists; re-fire → still exactly one).
