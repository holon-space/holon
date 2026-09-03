# Holon project task runner

set dotenv-load

# GPUI/blinc builds link libfontconfig on Linux; RUST_FONTCONFIG_DLOPEN=on makes the
# fontconfig -sys build script dlopen it at runtime instead of static-linking (needed
# for fresh clones/CI/jj workspaces). Previously lived in an untracked .env; tracked
# here so every checkout inherits it. `export` puts it in recipes' environment.
export RUST_FONTCONFIG_DLOPEN := "on"

# --- Canonical compile shape (D85.c) ----------------------------------------
# Every host-profile gate MUST resolve the same package set and the same feature
# set. Cargo's build-dir name is a metadata hash over the resolved features, so a
# recipe that spells its selection differently mints a fresh
# `target/debug/build/<crate>/<hash>/` for every target it touches and abandons
# the old one forever — one off-shape `cargo check` was measured adding 711 unit
# directories, and holon-app reached 878 of them for 34 test targets.
#
# Narrow what RUNS with `--test <name>` or nextest's `-E`, never with `-p`: `-p`
# narrows the feature resolve too, which is the churn. `--workspace` also
# satisfies D64.a for free (the `holon` crate's tests only compile alongside
# holon-app).
#
# `just target-gc` reclaims whatever churn predates this.
CANON_FEATURES := "holon-integration-tests/pbt,holon-integration-tests/web-arm,holon-gpui/pbt"
CANON := "--workspace --features " + CANON_FEATURES

# Off-shape by necessity, each for a reason no variable can absorb:
#   * `cargo run` / `cargo watch` app launches — `cargo run` rejects `--workspace`.
#   * wasm and android checks — a different `--target` triple owns its own
#     target subdirectory, so it cannot share a hash with the host profile.
#   * out-of-workspace manifests (holon-worker, dioxus-web) and `cargo mutants`.
#   * scripts/capability-cert-native.sh, which needs `holon/test-helpers`;
#     folding that into CANON_FEATURES would change what every gate compiles.

# Gate/check recipes write their logs to `target/gate-logs/<name>.log`, never to
# a fixed /tmp path: `target/` is per jj workspace, so parallel lanes no longer
# overwrite one another's verdict in a shared file (a lane was once observed
# reading another workspace's build result as its own). Basenames are unchanged,
# so `holon-build.log`, `pbt-general.log` etc. stay recognizable.

# List available recipes
default:
    @just --list

# --- Setup ------------------------------------------------------------------

# Install cargo plugins used by this workspace (idempotent, uses cargo-binstall).
setup:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-binstall >/dev/null 2>&1; then
        echo "Installing cargo-binstall..."
        cargo install cargo-binstall
    fi
    cargo binstall --no-confirm \
        cargo-llvm-cov \
        cargo-crap \
        cargo-deny \
        cargo-machete \
        cargo-mutants \
        cargo-ndk \
        cargo-nextest \
        cargo-watch \
        samply
    # polydup: cross-language duplicate detector. Pinned to nightscape's
    # incremental-rolling-hash fork (no crates.io / binstall release yet).
    cargo install --git https://github.com/nightscape/polydup-fork \
        --branch perf/incremental-rolling-hash polydup
    # archidoc: architecture-IR compiler — scans the tree into JSON, renders
    # C4 diagrams, and validates implementation against intended design.
    # Pinned to nightscape's fork `dev` branch, which bundles three upstream PRs
    # (GitSmart86/archidoc #4 crate-root lib.rs attachment, #5 paragraph
    # descriptions, #6 .gitignore/hidden-dir aware walking). No crates.io release.
    cargo install --git https://github.com/nightscape/archidoc \
        --branch dev archidoc-cli
    rustup component add llvm-tools-preview
    echo ""
    echo "Setup complete. Try: just analyze"

# --- Architecture (archidoc) ------------------------------------------------

archidoc_baseline := "docs/Architecture/baseline"

# Compile crate + frontend architecture IR into _context/ (gitignored).
arch-compile:
    archidoc ir compile "{{justfile_directory()}}/crates"    --output-dir _context/crates
    archidoc ir compile "{{justfile_directory()}}/frontends" --output-dir _context/frontends

# Regenerate the committed @c4 design baselines. Run after an *intentional*
# structural change (crate added/removed/relevelled), then commit the result.
arch-baseline:
    archidoc ir compile "{{justfile_directory()}}/crates"    --design --output-dir {{archidoc_baseline}}/crates
    archidoc ir compile "{{justfile_directory()}}/frontends" --design --output-dir {{archidoc_baseline}}/frontends

# Fail if the crate/frontend @c4 structure drifts from the committed baseline.
arch-validate: arch-compile
    archidoc ir validate --strict {{archidoc_baseline}}/crates/architecture.json    _context/crates/current.json
    archidoc ir validate --strict {{archidoc_baseline}}/frontends/architecture.json _context/frontends/current.json

# Fail if a crate's `@c4 uses` arrows drift from its real Cargo dependencies —
# a real dependency with no arrow (missing), or an arrow with no dependency
# (stale). Reads the workspace dep graph from `cargo metadata`. Frontends are
# intentionally arrow-free (and their dir names don't match their package
# names), so only the crate graph is gated.
arch-check-deps: arch-compile
    archidoc ir check-deps _context/crates/current.json --manifest-dir "{{justfile_directory()}}" --strict

# Regenerate the crate map + C4 diagrams from the @c4 annotations (the source of
# truth lives in each crate's src/lib.rs). Commit the regenerated files.
arch-docs: arch-compile
    python3 scripts/gen-crate-map.py _context/crates/current.json _context/frontends/current.json docs/Architecture/CrateMap.md
    archidoc ir render plantuml _context/crates/current.json    --output-dir docs/Architecture/c4/crates
    archidoc ir render plantuml _context/frontends/current.json --output-dir docs/Architecture/c4/frontends

# --- Property-Based Tests ---------------------------------------------------

# Run a PBT by name: general, petri, orgmode, loro
pbt name='general' cases='64' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    # inv-sql-budget, inherited by `keystone-smoke` and `keystone-full` (both
    # delegate here) — this is the one place to disarm the keystone's read
    # budget. Armed by default: budgets are checked against the DEDUPLICATED
    # read count, so the still-open redundancy roster (task #15) no longer
    # inflates them and is gated separately by MAX_READ_REPEAT_PER_BINDING.
    export HOLON_PERF_BUDGET=${HOLON_PERF_BUDGET-1}
    case "{{name}}" in
        general)
            PROPTEST_CASES={{cases}} cargo test \
                {{CANON}} --test general_e2e_composed_pbt \
                -- --nocapture {{FLAGS}} 2>&1 | tee target/gate-logs/pbt-general.log
            ;;
        petri)
            PROPTEST_CASES={{cases}} cargo test \
                {{CANON}} --test petri_e2e_pbt \
                -- --nocapture {{FLAGS}} 2>&1 | tee target/gate-logs/pbt-petri.log
            ;;
        orgmode)
            # BOTH org round-trip binaries. `org_block_round_trip_pbt` drives the
            # `FileFormatAdapter` seam write-back renders through, so it is the
            # only gate that sees body+source-child data loss; it was in no `just`
            # recipe until 2026-08-04, which is why a renderer regression reached
            # a green report.
            PROPTEST_CASES={{cases}} cargo test \
                {{CANON}} --test round_trip_pbt \
                -- --nocapture {{FLAGS}} 2>&1 | tee target/gate-logs/pbt-orgmode.log
            PROPTEST_CASES={{cases}} cargo test \
                {{CANON}} --test org_block_round_trip_pbt \
                -- --nocapture {{FLAGS}} 2>&1 | tee -a target/gate-logs/pbt-orgmode.log
            ;;
        loro)
            PROPTEST_CASES={{cases}} cargo test \
                {{CANON}} --test api_suite loro_backend_pbt \
                -- --nocapture {{FLAGS}} 2>&1 | tee target/gate-logs/pbt-loro.log
            ;;
        *)
            echo "Unknown PBT: {{name}}. Available: general, petri, orgmode, loro"
            exit 1
            ;;
    esac

# In-lane agent gate: single-sequence keystone smoke. Agents run THIS, never the
# full sweep (it exceeds the 600s foreground cap under parallel-lane load); the
# full sweep is the orchestrator's weave-time gate (keystone-full).
keystone-smoke:
    just pbt general 1

# Land-gate battery member: the Loro consolidator suite (projection atomicity,
# kind fidelity, unseeded-vault split, consolidator-epoch restart). It was in no
# gate recipe until 2026-09-01, so two of its tests sat red on `main` — a stale
# edge-column mirror and an invariant-10 epoch flip — with nothing to catch
# them. Seconds of runtime; run it wherever `keystone-smoke` runs.
loro-suite:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    L=target/gate-logs/loro-suite.log
    cargo nextest run {{CANON}} \
        --test loro_suite --no-fail-fast 2>&1 | tee "$L"
    grep -qE 'Summary \[.*\b[1-9][0-9]* tests run' "$L" \
        || { echo "loro-suite: ran 0 tests — the filter or target is wrong"; exit 1; }

# LogSeq-DB write gate: the two round-trip legs that judge Holon's bytes with
# LOGSEQ'S OWN validator and graph diff. They are `#[ignore]`d so a plain
# `cargo test` can never report them as passing when the oracle is absent, and
# this recipe is the only thing that runs them — a W-lane touching
# holon-logseq-db is not green without it.
#
# HOLON_LOGSEQ_ORACLE must name a LogSeq checkout at the schema version the
# graphs under test carry, with deps/db installed and LogSeq's two deleted
# scripts restored. docs/Testing/LogseqDbOracle.md has the setup; the tests
# assert those preconditions and fail loudly rather than skipping.
lsqdb-oracle:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    : "${HOLON_LOGSEQ_ORACLE:?set it to a prepared LogSeq checkout — see docs/Testing/LogseqDbOracle.md}"
    cargo test -p holon-logseq-db --all-targets -- --include-ignored --nocapture 2>&1 \
        | tee target/gate-logs/holon-lsqdb-oracle.log

# Certify the org format against its capability profile (real round trip)
capability-cert:
    scripts/capability-cert.sh

# Falsify every clause of the org profile — a clause that cannot go red is decoration
capability-sweep:
    scripts/capability-flip-sweep.sh

# Pattern-drift guard for the known-reds registry: replays the archived
# 2026-07-31 full-depth corpus through the classifier and asserts its verdict is
# unchanged. Cheap (no build) — run it after touching any assertion message that
# docs/Testing/KeystoneKnownReds.md quotes in a `Match pattern`, and after any
# edit to the classifier itself (the second half pins how it reads a log's
# pass/fail outcome, green corpus logs included).
known-reds-fixture *FLAGS:
    scripts/keystone-known-reds-fixture.sh {{FLAGS}}

# Replay hand-authored keystone regressions (concrete transition sequences from
# docs/Testing/HandAuthoredRegressions.md) through the keystone harness. Same
# --features pbt gate as the keystone; fail-loud on any red or parse error.
hand-authored *FLAGS:
    #!/usr/bin/env bash
    # pipefail is REQUIRED: without it the recipe's exit status is `tee`'s, so a
    # failing suite exited 0 and every weave/land gate using this recipe was a
    # silent false green (observed 2026-07-25).
    set -euo pipefail
    mkdir -p target/gate-logs
    # See `pbt` for why the budget is armed by default.
    export HOLON_PERF_BUDGET=${HOLON_PERF_BUDGET-1}
    export HOLON_HAND_AUTHORED_SKIP=${HOLON_HAND_AUTHORED_SKIP-}
    cargo test \
        {{CANON}} --test hand_authored_regressions \
        -- --nocapture {{FLAGS}} 2>&1 | tee target/gate-logs/pbt-hand-authored.log

# Weave-time full keystone sweep (orchestrator-run, typically in background)
keystone-full cases='16':
    just pbt general {{cases}}

# Nightly full-depth keystone tier — LOCAL, not CI.
#
# The per-weave gate stays `keystone-smoke` (ONE case). This tier runs the
# keystone at full depth N times serialized and judges the result against
# docs/Testing/KeystoneKnownReds.md: a failure whose signature matches a
# registered known red is a WARN (exit 0); ANY other signature exits non-zero.
#
# LOCAL nightly (Martin's machine / an orchestrator session), deliberately NOT a
# GitHub Actions job: `.github/workflows/ci.yml`'s `cargo test --workspace` has
# never reached the holon-integration-tests binaries — it spends ~14min
# compiling and then dies in the `holon` crate's own suite (200/200 recent runs
# red), and a full-depth keystone is hours of runtime on top. A scheduled job
# would be a gate that never actually ran the keystone. Re-evaluate once CI is
# green and the runner budget is known.
#
# Keystone runs must be serialized against every other keystone lane:
#   /opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-keystone -j1 \
#       --fg -- just keystone-nightly
keystone-nightly runs='2' cases='64':
    #!/usr/bin/env bash
    # pipefail is REQUIRED: `just pbt` pipes through tee, so without it this
    # recipe's status would be tee's and every red run would read as green.
    set -euo pipefail
    mkdir -p target/gate-logs
    stamp=$(date +%Y%m%d-%H%M%S)
    failed_logs=()
    for i in $(seq 1 {{runs}}); do
        log="target/gate-logs/keystone-nightly-${stamp}-run${i}.log"
        echo "== keystone-nightly run ${i}/{{runs}} (cases={{cases}}) -> $log =="
        rc=0
        just pbt general {{cases}} 2>&1 | tee "$log" || rc=$?
        if [ "$rc" -eq 0 ]; then
            echo "== run ${i}: GREEN =="
        else
            echo "== run ${i}: RED (exit $rc) — classifying =="
            failed_logs+=("$log")
        fi
    done
    echo ""
    if [ "${#failed_logs[@]}" -eq 0 ]; then
        echo "keystone-nightly: {{runs}}/{{runs}} runs GREEN at cases={{cases}}."
        exit 0
    fi
    scripts/keystone-known-reds.sh "${failed_logs[@]}"

# Vault-scale keystone lane: the ONE composed keystone over a ~25k-block
# synthetic vault — Martin's real vault is 24,369 blocks, and the keystone's
# scale knob defaults to 0, so no default run has ever been within three orders
# of magnitude of the regime where the turso IVM commit cost goes
# full-recompute (BugFunnel 2026-07-28). This lane exists so
# `inv-settle-budget` actually ENTERS that regime.
#
# EXPECTED RED at 25k until the turso-side fix lands: `inv-settle-budget` fires
# with the measured per-transition duration (seconds, not milliseconds). That
# red IS the deliverable — do not soften it.
#
# HOLON_SOAK_SETTLE_MS does double duty: it raises the convergence WAIT CAP (so
# a slow projection finishes and is MEASURED, instead of the settle giving up
# mid-projection) AND it scales the headless boot's LoroSyncControllerHandle
# poll budget (builder.rs). At 25k the ingest needs far more than the 120s a
# smaller value allows — boot fails loud with "LoroSyncControllerHandle never
# resolved" if this is too small. It deliberately does NOT move the
# invariant's hard-fail threshold.
keystone-scale size='25000' cases='1' settle_ms='900000' per_doc='200' *FLAGS:
    #!/usr/bin/env bash
    # pipefail is REQUIRED: without it the recipe's exit status is `tee`'s, so a
    # failing suite exits 0 and every gate using this recipe is a silent false
    # green (observed 2026-07-25).
    set -euo pipefail
    mkdir -p target/gate-logs
    stamp="$(date +%Y%m%d-%H%M%S)"
    log="target/gate-logs/pbt-keystone-scale-${stamp}.log"
    echo "keystone-scale: HOLON_SOAK_SEED_BLOCKS={{size}} cases={{cases}} settle={{settle_ms}}ms"
    echo "log: $log"
    HOLON_SOAK_SEED_BLOCKS={{size}} HOLON_SOAK_SETTLE_MS={{settle_ms}} \
        HOLON_SOAK_BLOCKS_PER_DOC={{per_doc}} HOLON_PBT_FORCE_FULL=1 \
        RUST_LOG="holon_latency=debug" \
        PROPTEST_CASES={{cases}} cargo test \
        {{CANON}} --test general_e2e_composed_pbt \
        -- --nocapture {{FLAGS}} 2>&1 | tee "$log"

# The ONE keystone driven over the LIVE MCP surface (LiveMcpE2E composition,
# windowless — same E2ETransition alphabet + invariant catalog as headless).
# Needs a running app serving MCP with reset enabled, e.g.:
#   HOLON_MCP_ALLOW_RESET=1 just live-verify 8710
# HOLON_MCP_ALLOW_RESET is REQUIRED at app launch: besides un-gating the
# reset_vault tool it routes the desktop window through the rebindable
# TEST-MODE launch that installs the reset builder (main.rs); without it the
# per-case reset fails with "no window/pump wired".
# Focus the alphabet with the standard weights knob, e.g. an MCP-data-tool-only
# walk: just keystone-mcp 8710 32 '*:0,DenseProjectionEdit:100'
keystone-mcp port='8710' cases='8' weights='':
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    if ! curl -sf "http://127.0.0.1:{{port}}/health" > /dev/null; then
        echo "no app serving http://127.0.0.1:{{port}} — start one first, e.g.:"
        echo "  HOLON_MCP_ALLOW_RESET=1 just live-verify {{port}}"
        exit 1
    fi
    HOLON_PBT_LIVE_MCP=1 MCP_SERVER_PORT={{port}} PROPTEST_CASES={{cases}} \
        HOLON_PBT_WEIGHTS="{{weights}}" cargo test \
        {{CANON}} --test general_e2e_composed_pbt \
        general_e2e_composed_pbt_live_mcp \
        -- --nocapture 2>&1 | tee target/gate-logs/pbt-keystone-mcp.log

# Launch the app for live MCP-driven verification: throwaway config+vault, own
# MCP port (registry: 8710/8720/8730/... — pick one per verifier). Leaves the
# app running in the background; caller drives it over http://127.0.0.1:PORT/mcp
# and kills the printed pid when done. Built with `--features pbt` — that
# feature is what wires the self-check suite behind the `run_self_checks` tool.
live-verify port='8710' dir='/tmp/holon-live-verify':
    #!/usr/bin/env bash
    set -euo pipefail
    rm -rf "{{dir}}"
    mkdir -p "{{dir}}/config" "{{dir}}/vault"
    HOLON_CONFIG_DIR="{{dir}}/config" HOLON_VAULT_ROOT="{{dir}}/vault" \
        MCP_SERVER_PORT={{port}} cargo run -p holon-gpui --features pbt \
        > "{{dir}}/app.log" 2>&1 &
    app_pid=$!
    echo "launched holon-gpui pid=${app_pid}, waiting for /health ..."
    for _ in $(seq 1 150); do
        if curl -sf "http://127.0.0.1:{{port}}/health" > /dev/null; then
            echo "READY pid=${app_pid} port={{port}} state-dir={{dir}}"
            exit 0
        fi
        sleep 2
    done
    echo "app failed to become healthy; log tail:"; tail -20 "{{dir}}/app.log"
    kill "${app_pid}" 2> /dev/null || true
    exit 1

# Run all PBTs sequentially
pbt-all cases='32':
    just pbt general {{cases}}
    just pbt petri {{cases}}
    just pbt orgmode {{cases}}
    just pbt loro {{cases}}

# Extended-generator sweep — the ONE keystone under wide-codepoint / empty / whitespace /
# org-special content arms (HOLON_PBT_EXTENDED_GEN gates them in the SHARED generators,
# so the composed keystone exercises them). Replaces the retired standalone extended_gen_pbt
# (§8.10: coverage lives in the ONE keystone, not a wiring-axis twin).
pbt-extended-gen cases='64' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    PROPTEST_CASES={{cases}} HOLON_PBT_EXTENDED_GEN=1 cargo test \
        {{CANON}} --test general_e2e_composed_pbt \
        -- --nocapture {{FLAGS}} 2>&1 | tee target/gate-logs/pbt-extended-gen.log

# Layout-override sweep — the ONE keystone with the index.org override arms (prql/gql/sql
# layouts + profile file); HOLON_PBT_LAYOUT_OVERRIDE gates them in write_org_file.rs.
# Replaces the retired standalone layout_override_pbt. KNOWN RED (pre-existing): the link-mark
# [[label]] extraction divergence on UI-origin ApplyMutation Update on block:root-layout
# (capture /tmp/layout_full_8case_finding.captured.json) still reproduces here — triage via
# the keystone, then remove this note.
pbt-layout-override cases='64' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    PROPTEST_CASES={{cases}} HOLON_PBT_LAYOUT_OVERRIDE=1 cargo test \
        {{CANON}} --test general_e2e_composed_pbt \
        -- --nocapture {{FLAGS}} 2>&1 | tee target/gate-logs/pbt-layout-override.log

# Measure end-to-end UI action latency (indent / outdent / cycle-state / split / ...).
# Drives the REAL pipeline (dispatch -> Loro commit -> LoroProjection resample ->
# Turso/matview CDC -> reactive rows) through the headless composed keystone with the
# `holon_latency` tracing target enabled, then prints a per-action count/p50/p95/max
# table plus per-stage cost. Measures everything EXCEPT final GPU paint, and
# records OTel spans at their default `info` so a slow run can be explained.
measure-latency cases='16' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    RUST_LOG="holon_latency=debug" PROPTEST_CASES={{cases}} \
        cargo test {{CANON}} \
        --test general_e2e_composed_pbt -- --nocapture {{FLAGS}} \
        > target/gate-logs/holon-latency.log 2>&1 || true
    echo "raw log: target/gate-logs/holon-latency.log ($(grep -c holon_latency target/gate-logs/holon-latency.log || true) events)"
    python3 scripts/measure_latency.py target/gate-logs/holon-latency.log --max-contention-ms 30

# Latency RATCHET gate — per-rung interaction->visible ceilings over two stages:
# the PROD `stage=e2e` measurement (the exact quantity the runtime
# `holon_oracles` latency-slo oracle judges — see crates/holon-api/src/
# latency_e2e.rs "SLO endpoint") and the harness's `stage=action_total`.
# Ceilings live in docs/Testing/latency-ceilings.txt and only ever move DOWN;
# the 200ms SLO stays the documented target, not the gate threshold.
#
# Every GATED rung is a p50. p95 is printed but not gated: it interpolates, so
# at replay-scale n it is dominated by the single largest sample and flaps on
# unmodified code. That file also carries two REPORT-ONLY rungs (the SplitBlock
# pair), printed every run but unable to fail the build until their bimodal slow
# mode is attributed. See the ceilings file for the measured spreads.
#
# Unlike `measure-latency` (a report), this FAILS the build on a regression, and
# refuses to pass vacuously: every GATED rung must produce at least
# --min-samples samples, and a file with no gated rung at all is an error.
#
# The drive is the DETERMINISTIC hand-authored replay (latency-ratchet.jsonl),
# not a random keystone sweep: a ratchet needs the same rungs and the same
# sample counts every run, or the statistic it compares against a fixed ceiling
# moves for reasons that have nothing to do with the code.
#
#   just latency-gate                          # the gate
#   just latency-gate /tmp/tightened.txt       # prove-it-can-fail run
latency-gate ceilings='docs/Testing/latency-ceilings.txt':
    #!/usr/bin/env bash
    # pipefail so a `tee`'d failure cannot report success (see `hand-authored`).
    set -euo pipefail
    # Tree assertion: a failed `cd` must never yield a green that ran nothing
    # (a gate exited 0 having run 0 tests in the WRONG tree, 2026-07-25).
    corpus=crates/holon-integration-tests/hand-authored-regressions/latency-ratchet.jsonl
    for f in scripts/measure_latency.py {{ceilings}} "$corpus" \
             crates/holon-integration-tests/tests/hand_authored_regressions.rs; do
        [ -f "$f" ] || { echo "latency-gate: wrong tree — missing $f" >&2; exit 2; }
    done
    # Per-invocation log. A fixed /tmp path collides when two trees (or a lane
    # and its verifier) run this gate at once, and the loser reads the other's
    # output — which reads as an inexplicable failure in a tree that is fine.
    log="$(mktemp -t holon-latency-gate)"
    echo "latency-gate: run log $log"
    # The replay's own correctness has its own gates (`keystone-smoke`,
    # `hand-authored`); this gate's subject is latency. So a non-zero test exit is
    # DISCLOSED, not swallowed and not fatal — a run that died early loses
    # samples, which the --min-samples floor below turns into a hard failure.
    status=0
    RUST_LOG="holon_latency=debug" \
        HOLON_HAND_AUTHORED_SIDECAR="hand-authored-regressions/latency-ratchet.jsonl" \
        cargo test {{CANON}} \
        --test hand_authored_regressions -- --nocapture > "$log" 2>&1 || status=$?
    [ "$status" -eq 0 ] || echo "latency-gate: NOTE — replay exited $status; latency data below is still judged (see $log)"
    # Floor of 18: every rung is driven >= 20 times by construction, so a count
    # below this means the replay lost transitions, not that the workload is
    # small. See the corpus header for why the counts are >= 20.
    # Three outcomes, and a caller must read $? not just its truthiness: 0 green,
    # 1 a rung over its ceiling, 3 the host was too busy to judge (see the
    # ceilings header). 3 propagates rather than being swallowed, so an
    # unjudgeable run can never be mistaken for a passing one.
    gate=0
    python3 scripts/measure_latency.py "$log" --ratchet {{ceilings}} --min-samples 18 \
        --max-contention-ms 30 || gate=$?
    if [ "$gate" -eq 3 ]; then
        echo "latency-gate: INVALID (not red) — nothing was judged; re-run on a quiet machine."
    fi
    exit "$gate"

# The latency SLO as a GATE (Martin's ruling D50.a) — two rungs plus their own
# wiring check, three tests in one headless binary; the last step of
# `landing-gate`.
# Distinct from `latency-gate`
# above: that one is a per-rung REGRESSION RATCHET against ceilings that only
# move down, this one judges the fixed 200ms SLO and a throughput floor. A tree
# can regress within its ratchet and still be inside the SLO, or blow the SLO
# without moving a p50 — neither gate subsumes the other.
#
# Serialized (`--test-threads=1`): the rungs measure wall-clock latency, so two
# of them running at once would each measure the other's load.
latency-slo-gate *FLAGS:
    #!/usr/bin/env bash
    # pipefail so a `tee`'d failure cannot report success (see `hand-authored`).
    set -euo pipefail
    mkdir -p target/gate-logs
    # Tree assertion: a failed `cd` must never yield a green that ran nothing
    # (a gate exited 0 having run 0 tests in the WRONG tree, 2026-07-25).
    for f in crates/holon-integration-tests/tests/latency_slo_gate.rs \
             crates/holon-api/src/latency_slo.rs; do
        [ -f "$f" ] || { echo "latency-slo-gate: wrong tree — missing $f" >&2; exit 2; }
    done
    cargo test {{CANON}} --test latency_slo_gate \
        -- --nocapture --test-threads=1 {{FLAGS}} 2>&1 \
        | tee target/gate-logs/latency-slo-gate.log

# Scale-soak: drive the REAL pipeline against a seeded 5–10k-block vault WITH CRDT on,
# measuring per-action latency vs the p95<200ms SLO plus RSS growth. Boots the keystone
# (`general_e2e_composed_pbt`, forced to the full_headless/CRDT wiring) over a synthetic
# vault of `size` extra blocks (deep trees, tasks, links, unicode; deterministic seed),
# then drives ~`actions` mixed actions (edit / indent / outdent / split / toggle / nav)
# and prints the per-action-type latency table + dominator + RSS start→peak→end.
# Results land in docs/Testing/soak/ . Runtime: minutes (boot re-seeds the vault per
# proptest case). See DEVELOPMENT.md "Scale Soak".
#
#   just soak                 # 5000 blocks, ~320 actions
#   just soak 10000 480       # 10k blocks
soak size='5000' actions='320' settle_ms='30000' per_doc='200' soften='':
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    mkdir -p docs/Testing/soak
    stamp="$(date +%Y%m%d-%H%M%S)"
    log="target/gate-logs/holon-soak-${stamp}.log"
    rss="target/gate-logs/holon-soak-rss-${stamp}.csv"
    out="docs/Testing/soak/soak-{{size}}-blocks-${stamp}.txt"
    # ~20 actions per proptest case (draws 1..40); derive case count from target actions.
    cases=$(( ({{actions}} + 19) / 20 )); [ "$cases" -lt 1 ] && cases=1
    echo "soak: size={{size}} blocks  actions≈{{actions}} (cases=$cases)  settle={{settle_ms}}ms  CRDT=on"
    echo "raw log: $log   rss: $rss   report: $out"
    # Background RSS sampler (self-terminates when the test process exits).
    bash scripts/soak_rss_sampler.sh "$rss" 'general_e2e_composed_pb[t]' 2 &
    sampler=$!
    HOLON_SOAK_SEED_BLOCKS={{size}} HOLON_SOAK_SETTLE_MS={{settle_ms}} \
        HOLON_SOAK_BLOCKS_PER_DOC={{per_doc}} HOLON_PBT_FORCE_FULL=1 \
        HOLON_PBT_INVARIANTS="{{soften}}" \
        RUST_LOG="holon_latency=debug" PROPTEST_CASES="$cases" \
        cargo test {{CANON}} \
        --test general_e2e_composed_pbt -- --nocapture \
        > "$log" 2>&1 || echo "NOTE: test exited non-zero (see $log tail) — latency data below is still valid"
    wait "$sampler" 2>/dev/null || true
    {
        echo "# Holon scale-soak — $stamp"
        echo "# size={{size}} blocks  actions≈{{actions}} (cases=$cases)  settle={{settle_ms}}ms  per_doc={{per_doc}}  CRDT=on"
        [ -n "{{soften}}" ] && echo "# DISCLOSED SOFTENING: HOLON_PBT_INVARIANTS={{soften}} ($(grep -c 'softened (DISCLOSED degraded run)' "$log" || true) softened failures — see raw log)"
        echo "# raw log: $log"
        echo ""
        echo "action_total events: $(grep -c 'stage=action_total' "$log" || true)"
        echo ""
        # No contention precondition here: its threshold is calibrated on the
        # ratchet corpus, and a seeded 5-10k-block vault raises boot DDL by
        # workload rather than by contention.
        python3 scripts/measure_latency.py "$log" --fail-over-p95 200 --max-contention-ms 0 || true
        echo ""
        echo "== RSS (resident set, MB) =="
        awk -F, 'NR>1{v=$2; if(NR==2)start=v; if(v>peak)peak=v; end=v; n++}
            END{ if(n>0) printf "samples=%d  start=%.0f  peak=%.0f  end=%.0f  growth=%+.0f MB\n", n,start,peak,end,end-start;
                 else print "no RSS samples" }' "$rss"
    } | tee "$out"
    echo ""
    echo "SLO verdict + full table written to: $out"

# --- Memory & async-stall profiling -----------------------------------------

# Heap-profile a headless soak workload with dhat, then print top allocators
# (no web viewer needed). Writes dhat-heap.json in the repo root.
heap-profile blocks='2000':
    #!/usr/bin/env bash
    set -euo pipefail
    HOLON_SOAK_SEED_BLOCKS={{blocks}} \
        cargo run --release --example diag_harness -p holon-integration-tests \
        --features heap-profile
    bash scripts/analyze_dhat.sh dhat-heap.json

# Async-stall profile a headless soak workload; attach the tokio-console CLI to
# it. Needs: cargo install tokio-console. Requires --cfg tokio_unstable (set).
tokio-console-harness blocks='2000' hold='120':
    #!/usr/bin/env bash
    set -euo pipefail
    echo "console aggregator -> 127.0.0.1:6669 (holding {{hold}}s after ingest)"
    echo "attach from another shell:  tokio-console http://127.0.0.1:6669"
    RUSTFLAGS="--cfg tokio_unstable" \
        HOLON_SOAK_SEED_BLOCKS={{blocks}} HOLON_DIAG_HOLD_SECS={{hold}} \
        cargo run --example diag_harness -p holon-integration-tests \
        --features tokio-console

# Run the REAL GPUI desktop app with tokio-console attached to its live runtime
# (the DatabaseActor, file-sync, save-worker tasks). Attach as above.
tokio-console-app:
    RUSTFLAGS="--cfg tokio_unstable" \
        cargo run -p holon-gpui --features tokio-console

# --- Lib slices (composed catch triads + slice component tests) ---------------
# The declare_pbt_slice!/component_pbt! standalone slice binaries were retired
# (§8.10: coverage lives in the ONE composed keystone). What remains are the
# cfg(test) lib slice tests (catch triads, component integration tests) —
# nextest's default filter EXCLUDES lib targets, so run them explicitly:
pbt-lib-slices:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo nextest run {{CANON}} --lib -E 'package(holon-integration-tests)' \
        2>&1 | tee target/gate-logs/pbt-lib-slices.log

# --- Mutation Testing -------------------------------------------------------

# Run cargo-mutants on a specific file (defaults to petri.rs)
mutants file='crates/holon/src/petri.rs' timeout='300':
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo mutants \
        --manifest-path crates/holon/Cargo.toml \
        --file {{file}} \
        --timeout {{timeout}} \
        --output target/gate-logs/mutants-out 2>&1 | tee target/gate-logs/mutants.log

# Show last mutants results
mutants-results:
    @cat target/gate-logs/mutants-out/outcomes.json 2>/dev/null | python3 -m json.tool || echo "No results found. Run 'just mutants' first."

# --- Assets ----------------------------------------------------------------

# Download icons listed in assets/icons/manifest.toml
icons *FLAGS:
    ./assets/icons/download.sh {{FLAGS}}

# --- Build & Check ----------------------------------------------------------

# Workspace build
build *FLAGS: icons
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo build {{CANON}} {{FLAGS}} 2>&1 | tee target/gate-logs/holon-build.log

# Clippy across workspace
clippy:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo clippy {{CANON}} --all-targets 2>&1 | tee target/gate-logs/holon-clippy.log

# Run all workspace tests (not PBTs — those are slow). Deliberately OFF the
# canonical feature shape above: unifying would pull in every pbt-gated test
# this recipe exists to skip, which is a "which tests run" change, not a
# compile-shape one.
test:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo nextest run --workspace 2>&1 | tee target/gate-logs/holon-test.log

# Rot guard for the out-of-workspace wasi worker: `cargo check --workspace`
# cannot see it, so an API change in holon-api compiles clean and breaks the
# worker silently. Mirrors the CI rust-checks step; wired into `precommit`.
# EMNAPI_LINK_DIR: napi-build's wasi shim demands it, but `cargo check` never
# links, so an empty stub dir is safe — the real dir comes from `napi build`
# (frontends/holon-worker/scripts/build.sh). The native `--no-default-features`
# run is what actually EXECUTES the worker's serde-glue tests (`browser` pulls
# wasm-only Turso IO, and the wasm target has no test harness).
check-worker-wasm:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    # Install only when missing: `rustup target add` reaches the network even for
    # an already-installed target, and this recipe runs at every commit.
    if ! rustup target list --installed | grep -qx wasm32-wasip1-threads; then
        rustup target add wasm32-wasip1-threads
    fi
    EMNAPI_LINK_DIR="$(mktemp -d)" cargo check \
        --manifest-path frontends/holon-worker/Cargo.toml \
        --target wasm32-wasip1-threads --features browser \
        2>&1 | tee target/gate-logs/holon-worker-wasm-check.log
    cargo test \
        --manifest-path frontends/holon-worker/Cargo.toml \
        --lib --no-default-features \
        2>&1 | tee target/gate-logs/holon-worker-native-test.log

# Rot guard for the BROWSER target. `check-worker-wasm` above covers only
# wasm32-wasip1-threads, so a crate that compiles native and wasi but not
# wasm32-unknown-unknown was caught by no gate at all — a native-only dep in
# holon-frontend's graph (tracing-appender → the unmaintained `symlink` crate)
# sat there until someone ran this by hand (BugFunnel 2026-08-15 D15.b). The
# `getrandom_backend` rustflag this target needs lives in .cargo/config.toml.
# MEASURED (2026-08-15): warm 2s. Only the first build of the wasm dep graph
# costs minutes — the same warm/cold profile `gate-compile` already has.
check-frontend-wasm:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    # Install only when missing — see check-worker-wasm: `rustup target add` is a
    # network touch even when the target is already there.
    if ! rustup target list --installed | grep -qx wasm32-unknown-unknown; then
        rustup target add wasm32-unknown-unknown
    fi
    cargo check -p holon-frontend --target wasm32-unknown-unknown \
        2>&1 | tee target/gate-logs/holon-frontend-wasm-check.log

# Rot guard for the browser FRONTEND crate. `check-frontend-wasm` above covers
# holon-frontend, the shared library; dioxus-web is the app that consumes it and
# is OUTSIDE the cargo workspace (wasm32-only, root Cargo.toml `exclude`), so
# `cargo check --workspace` never sees it and its call sites rot silently
# whenever a holon-frontend signature changes. Hence --manifest-path rather than
# -p. Normally built via `trunk`; this is the cheap typecheck-only leg.
check-dioxus-web-wasm:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    if ! rustup target list --installed | grep -qx wasm32-unknown-unknown; then
        rustup target add wasm32-unknown-unknown
    fi
    cargo check --manifest-path frontends/dioxus-web/Cargo.toml \
        --target wasm32-unknown-unknown \
        2>&1 | tee target/gate-logs/holon-dioxus-web-wasm-check.log

# Rot guard for the ANDROID target, via holon-turso — the crate whose graph
# actually drives the NDK C toolchain (ring + turso compile .S/.c through
# cc-rs). `cargo ndk` rather than bare `cargo`: cc-rs defaults to the
# UNVERSIONED tool name `aarch64-linux-android-clang`, which no NDK ships, so
# a bare cross-build fails in ring's build script before the linker is ever
# reached. cargo-ndk puts the toolchain on PATH and exports the versioned
# CC_/AR_/LINKER vars for the -P API level, and discovers the NDK itself
# (ANDROID_NDK_HOME, else the highest $ANDROID_HOME/ndk/*), so no version is
# pinned here.
check-android:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    # Install only when missing — see check-worker-wasm: `rustup target add` is a
    # network touch even when the target is already there.
    if ! rustup target list --installed | grep -qx aarch64-linux-android; then
        rustup target add aarch64-linux-android
    fi
    cargo ndk -t arm64-v8a -P 33 check -p holon-turso \
        2>&1 | tee target/gate-logs/holon-android-check.log

# --- Code Quality -----------------------------------------------------------

# Check formatting
fmt-check:
    cargo fmt --check

# Audit dependencies for vulnerabilities, license issues, and bans
deny:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo deny check 2>&1 | tee target/gate-logs/holon-deny.log

# Find unused dependencies
machete:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo machete 2>&1 | tee target/gate-logs/holon-machete.log

# Detect copy-pasted code (requires: npx or npm i -g jscpd)
duplication:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    npx jscpd . 2>&1 | tee target/gate-logs/holon-duplication.log

# Run all lints and quality checks locally
lint:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    failed=0
    echo "=== cargo fmt ==="
    cargo fmt --check || { echo "FAIL: formatting"; failed=1; }
    echo ""
    echo "=== cargo clippy ==="
    cargo clippy {{CANON}} --all-targets -- -D warnings 2>&1 | tee target/gate-logs/holon-clippy.log || { echo "FAIL: clippy"; failed=1; }
    echo ""
    echo "=== cargo deny ==="
    cargo deny check 2>&1 | tee target/gate-logs/holon-deny.log || { echo "FAIL: deny"; failed=1; }
    echo ""
    echo "=== cargo machete ==="
    cargo machete 2>&1 | tee target/gate-logs/holon-machete.log || { echo "FAIL: machete"; failed=1; }
    echo ""
    echo "=== jscpd (duplication) ==="
    npx jscpd . 2>&1 | tee target/gate-logs/holon-duplication.log || { echo "FAIL: duplication"; failed=1; }
    echo ""
    if [ "$failed" -ne 0 ]; then
        echo "Some checks failed. See output above."
        exit 1
    fi
    echo "All checks passed."

# --- Code Analysis ----------------------------------------------------------
# Individual analyzers write logs to target/gate-logs/holon-analyze-*.log. These are
# per-workspace developer logs — nothing collects them; CI reads only exit codes.

# CRAP metric (complexity × inverse coverage). Requires lcov.info.
analyze-crap:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    if [ ! -f lcov.info ] || [ $(find lcov.info -mmin -60 2>/dev/null | wc -l) -eq 0 ]; then
        echo "Generating fresh lcov.info via cargo-llvm-cov..."
        # Coverage runs the whole test suite. Use nextest so .config/nextest.toml
        # enforces per-test timeouts (2 min default, 10 min for E2E PBTs).
        # cucumber-rs uses its own CLI; nextest can't enumerate it.
        # --no-fail-fast so individual failing tests don't abort coverage.
        # cucumber-rs uses its own CLI; nextest can't enumerate it.
        # --ignore-run-fail: llvm-cov writes lcov.info even if nextest exits non-zero.
        # (Mutually exclusive with --no-fail-fast in cargo-llvm-cov.)
        # Excluded:
        #   cucumber          — uses its own CLI, nextest can't enumerate it
        #   tui_ui_pbt        — process::exit on PBT failure aborts coverage write
        cargo llvm-cov nextest --workspace --lcov --output-path lcov.info \
            --ignore-run-fail \
            -E 'not (binary(cucumber) + binary(tui_ui_pbt))' 2>&1 \
            | tee target/gate-logs/holon-analyze-coverage.log
    fi
    # Threshold / examples-exclude / missing-coverage policy live in
    # .cargo-crap.toml and are picked up automatically from the repo root.
    # Human report — every function over threshold, for visibility.
    cargo crap --lcov lcov.info 2>&1 | tee target/gate-logs/holon-analyze-crap.log
    # Regression gate — fail ONLY when a function's CRAP score rose vs the
    # recorded baseline. New code can't make the pre-existing hotspots worse;
    # the backlog (Phase 5) is paid down incrementally, not blocked. We compare
    # via tools/crap_check_regression.py rather than `cargo crap --fail-regression`
    # because the latter pairs functions by name only and mispairs the many
    # duplicate-named functions in this repo (see the script's docstring).
    # Regenerate the baseline with `just crap-baseline` after intentional changes.
    if [ -f crap-baseline.json ]; then
        cargo crap --lcov lcov.info --format json --output target/gate-logs/holon-crap-current.json
        python3 tools/crap_check_regression.py \
            --baseline crap-baseline.json --current target/gate-logs/holon-crap-current.json \
            2>&1 | tee -a target/gate-logs/holon-analyze-crap.log
    else
        echo "No crap-baseline.json — skipping regression gate. Run 'just crap-baseline'." \
            | tee -a target/gate-logs/holon-analyze-crap.log
    fi

# Record the current CRAP scores as the regression baseline (crap-baseline.json).
# Run after intentionally accepting new complexity, or to lower the bar as the
# Phase 5 backlog is paid down. Requires a fresh lcov.info (run analyze-crap first).
crap-baseline:
    #!/usr/bin/env bash
    set -euo pipefail
    if [ ! -f lcov.info ]; then
        echo "lcov.info missing — run 'just analyze-crap' first to generate coverage."
        exit 1
    fi
    cargo crap --lcov lcov.info --format json --output crap-baseline.json
    echo "Wrote crap-baseline.json"

# Dependency audit (vulnerabilities, licenses, bans).
analyze-deny:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo deny check 2>&1 | tee target/gate-logs/holon-analyze-deny.log

# Unused dependency detection.
analyze-machete:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo machete 2>&1 | tee target/gate-logs/holon-analyze-machete.log

# Lint with clippy at the workspace level.
# Report-only: clippy findings are surfaced but don't fail the recipe. Phase 6
# of the code-quality plan re-tightens this gate (`-D warnings`) once the
# workspace backlog has been paid down incrementally.
analyze-clippy:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo clippy {{CANON}} --all-targets 2>&1 \
        | tee target/gate-logs/holon-analyze-clippy.log

# Copy-paste / duplication detection via polydup.
analyze-duplication:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    polydup scan . 2>&1 | tee target/gate-logs/holon-analyze-duplication.log

# Architecture lints (cycles, banned imports, etc.).
analyze-arch:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    ./archlint/archlint --all 2>&1 | tee target/gate-logs/holon-analyze-arch.log

# Run every analyzer. Continues on failure; reports a summary at the end.
analyze:
    #!/usr/bin/env bash
    set -uo pipefail
    failed=()
    for step in clippy deny machete arch duplication crap; do
        echo ""
        echo "=== analyze-${step} ==="
        if ! just "analyze-${step}"; then
            failed+=("${step}")
        fi
    done
    echo ""
    if [ "${#failed[@]}" -ne 0 ]; then
        echo "Failed analyzers: ${failed[*]}"
        exit 1
    fi
    echo "All analyzers passed."

# Watch & run a UI frontend (recompiles on source changes)
# chrome-trace available for: gpui, blinc
# Only kills the old app if the new build succeeds.
watch ui='gpui' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    UI="{{ui}}"
    EXTRA_FLAGS="{{FLAGS}}"
    BIN="target/debug/holon-${UI}"
    APP_PID=""
    OUTER_PID=$$

    restart_app() {
        if [ -n "$APP_PID" ]; then
            kill "$APP_PID" 2>/dev/null || true
            wait "$APP_PID" 2>/dev/null || true
        fi
        "$BIN" $EXTRA_FLAGS &
        APP_PID=$!
        echo ">>> App started (PID $APP_PID) <<<"
    }

    cleanup() {
        [ -n "$APP_PID" ] && kill "$APP_PID" 2>/dev/null || true
        [ -n "${WATCH_PID:-}" ] && kill "$WATCH_PID" 2>/dev/null || true
    }
    trap cleanup EXIT
    trap restart_app USR1

    # Initial build and run
    cargo build -p "holon-${UI}" --features chrome-trace 2>&1 | tee target/gate-logs/holon-build.log
    restart_app

    # cargo-watch only builds; signals outer script on success
    cargo watch -s "cargo build -p holon-${UI} --features chrome-trace 2>&1 | tee target/gate-logs/holon-build.log && kill -USR1 ${OUTER_PID} || echo '>>> Build failed — keeping old instance running <<<'" &
    WATCH_PID=$!

    # Block until cargo-watch exits; USR1 interrupts wait to trigger restart_app
    while kill -0 "$WATCH_PID" 2>/dev/null; do
        wait "$WATCH_PID" 2>/dev/null || true
    done

# --- Profiling -------------------------------------------------------------

# Profile a PBT with samply (opens Firefox Profiler UI)
profile name='petri' cases='4' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{name}}" in
        general)  pkg="holon-integration-tests"; test="general_e2e_composed_pbt"; feat="--features pbt" ;;
        petri)    pkg="holon"; test="petri_e2e_pbt"; feat="" ;;
        orgmode)  pkg="holon-orgmode"; test="round_trip_pbt"; feat="" ;;
        *)        echo "Unknown: {{name}}"; exit 1 ;;
    esac
    bin=$(cargo test -p "$pkg" $feat --test "$test" --no-run --message-format=json 2>/dev/null \
        | jq -r 'select(.executable) | .executable' | head -1)
    PROPTEST_CASES={{cases}} samply record "$bin" --nocapture {{FLAGS}}

# Sample stack traces of a stuck PBT (finds the right child process automatically)
sample-pbt name='general' cases='1' duration='5':
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    case "{{name}}" in
        general)  pkg="holon-integration-tests"; test="general_e2e_composed_pbt"; feat="--features pbt" ;;
        petri)    pkg="holon"; test="petri_e2e_pbt"; feat="" ;;
        orgmode)  pkg="holon-orgmode"; test="round_trip_pbt"; feat="" ;;
        *)        echo "Unknown: {{name}}"; exit 1 ;;
    esac
    bin=$(cargo test -p "$pkg" $feat --test "$test" --no-run --message-format=json 2>/dev/null \
        | jq -r 'select(.executable) | .executable' | head -1)
    binary_name=$(basename "$bin")
    echo "Binary: $bin"
    echo "Starting PBT in background..."
    PROPTEST_CASES={{cases}} "$bin" --nocapture > target/gate-logs/pbt-sample-out.log 2>&1 &
    root_pid=$!
    echo "Root PID: $root_pid"
    echo "Waiting for child processes to spawn..."
    sleep 15
    # Find the leaf child process (the one actually running test logic, not waiting on fork)
    # proptest fork mode: root → cargo child → forked test child
    # We want the deepest descendant that's using CPU
    leaf_pid=$(ps -eo pid,ppid,pcpu,comm | grep "$binary_name" | grep -v grep \
        | awk '{print $1, $2, $3}' \
        | sort -t' ' -k3 -rn \
        | head -1 | awk '{print $1}')
    if [ -z "$leaf_pid" ]; then
        echo "No child process found. Test may have finished. Output:"
        cat target/gate-logs/pbt-sample-out.log
        exit 1
    fi
    echo "Sampling PID $leaf_pid for {{duration}}s..."
    sample "$leaf_pid" {{duration}} -f target/gate-logs/pbt-sample.txt
    kill "$root_pid" 2>/dev/null || true
    pkill -P "$root_pid" 2>/dev/null || true
    echo "Stack trace saved to target/gate-logs/pbt-sample.txt"
    echo ""
    echo "=== Top of stack (where time is spent) ==="
    grep -E '^\s+\d+\s' target/gate-logs/pbt-sample.txt | sort -rn | head -20
    echo ""
    echo "=== Test output ==="
    tail -30 target/gate-logs/pbt-sample-out.log

# Profile an arbitrary binary with samply
profile-bin *ARGS:
    samply record {{ARGS}}

# --- Coverage ---------------------------------------------------------------

# Run app with coverage instrumentation
coverage:
    ./scripts/run-with-coverage.sh -d macos

# Process Rust coverage data
coverage-rust:
    ./scripts/process-rust-coverage.sh html

# Process Flutter coverage data
coverage-flutter:
    ./scripts/process-flutter-coverage.sh

# --- Quality gates (two-tier) -------------------------------------------------
# Tier 1: cheap checks at every commit. Tier 2: full keystone before every push.
# jj does not fire git hooks — run these by hand (or scripts/install-git-hooks.sh
# wires them up for plain-git users). See DEVELOPMENT.md "Quality gates".

# The canonical typecheck. `--all-targets` plus the two pbt features is what it
# takes to compile a TEST target at all: a bare `cargo check --workspace` builds
# only lib+bin, so eight windowed GPUI test binaries were uncompilable for days
# while every gate reported green (BugFunnel 2026-08-14 PERCEPTION). Lanes
# compose THIS recipe instead of retyping the feature list — a lane that spells
# the features differently is a lane running a different gate.
gate-compile:
    #!/usr/bin/env bash
    # pipefail is REQUIRED, exactly as on `hand-authored` above: without a
    # shebang `just` runs the body under `sh -cu`, the exit status is `tee`'s,
    # and the recipe passes however red the compile is.
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo check {{CANON}} --all-targets \
        2>&1 | tee target/gate-logs/gate-compile.log
    just check-web-arm

# Reclaim superseded `target/<profile>/build/<crate>/<hash>/` directories
# (D85.c part A): a hash dir is garbage once a newer dir for the same
# crate/target exists. The CANON feature shape above is what stops NEW churn;
# this sweeps whatever churn predates it, or slips in from an off-shape recipe
# (`profile`, `sample-pbt`, `scripts/capability-cert*.sh`, wasm/android checks).
# Refuses to run while cargo/rustc is using the target dir. Dry run by default.
target-gc target_dir='target' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    /usr/bin/python3 scripts/target-gc.py {{target_dir}} --apply {{FLAGS}}

# Web arm (dioxus-web under test) typecheck — the SAME shape as `gate-compile`
# above (CANON_FEATURES carries web-arm too, D85.c), so this call costs cargo
# nothing once gate-compile has run: it re-asks the identical question and gets
# a cached no-op. Kept as its own recipe/log so `precommit`/`landing-gate` can
# report it as a distinct step and so `just check-web-arm` still works standalone.
check-web-arm:
    #!/usr/bin/env bash
    # pipefail is REQUIRED, exactly as in gate-compile above: without it the
    # exit status is `tee`'s and the recipe passes however red the compile is.
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo check {{CANON}} --all-targets \
        2>&1 | tee target/gate-logs/check-web-arm.log

# Architecture rules (archlint + the Rust-side structural tests). Its own
# package, so `cargo nextest run --workspace` was the only thing that ran it and
# no gate runs that — a red architecture test sat on main for 4 days
# (BugFunnel 2026-08-12). Cheap (~35s): any per-lane or per-landing gate list,
# even a hand-picked subset, should include THIS recipe — omitting it is how a
# red architecture rule escapes a landing.
gate-arch:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    cargo nextest run {{CANON}} --test architecture_rules 2>&1 | tee target/gate-logs/gate-arch.log

# Tier 1 pre-commit gate: gate-integrity lints, defensive-code ratchet, typecheck.
# The justfile guard runs FIRST and costs ~10ms: a never-fail recipe invalidates
# every later verdict in this file, so it is checked before anything trusts one.
# MEASURED (2026-08-15): the whole warm tier = 64s end-to-end (gate-compile 12s,
# ratchet ~5s, check-frontend-wasm 2s, the rest worker). The FIRST run after a
# rebase compiles every test target and costs minutes — the one-off both wasm
# checks pay too.
# A keystone smoke was CUT from this tier: even PROPTEST_CASES=2 takes ~4.5min
# because proptest unconditionally replays the persisted regression seeds and
# each case pays full composed-SUT boot — it belongs in `just prepush` (Tier 2).
precommit:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "== Tier 1 [1/6]: justfile pipefail guard =="
    ./scripts/check-justfile-pipefail.sh
    echo "== Tier 1 [2/6]: defensive-code ratchet =="
    ./scripts/defensive-ratchet.sh
    echo "== Tier 1 [3/6]: workspace typecheck incl. every test target =="
    just gate-compile
    echo "== Tier 1 [4/6]: browser-target typecheck =="
    just check-frontend-wasm
    echo "== Tier 1 [5/6]: out-of-workspace browser frontend =="
    just check-dioxus-web-wasm
    echo "== Tier 1 [6/6]: out-of-workspace worker =="
    just check-worker-wasm
    echo "== Tier 1 PASS =="

# Tier 2 pre-push gate: architecture rules, then the full keystone at default
# PROPTEST_CASES=16 (includes the persisted regression seeds in
# tests/general_e2e_composed_pbt.proptest-regressions). The two cheap checks run
# FIRST — architecture 40s, browser-target 2s warm, against the keystone's 5min —
# so a structural or browser-target red fails fast.
# MEASURED (2026-07-07): green keystone ~5min quiet; a RED run that shrinks ~15min.
prepush:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p target/gate-logs
    echo "== Tier 2 [1/4]: architecture rules =="
    just gate-arch
    echo "== Tier 2 [2/4]: browser-target typecheck =="
    just check-frontend-wasm
    echo "== Tier 2 [3/4]: out-of-workspace browser frontend =="
    just check-dioxus-web-wasm
    echo "== Tier 2 [4/4]: full keystone (PROPTEST_CASES=16) =="
    PROPTEST_CASES=16 cargo test \
        {{CANON}} --test general_e2e_composed_pbt \
        2>&1 | tee target/gate-logs/prepush-keystone.log
    echo "== Tier 2 PASS =="

# The composed landing gate: what a lane runs before reporting done and what the
# orchestrator runs before weaving. One recipe name, so it survives being passed
# through `parallel ... -- <cmd>` (which sheds a quote layer, so no gate string
# may carry parens or nested quotes).
landing-gate:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "== landing [1/11]: fmt =="
    cargo fmt --all -- --check
    echo "== landing [2/11]: typecheck incl. every test target =="
    just gate-compile
    echo "== landing [3/11]: browser-target typecheck =="
    just check-frontend-wasm
    echo "== landing [4/11]: out-of-workspace browser frontend =="
    just check-dioxus-web-wasm
    echo "== landing [5/11]: out-of-workspace wasi worker =="
    just check-worker-wasm
    echo "== landing [6/11]: architecture rules =="
    just gate-arch
    echo "== landing [7/11]: keystone smoke =="
    just keystone-smoke
    echo "== landing [8/11]: loro consolidator suite =="
    just loro-suite
    echo "== landing [9/11]: hand-authored regressions =="
    just hand-authored
    echo "== landing [10/11]: latency SLO (D50.a) =="
    just latency-slo-gate
    echo "== landing [11/11]: target-gc (D85.c, this lane's own target/ only) =="
    just target-gc || echo "target-gc: non-fatal (busy or nothing to reclaim), see above"
    echo "== landing gate PASS =="

# --- Observability ----------------------------------------------------------
# Run records are emitted by every keystone/GPUI PBT run as a side effect (see
# crates/holon-integration-tests/src/pbt/run_result.rs) into
# .observability/runs/*.result.json. These recipes convert them to canonical
# YAML and publish to the GitHub `observability` release (indefinite retention,
# CORS-open for the Pages report). Local runs are a first-class source — see
# docs/Proposals/observability-report.md.

# Convert local .result.json -> canonical .yaml + rebuild the cumulative
# runs.json index. Idempotent; reads only, writes under .observability/.
obs-collect:
    python3 scripts/holon_obs.py collect

# Collect, then upload runs.json (--clobber) and any new runs/<id>.yaml to the
# GitHub `observability` release via `gh`. Pure upload — runs NO tests.
obs-push:
    python3 scripts/holon_obs.py push

# Serve .observability/ over HTTP so the report can be opened locally (browsers
# block file:// fetch of the JSON data).
obs-serve port='8000':
    python3 scripts/holon_obs.py serve --port {{port}}

# Run the GPUI Gherkin replay with offscreen PIXEL capture (macOS only — the
# Metal headless renderer; on Linux no renderer exists and frames are skipped).
# Writes a frame sequence + events.jsonl into
# .observability/captures/<stamp>-gherkin/, refreshes runs.json + captures.json,
# then serves the report so the flipbook is immediately viewable at
# http://127.0.0.1:8000/observability/report/.
gherkin-replay-capture feature='frontends/gpui/tests/features/ordinary_block_interaction.feature':
    #!/usr/bin/env bash
    set -euo pipefail
    stamp="$(date -u +%Y%m%dT%H%M%SZ)"
    dir=".observability/captures/${stamp}-gherkin"
    mkdir -p "$dir"
    echo "capturing frames to $dir"
    HOLON_CAPTURE_DIR="$dir" GHERKIN_FEATURE="$(pwd)/{{feature}}" \
        cargo test {{CANON}} --test gpui_gherkin_replay -- --test-threads=1
    just obs-collect
    echo "frames: $dir"
