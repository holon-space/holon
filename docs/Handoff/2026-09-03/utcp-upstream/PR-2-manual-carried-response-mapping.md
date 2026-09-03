# PR 2 — A response mapping that travels with the manual

**Status: DRAFT. Nothing here has been pushed, forked, or commented anywhere.**

| Repo | Branch (local, in this directory) | Target | Diff |
|---|---|---|---|
| `utcp-specification` | `feature/manual-carried-response-mapping` | `main` | `spec-PR2-response-mapping.diff` (98 lines) |

Spec-only. No reference-implementation diff: unlike PR 1, this adds a field and
an evaluator rather than relaxing a rule, so the maintainers' answer to "do you
want `jq` in the core?" has to come first. The implementation sketch is at the
end.

---

## PR description

### Title

`docs: add an optional per-tool response mapping carried by the manual`

### Body

**The gap.** A UTCP manual says everything about how to *call* a tool and
nothing about the shape of what comes back. `outputs` documents the shape but
cannot produce it, so a manual can only promise the API's raw shape. Anything
else — pulling two fields out of a nested envelope, flattening a list, renaming
`quantity` to `qty` — has to be written by the consumer, once per consumer.

Response handling does exist in the reference implementation, as
`ToolPostProcessor` (`core/src/utcp/interfaces/tool_post_processor.py`,
declared in `UtcpClientConfig.post_processing`). It is **client configuration**.
It cannot be published by the provider, cannot travel with the manual, and the
two built-ins (`filter_dict`, `limit_strings`) only prune keys and truncate
strings — neither can extract a value into a new shape. Measured against
`python-utcp` at `89a9832`: `UtcpManual.model_fields` is exactly
`utcp_version, manual_version, tools`, and `Tool.model_fields` is exactly
`name, description, inputs, outputs, tags, average_response_size,
tool_call_template`. There is no response field anywhere in the manual.

The result is that the one piece of integration knowledge a provider is best
placed to supply — "here is how my envelope maps onto the `outputs` I
documented" — is the one piece the manual cannot carry. Every consumer
rediscovers it, and `outputs` stays decorative.

**The proposal.** An optional `response` object on a tool, carrying a mapping
expression and a declared language tag, applied to the result before it is
returned. `jq` is proposed as the first registered language.

Explicitly *not* proposed: making `response` mandatory, changing `outputs`,
or removing `post_processing`. The two compose — `response` is
provider-authored and per-tool, `post_processing` stays consumer-authored and
cross-tool, and `response` runs first.

**Two points I expect discussion on, stated up front:**

1. **The unsupported-language rule is refuse, not ignore.** This is deliberately
   the opposite of the skip-unknown-content rule in the companion PR. A tool
   whose mapping was skipped would return data contradicting its own `outputs`,
   and the agent has no way to tell. Loud refusal of one tool is the cheaper
   failure.
2. **`jq` is a dependency, and an evaluator of provider-supplied expressions is
   an attack surface.** The spec text therefore mandates a sandbox — no
   filesystem, network or environment access, bounded time and output — and
   names the builtins authors must not rely on. Naming a language rather than
   inventing one is the point: `jq` has an existing grammar, existing tests, and
   at least two independent engines (`jq` 1.7, and `jaq` for hosts where a
   native `libjq` binding is impractical), so a Rust, Go or TypeScript client is
   not forced through C.

### The spec change

Adds `### Response Mapping` to `docs/implementation.md` and the field to the
manual example under Core Concepts. Only `docs/` is touched; `versioned_docs/`
are frozen snapshots.

```diff
--- a/docs/implementation.md
+++ b/docs/implementation.md
@@ -120,12 +120,19 @@ A UTCP manual is a JSON document that describes available tools and how to call
         "call_template_type": "http",
         "url": "https://api.example.com/endpoint",
         "http_method": "POST"
+      },
+      "response": {
+        "language": "jq",
+        "expression": "{result: .data.value}"
       }
     }
   ]
 }
 ```

+The optional `response` field maps the raw result into the shape `outputs` describes — see
+[Response Mapping](#response-mapping).
+
 ### Call Templates

 Call templates define how to invoke tools using specific protocols:
@@ -178,6 +185,73 @@ Variables in call templates are replaced with actual values using two different

 For example, `https://api.example.com/users/{user_id}` uses the `user_id` tool argument, while `${API_KEY}` references a configuration or environment variable.

+### Response Mapping
+
+A tool MAY carry an optional `response` object that reshapes the raw result before the
+client returns it. It is the counterpart of `inputs`: `inputs` describes what goes in,
+`response` describes how what comes out becomes the value the agent sees.
+
+```json
+{
+  "name": "list_items",
+  "inputs": {"type": "object", "properties": {"list_id": {"type": "string"}}},
+  "outputs": {
+    "type": "object",
+    "properties": {"items": {"type": "array"}, "version": {"type": "integer"}}
+  },
+  "tool_call_template": {
+    "call_template_type": "http",
+    "url": "https://api.example.com/lists/{list_id}",
+    "http_method": "GET"
+  },
+  "response": {
+    "language": "jq",
+    "expression": "{items: [.data.entries[] | {sku: .id, qty: .quantity}], version: .meta.rev}"
+  }
+}
+```
+
+#### Fields
+
+| Field | Required | Meaning |
+|---|---|---|
+| `language` | yes | Identifier of the mapping language. Registered values: `jq`. |
+| `expression` | yes | The mapping, in that language. |
+
+#### Semantics
+
+- The expression is applied to the **decoded** tool result — the parsed JSON body for
+  protocols that produce one — and its output **replaces** that result.
+- `outputs`, when present, describes the **mapped** value, not the raw one.
+- An error while evaluating the expression is a **tool error**. The client reports it the
+  way it reports a transport failure. It MUST NOT fall back to returning the raw result:
+  the agent would receive a shape that contradicts `outputs`.
+- For a streaming tool the expression is applied to each chunk the protocol yields.
+- `response` travels with the manual and is authored by the tool provider. It is
+  independent of any client-side post-processing the consumer configures; where a client
+  offers both, `response` runs first, on the raw result.
+
+#### Unsupported languages
+
+`response` changes the meaning of the data, so a client that cannot evaluate it MUST NOT
+ignore it. On meeting a `language` it does not support, the client MUST refuse the tool:
+skip it, and warn naming the tool and the language. This is a deliberate exception to the
+rule that unknown content is ignored — silently returning the unmapped result would hand
+the agent data that does not match the tool's declared `outputs`.
+
+#### The `jq` language
+
+`language: "jq"` means the filter language of [jq](https://jqlang.github.io/jq/) 1.7.
+Implementations MAY use any compatible engine — for example
+[jaq](https://github.com/01mf02/jaq) where a native `libjq` binding is impractical.
+Manual authors SHOULD stay inside the common subset: expressions MUST NOT rely on jq's
+I/O and environment builtins (`input`, `inputs`, `$ENV`, `env`, `input_filename`,
+`debug`, `getpath` on external state), which engines are not required to provide.
+
+Clients MUST evaluate the expression as untrusted input from the manual's provider: with
+no filesystem, network or environment access, and under a bounded evaluation time and
+output size.
+
 ## Tool Provider Implementation

 ### Manual Structure
```

---

## Implementation sketch (offered, not submitted)

Should the maintainers want the reference implementation in the same PR, the
shape in `python-utcp` is:

* `core/src/utcp/data/response_mapping.py` — a `ResponseMapping` model
  (`language`, `expression`) plus a `ResponseMappingSerializer`, mirroring
  `CallTemplate`/`CallTemplateSerializer`.
* `Tool` gains `response: Optional[ResponseMapping] = None`.
* An evaluator registry keyed by language, matching the existing plugin
  discovery pattern (`utcp.plugins` entry points, `register_*`), so `jq` ships
  as `utcp-jq` and core carries no `jq` dependency. A client with no evaluator
  for a declared language refuses the tool — which is `UtcpManual.validate_tools`
  raising a language-specific error that the loader turns into a skip plus
  warning, the same machinery PR 1 adds.
* Application point: `UtcpClient.call_tool`, on the decoded result, before
  `post_processing` runs.

This sequencing is why the two PRs are separate: PR 1's per-tool skip is the
mechanism PR 2's "refuse this one tool" rule needs. PR 2 depends on PR 1; PR 1
stands alone.

## Measured facts these claims rest on

`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/utcp-shopping/utcp-claims-verify.md`,
claim row 2 ("No response mapping — PARTIAL, confirmed for the manual"): the
`UtcpManual` and `Tool` field lists, the `ToolPostProcessor` location, and the
finding that the two built-in post-processors only prune and truncate.
