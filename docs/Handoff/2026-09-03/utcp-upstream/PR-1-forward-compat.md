# PR 1 — Forward compatibility: skip what you don't know, don't drop the manual

**Status: DRAFT. Nothing here has been pushed, forked, or commented anywhere.**

Two repositories, two PRs that land together:

| Repo | Branch (local, in this directory) | Target | Diff |
|---|---|---|---|
| `utcp-specification` | `feature/forward-compatibility-rule` | `main` | `spec-PR1-forward-compat.diff` (71 lines) |
| `python-utcp` | `feature/forward-compatible-manual-loading` | `dev` | `python-utcp-PR1.diff` (286 lines) |

---

## PR description (spec repo)

### Title

`docs: specify forward compatibility for unknown call template types, unknown keys and x- extensions`

### Body

The specification does not say what a client must do with content it does not
recognise. `docs/implementation.md` tells providers how to add a custom protocol
("Define Call Template, Implement Communication Handler, Register Protocol") but
never says what happens on the reading side when the plugin is absent. This PR
fills that gap with three rules: skip the unloadable tool, ignore unknown keys,
and reserve `x-` for implementations.

**Why this matters more than a documentation tidy-up.** The reference client
currently reads that silence as "reject everything". Measured against
`python-utcp` at `89a9832` (`utcp` 1.1.3, `utcp-http` 1.1.11, `utcp-text` 1.1.0):

* A manual holding one `http` tool and one tool with an unregistered
  `call_template_type` registers **zero** tools.
  `CallTemplateSerializer.validate_dict` raises `ValueError("Invalid call
  template type: ...")` for the one tool, and that failure propagates out of
  `UtcpManual.validate_tools` and takes the valid tool with it.
* A manual carrying the `info` key — **the key this repository's own
  `docs/implementation.md` puts in its first example manual, lines 31–34** —
  fails to load: `TypeError: UtcpManual.__init__() got an unexpected keyword
  argument 'info'`. The documented example is not loadable by the reference
  implementation.
* An `x-` key on an `http` call template loads, but is dropped. A client that
  reads and rewrites a manual erases the extension.

The consequence for the ecosystem is a flag day: a provider who adds one tool on
a new protocol withdraws every tool it already published from every client that
has not installed that plugin yet. Providers therefore cannot adopt a new
protocol incrementally, which is the opposite of what the plugin architecture is
for.

A companion PR against `python-utcp` (`feature/forward-compatible-manual-loading`)
implements these rules and adds tests.

### The spec change

Adds `### Forward Compatibility` to `docs/implementation.md` (under Core
Concepts, after Variable Substitution) and one cross-reference from the existing
"Custom Protocol Plugins" section. Only `docs/` is touched; `versioned_docs/` are
frozen snapshots of released versions.

```diff
--- a/docs/implementation.md
+++ b/docs/implementation.md
@@ -178,6 +178,54 @@ Variables in call templates are replaced with actual values using two different

 For example, `https://api.example.com/users/{user_id}` uses the `user_id` tool argument, while `${API_KEY}` references a configuration or environment variable.

+### Forward Compatibility
+
+A manual outlives the client that reads it. Providers add tools on new protocols, and
+later specification versions add keys. A client MUST keep a manual usable when it meets
+either.
+
+#### Unknown `call_template_type`
+
+When a tool's `tool_call_template.call_template_type` names a protocol the client has no
+handler for, the client MUST:
+
+1. **Skip that tool** — do not register it and do not offer it to the agent.
+2. **Warn**, naming the tool and the unrecognised type.
+3. **Register every other tool** in the manual normally.
+
+Rejecting the whole manual is not conformant. Under that behaviour, a provider that adds
+one tool on a new protocol withdraws every tool it already published from every client
+that has not installed the plugin yet — so the ecosystem cannot adopt a new protocol
+without a flag day.
+
+#### Unknown keys
+
+Manuals, tools and call templates are **open objects**. A client that reads a key it does
+not know MUST ignore that key for its own behaviour and MUST NOT reject the object that
+carries it. A client that writes a manual back out SHOULD preserve the unknown keys it
+read, so a manual survives a load/store round trip through a client that is older than the
+manual.
+
+#### Extension keys
+
+Keys beginning with `x-` are reserved for implementations. An `x-` key carries data for one
+implementation and never changes the meaning of the standard keys beside it, so a tool that
+carries one stays callable by clients that ignore it:
+
+```json
+{
+  "call_template_type": "http",
+  "url": "https://api.example.com/lists/{list_id}/commit",
+  "http_method": "POST",
+  "x-acme-retry": {"attempts": 3}
+}
+```
+
+Put an extension in an `x-` key, never in a new `call_template_type`: a custom
+`call_template_type` costs the tool its standard callers, an `x-` key costs nothing. Names
+without an `x-` prefix are reserved for this specification; to have a field standardised,
+open an [RFC](/about/RFC).
+
 ## Tool Provider Implementation

 ### Manual Structure
@@ -307,6 +355,11 @@ Example custom protocol structure:
 }
 ```

+A manual containing such a tool stays loadable by clients without the plugin: they skip the
+one tool and register the rest, per [Forward Compatibility](#forward-compatibility). Use a
+custom `call_template_type` for a genuinely new transport; to add data to an existing
+transport, use an `x-` key instead.
+
 ### Custom Tool Repositories

 Implement custom tool storage:
```

---

## PR description (python-utcp repo)

### Title

`feat(core): skip tools with unknown call template types instead of failing the manual`

### Body

Implements the forward-compatibility rules proposed in
`utcp-specification#<spec PR number>`.

Behaviour change, measured on this branch against `dev` at `89a9832`:

| Manual content | Before | After |
|---|---|---|
| one `http` tool + one tool of an unregistered type | 0 tools, raises | 1 tool, one `WARNING` naming tool and type |
| the `info` key from `docs/implementation.md`'s own example | raises `TypeError` | loads; `info` kept in `model_extra` and re-emitted by `to_dict` |
| `x-acme-retry` on an `http` call template | loads, key dropped | loads, key kept and re-emitted |
| call template with no `call_template_type` | raises | raises (`UtcpSerializerValidationError`) — unchanged |

**Four changes:**

1. **`UtcpUnknownCallTemplateTypeError`** (new, in `utcp.exceptions`). A distinct
   type is what makes the skip possible: the manual loader must tell "this
   client has no plugin for that protocol" apart from "this template is
   malformed", and only skip the first. It derives from `Exception`, not
   `ValueError`, so pydantic propagates it through the nested `Tool` validator
   rather than folding it into a `ValidationError`.
2. **`CallTemplateSerializer.validate_dict`** raises it for an unregistered
   type. The lookup moved out of the `try` block: previously a `KeyError` from
   the serializer's own body was misreported as an invalid type, and the
   `except KeyError` handler itself raised `KeyError` when
   `call_template_type` was absent. A missing key is now its own explicit
   error.
3. **`UtcpManual`** gains `extra="allow"` and loads tools one at a time, warning
   past the unknown ones. Its `__init__` now takes `**data`; the previous
   explicit signature was the actual reason `info` was rejected — pydantic
   passed the extra key to `__init__`, which had no parameter for it.
   `CallTemplate` gains `extra="allow"` for the same round-trip reason.
4. **`ToolSerializer.validate_dict`** re-raises the new error instead of
   wrapping it, so the manual loader can see it.

**Tests:** `core/tests/data/test_manual_forward_compatibility.py`, three cases —
the skip (asserting both the surviving tool and the warning), the round trip of
`info` and `x-`, and that a malformed template still fails loudly.

**Suite:** `core` 40 passed (37 before + 3 new). Plugin suites
`http`/`text`/`file`/`cli`: 226 passed, 5 skipped, 61 errors — byte-identical to
the same run on the unmodified tree; those errors are pre-existing
`pytest-asyncio` fixture-setup failures in this environment, untouched by this
change.

### The code change

Full diff: `python-utcp-PR1.diff` in this directory (286 lines, 6 files, +156/−13).
The three load-bearing hunks:

```diff
--- a/core/src/utcp/data/call_template.py
+++ b/core/src/utcp/data/call_template.py
@@ class CallTemplateSerializer
-        try:
-            return CallTemplateSerializer.call_template_serializers[obj["call_template_type"]].validate_dict(obj)
-        except KeyError:
-            raise ValueError(f"Invalid call template type: {obj['call_template_type']}")
-        except Exception as e:
-            raise UtcpSerializerValidationError("Invalid CallTemplate: " + traceback.format_exc()) from e
+        if "call_template_type" not in obj:
+            raise UtcpSerializerValidationError("Invalid CallTemplate: missing 'call_template_type'")
+        serializer = CallTemplateSerializer.call_template_serializers.get(obj["call_template_type"])
+        if serializer is None:
+            raise UtcpUnknownCallTemplateTypeError(obj["call_template_type"])
+        try:
+            return serializer.validate_dict(obj)
+        except Exception as e:
+            raise UtcpSerializerValidationError("Invalid CallTemplate: " + traceback.format_exc()) from e
```

```diff
--- a/core/src/utcp/data/utcp_manual.py
+++ b/core/src/utcp/data/utcp_manual.py
@@ class UtcpManual(BaseModel)
+    model_config = ConfigDict(extra="allow")
+
     utcp_version: str = __version__
     manual_version: str = "1.0.0"
     tools: List[Tool]

-    def __init__(self, tools: List[Tool], manual_version: str = "1.0.0", utcp_version: str = __version__):
-        super().__init__(utcp_version=utcp_version, manual_version=manual_version, tools=tools)
-        """Initializes the UtcpManual, ensuring plugins are loaded."""
-        ensure_plugins_initialized()
+    def __init__(self, **data):
+        """Initializes the UtcpManual, ensuring plugins are loaded."""
+        ensure_plugins_initialized()
+        super().__init__(**data)
@@ validate_tools
-        return [v if isinstance(v, Tool) else ToolSerializer().validate_dict(v) for v in tools]
+        validated: List[Tool] = []
+        for v in tools:
+            if isinstance(v, Tool):
+                validated.append(v)
+                continue
+            try:
+                validated.append(ToolSerializer().validate_dict(v))
+            except UtcpUnknownCallTemplateTypeError as e:
+                logger.warning(
+                    "Skipping tool '%s' in manual: %s The rest of the manual is unaffected.",
+                    v.get("name", "<unnamed>") if isinstance(v, dict) else "<unnamed>",
+                    e,
+                )
+        return validated
```

```diff
--- a/core/src/utcp/data/tool.py
+++ b/core/src/utcp/data/tool.py
@@ class ToolSerializer
         try:
             return Tool.model_validate(obj)
+        except UtcpUnknownCallTemplateTypeError:
+            raise
         except Exception as e:
             raise UtcpSerializerValidationError("Invalid Tool: " + traceback.format_exc()) from e
```

New files: `core/src/utcp/exceptions/utcp_unknown_call_template_type_error.py`
and `core/tests/data/test_manual_forward_compatibility.py`.

---

## What a reviewer will push back on, and the answer

**"Skipping hides provider mistakes."** It does not: every skip emits a
`WARNING` naming the tool and the type, and a malformed template of a *known*
type still raises. Only the case a client provably cannot handle — a protocol it
has no plugin for — is downgraded from fatal to skipped.

**"`extra='allow'` weakens validation."** It weakens it exactly where the spec
change says it should be weak, and nowhere else. Every declared field keeps its
type. The alternative, `extra='ignore'`, would load the manual but silently
delete `x-` data on rewrite, which is the round-trip failure this PR is fixing.

**"This is a breaking change."** For callers, no: `UtcpManual(**kwargs)` accepts
everything it accepted before, and the new exception type is not raised anywhere
a caller previously caught something specific — the old code raised a bare
`ValueError` wrapped in `UtcpSerializerValidationError`, which nothing in-tree
catches by type. For manual *authors*, it only widens what loads.

## Measured facts these claims rest on

`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/utcp-shopping/utcp-claims-verify.md`
(§ Extension path assessment, claim rows 5 and 7), independently re-run in this
directory against a fresh clone: `repro.py`, with `repro-before.log` and
`repro-after.log` holding the two outputs.
