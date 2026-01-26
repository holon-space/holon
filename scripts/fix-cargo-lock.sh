#!/usr/bin/env bash
# jj fix tool: regenerate Cargo.lock from Cargo.toml files
# stdin is ignored; stdout must be the updated Cargo.lock
cat > /dev/null
cargo generate-lockfile --manifest-path "$(git rev-parse --show-toplevel)/Cargo.toml" 2>/dev/null
cat "$(git rev-parse --show-toplevel)/Cargo.lock"
