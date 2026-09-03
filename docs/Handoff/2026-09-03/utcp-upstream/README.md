# Two draft UTCP upstream contributions

Per ruling **D84.d** — *"embed the standard, extend beside it; file the upstream
PRs from a spec fork as a side lane."*

**Nothing has been pushed. No fork exists. No PR or comment has been opened.**
Everything below is local: two read-only clones, four local branches, four diff
files. The last section has the commands to turn this into real PRs.

## What is here

| File | What it is |
|---|---|
| `PR-1-forward-compat.md` | PR bodies + spec diff + code diff for the forward-compatibility rules |
| `PR-2-manual-carried-response-mapping.md` | PR body + spec diff for a per-tool `response` mapping |
| `spec-PR1-forward-compat.diff` | 71 lines — `utcp-specification`, branch `feature/forward-compatibility-rule` |
| `python-utcp-PR1.diff` | 286 lines — `python-utcp`, branch `feature/forward-compatible-manual-loading` |
| `spec-PR2-response-mapping.diff` | 98 lines — `utcp-specification`, branch `feature/manual-carried-response-mapping` |
| `repro.py`, `repro-before.log`, `repro-after.log` | The three behaviours, before and after PR 1's code change |
| `utcp-specification/`, `python-utcp/` | `git clone --depth 50`, no remotes written |
| `venv/` | Python 3.12, the two clones installed editable, for running the reference test suite |

Clone bases: `utcp-specification` `main` at `3aa837d`; `python-utcp` `dev` at
`89a9832`. Reference versions under test: `utcp` 1.1.3, `utcp-http` 1.1.11,
`utcp-text` 1.1.0.

## PR 1 — Forward compatibility

**Proposes:** a client that meets a `call_template_type` it has no plugin for
skips *that tool* with a warning and registers the rest; unknown keys are
ignored, not rejected, and preserved on rewrite; `x-` is reserved for
implementation extensions.

**Why.** Three measured behaviours of the reference client, in
`utcp-claims-verify.md` (§ *Extension path assessment*, claim rows 5 and 7) and
re-run here in `repro.py`:

* A manual with one valid `http` tool and one tool of an unknown type registers
  **zero** tools. `CallTemplateSerializer.validate_dict` raises, and the failure
  takes the valid tool with it.
* A manual carrying `info` — *the key the spec's own
  `docs/implementation.md` example uses, lines 31–34* — fails to load with
  `TypeError: UtcpManual.__init__() got an unexpected keyword argument 'info'`.
  The documented example is not loadable by the reference implementation. This
  is a strong argument to open with: it is the project's own inconsistency, not
  an outside request.
* An `x-` key on an `http` call template loads but is silently dropped, so
  there is today no extension point that survives a round trip.

Without this rule, adding one tool on a new protocol withdraws every tool a
provider already published, from every client without the plugin.

**Includes a code diff.** `python-utcp` is small enough: 6 files, +156/−13,
three new tests. Core suite 40 passed (37 + 3). Plugin suites 226 passed /
5 skipped / 61 errors — identical to the unmodified tree; those errors are
pre-existing `pytest-asyncio` fixture failures.

## PR 2 — Manual-carried response mapping

**Proposes:** an optional per-tool `response` object — `{language, expression}`
— applied to the decoded result before it is returned. `jq` as the first
registered language, `jaq` named as a compatible engine. Evaluation errors are
tool errors, never a silent pass-through of the raw result. A client that does
not support the declared language **refuses the tool loudly**; it must not
ignore the mapping.

**Why.** Response handling exists only as client-side `ToolPostProcessor`
config (`UtcpClientConfig.post_processing`), so a mapping cannot travel with the
integration. `UtcpManual` has exactly three fields and `Tool` seven; none is a
response field (`utcp-claims-verify.md`, claim row 2). The one thing a provider
is best placed to publish — how its envelope maps onto the `outputs` it already
documents — is the one thing the manual cannot carry, so `outputs` stays
decorative and every consumer rediscovers the mapping.

Spec-only. PR 2 depends on PR 1 (its "refuse one tool" rule needs PR 1's
per-tool skip); PR 1 stands alone.

## What Holon does meanwhile

Holon does **not** wait for either PR. Per D84.d the sidecar carries two
top-level sections (design write-up: `../discuss-D84-utcp-fork.md`, option (d)):

```yaml
utcp:            # a VERBATIM UTCP 1.x manual — importable and exportable unchanged
  utcp_version: "1.0"
  tools: [...]   # standard http call templates, ${VAR} secrets
holon:           # keyed by tool name; what the standard lacks
  commit-items:
    query: {version: "{version}"}
    body: {oldVersion: "{version}", device: {id: "{deviceId}"}, lang: en, commands: "{commands}"}
    response: ".version as $v | {version: $v}"
  poll_interval: 60s
```

The `utcp:` section is valid UTCP today — a user pastes in a published manual
and authors only the `holon:` section. `rs-utcp` is **not** adopted: it is
0.3.2 against a 1.x spec, and Holon's own
`crates/holon-mcp-client/src/rest_transport.rs` already does the envelope,
query, cadence and secret redaction it lacks.

The two designs converge if the PRs land. PR 1 makes the whole file a single
valid manual, with the `holon:` content moving into `x-holon` keys on each call
template. PR 2 then absorbs `holon.<tool>.response` into the standard
`response` field. Neither convergence step changes what a user writes.

## Exact commands to fork and open these

Nothing below has been run. Substitute your GitHub handle for `<you>`.

### 1. Fork on GitHub

```bash
gh repo fork universal-tool-calling-protocol/utcp-specification --clone=false --remote=false
gh repo fork universal-tool-calling-protocol/python-utcp          --clone=false --remote=false
```

### 2. Push the three branches

The branches already exist in the clones here, with the commits made.

```bash
cd /private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/utcp-upstream

git -C utcp-specification remote add fork git@github.com:<you>/utcp-specification.git
git -C utcp-specification push fork feature/forward-compatibility-rule
git -C utcp-specification push fork feature/manual-carried-response-mapping

git -C python-utcp remote add fork git@github.com:<you>/python-utcp.git
git -C python-utcp push fork feature/forward-compatible-manual-loading
```

Set the author on the commits first if the placeholder identity should not
appear upstream:

```bash
git -C utcp-specification rebase --exec 'git commit --amend --no-edit --reset-author' main
git -C python-utcp        rebase --exec 'git commit --amend --no-edit --reset-author' FETCH_HEAD
```

### 3. Open the PRs

`python-utcp` takes PRs against **`dev`** (its `CONTRIBUTING`: Gitflow, feature
branches off `dev`). `utcp-specification` has **no `dev` branch** — PRs go
against `main`.

```bash
gh pr create --repo universal-tool-calling-protocol/utcp-specification \
  --base main --head <you>:feature/forward-compatibility-rule \
  --title "docs: specify forward compatibility for unknown call template types, unknown keys and x- extensions" \
  --body-file PR-1-forward-compat.md

gh pr create --repo universal-tool-calling-protocol/python-utcp \
  --base dev --head <you>:feature/forward-compatible-manual-loading \
  --title "feat(core): skip tools with unknown call template types instead of failing the manual" \
  --body-file PR-1-forward-compat.md

gh pr create --repo universal-tool-calling-protocol/utcp-specification \
  --base main --head <you>:feature/manual-carried-response-mapping \
  --title "docs: add an optional per-tool response mapping carried by the manual" \
  --body-file PR-2-manual-carried-response-mapping.md
```

`--body-file` takes the whole document. Each PR file's *"PR description"*
section is the part meant for the PR body — trim the rest before posting, or
paste that section by hand.

Cross-link the two PR 1 halves once both numbers exist, and mention PR 2's
dependency on PR 1 in PR 2's body.

### 4. Re-run the evidence before posting

The `repro.py` numbers are what the PR bodies claim. Re-run them on whatever
`dev` is at on the day:

```bash
cd /private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/utcp-upstream
./venv/bin/python repro.py
./venv/bin/python -m pytest python-utcp/core/tests -q
```

Announce the project in the PR body if the maintainers ask who is asking; the
Discord in `docs/index.md` is where the maintainers discuss spec changes, and a
message there before opening PR 2 is likely to save a round trip — PR 2 adds a
field and a dependency, which is a design call they may want to make first.
