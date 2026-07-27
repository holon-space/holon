# Holon project task runner

set dotenv-load

# GPUI/blinc builds link libfontconfig on Linux; RUST_FONTCONFIG_DLOPEN=on makes the
# fontconfig -sys build script dlopen it at runtime instead of static-linking (needed
# for fresh clones/CI/jj workspaces). Previously lived in an untracked .env; tracked
# here so every checkout inherits it. `export` puts it in recipes' environment.
export RUST_FONTCONFIG_DLOPEN := "on"

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
    case "{{name}}" in
        general)
            PROPTEST_CASES={{cases}} cargo test \
                -p holon-integration-tests --features pbt --test general_e2e_composed_pbt \
                -- --nocapture {{FLAGS}} 2>&1 | tee /tmp/pbt-general.log
            ;;
        petri)
            PROPTEST_CASES={{cases}} cargo test \
                -p holon --test petri_e2e_pbt \
                -- --nocapture {{FLAGS}} 2>&1 | tee /tmp/pbt-petri.log
            ;;
        orgmode)
            PROPTEST_CASES={{cases}} cargo test \
                -p holon-orgmode --test round_trip_pbt \
                -- --nocapture {{FLAGS}} 2>&1 | tee /tmp/pbt-orgmode.log
            ;;
        loro)
            PROPTEST_CASES={{cases}} cargo test \
                -p holon --test api_suite loro_backend_pbt \
                -- --nocapture {{FLAGS}} 2>&1 | tee /tmp/pbt-loro.log
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

# Replay hand-authored keystone regressions (concrete transition sequences from
# docs/Testing/HandAuthoredRegressions.md) through the keystone harness. Same
# --features pbt gate as the keystone; fail-loud on any red or parse error.
hand-authored *FLAGS:
    #!/usr/bin/env bash
    # pipefail is REQUIRED: without it the recipe's exit status is `tee`'s, so a
    # failing suite exited 0 and every weave/land gate using this recipe was a
    # silent false green (observed 2026-07-25).
    set -euo pipefail
    # quarantined: turso IVM retract-race under concurrent CDC — BugFunnel row 2026-07-27; un-quarantine when the turso-side fix lands (delete this line)
    export HOLON_HAND_AUTHORED_SKIP=${HOLON_HAND_AUTHORED_SKIP-watch-matview-retains-outdent-intermediate-row}
    cargo test \
        -p holon-integration-tests --features pbt --test hand_authored_regressions \
        -- --nocapture {{FLAGS}} 2>&1 | tee /tmp/pbt-hand-authored.log

# Weave-time full keystone sweep (orchestrator-run, typically in background)
keystone-full cases='16':
    just pbt general {{cases}}

# Launch the app for live MCP-driven verification: throwaway config+vault, own
# MCP port (registry: 8710/8720/8730/... — pick one per verifier). Leaves the
# app running in the background; caller drives it over http://127.0.0.1:PORT/mcp
# and kills the printed pid when done.
live-verify port='8710' dir='/tmp/holon-live-verify':
    #!/usr/bin/env bash
    set -euo pipefail
    rm -rf "{{dir}}"
    mkdir -p "{{dir}}/config" "{{dir}}/vault"
    HOLON_CONFIG_DIR="{{dir}}/config" HOLON_VAULT_ROOT="{{dir}}/vault" \
        MCP_SERVER_PORT={{port}} cargo run -p holon-gpui \
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
    PROPTEST_CASES={{cases}} HOLON_PBT_EXTENDED_GEN=1 cargo test \
        -p holon-integration-tests --features pbt --test general_e2e_composed_pbt \
        -- --nocapture {{FLAGS}} 2>&1 | tee /tmp/pbt-extended-gen.log

# Layout-override sweep — the ONE keystone with the index.org override arms (prql/gql/sql
# layouts + profile file); HOLON_PBT_LAYOUT_OVERRIDE gates them in write_org_file.rs.
# Replaces the retired standalone layout_override_pbt. KNOWN RED (pre-existing): the link-mark
# [[label]] extraction divergence on UI-origin ApplyMutation Update on block:root-layout
# (capture /tmp/layout_full_8case_finding.captured.json) still reproduces here — triage via
# the keystone, then remove this note.
pbt-layout-override cases='64' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    PROPTEST_CASES={{cases}} HOLON_PBT_LAYOUT_OVERRIDE=1 cargo test \
        -p holon-integration-tests --features pbt --test general_e2e_composed_pbt \
        -- --nocapture {{FLAGS}} 2>&1 | tee /tmp/pbt-layout-override.log

# Measure end-to-end UI action latency (indent / outdent / cycle-state / split / ...).
# Drives the REAL pipeline (dispatch -> Loro commit -> LoroProjection resample ->
# Turso/matview CDC -> reactive rows) through the headless composed keystone with the
# `holon_latency` tracing target enabled, then prints a per-action count/p50/p95/max
# table plus per-stage cost. Measures everything EXCEPT final GPU paint. HOLON_OTEL_FILTER=off
# silences the OTel span layer so its recording cost doesn't distort the numbers.
measure-latency cases='16' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    RUST_LOG="holon_latency=debug" HOLON_OTEL_FILTER=off PROPTEST_CASES={{cases}} \
        cargo test -p holon-integration-tests --features pbt \
        --test general_e2e_composed_pbt -- --nocapture {{FLAGS}} \
        > /tmp/holon-latency.log 2>&1 || true
    echo "raw log: /tmp/holon-latency.log ($(grep -c holon_latency /tmp/holon-latency.log || true) events)"
    python3 scripts/measure_latency.py /tmp/holon-latency.log

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
    mkdir -p docs/Testing/soak
    stamp="$(date +%Y%m%d-%H%M%S)"
    log="/tmp/holon-soak-${stamp}.log"
    rss="/tmp/holon-soak-rss-${stamp}.csv"
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
        RUST_LOG="holon_latency=debug" HOLON_OTEL_FILTER=off PROPTEST_CASES="$cases" \
        cargo test -p holon-integration-tests --features pbt \
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
        python3 scripts/measure_latency.py "$log" --fail-over-p95 200 || true
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
    cargo nextest run -p holon-integration-tests --lib --features pbt \
        2>&1 | tee /tmp/pbt-lib-slices.log

# --- Mutation Testing -------------------------------------------------------

# Run cargo-mutants on a specific file (defaults to petri.rs)
mutants file='crates/holon/src/petri.rs' timeout='300':
    cargo mutants \
        --manifest-path crates/holon/Cargo.toml \
        --file {{file}} \
        --timeout {{timeout}} \
        --output /tmp/mutants-out 2>&1 | tee /tmp/mutants.log

# Show last mutants results
mutants-results:
    @cat /tmp/mutants-out/outcomes.json 2>/dev/null | python3 -m json.tool || echo "No results found. Run 'just mutants' first."

# --- Assets ----------------------------------------------------------------

# Download icons listed in assets/icons/manifest.toml
icons *FLAGS:
    ./assets/icons/download.sh {{FLAGS}}

# --- Build & Check ----------------------------------------------------------

# Workspace build
build *FLAGS: icons
    cargo build --workspace {{FLAGS}} 2>&1 | tee /tmp/holon-build.log

# Clippy across workspace
clippy:
    cargo clippy --workspace --all-targets 2>&1 | tee /tmp/holon-clippy.log

# Run all workspace tests (not PBTs — those are slow)
test:
    cargo nextest run --workspace 2>&1 | tee /tmp/holon-test.log

# Check the wasi worker frontend compiles (mirrors the CI rust-checks step).
# EMNAPI_LINK_DIR: napi-build's wasi shim demands it, but `cargo check` never
# links, so an empty stub dir is safe — the real dir comes from `napi build`
# (frontends/holon-worker/scripts/build.sh).
check-worker-wasm:
    #!/usr/bin/env bash
    set -euo pipefail
    rustup target add wasm32-wasip1-threads
    EMNAPI_LINK_DIR="$(mktemp -d)" cargo check \
        --manifest-path frontends/holon-worker/Cargo.toml \
        --target wasm32-wasip1-threads --features browser \
        2>&1 | tee /tmp/holon-worker-wasm-check.log

# --- Code Quality -----------------------------------------------------------

# Check formatting
fmt-check:
    cargo fmt --check

# Audit dependencies for vulnerabilities, license issues, and bans
deny:
    cargo deny check 2>&1 | tee /tmp/holon-deny.log

# Find unused dependencies
machete:
    cargo machete 2>&1 | tee /tmp/holon-machete.log

# Detect copy-pasted code (requires: npx or npm i -g jscpd)
duplication:
    npx jscpd . 2>&1 | tee /tmp/holon-duplication.log

# Run all lints and quality checks locally
lint:
    #!/usr/bin/env bash
    set -euo pipefail
    failed=0
    echo "=== cargo fmt ==="
    cargo fmt --check || { echo "FAIL: formatting"; failed=1; }
    echo ""
    echo "=== cargo clippy ==="
    cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tee /tmp/holon-clippy.log || { echo "FAIL: clippy"; failed=1; }
    echo ""
    echo "=== cargo deny ==="
    cargo deny check 2>&1 | tee /tmp/holon-deny.log || { echo "FAIL: deny"; failed=1; }
    echo ""
    echo "=== cargo machete ==="
    cargo machete 2>&1 | tee /tmp/holon-machete.log || { echo "FAIL: machete"; failed=1; }
    echo ""
    echo "=== jscpd (duplication) ==="
    npx jscpd . 2>&1 | tee /tmp/holon-duplication.log || { echo "FAIL: duplication"; failed=1; }
    echo ""
    if [ "$failed" -ne 0 ]; then
        echo "Some checks failed. See output above."
        exit 1
    fi
    echo "All checks passed."

# --- Code Analysis ----------------------------------------------------------
# Individual analyzers write logs to /tmp/holon-analyze-*.log so CI can collect.

# CRAP metric (complexity × inverse coverage). Requires lcov.info.
analyze-crap:
    #!/usr/bin/env bash
    set -euo pipefail
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
            | tee /tmp/holon-analyze-coverage.log
    fi
    # Threshold / examples-exclude / missing-coverage policy live in
    # .cargo-crap.toml and are picked up automatically from the repo root.
    # Human report — every function over threshold, for visibility.
    cargo crap --lcov lcov.info 2>&1 | tee /tmp/holon-analyze-crap.log
    # Regression gate — fail ONLY when a function's CRAP score rose vs the
    # recorded baseline. New code can't make the pre-existing hotspots worse;
    # the backlog (Phase 5) is paid down incrementally, not blocked. We compare
    # via tools/crap_check_regression.py rather than `cargo crap --fail-regression`
    # because the latter pairs functions by name only and mispairs the many
    # duplicate-named functions in this repo (see the script's docstring).
    # Regenerate the baseline with `just crap-baseline` after intentional changes.
    if [ -f crap-baseline.json ]; then
        cargo crap --lcov lcov.info --format json --output /tmp/holon-crap-current.json
        python3 tools/crap_check_regression.py \
            --baseline crap-baseline.json --current /tmp/holon-crap-current.json \
            2>&1 | tee -a /tmp/holon-analyze-crap.log
    else
        echo "No crap-baseline.json — skipping regression gate. Run 'just crap-baseline'." \
            | tee -a /tmp/holon-analyze-crap.log
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
    cargo deny check 2>&1 | tee /tmp/holon-analyze-deny.log

# Unused dependency detection.
analyze-machete:
    cargo machete 2>&1 | tee /tmp/holon-analyze-machete.log

# Lint with clippy at the workspace level.
# Report-only: clippy findings are surfaced but don't fail the recipe. Phase 6
# of the code-quality plan re-tightens this gate (`-D warnings`) once the
# workspace backlog has been paid down incrementally.
analyze-clippy:
    #!/usr/bin/env bash
    set -euo pipefail
    cargo clippy --workspace --all-targets 2>&1 \
        | tee /tmp/holon-analyze-clippy.log

# Copy-paste / duplication detection via polydup.
analyze-duplication:
    #!/usr/bin/env bash
    set -euo pipefail
    polydup scan . 2>&1 | tee /tmp/holon-analyze-duplication.log

# Architecture lints (cycles, banned imports, etc.).
analyze-arch:
    #!/usr/bin/env bash
    set -euo pipefail
    ./archlint/archlint --all 2>&1 | tee /tmp/holon-analyze-arch.log

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
# chrome-trace available for: gpui, blinc, ply
# Only kills the old app if the new build succeeds.
watch ui='gpui' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
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
    cargo build -p "holon-${UI}" --features chrome-trace 2>&1 | tee /tmp/holon-build.log
    restart_app

    # cargo-watch only builds; signals outer script on success
    cargo watch -s "cargo build -p holon-${UI} --features chrome-trace 2>&1 | tee /tmp/holon-build.log && kill -USR1 ${OUTER_PID} || echo '>>> Build failed — keeping old instance running <<<'" &
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
    PROPTEST_CASES={{cases}} "$bin" --nocapture > /tmp/pbt-sample-out.log 2>&1 &
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
        cat /tmp/pbt-sample-out.log
        exit 1
    fi
    echo "Sampling PID $leaf_pid for {{duration}}s..."
    sample "$leaf_pid" {{duration}} -f /tmp/pbt-sample.txt
    kill "$root_pid" 2>/dev/null || true
    pkill -P "$root_pid" 2>/dev/null || true
    echo "Stack trace saved to /tmp/pbt-sample.txt"
    echo ""
    echo "=== Top of stack (where time is spent) ==="
    grep -E '^\s+\d+\s' /tmp/pbt-sample.txt | sort -rn | head -20
    echo ""
    echo "=== Test output ==="
    tail -30 /tmp/pbt-sample-out.log

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

# Tier 1 pre-commit gate: defensive-code ratchet + workspace typecheck.
# MEASURED (2026-07-07): warm `cargo check --workspace` = 5.4s; ratchet ~5s CPU.
# A keystone smoke was CUT from this tier: even PROPTEST_CASES=2 takes ~4.5min
# because proptest unconditionally replays the persisted regression seeds and
# each case pays full composed-SUT boot — it belongs in `just prepush` (Tier 2).
# Assumes a warm build cache; the first run after a big rebase pays compile cost.
precommit:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "== Tier 1 [1/2]: defensive-code ratchet =="
    ./scripts/defensive-ratchet.sh
    echo "== Tier 1 [2/2]: cargo check --workspace =="
    cargo check --workspace 2>&1 | tee /tmp/precommit-check.log
    echo "== Tier 1 PASS =="

# Tier 2 pre-push gate: full keystone at default PROPTEST_CASES=16 (includes the
# persisted regression seeds in tests/general_e2e_composed_pbt.proptest-regressions).
# MEASURED (2026-07-07): green run ~5min quiet; a RED run that shrinks can take ~15min.
prepush:
    #!/usr/bin/env bash
    set -euo pipefail
    echo "== Tier 2: full keystone (PROPTEST_CASES=16) =="
    PROPTEST_CASES=16 cargo test \
        -p holon-integration-tests --features pbt --test general_e2e_composed_pbt \
        2>&1 | tee /tmp/prepush-keystone.log
    echo "== Tier 2 PASS =="
