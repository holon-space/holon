# Adversarial verification: UTCP capability claims

**Verdict: the lane report is essentially CORRECT.** Six of seven claims CONFIRMED against evidence
I produced myself. One claim (response mapping) is PARTIAL — a post-processor plugin interface does
exist, but it cannot live in the manual and cannot do what the shopping API needs.

**The extension question has a hard answer, and it is worse than the lane report suggested.** An
unknown `call_template_type` does not get ignored. It fails validation of the *entire manual*, so a
Holon extension would make every standard UTCP client drop the standard tools alongside it.

## Versions under test (measured, not assumed)

| Component | Version | Source |
|---|---|---|
| Python client `utcp` | 1.1.3 | `pip list` in the lane venv |
| `utcp-http` plugin | 1.1.11 | same |
| `utcp-file` / `utcp-text` | 1.1.0 | same |
| Spec (implementation guide) | 1.0.1 | docs/implementation.md |
| Spec docs tree | has `migration-v1.0-to-v1.1.md` | docs/ listing |
| `rs-utcp` (Rust) | 0.3.2, published 2026-03-05, 3,329 downloads | crates.io API |
| `rs-utcp` example manual | `"utcp_version": "0.3.0"` | its README |

Registered call-template types in a fully-plugged Python client, logged at import:
`http, sse, streamable_http, text, file`. Nothing else.

## Claim table

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 1 | No request **body template**; `body_field` names one argument that becomes the whole body | **CONFIRMED** | `HttpCallTemplate.model_fields` (utcp-http 1.1.11) is exactly `name, call_template_type, auth, allowed_communication_protocols, http_method, url, content_type, auth_tools, headers, body_field, header_fields` — `body_field: str \| None = 'body'`, a single name. Live echo server: with `body_field="commands"`, the wire body was exactly `[{"op": "add"}]`. No envelope, no literal fields, no `{version}` interpolation. |
| 2 | No **response mapping** | **PARTIAL — confirmed for the manual** | The manual cannot carry it: `UtcpManual.model_fields` is exactly `utcp_version, manual_version, tools`, and `Tool` is `name, description, inputs, outputs, tags, average_response_size, tool_call_template`. No response field anywhere. A `ToolPostProcessor` interface *does* exist (`utcp/interfaces/tool_post_processor.py`, `post_process(caller, tool, manual_call_template, result)`), but it is declared in `UtcpClientConfig.post_processing`, i.e. **client-side config, not shippable in the manual**. The two built-ins only prune keys (`filter_dict`) or truncate strings (`limit_strings`). Neither extracts a field to feed the next call. Spec RFC has no response-handling section. |
| 3 | Query parameters are undeclared leftovers | **CONFIRMED** | Live echo server, same call: path came out `/api/list/L1/commit?version=7&_nocache=12345`. `listId` was consumed by the `{listId}` path placeholder, `commands` by `body_field`, `deviceId` by `header_fields`; `version` and `_nocache` became query params purely by elimination. The template names them nowhere. |
| 4 | No **polling cadence** | **CONFIRMED** | No scheduling field in `HttpCallTemplate`, `CallTemplate`, `Tool`, `UtcpManual`, or `UtcpClientConfig`. The spec has no scheduling concept. |
| 5 | File-sourced manual **silently drops HTTP tools**, client reports success | **CONFIRMED** | Reproduced. A `file` manual containing one `http` tool with `allowed_communication_protocols` unset registered **`[]` tools and raised nothing**. Cause is in `utcp_client_implementation.py`: "If `allowed_communication_protocols` is None or empty, it defaults to only allowing the manual's own `call_template_type`. This provides secure-by-default behavior." Setting `["file","http"]` registered `['b.ok']`. So it is a fail-quiet footgun with a one-line workaround, not a capability limit. |
| 6 | `${VAR}` is substitution only; the resolved secret reaches exception text | **CONFIRMED, and worse** | Live 401 against a local server with the secret in the URL path: the secret appeared in `str(exception)` (`ClientResponseError: 401, message='Unauthorized', url='http://.../api/<SECRET>/commit'`) **and** in an ERROR log line from `utcp_http.http_communication_protocol`. Refinement: a *connect* failure does not leak (aiohttp names only host:port); an HTTP error status does. Also `http_communication_protocol.py:159` logs the full resolved discovery URL at INFO. The spec's `security.md` says only "Avoid logging sensitive information like passwords or tokens" and "Error messages don't leak sensitive information" — guidance, no mandated redaction mechanism, and the reference client violates it. |
| 7 | rs-utcp 0.3.2 tracks spec 0.3.0 while Python is 1.1.3; the two disagree on the manual's top-level schema | **CONFIRMED** | rs-utcp README's example manual carries `"utcp_version": "0.3.0"` and a top-level `allowed_communication_protocols`. Python 1.1.3 `UtcpManual` has exactly three fields and **rejects** an extra `info` key: passing it raised `UtcpSerializerValidationError`. Pydantic's default would ignore extras, so this rejection is deliberate. |

## Extension path assessment

- **Unknown `call_template_type` is a hard failure, not a skip.** `CallTemplateSerializer.validate_dict`
  raises `ValueError(f"Invalid call template type: {...}")`. I loaded a file manual holding one valid
  `http` tool plus one `http+template` tool: the **whole manual failed** and **zero** tools registered,
  including the valid one. A Holon extension is therefore not backward-safe in a shared manual.
- **The spec states no forward-compatibility rule.** `docs/implementation.md` describes adding custom
  protocols ("Define Call Template, Implement Communication Handler, Register Protocol") but never says
  what a client must do with a type it does not know. That silence is the whole problem.
- **A plugin is real but per-implementation.** Python discovers plugins via the `utcp.plugins` entry-point
  group and `register_call_template(...)`. Holon would need one plugin per client language, and `rs-utcp`
  has no equivalent documented. A manual using it is portable only to clients that installed it.
- **Safe extension is possible only outside the standard fields**, e.g. a sidecar file keyed by tool name
  that carries the body template, the response paths and the cadence, leaving `call_template_type: "http"`
  intact. Standard clients then still parse the manual and simply ignore the sidecar. That is a Holon
  format wearing a UTCP jacket, not a UTCP extension.
- **The OpenAPI converter is not part of the spec.** It lives at `utcp_http/openapi_converter.py` in the
  Python HTTP plugin. Zero `openapi` hits in the core `utcp` package. Borrowing the idea is free;
  depending on it means depending on Python.

## Recommendation for Martin

**Both, with an importer — keep the sidecar as the runtime format, take UTCP at the edges.** The
standards instinct is right, but UTCP's standard part does not cover what this integration needs. Four
of the shopping sidecar's capabilities have no UTCP field, and the one extension point that exists
(post-processors) is client config rather than manual content, so it cannot travel with the integration.

Adopting-and-extending is the option I would rule out. The extension would either use a custom
`call_template_type`, which makes every standard client reject the whole manual, or hide in a sidecar,
which gives up the interoperability that was the reason to adopt UTCP. Meanwhile UTCP would add a young
Rust crate on the hot path that trails the Python client by a full major version, plus a client that
leaks resolved secrets into logs and error text — a direct conflict with Holon's rule that `${VAR}` marks
a value secret.

So: keep the sidecar, and spend the effort on the response-mapping layer, which is the actual gap and
which UTCP does not close. Then add a UTCP **importer** that reads a published manual and emits a Holon
sidecar. That buys the ecosystem and the low-code promise at import time, with no runtime dependency and
no schema-drift exposure. If UTCP later specifies response mapping and an unknown-type skip rule, the
importer is the seam where adoption gets reconsidered.

One caveat worth stating: this verifies the *protocol*, not the lane's live-API blocker. The real
`SHOPPING_LIST_URL` is still unknown, and that is independent of which format wins.

## What I ran

All probes used the lane's own venv (`utcp` 1.1.3 / `utcp-http` 1.1.11), against local servers only.
No live peer, no secret file opened.

1. Pydantic model introspection of `HttpCallTemplate`, `UtcpManual`, `Tool`, `CallTemplate`, `UtcpClientConfig`.
2. Source read of `utcp_client_implementation.py`, `call_template.py`, `tool_post_processor.py`,
   `filter_dict_post_processor.py`, `discovery.py`, `plugin_loader.py`, `http_communication_protocol.py`.
3. Three manual-registration experiments (http-tool-dropped, allowed-protocols-fixes-it, unknown-type-kills-manual).
4. A local echo server capturing the exact wire request for body/path/header/query routing.
5. Two secret-leak probes: a connect failure (no leak) and a 401 (leak into exception text and ERROR log).
6. Primary-source fetches: spec `docs/protocols/http.md`, `docs/security.md`, `docs/implementation.md`,
   `docs/` listing, `www.utcp.io/about/RFC`, rs-utcp README, crates.io API.
