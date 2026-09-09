# Handoff — 2026-09-09 (orchestrator session bc7b1e67, wave 11 LANDED, wave 12 queued)

Resume from THIS file plus `followup-queue-2026-09-07.md` (same directory; the running log, newest at the bottom). Do not reread session transcripts.
Verify every load-bearing claim with the check command given before acting on it.

**Clock warning for the log.** In `followup-queue-2026-09-07.md`, timestamps from the `00:10` entry onward are ~6 h AHEAD of local CEST (orchestrator clock slip; the note is at line 228). Entries after that note are labelled `CEST` explicitly — trust those, and read an unlabelled `HH:MM` in that range as `HH-6:MM`.

## 0. Where the code is

| Thing | Where | Check |
|---|---|---|
| main | `8c8c564d5890` (= `main@origin`, wave 11 = 27 commits) | `jj log -r main@origin -T commit_id` |
| previous main | `830d794f878f` (wave 10) | `jj log -r 830d794f878f -T description.first_line()` |
| integration chain | empty — wave 11 fully landed; `integration` = `main` | `jj log -r 'main@origin..integration' --no-graph` |
| lane workspaces | `.claude/worktrees/<lane>/` (jj workspaces) | `jj workspace list` |
| this bundle | `docs/Handoff/2026-09-09/` | `ls docs/Handoff/2026-09-09` |
| verifier verdicts + lane reports (51 files) | `docs/Handoff/2026-09-09/reports/` | `ls docs/Handoff/2026-09-09/reports` |
| gate/land scripts as they ran | `gate-tip.sh`, `weave-gate.sh`, `land-gate-w11.sh`, `land-check-w11.sh`, `land-when-quiet-w11.sh` in this directory | `ls docs/Handoff/2026-09-09/*.sh` |
| lane brief rules handed to every lane | `lane-rules.md` in this directory | — |
| previous bundle | `docs/Handoff/2026-09-08/HANDOFF.md` | — |

## 1. Landed state — wave 11, `830d794f878f` → `8c8c564d5890`

Landed 2026-09-09 ~09:40 CEST via `sw land --push --check land-check-w11.sh`. Land gate r3: battery PASS, clippy green (D106.a), `holon`/`holon-app` 726/731 with the 5 registered `e2e_backend_engine_test` matview reds.

Listed newest first, exactly as `jj log -r '830d794f878f..main'` prints them. Check any row with `jj log -r <id> -T description.first_line()`; the owning lane's report is in `reports/`.

| Commit | Subject | What / why |
|---|---|---|
| `8c8c564d5890` | docs(featuremap): regenerate for the 77-transition keystone alphabet | Land gate r2 went red on feature-map drift (76→77 transitions after the `Reboot` transition). Doc-only regeneration at the tip. |
| `45f1103ae26a` | chore(lint): silence dead_code on the shared shutdown fixture | Land gate r1 red at the clippy step: the session-shutdown test harness structs have never-read fields. `allow` with a reason ×2, `_`-prefixed ×2, plus `arc_with_non_send_sync` allows in `wide_e2e.rs`. |
| `e6ef710565ed` | fix(org): keep an empty link literal's bytes at the parse boundary | D110.a. `[[]]` was silently erased on parse (pre-existing since 2026-07-05); it now survives as plain text. Three erasure pins updated. |
| `9fc552d175dc` | test(keystone): add the Reboot transition and its persistence model | Keystone Inc 2. Reboot replays through `shutdown_session`, deterministic replay un-parked. Env-gated OFF for wave 11 per D104.a. |
| `686ec60895ed` | test(keystone): split the headless component's store from its boot | Keystone Inc 1 — the store must outlive one boot for `Reboot` to mean anything. |
| `d65ea94c146a` | fix(gpui-pbt): wait for a painted frame before resolving entity bounds | The windowed driver resolved bounds before paint, so clicks landed nowhere. `holon-gpui` deterministic reds 16→12. |
| `3ca3f3f80f51` | fix(frontend): seat the caret in the destination on navigation | D97.a with headless teeth. The empty-destination half is PARKED and disclosed, pending D109. |
| `9a882e53f359` | fix(app): stop session watchers before the store closes | Orderly `SessionShutdown` + `holon_app::shutdown_session` over 13 task families; reboot orphan errors 4–15 → 0. |
| `e6f5f9de40de` | fix(kitchen): parse German timers with a units file for the guest | D100.a. German recipe timers were unparsed; a German units file in the cooklang guest, red-first differential test. +215 KB accepted per D107.a. |
| `046a7c7e4a74` | chore(lint): make clippy, fmt and machete green workspace-wide | The `clippy-tip` sweep: 11 sites / 13 files, `PageAncestor` boxed, a dead `doc_store` field and ctor param removed. |
| `1b64883201f5` | chore(lint): clear 11 clippy reds so the workspace gate exits 0 | The pre-existing `holon-api` reds (54 `double_must_use` from `#[async_trait]`, 1 `large_enum_variant`) that D106.a promotes to a land-gate step. |
| `4306ba0b1a1a` | docs(known-reds): opentab-sql-reads-budget matches any pinned ceiling | The known-reds row was ceiling-specific and stopped matching after a budget change. |
| `6558215f7209` | chore(tree): delete the lane-local gate helper scripts leaked into scripts-lane/ | D108.b. 14 leaked `scripts-lane/*.sh` removed; the tree says what it is. |
| `4732dcfa725a` | fix(org): rank priorities A-first and stop erasing drawer-authored ones | D101.a. `Priority(u8)` typed at the parse boundary, A-first ordering, toon reader parity, migration, round-trip PBT for org byte-stability. |
| `bb4a587af1ee` | docs(testing): register the 58 holon-gpui reds and propose its gate | All 15 attributed reds were pre-existing on main; `holon-gpui` was never in a land gate. `docs/Testing/GpuiCrateReds-2026-09-10.md`. |
| `3fce037d5350` | test(keystone): pin the read-only-home write boundary | `ReadOnlyMembers` + one `file` UPSERT carrying `read_only_blocks`. Landed with the disclosure per D105.a. |
| `0c155047c032` | fix(sharing): gate the subtree-share lifecycle on an enrollment roster | Keychain `ShareCredentials`, roster, revoke, a tmp-path race fix, `doc()` escapes consolidated. |
| `fcae936b1539` | fix(ingest): one filter decides what the vault contains | D102.a. Two files claiming the same block `:ID:` now refuse the whole second file loudly; `VaultFilter` replaces the scattered predicates. |
| `f972dd37de64` | fix(plaintext): refuse a 0-byte file only where it can delete blocks | A 0-byte write during an editor save was wiping a page. |
| `e64e29216a19` | fix(plaintext): disclose why routing broke and when a file stays empty | Fail loud, never fake: the routing failure was silent. |
| `7e00a965f01e` | perf(projection): install the Loro subscription before the org scan | Boot-path quadratic: 121 → 1 full walks, projection p50 258 → 8 ms, boot 110 → ~50 s on a vault copy. |
| `91b1501d4016` | feat(lowcode): the cooklang plugin is the only parser, and the scan is incremental | D90.a — `cook.rs` deleted, content-hash incremental scan. |
| `ec6ae4c7431a` | test(shopping): pin the DEL fixture's tombstone relative to now | An absolute tombstone date made the fixture expire. |
| `6452ac2dc083` | fix(frontend): make every accent-filled quick-open row legible | D96.a — overlay-local selected-subtitle token and colour in the layout record. |
| `0d28c42396e3` | docs(arch): sync @c4 uses arrows, ratchet archlint, de-churn baseline | Architecture docs drifted from the code; the ratchet stops it recurring. |
| `5644b6d3debf` | fix(sharing): gate every iroh peer import on an admit decision | D86.a. |
| `3ad044011f21` | build(gate): fail the landing gate on generated-doc and architecture drift | The gate that caught the feature-map drift above. |

## 2. Rulings applied this session

Memory: `rulings-2026-09-09-d103-d108`, `rulings-2026-09-10-d98-d102`.

- **D98.b** — the D97 caret fix stays small (one writer seats the caret, no `nav_root` field) AND the fleet-wide rename `focused_block` → `caret_block` is wanted (276 occurrences / 66 files). Sequence: a mech lane on the chain tip AFTER the caret lane weaves. **Not done — queued for wave 12.**
- **D100.a** — cooklang `ADVANCED_UNITS` stays ON (it always was; D91.a SUPERSEDED). German timers close via a German units file in the guest with a red-first differential test. → `e6f5f9de40de`.
- **D101.a** — org priority is fixed at the parse boundary: canonical typed value sorting A-first, a migration re-projects existing blocks, a round-trip PBT proves byte-stability, the vault `now-query` keeps `ORDER BY priority`, ONE bugfunnel entry. → `4732dcfa725a`.
- **D102.a** — two ingested files claiming the same block `:ID:`: refuse the WHOLE second file (walk order decides), loud degraded signal naming both paths and the slug. → `fcae936b1539`.
- **D103.a (+ note)** — the owner recovery code may stay unshown, but it must be STORED in secret storage, not dropped. Answer to the note: `crates/holon-secrets` (`KeychainStore`) already backs `ShareCredentials`. Follow-up lane `recovery-code-keychain` (security-executor). **Queued.**
- **D104.a** — `HOLON_PBT_REBOOT` stays OFF for wave 11; flip ON in wave 12 after the `reboot-reds-triage` lane A/Bs the remaining red at reboot weight 40 vs main. The `[[]]` half is fixed by `e6ef710565ed`.
- **D105.a** — readonly-invariant lands with the disclosure; `loro-import-origin` is the FIRST lane of wave 12.
- **D106.a** — `cargo clippy --workspace --all-targets -- -D warnings` is now a LAND-gate step, check-only, never `--fix`.
- **D107.a** — accept the +215 KB cooklang guest; `cook-units-buildrs` mech follow-up turns `german.toml` into a build-time literal. **Queued.**
- **D108.b** — delete the 14 leaked `scripts-lane/*.sh` in a one-commit hygiene lane. → `6558215f7209`.
- **D110.a** — the `[[]]` empty-link literal survives as plain text. → `e6ef710565ed`.

Why, in one line: fail loud never fake (D100, D102, D103, D110), parse-don't-validate (D101), gates that stay green between waves (D106), the tree says what it is (D108), keep the chain moving with disclosed gaps (D105, D107).

## 3. OPEN — D109: order birth before the first keystroke's write

Full memo, both rounds: `reports/d109-petri-net-ordering.md`. Read it before acting; the summary below is a pointer, not a substitute.

**The bug.** One keystroke on a caret seated in an empty destination expands into two effects — the block comes into existence, and its content becomes the typed character. Birth is spawned fire-and-forget (`crates/holon-frontend/src/reactive.rs:3039-3048`) while the write dispatches independently, so on the Loro leg `set_field` reads prior state first and returns a hard `Block not found`. It is a happens-before, not a merge — no CRDT algebra repairs it.

**Round 1 (option e).** The precondition is undeclared: `set_field` declares `existence = untouched` (`crates/holon-core/src/traits.rs:619-622`) where `create` declares `produces`. Recommendation was a dispatcher in-flight-births register acting as the Petri net's `in-flight(id)` place, plus `set_field` re-declared `existence = reads`, plus a small ADR 0032 amendment. Rejected alternatives: a per-entity FIFO does not fix it (it orders what has arrived, and the create is spawned), and a synchronous create is deadlock-shaped because `edit_target_id` is sync.

**Martin's objection.** "Dispatcher state works around the PN — what prevents the PN itself via its ordinary mechanisms?"

**Round 2 (option f, current recommendation).** The objection lands. ADR 0032 §6's firing axiom licenses Martin's design and forbids round 1's: a blocked rule re-evaluates when the token is released and never queues a firing to replay later — which is exactly what a dispatcher register does. The net-native shape: a durable base table `pending_firing` carrying `create-requested` / `write-requested` tokens, `creating(id)` → `exists(id)` places, and one CDC-woken firing loop that dispatches through `execute_operation` when both tokens are present. `birth_creation_affordance` stops spawning and forgetting; `edit_target_id` stays sync. `set_field` still must declare `existence = reads`. It is a *base* table, not a matview, so `query_and_watch` over it is CDC-eligible with no chained matview — the same escape hatch `clock` gives the rule watcher.

**The decisive tradeoff.** The net-native design forces the firing request to become durable state (ADR 0032 §1 leaves no other option): a schema, a watcher, and dedupe discipline on the keystroke path. The register avoids all three and works, but it is unobservable, un-analysable, lost on restart, and will be reinvented at the next such race.

**What Martin must decide.** Whether D109 may grow from a race fix into ADR 0032's next increment — the durable firing-request marking and its loop — or whether that increment is scheduled separately and D109 ships an explicitly-labelled stopgap now. The memo recommends the former, scoped to existence arcs only (`set_field`, `delete`, `move_block` against an id with an in-flight create); everything else keeps dispatching directly.

**Red-first pin, already authored.** The parked empty-destination replay in `hand-authored-regressions/keystone.jsonl`, driven by `TypeChars` through `birth_block_via_creation_slot` (`crates/holon-integration-tests/src/pbt/transitions/type_chars.rs`). Red today for the right reason on the `["Loro","Turso"]` wiring.

**Unresolved without a build:** whether `query_and_watch` over a new base table is free of the chained-matview wall in Loro mode, what the CDC wake latency is on the keystroke path, and why the windowed GPUI rung is green (the per-keystroke Loro cell path bypasses the dispatcher per ADR 0032 §3; the lane's own claim was retracted).

## 4. Queued wave-12 lanes

From memory `night-2026-09-09-wave11-landed` and the CEST-labelled log entries.

Ordered where the order is fixed:

1. **loro-import-origin** — FIRST per D105.a. Loro import must emit `OpOrigin::Sync`; red-first PBT on a read-only member arriving via pair sync.
2. **recovery-code-keychain** — D103.a, security-executor. Store the owner recovery code under its own keychain entry; the degraded signal becomes "stored, not shown".
3. **caret-rename** — D98.b, mech lane. `focused_block` → `caret_block` across holon-frontend, pbt-core capabilities, the ref model, MCP and the drivers. Do it on a quiet tip; it conflicts with everything live.
4. **D109 fix lane** — blocked on the ruling in §3.

Unordered:

- **reboot-reds-triage** — D104.a remainder. A/B the "org render is DEGRADED … NO emission settles" red at reboot weight 40 vs main, then flip `HOLON_PBT_REBOOT` ON.
- **cook-units-buildrs** — D107.a. Turn `german.toml` into a build-time literal via `build.rs`/`OnceCell`.
- **holon-priority** — extract the typed priority into a shared crate.
- **diff-params-sentinel** — `task_state` / `scheduled` / `deadline` share the `parent_id` defect: emitted only when old ≠ new, so the receiver's reseed stays load-bearing.
- **panel-blank-frame** — the main panel's `ReactiveShell` is re-created EMPTY for ≥1 frame per projection rebuild; plus the oracle hole, `assert_content_fidelity` is gated on `total_descendants > 0`. Prod defect recorded, not fixed.
- **windowed-split-caret** — `surface_chars_before_content` is unimplemented on BOTH drivers, so windowed SplitBlock caret never worked. Feature lane.
- **file-projection-port-seam**
- **lint-jscpd-config**
- **readonly-cook-fastpath**
- **sql-sink-latency** — the navigate SLO dominator is now `consolidator.apply`'s SQL sink write at 193/201 ms. Wall-clock rungs cannot gate under load; a quiet-machine 3-rung run is queued with it.
- **G2 mirror-table reconcile** — carried from wave 9: `QueryableCache::initialize_schema` never reconciles an existing mirror table with a changed sidecar schema.
- **dogfood-explorer rotation** — Martin's standing order: settings → rules → newest feature → visual.

## 5. Landing recipe as it actually ran

1. **Per-lane weave gate.** `sw weave <lane> -m "<msg>" --check "bash weave-gate.sh <lane>"`, run from `_sw_integ`. Every case starts with a sentinel grep (`WRONG TREE` → exit 9), then a crate list, then extras. Two rules learned the hard way this session: never a bare `-p holon-integration-tests` (name a `--test <binary>`), and `hand` (i.e. `just hand-authored`) is the DEFAULT for any lane whose diff touches `crates/`.
2. **`gate-tip.sh <lane>`** — re-gate at the chain TIP after every rebase, conflict resolution or squash. Every wave-11 commit was gated at the tip it ended on, not the tip it was written against.
3. **`land-when-quiet-w11.sh`** — polls `sysctl -n vm.loadavg` and `pgrep -x rustc` every 120 s, runs the land gate only at `load1 < 10 && rustc < 3`. Keep the `(pgrep -x rustc || true)`; under `set -euo pipefail` a rustc count of 0 kills the loop. Launch it nohup-detached with TERM/HUP ignored.
4. **`land-gate-w11.sh`** — 13 lane sentinels, a token-shaped-segment scan, `cargo clippy --workspace --all-targets -- -D warnings` (D106.a), `just landing-gate` under `sem --id holon-build -j4` in parallel with `cargo nextest run --no-fail-fast -p holon -p holon-app`, and the widened KNOWN allowlist. Anything outside the allowlist is NOVEL and blocks.
5. **`land-check-w11.sh`** — the binding check at land time; refuses unless the tree is still the measured one (tip commit, lane sentinels, the `== landing gate PASS ==` marker in the newest battery log, and the exact test counts). Its allowlist is narrower than the gate's.
6. **Land.** `sw land --push`, confirm `main@origin` moved, `jj new main` in the session repo and every landed workspace, purge landed lanes' `target/`.

It took three land-gate rounds: r1 red at clippy (`45f1103ae26a`), r2 red at feature-map drift (`8c8c564d5890`), r3 green.

## 6. Hazards learned (orchestrator `IMPROVEMENTS.md`, entries dated 2026-09-09)

- **`sw weave <stream>` places into the stream's REGION, not at the tip.** Re-using `hygiene` for a land-gate clippy fix put the commit below 22 chain commits whose files it edits did not exist there → the commit and 11 descendants conflicted. A chain-tip fix needs `sw new <fresh-name> --base <tip>`. Recovery is `jj rebase -r <fix> -d <tip>` + `jj bookmark set integration -r <fix>` + re-gate.
- **Chained weave scripts keyed on predecessor LOGS fire out of order.** An appended (not truncated) log kept an old RED and held every downstream script; a GREEN from a different run released a lane early, so two `sw weave` gates ran concurrently in `_sw_integ` for 20 minutes (both invalid). Build ONE serial `weave-queue.sh` with per-lane `verified.ok` markers and a flock on `_sw_integ`.
- **A weave-gate `case` arm below the `*)` default reports "unknown lane" and the weave proceeds ungated** — `sw weave --check` exits 0 on a red gate and still advances `integration`. Second occurrence.
- **Editing a gate script while a gate is running it** shifts lines under the running bash and produces a syntax error after every build step passed. Run a per-run copy.
- **A "no reply needed" broadcast SendMessage RESUMES a completed agent.** One kept editing a workspace already assigned to its successor for ~40 minutes. TaskStop before handing a workspace on; stopping is what prevents resumption.
- **Build-slot wrapper plus an inline nextest filter with parens = 0 tests run, exit 0.** Gates must be script FILES, never inline filter expressions through the wrapper. Also observed: GNU parallel's semaphore is not FIFO — one lane waited 2 h.
- **11 concurrent Opus lanes tripped the API session limit and every lane died mid-build**, leaving in-flight builds and one probe-revert half-done. Cap frontier-tier agents at ~6–8 per session window and stagger resumes; a lane's resume message must say restore-first.
- **wasm checks belong in every lane gate touching a wasm-compiled crate.** `ingest-dupslug` passed check/nextest/smoke/hand-authored, wove, and then failed the CHAIN gate's `just check-worker-wasm`.
- **`-p holon-integration-tests` is never a gate crate** — it holds env-bound suites (live_mcp, latency_slo, logseq import, two-instance pairing, span capture) that red outside their harness. Cost this session: ~2 h of serial gate time across two lanes.
- **Swap is 0 bytes on this Mac**, so under memory pressure the kernel SIGKILLs test processes instead of swapping. A killed test is not a red.
- **`find` resolves to `bfs` here** and rejects relative `-newermt` strings, so a heartbeat using it silently reports `recent-files=0`. Use `-mmin -N`.
- **A chain-tip flake attribution expires at the next weave** — name the tip commit it measured.
- **Lanes keep dropping helpers into `scripts-lane/`.** It is deleted from main; lane-rules now say helpers go under `<workspace>/lane-logs/`.

## 7. What is pushed

`main` = `8c8c564d5890` on origin (wave 11, 27 commits, pushed 2026-09-09 ~09:40 CEST). `integration` sits at the same rev. This bundle is written in the `hygiene` workspace and is unpushed until the orchestrator commits it and pushes `wip/handoff-2026-09-09`.
