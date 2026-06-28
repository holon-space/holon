# Holon project task runner

set dotenv-load

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

# Run all PBTs sequentially
pbt-all cases='32':
    just pbt general {{cases}}
    just pbt petri {{cases}}
    just pbt orgmode {{cases}}
    just pbt loro {{cases}}

# --- Predefined slices (ADR 0009: declare_pbt_slice! / component_pbt!) --------
# Slices are discovered from source — no hardcoded list. Each `test_fn:` in
# crates/holon-integration-tests/tests/ is one runnable slice; the file stem may
# differ from the slice name (one file can declare several slices), so slices are
# run by exact test-fn name. `pbt` is a default feature of holon-integration-tests.

_slice_dir := "crates/holon-integration-tests/tests"

# Discover every predefined slice with the ComponentSet/Wiring it composes.
pbt-list:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{justfile_directory()}}
    printf '%-32s %-22s %s\n' SLICE COMPOSITION FILE
    printf '%-32s %-22s %s\n' '-----' '-----------' '----'
    rg -lU 'test_fn:' {{_slice_dir}} --type rust | sort | while read -r f; do
        rg -UoN 'test_fn:\s*([A-Za-z0-9_]+)\s*,\s*(?:wiring|set):\s*([^,\n]+)' \
           -r '$1|$2' "$f" \
        | sed -E 's/holon_pbt_core:://; s/Wiring:://; s/ComponentSet:://' \
        | while IFS='|' read -r name comp; do
            printf '%-32s %-22s %s\n' "$name" "$comp" "$(basename "$f")"
          done
    done

# Run one predefined slice by exact name; e.g. `just pbt-slice storage_consistency_pbt 64`.
pbt-slice name cases='64' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    cd {{justfile_directory()}}
    file=$(rg -lU 'test_fn:\s*{{name}}\b' {{_slice_dir}} --type rust | head -1 || true)
    if [ -z "${file:-}" ]; then
        echo "Unknown slice '{{name}}'. Available:" >&2
        just pbt-list >&2
        exit 1
    fi
    stem=$(basename "$file" .rs)
    echo ">>> slice {{name}}  (binary: $stem, cases: {{cases}})"
    PROPTEST_CASES={{cases}} cargo test -p holon-integration-tests \
        --test "$stem" -- --exact {{name}} --nocapture {{FLAGS}} \
        2>&1 | tee "/tmp/pbt-slice-{{name}}.log"

# Run every discovered slice sequentially; continues on failure, summary at end.
pbt-slices cases='32':
    #!/usr/bin/env bash
    set -uo pipefail
    cd {{justfile_directory()}}
    slices=$(rg -UoN 'test_fn:\s*([A-Za-z0-9_]+)' -r '$1' {{_slice_dir}} --type rust | sort -u)
    echo "Discovered $(echo "$slices" | wc -l | tr -d ' ') slices."
    failed=""
    count=0
    while read -r s; do
        [ -z "$s" ] && continue
        count=$((count + 1))
        echo ""
        echo "=== $s ==="
        just pbt-slice "$s" {{cases}} || failed="$failed $s"
    done <<< "$slices"
    echo ""
    if [ -n "$failed" ]; then
        echo "Failed slices:$failed"
        exit 1
    fi
    echo "All $count slices passed."

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
    cargo clippy --workspace --all-targets 2>&1 \
        | tee /tmp/holon-analyze-clippy.log

# Copy-paste / duplication detection via polydup.
analyze-duplication:
    polydup scan . 2>&1 | tee /tmp/holon-analyze-duplication.log

# Architecture lints (cycles, banned imports, etc.).
analyze-arch:
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
