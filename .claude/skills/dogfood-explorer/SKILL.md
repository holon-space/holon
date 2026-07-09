---
name: dogfood-explorer
description: Exploratory test-drive the running Holon GPUI desktop app through its embedded `holon` MCP server to hunt platform/wiring/visual/latency defects the headless keystone PBT structurally cannot see. Use when asked to dogfood, exploratory-test, or manually drive Holon, or to reproduce a UI/platform bug against a live instance. Every finding is triaged with the `bug-gap-triage` skill and appended to docs/Testing/BugFunnel.md.
---

# Dogfood Explorer

The headless keystone PBT (`crates/holon-integration-tests/tests/general_e2e_composed_pbt.rs`)
cannot see platform-only code paths, embedder-wiring divergence, visual/UX defects, or real-latency
regressions. The 2026-07 escape audit was **ENVIRONMENT 12 · COVERAGE 7 · PERCEPTION 2 · ORACLE 0**
— this channel attacks exactly the gaps that dominate. You drive the *real* GPUI app through its
embedded MCP server and cross-check rendered state against internal state.

Pairs with `bug-gap-triage` (`.claude/skills/bug-gap-triage/SKILL.md`): every finding gets
classified and written to `docs/Testing/BugFunnel.md`.

Proven end-to-end 2026-07-07: found 1 P1 (block-create panics, swallowed), 1 undo-ordering bug,
2 fresh-boot data bugs, 2 visual bugs in a single ~30-min session. Session evidence:
`logs/dogfood-session-2026-07-07/` (screenshots + app logs).

---

## 0. SAFETY — read before you launch (data-loss hazards)

Holon resolves its data locations from config/env. The DEFAULTS point at Martin's REAL data.
You MUST override all three or you will drive live data:

| Concern | Default (DANGER) | You MUST pass |
|---|---|---|
| Config dir (holon.toml + default db) | `~/.config/holon` (real config, real `holon.db`) | `HOLON_CONFIG_DIR=<throwaway>` |
| Vault root (org/md files synced) | from real holon.toml | `HOLON_VAULT_ROOT=<throwaway>` |
| MCP server port | `8520` = **Martin's LIVE instance — NEVER touch** | `MCP_SERVER_PORT=8620`+ |

Verified in code (re-check if these files changed):
- `crates/holon-frontend/src/config.rs::resolve_config_dir` — no override → `~/.config/holon` (macOS).
  `resolve_db_path` → `{config_dir}/holon.db`. `VaultConfig.root` ← `HOLON_VAULT_ROOT`/`--vault-root`.
- `frontends/gpui/src/di.rs` — MCP port from `MCP_SERVER_PORT`, `unwrap_or(8520)`.

**Gotchas found live:**
- `HOLON_CONFIG_DIR` pointing at a dir without `holon.toml` is a fail-loud config error —
  `touch $CONFIG_DIR/holon.toml` first (empty file is fine).
- The first stderr line prints `Config dir: …, db: …, orgmode: …` — READ IT and verify every path
  is inside your sandbox. This is the last safety gate.
- Teardown kills the PID that owns your port — NEVER `pkill holon-gpui` (kills the live instance).

**Rule:** port >= 8620, config-dir + vault-root under /tmp, verified in the boot line — or do not launch.

---

## 1. Launch protocol (verified working 2026-07-07)

```bash
WS=<absolute path to your holon worktree>
PORT=8620                                    # >=8620; check: lsof -iTCP:$PORT -sTCP:LISTEN
SANDBOX=$(mktemp -d /tmp/holon-dogfood.XXXX)
mkdir -p $SANDBOX/config $SANDBOX/vault $SANDBOX/logs $SANDBOX/shots
touch $SANDBOX/config/holon.toml             # REQUIRED — missing file = fail-loud boot error

# 1. Build debug (NOT --release: debug_assertions gates inspector/reset paths).
export CARGO_BUILD_JOBS=4
bash -c "cd $WS && cargo build -p holon-gpui > $SANDBOX/logs/build.log 2>&1"   # ~40 min cold
grep Finished $SANDBOX/logs/build.log

# 2. (Optional) seed $SANDBOX/vault with .org files — see §3. Empty vault = default-layout seed.

# 3. Launch WITH latency logging (without RUST_LOG the holon_latency events never emit).
bash -c "cd $WS && RUST_LOG=info,holon_latency=debug \
  HOLON_CONFIG_DIR=$SANDBOX/config HOLON_VAULT_ROOT=$SANDBOX/vault \
  MCP_SERVER_PORT=$PORT HOLON_MCP_ALLOW_RESET=1 \
  nohup ./target/debug/holon-gpui > $SANDBOX/logs/app.log 2>&1 &"

# 4. Wait for MCP, then record the REAL PID **by port** ($! is the wrapper shell, not the app):
for i in $(seq 1 40); do curl -s -m2 http://127.0.0.1:$PORT/health | grep -q OK && break; sleep 2; done
lsof -tiTCP:$PORT -sTCP:LISTEN > $SANDBOX/app.pid
head -1 $SANDBOX/logs/app.log                # VERIFY: all paths inside $SANDBOX

# 5. The window opens MINIMIZED when launched via nohup — screenshots come back blank/failed
#    ("minimized=true"). Un-minimize + front it:
osascript -e "tell application \"System Events\"
  set p to first process whose unix id is $(cat $SANDBOX/app.pid)
  set frontmost of p to true
  repeat with w in windows of p
    try
      set value of attribute \"AXMinimized\" of w to false
    end try
  end repeat
end tell"
```

Driving the MCP: use `holon_mcp_cli.py` (beside this SKILL) — a stdlib+`requests` streamable-HTTP
client that does the initialize handshake per call:
```bash
python3 holon_mcp_cli.py $PORT --list
python3 holon_mcp_cli.py $PORT describe_ui '{"block_id":"block:root-layout"}'
python3 holon_mcp_cli.py $PORT screenshot '{}' --out $SANDBOX/shots/01.png
```
Do NOT use a harness-configured `holon` MCP tool unless you are certain which port it targets
(the default config targets 8520 — the live instance).

**Shell caveat:** the harness Bash tool may run under **nushell**, not bash. `$PORT`/`$SANDBOX`
shell vars and the bash idiom `CLI="python3 …"; $CLI describe_ui …` (var-as-command) do NOT
expand in nushell — the whole var is treated as one command name. Either invoke `python3
<abs-path>/holon_mcp_cli.py <port> …` directly each call, or wrap a block in `bash -c "…"`.
The CLI prints a `NotOpenSSLWarning` on stderr; filter with `2>&1 | grep -v NotOpenSSL` when
piping to a JSON parser, or read stdout only with `2>/dev/null`.

---

## 2. Session protocol (per exploratory step)

1. **Known state.** Fresh launch on an empty/seeded vault IS the known state. `reset_vault` only
   exists on branches with the iOS Option-A reset (check `--list`); otherwise relaunch = reset.
   Note: the app WRITES BACK seeded pages into the vault (`__default__.org`, `Journals.org`
   appear on disk after first boot) — a "reused" sandbox vault is not pristine.
2. **Observe before acting.** `describe_ui {"block_id":"block:root-layout"}` for the whole layout;
   `block:default-main-panel` / `block:default-left-sidebar` for panels. Use `format:"json"` and
   harvest `entity_id`s for clicking. `describe_ui` on an arbitrary block id may return
   `(loading)` — describe the enclosing panel instead.
3. **Act via entity ids.** `click {"entity_id":…, "region":"main"|"left_sidebar"}` (self-verifying
   hit-test; a click places the caret and focuses the editor — observed caret lands at text end).
   `type_text {"text":"…"}` sends per-char keystrokes to the FOCUSED editor; special names
   (`"enter"`, `"backspace"`, `"escape"`, `"tab"`) are single keys.
   `send_key_chord {"entity_id":…, "keys":["shift","tab"]}` for chords — it reports
   `"No handler matched"` for unbound chords (e.g. cmd+left is NOT bound; don't assume
   platform-standard editing chords exist).
   `execute_operation {"entity_name":"block","operation":"…","params":{…}}` (field is
   `operation`, not `operation_name`) when no editor is focused.
4. **Fresh-boot trap (root-caused 2026-07-09).** The empty main panel shows a creation slot
   `block:__virtual:<panel-id>` — clicking it + typing + Enter builds a `create` op with NO `id`,
   and `sql_operation_provider.rs` create branch `.expect("create: missing 'id'")` panics on a
   `tokio-rt-worker` the UI silently swallows (the typed block vanishes). The SAME root fires the
   journal auto-create Rhai action at boot (`block.create(#{parent_id, name})`, no id). `split_block`
   is unaffected — it mints a uuid before dispatch. FIX (this dogfood session): the create branch now
   mints `{entity}:{uuid}` when `id` is absent, so slot-commit and action create both work. If you're
   on a build BEFORE that fix, create the first block via `execute_operation` create with an explicit
   `id`, or seed the vault. Typing into EXISTING blocks always works.
5. **Verify BOTH surfaces after every mutating step:**
   - Rendered: `describe_ui` (+ screenshot for anything visual).
   - Internal: `execute_raw_sql` / `execute_query`. NOTE: `task_state`/`task_state_category` are
     inside the `properties` JSON column, not top-level columns — `SELECT *` and inspect.
     TABLE CHOICE: query the **`block`** view (has `tags`, `requires`, `advice_suppressed`) for
     page/tag/edge inspection — NOT `block_raw` (the base table lacks those columns, and selecting
     them fails with the swallowed "Failed to execute raw SQL" error). `SELECT id, content, tags,
     parent_id, sort_key FROM block ORDER BY parent_id, sort_key` gives the whole forest.
     Disk: read `$SANDBOX/vault/*.org` directly (the `read_org_file` MCP tool takes `doc_id`,
     not a path).
   - **Divergence between the two is itself a bug** — e.g. sidebar rendering 3 rows where its
     backing SQL returns 2 (found live: phantom `__virtual:` row).
6. **After EVERY step, grep the log:** `grep -E "PANIC|ERROR" $SANDBOX/logs/app.log` — the UI
   swallows engine panics silently; the log is the only place they surface. A user-visible
   failure with no banner + a PANIC line = a fail-loud violation to report.
7. **Latency.** With `RUST_LOG=…,holon_latency=debug`, run
   `python3 $WS/scripts/measure_latency.py $SANDBOX/logs/app.log`. SLO: p95
   interaction→projection-visible < 200ms. CAVEAT (updated 2026-07-09): prod desktop now emits an
   `e2e` (interaction→visible) stage for `set_field` — `measure_latency.py` reports it under
   "PROD END-TO-END". But `split_block` still emits only `dispatch` (no `e2e`), and the named
   `projection` stage never fires, so e2e coverage is partial; report the `e2e` p50/p95 where
   present and the `dispatch` p95 elsewhere, and note which stages were absent. (`action_total`
   is harness-only.) At tiny fresh-boot scale expect single-digit ms (e.g. set_field e2e p95 ~8ms);
   latency bugs need vault scale to surface.

## 3. Exploration heuristics — vary seeds, replay known-breakers

**Seeds (fresh sandbox per seed):**
- Empty vault → default-layout seed path (first-boot seeding, journals, sidebar SQL).
- Org-seeded vault: `.org` files in `$VAULT` before launch. A heading with `:ID: root-layout`
  becomes THE layout (suppresses the default). Bare IDs without `block:` prefix
  (docs/Reference/ORG_SYNTAX.md). `:ID: foo` → `block:foo` in SQL.
- Deep trees (5+ levels), unicode (CJK/emoji/combining), long unwrappable lines, mid-word `_`.

**Sequences that historically break things (replay each):**
- Indent/outdent chains (send_key_chord tab / shift+tab). Indent with no previous sibling
  silently no-ops (verify parent_id in SQL, not just the tool's reply).
- Split (Enter mid-text) + join (Backspace at start), then **undo** — found live: undo SKIPPED
  the join and undid an older outdent instead (join not on the undo stack).
- Task-state cycling (`cycle_task_state`) — verify BOTH `task_state` and `task_state_category`
  in properties, and the org-file keyword on disk.
- Navigation mid-propagation: click page A→B and `describe_ui` IMMEDIATELY (no settle), then
  again after 300ms — stale rows are a known open bug class (may need vault scale to reproduce).
- Undo/redo after every structural op — check the RIGHT op was undone (see join finding).
- Page delete + sidebar refresh (keystone never deletes pages).

**Transients are in scope.** The keystone settles-then-asserts; a wrong render that self-heals is
still a finding. Screenshot + describe_ui before it settles.

## 4. Bug handling — triage every finding

Run `bug-gap-triage` per finding; produce rows ready for `docs/Testing/BugFunnel.md`:

```
| YYYY-MM-DD | <one-line bug> | <COVERAGE|ORACLE|ENVIRONMENT|PERCEPTION> | <secondary|—> | <missing piece> | <status> |
```

Litmus: COVERAGE = keystone can't generate the interaction · ORACLE = generatable but no invariant
flags it · ENVIRONMENT = failing path/wiring/timing/platform absent from the test env ·
PERCEPTION = visual/UX, no formal invariant possible. Latency-over-budget is ORACLE or
ENVIRONMENT, never PERCEPTION. Then attempt keystone repro (CLAUDE.md rule) or name the parity
work, and update the distribution line at the top of BugFunnel.md.

**Report deliverables:** what you tried (seed + call sequence), what you observed (rendered vs
internal vs disk, screenshots, latency table), triaged rows, DISCLOSED CASUALTIES (unverifiable
observations, tool errors, flakiness).

## 5. Teardown (always)

```bash
kill $(cat $SANDBOX/app.pid)     # PID from lsof-by-port — NEVER pkill (8520 = live instance)
# verify: lsof -tiTCP:8520 -sTCP:LISTEN still listening (live app untouched)
# copy evidence out of $SANDBOX (screenshots, app.log), then:
rm -rf $SANDBOX
```

## Tool surface quick reference (verified live 2026-07-07)

Drive: `describe_ui{block_id,format}` · `click{entity_id,region|x,y}` · `type_text{text,modifiers}` ·
`send_key_chord{entity_id,keys[]}` · `send_navigation{from_entity_id,direction}` ·
`scroll{entity_id|x,y,dx,dy}` · `screenshot{window_title?}` (fails with "minimized=true" detail if
the window is minimized) · `execute_operation{entity_name,operation,params}` · `execute_command` ·
`list_operations`/`list_commands` · `undo`/`redo`/`can_undo`/`can_redo`.
Introspect: `execute_query{query,language,…}` · `execute_raw_sql{sql}` (error detail is swallowed —
"Failed to execute raw SQL" with no cause; iterate on the SQL) · `diff_loro_sql` ·
`inspect_loro_blocks{doc_id}` · `list_loro_documents` · `read_org_file{doc_id}` ·
`render_org_from_blocks` · `watch_query`/`poll_changes`.
NOT present on all branches: `reset_vault` (check `--list` first).
