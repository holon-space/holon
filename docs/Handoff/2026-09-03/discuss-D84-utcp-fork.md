# D84 rev 2 — "use our own fork" of UTCP: what it means, pros and cons

Martin chose (c) "contribute upstream" but wants to discuss doing it on OUR
OWN FORK instead of waiting. This document lays out what a fork concretely
is, an example, the decisive tradeoff, and a fourth option.

Facts this rests on (double-verified, file
`utcp-shopping/utcp-claims-verify.md` in this scratchpad):

- The UTCP manual (spec 1.x, Python `utcp` 1.1.3) has no request-body
  template, no declared query parameters, no polling cadence, and no
  response mapping that travels WITH the manual.
- The reference client REJECTS unknown keys, deliberately (an extra top-level
  key raises a validation error; pydantic's default would ignore it). An
  unknown `call_template_type` fails validation of the WHOLE manual.
- The Rust client `rs-utcp` is 0.3.2 and tracks spec 0.3.0; the spec is at
  1.x. The two disagree on the manual's top-level schema.
- The reference client prints the resolved secret URL into exception text and
  an ERROR log line on a 401.

## 1. What "our own fork" concretely means

There are TWO things one can fork, and they are different decisions.

**(F1) Fork the spec** — a JSON manual format "UTCP + Holon fields":
`body` template, `query` map, `poll_interval`, and a `response` mapping
(a jaq expression, per D81.a) on each `http` call template.

**(F2) Fork `rs-utcp`** — take the Rust client as the base of Holon's
connector runtime and add the fields above.

Example — the shopping list, as an F1 manual (the secret stays a `${VAR}`,
exactly as in today's sidecar):

```json
{
  "utcp_version": "1.0",
  "manual_version": "1",
  "tools": [{
    "name": "commit-items",
    "inputs": {"type": "object", "properties": {"commands": {"type": "array"}}},
    "tool_call_template": {
      "call_template_type": "http",
      "http_method": "POST",
      "url": "${SHOPPING_LIST_URL}/commit",
      "x-holon": {
        "query": {"version": "{version}"},
        "body": {"oldVersion": "{version}", "device": {"id": "{deviceId}"},
                 "lang": "en", "commands": "{commands}"},
        "response": ".version as $v | {version: $v}"
      }
    }
  }]
}
```

Every field outside `x-holon` is standard UTCP. Every field inside it is what
the standard lacks. Compare with `assets/integrations/shopping.yaml`
(74 lines): the SAME information, in Holon's own YAML today.

## 2. Options

### (c′) Fork the spec, submit upstream, run on the fork meanwhile (Martin's proposal)

- Pro: the vocabulary (tools, inputs schema, call templates, auth) is the
  standard's; documentation for the 80% points at utcp.io; if upstream merges
  our fields, the fork disappears and Holon manuals are plain UTCP.
- Pro: a user who already has a UTCP manual for a service pastes it in and
  adds only the `x-holon` block.
- Con: until upstream merges, a Holon manual is NOT readable by any standard
  client (unknown keys are rejected, measured). So "one public format" is a
  hope, not a property, and the fork can stay a fork forever.
- Con: the fields we add are 100% of the runtime behaviour (envelope, query,
  cadence, mapping). The standard part is the 20% that is easy anyway.
- Con: two upstream PRs are needed first for the fork to be safe in a shared
  manual — an "ignore unknown keys" rule and a manual-carried post-processor
  — and the second is a design change the maintainers may not want.

### (c″) Fork `rs-utcp` too (F1 + F2)

- Pro: a Rust HTTP call runtime and auth handling exist there.
- Con: it is a full major version behind the spec; forking it means we first
  do the 0.3 → 1.x migration ourselves, work that is not on Holon's path.
- Con: Holon's transport already exists
  (`crates/holon-mcp-client/src/rest_transport.rs`) and already does the
  envelope, query, cadence and secret redaction the crate lacks. The fork
  would replace working code with a stale base.
- Con: the secret-leak behaviour is in the reference client family; we would
  own auditing it.
- Verdict: F2 has no upside over the current transport. Recommend NO on F2
  regardless of F1.

### (d) Embed the standard, extend beside it — recommended

Holon's sidecar file gets two top-level sections:

```yaml
utcp:            # a VERBATIM UTCP 1.x manual; importable/exportable unchanged
  utcp_version: "1.0"
  tools: [...]   # standard http call templates, ${VAR} secrets
holon:           # keyed by tool name; what the standard lacks
  commit-items:
    query: {version: "{version}"}
    body: {oldVersion: "{version}", device: {id: "{deviceId}"}, lang: en, commands: "{commands}"}
    response: ".version as $v | {version: $v}"
  poll_interval: 60s
```

- Pro: the `utcp` section round-trips to and from any standard client TODAY
  (no unknown keys inside it). A user can import a published UTCP or OpenAPI
  manual and only author the `holon` section. Export gives back a standard
  manual.
- Pro: identical to (c′) in what the user writes, minus the hope that
  standard clients accept it; identical in what we PR upstream (an
  unknown-key rule + a manual-carried mapping). If upstream merges, the
  `holon` section folds into the manual and (d) becomes (c′) mechanically.
- Pro: Holon parses the manual with its own small serde types for spec 1.x
  (the schema is three top-level fields + one call-template struct); no
  dependency on `rs-utcp`.
- Con: two sections instead of one file; a tool named in `holon` but absent
  in `utcp` must fail loud at load (parse, don't validate).
- Con: it is, honestly, "a Holon format wearing a UTCP jacket". The jacket
  is still worth wearing: it is the part users can copy from elsewhere.

### (a) Importer only (rev 1 recommendation)

Keep today's YAML; ship a UTCP/OpenAPI → sidecar importer. Still valid, but
weaker than (d): the standard vocabulary is lost after import, and a
round-trip back to UTCP is a second tool.

## 3. Decisive tradeoff

The single fact that decides between (c′) and (d): the reference client
rejects unknown keys. With (c′) the Holon manual is invalid UTCP until
upstream changes that rule. With (d) the UTCP part is valid UTCP from day
one, and the two designs converge the moment upstream merges. So (d) buys
the same future as (c′) at no cost, and keeps the present honest.

## 4. Recommendation

(d) for the format, NO on forking `rs-utcp`, and the two upstream PRs filed
from a fork of the SPEC repository as normal contribution work (that fork is
a staging area, not a runtime). Order in the plan: Inc 1 (JSON-Lines
contract) is unchanged; the sidecar schema change is Inc 4 of
`plan-lowcode-connections.md`; the upstream PRs are a side lane with no
dependency on the plan.

## 5. Open questions Martin may want to rule on with this

- Should the `holon` section's `response` mapping be jaq only (D81.a) or
  jaq with a field-path shorthand for the common case? (Cost: two syntaxes.)
- Does the `utcp` section replace `transport.rest` in today's sidecars in
  ONE increment (no dual path, per the refactor rule) — recommended yes.
