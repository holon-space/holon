# Holon project task runner

set dotenv-load

# List available recipes
default:
    @just --list

# --- Property-Based Tests ---------------------------------------------------

# Run a PBT by name: general, petri, orgmode, loro
pbt name='general' cases='64' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{name}}" in
        general)
            PROPTEST_CASES={{cases}} cargo test \
                -p holon-integration-tests --test general_e2e_pbt \
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
                -p holon --lib api::loro_backend_pbt \
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

# --- Build & Check ----------------------------------------------------------

# Workspace build
build *FLAGS:
    cargo build --workspace {{FLAGS}} 2>&1 | tee /tmp/holon-build.log

# Clippy across workspace
clippy:
    cargo clippy --workspace --all-targets 2>&1 | tee /tmp/holon-clippy.log

# Run all workspace tests (not PBTs — those are slow)
test:
    cargo test --workspace 2>&1 | tee /tmp/holon-test.log

# --- Flutter Frontend (submodule) -------------------------------------------

mod flutter 'frontends/flutter'

# --- Profiling -------------------------------------------------------------

# Profile a PBT with samply (opens Firefox Profiler UI)
profile name='petri' cases='4' *FLAGS:
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{name}}" in
        general)  pkg="holon-integration-tests"; test="general_e2e_pbt" ;;
        petri)    pkg="holon"; test="petri_e2e_pbt" ;;
        orgmode)  pkg="holon-orgmode"; test="round_trip_pbt" ;;
        *)        echo "Unknown: {{name}}"; exit 1 ;;
    esac
    bin=$(cargo test -p "$pkg" --test "$test" --no-run --message-format=json 2>/dev/null \
        | jq -r 'select(.executable) | .executable' | head -1)
    PROPTEST_CASES={{cases}} samply record "$bin" --nocapture {{FLAGS}}

# Sample stack traces of a stuck PBT (finds the right child process automatically)
sample-pbt name='general' cases='1' duration='5':
    #!/usr/bin/env bash
    set -euo pipefail
    case "{{name}}" in
        general)  pkg="holon-integration-tests"; test="general_e2e_pbt" ;;
        petri)    pkg="holon"; test="petri_e2e_pbt" ;;
        orgmode)  pkg="holon-orgmode"; test="round_trip_pbt" ;;
        *)        echo "Unknown: {{name}}"; exit 1 ;;
    esac
    bin=$(cargo test -p "$pkg" --test "$test" --no-run --message-format=json 2>/dev/null \
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
