# Verdict: REFUTED (narrow) — one enumerated sub-claim fails; every architectural claim holds

The substance of lane `lowcode-inc4` is confirmed: `transport.rest` is gone as a
readable sidecar key, `utcp:` + `holon:` is the only path, the bespoke kitchen
parse is deleted, the trait defaults refuse loudly, and the gate subset is green.
One enumerated claim — "mutation of the shipped filter turns **both** legs red" —
is false at default proptest settings. Four further defects are recorded below.

Workspace: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/lowcode-inc4`
(`@` = `rtyttoyz 0c169153`, uncommitted diff, 45 files). Read-only jj throughout.

---

## Step 0 — tree identity

| Command | Result |
|---|---|
| `grep -rn "schema_version: 2" assets/integrations/*.yaml \| wc -l` | 6 |
| `grep -rn "transport:" assets/integrations/ crates/holon-mcp-client/src \| grep -i rest` | hits are `rest_transport::` module paths only |

Log: `lane-logs/verify-check0-sidecars.log`

The literal Step-0 grep is not decisive: it matches the `rest_transport` module
name, not a `transport: rest` reader. Checked on substance instead. No sidecar
declares `transport: rest`. Two sidecars keep a `transport:` block
(`todoist.yaml:27`, `claude-history.yaml:17`) and both are legitimate MCP
transports: `TransportConfig` at
`crates/holon-mcp-client/src/integration_config.rs:40-44` carries
`deny_unknown_fields` over exactly `child_process` and `http`, so a
`transport: {rest: …}` sidecar is refused at parse. The other four sidecars
(`gcal`, `gmail`, `jsonplaceholder`, `shopping`) carry `utcp:` + `holon:`.
Tree is the right one.

---

## Check 1 — dual-path hunt

Command: `rg -n "transport\.rest|RestTransport|fn parse_shopping|from_shopping_json|to_wire" crates/`
Log: `lane-logs/verify-check1-dualpath.log`
Summary: 11 hits, zero of them a live reader.

- `parse_shopping` / `from_shopping_json`: **no hits**. The bespoke prod parse is
  gone; `crates/holon-kitchen/src/shopping.rs` shrank by 296 lines and retains no
  `serde_json` response walk.
- `RestTransport`: no hits.
- `to_wire`: only `crates/holon-loro/src/share_enrollment.rs` (unrelated crate).
- `transport.rest`: **five hits, all inside user-facing strings or doc comments.**
  See Defect 2.

The differential test's "bespoke parse" is a test-local transcription
(`crates/holon-kitchen/tests/shopping_mapping_differential.rs:1-14` states this
explicitly), not a surviving production path. Correct pattern.

**No dual path.**

---

## Check 2 — loud refusal on `map_response` / `map_request`

Trait defaults: `crates/holon-mcp-client/src/mcp_call_surface.rs:36-45` and `:49-…`.
Probe: a bare `McpCallSurface` impl (no REST manual), asked for rows.
Log: `lane-logs/verify-check245-probe.log`

```
PROBE-MAP-RESPONSE-REFUSAL: call 'pull_list' reaches an MCP peer, which declares
  no `response` mapping; only a `utcp:` connection maps a response into rows
PROBE-MAP-REQUEST-REFUSAL: call 'commit' reaches an MCP peer, which declares
  no `request` mapping; only a `utcp:` connection maps rows into a call
```

Both `Err`, both name the call. Never `Ok(vec![])`, so replace-scope semantics
cannot silently delete the call's rows. **CONFIRMED.**

Probe file `crates/holon-mcp-client/tests/zz_verify_probe.rs` created and
removed; `jj status` shows zero trace of it.

---

## Check 3 — red-first teeth (filter mutation)

Cold rerun of the differential passed as part of the gate below.

Mutation applied by me to `assets/integrations/shopping.yaml`, inside the
shipped `holon.tools.pull_list.response` filter — the duplicate-fold arm:

```
count: (if length == 1 then .[0].count else (map(.count // 1) | add) end),
→     (if length == 1 then .[0].count else (map(.count // 1) | add) + 1 end),
```

Log: `lane-logs/verify-check3-mutant.log`
Summary: `2 tests run: 1 passed, 1 failed`

The captured-shapes leg went red for exactly the right reason, naming the
divergence rather than allowlisting it:

```
1 divergence(s) between the sidecar mapping and the parse it replaces.
both accepted, differently
  old: items: {("Milk","R"): (Some(6.0), true, true)}
  new: items: {("Milk","R"): (Some(7.0), true, true)}
```

**The generated-responses PBT leg PASSED under the same mutation.** Isolated
rerun at `PROPTEST_CASES=4096` does catch it, after 91 successes
(`lane-logs/verify-check3-mutant-generated.log`). So the mutation is caught by
the suite, but not by the leg the claim names, at the settings the gate runs.

Restored byte-for-byte:

| | sha256 |
|---|---|
| before | `fca336ee09382784ca7645ebe7bf7a4c052fdd3c16062050f82d0ea2b0cf64ec` |
| mutated | `bf870edff44a98516cb618d76de033078140e358e8b338024776c8d84e9e1b44` |
| after restore | `fca336ee09382784ca7645ebe7bf7a4c052fdd3c16062050f82d0ea2b0cf64ec` |

### Defect 1 (REFUTES a claim) — the generated leg has weaker teeth than claimed

`crates/holon-kitchen/tests/shopping_mapping_differential.rs`, test
`the_mapping_matches_the_bespoke_parse_on_generated_responses`. The generator
reaches the duplicate-`(name, cat)` fold arm rarely enough that ~256 default
cases miss it. Only the captured-shapes example leg defends that arm at gate
settings. The claim "mutation of the shipped filter turns both legs red" does
not hold as stated.

---

## Check 4 — sidecar contract under unknown keys

Log: `lane-logs/verify-check245-probe.log`, census
`lane-logs/verify-check4-deny.log`

Actual behavior, both probed with a scratch sidecar:

```
PROBE-HOLON-UNKNOWN:        holon: unknown field `nonsense_key`, expected one of
                            `auth`, `poll_interval`, `tools` at line 16 column 3
PROBE-HOLON-TOOL-TYPO:      holon.tools.get-things: unknown field `responze`,
                            expected one of `query`, `body`, `format`,
                            `result_key`, `pagination`, `response_version_path`,
                            `response`, `request` at line 17 column 7
PROBE-HOLON-UNKNOWN-TOOLNAME: holon.tools.no-such-tool: the manual declares no
                            tool named 'no-such-tool' (it declares: ["get-things"])

PROBE-UTCP-UNKNOWN-TOPLEVEL:     utcp: unknown field `info`, expected one of
                                 `utcp_version`, `manual_version`, `tools`
PROBE-UTCP-UNKNOWN-CALLTEMPLATE: utcp.tools[0].tool_call_template: unknown field
                                 `headers`, expected one of `name`,
                                 `call_template_type`, `url`, `http_method`,
                                 `content_type`, `body_field`
PROBE-UTCP-CLI-PARSE:  None            (parses fine)
PROBE-UTCP-CLI-BUILD:  tool 'get-things' declares call_template_type 'cli', and
                       this build serves only 'http'
```

- **Unknown `holon:` key → REFUSED loudly, with the accepted-field list and a
  line/column.** Matches the intent. Errors are precise enough to act on.
- **Unknown `call_template_type` → parses, then refused BY NAME at build.**
  Matches the documented intent
  (`crates/holon-mcp-client/src/utcp_manual.rs:88-100`).
- **Unknown `utcp:` key → REFUSED, not ignored-and-preserved.** This contradicts
  the intent as it was handed to me, but it *matches* the ADR on disk. ADR 0034
  §5 (`docs/adr/0034-…md:172-196`) states the reference client "rejects unknown
  keys deliberately" and files the ignore-unknown-keys rule as an **upstream PR
  not yet in force**. The implementation is the ADR's position, not a drift from
  it. The brief's stated D84.d intent is what is out of date.

### Defect 2 (risk, not a break) — "import a published manual" is narrower than the prose promises

`crates/holon-mcp-client/src/utcp_manual.rs:1-13` and
`assets/integrations/shopping.yaml:12-16` both say a user can import a published
UTCP manual unchanged. In practice a manual loads only if its fields are a
subset of three manual keys, six tool keys and six call-template keys. A real
1.1.x manual carrying `info`, `tool_call_template.headers`, `auth`, or
`query_params` fails the whole load, not just the field. That is a defensible
design under the ADR, but the import promise in the module doc and the sidecar
header overstates it.

---

## Check 5 — secret handling

Command: `rg -n "redact|<redacted>" crates/holon-mcp-client/src`
Probe with a fake credential `FAKE-TOKEN-XYZ` resolved through `${VAR}`.
Log: `lane-logs/verify-check245-probe.log`

```
PROBE-DEBUG-TRANSPORT: Rest { manual: RestManual { auth: Static { header:
  "Authorization", value: <redacted> }, calls: ["get-things GET
  https://example.invalid/things"] }, poll_interval: None }
PROBE-DEBUG-WHOLECONFIG-LEAK: false
```

The token appears in neither the transport `Debug` nor the whole-config `Debug`.
Both assertions in the probe passed. `RestManual`'s `Debug` is hand-written
(`crates/holon-mcp-client/src/rest_transport.rs:252-264`) and shows URLs through
the redactor; `RestAuth`'s `Debug` (`:202-211`) blanks the value in every arm,
including the OAuth2 provider. The report's description is accurate. **CONFIRMED.**

---

## Check 6 — gate rerun (cheap subset)

Command:
`/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-build -j4 --fg -- bash lane-logs/verify-gate-subset.sh`
(script PATH-prefixes `/opt/homebrew/opt/rustup/bin`, unsets `RUSTC_WRAPPER`,
pins `CARGO_BUILD_JOBS=6`)
Log: `lane-logs/verify-check6-gate.log`
Summary: `Summary [7.981s] 442 tests run: 442 passed, 0 skipped`

Toolchain in-lane: `nightly-2026-08-16-aarch64-apple-darwin`. Consistent with the
claimed 615 across five crates; the three-crate subset is 442. Includes
`shopping_mapping_differential` (both legs), `shopping_mapping_cost`
(10k-item list inside the SLO), `utcp_manual_roundtrip` (4 tests),
`sidecar_conformance`. `just guests-verify` skipped per brief.

---

## Check 7 — ADR 0034 §5 vs the sidecars

Command: `rg -n "holon\.tools|holon:|utcp:" docs/adr/0034-…md`
Log: `lane-logs/verify-check7-adr.log`

ADR §5 line 162: "Per-tool entries sit under `holon.tools` rather than directly
under `holon`, so a peer that named a tool `auth` or `poll_interval` is still
representable." The shipped `HolonSection`
(`crates/holon-mcp-client/src/integration_config.rs:74-92`) carries that exact
reasoning in its own doc comment, and all four `utcp:` sidecars key tools under
`holon.tools`. The ADR's worked YAML matches `shopping.yaml` field for field.
**CONFIRMED.**

---

## Further defects found outside the enumerated checks

### Defect 3 — three live error messages send sidecar authors to keys the parser rejects

The remedy text in these user-facing errors names `transport.rest.*`, which
`TransportConfig` (`integration_config.rs:40-44`, `deny_unknown_fields` over
`child_process`/`http`) now refuses. Following the instruction produces a load
failure.

- `crates/holon-mcp-client/src/rest_transport.rs:707` — "describe reads as GET
  calls under `transport.rest.calls`". Real path: `utcp.tools[].tool_call_template`.
- `crates/holon-mcp-client/src/mcp_integration.rs:1170` — "use `sync.list_tool`
  naming a `transport.rest.calls` entry instead".
- `crates/holon-mcp-client/src/oauth_bootstrap.rs:299` and `:306` — "declares no
  `transport.rest.auth.oauth2.auth_url`" / "`…scopes`". Real path is
  `holon.auth.oauth2.auth_url`, as `assets/integrations/gcal.yaml:129-137`
  demonstrates.

Also stale, doc-comment only: `crates/holon-mcp-client/src/rest_oauth2.rs:55`,
`crates/holon-mcp-client/tests/sidecar_conformance.rs:18`,
`crates/holon-mcp-client/src/mcp_integration.rs:44`.

### Defect 4 — a comment asserts the opposite of the code it sits on

`crates/holon-kitchen/src/shopping_sync.rs:132-135`:

> The verbs, the `good` wrapper and the `new: true` literal the capture pinned
> are NOT here — they are one peer's spelling, and they live in that peer's
> sidecar. What travels is the intent.

The `good` wrapper and `new: true` are indeed sidecar-only
(`assets/integrations/shopping.yaml:279`). The **verbs are not**:
`CommandVerb::as_wire()` at `crates/holon-kitchen/src/shopping_sync.rs:40-47`
returns the literals `"add"` / `"del"`, `to_row` writes them into the row under
key `verb` (`:59-71`), and the request filter passes them straight through as
`cmd: .verb` (`shopping.yaml:279`). The peer's verb vocabulary lives in Rust,
inside a method literally named `as_wire`. Either the comment or the placement
is wrong.

### Defect 5 — dead keys in the migrated sidecars

`assets/integrations/shopping.yaml` ends with `entities: {}` and `tools: {}`.
Empty leftovers of the pre-migration shape, carried by every migrated sidecar.
Harmless, but they teach the next author that a `utcp:` sidecar needs them.

---

## Probe hygiene

- `crates/holon-mcp-client/tests/zz_verify_probe.rs`: created, run, deleted.
  `jj status` shows no such path.
- `assets/integrations/shopping.yaml`: mutated, restored, sha256 identical to
  the pre-probe value (table under Check 3).
- Everything else read-only. No `jj restore` / `abandon` / `commit` / `describe`.
- Files I added under `lane-logs/` are verify logs and two runner scripts; they
  are new untracked paths, not edits to lane work.
