#!/usr/bin/env bash
# Build one guest to `wasm32-unknown-unknown` and install it beside its
# sidecar. `just guests-verify` compares the result's sha256 against the
# tracked artifact, so the bytes must be a function of the sources, the
# toolchain and the stage root alone: cargo hashes a path package's absolute
# directory into `-C metadata`, which feeds symbol disambiguators and reorders
# the LTO output, and a floating `nightly` channel picks up a new compiler
# every day. The sources are therefore staged at $HOLON_GUEST_STAGE/<guest>, a
# path that depends on nothing but the guest's name, and built with the channel
# pinned in the repository's rust-toolchain.toml.
set -euo pipefail
export PATH=/opt/homebrew/opt/rustup/bin:$PATH
export RUSTC_WRAPPER=
export CARGO_BUILD_JOBS=6

GUEST=${1:?usage: build.sh <guest-dir-name> [install-dir]}
HERE=$(cd "$(dirname "$0")" && pwd)
# Resolved before the `cd` below, so a relative install dir means what the
# caller typed rather than a path under the guest's own directory.
INSTALL_DIR=$(cd "${2:-$HERE/../crates/holon-plugin-host/plugins}" && pwd)
OUT=$INSTALL_DIR/$GUEST.wasm

TOOLCHAIN_FILE=$HERE/../rust-toolchain.toml
CHANNEL=$(sed -n 's/^[[:space:]]*channel[[:space:]]*=[[:space:]]*"\([^"]*\)".*/\1/p' "$TOOLCHAIN_FILE")
if [ -z "$CHANNEL" ]; then
    echo "guests/build.sh: $TOOLCHAIN_FILE has no channel line" >&2
    exit 1
fi
export RUSTUP_TOOLCHAIN=$CHANNEL

if ! rustup target list --installed --toolchain "$CHANNEL" | grep -qx wasm32-unknown-unknown; then
    rustup target add wasm32-unknown-unknown --toolchain "$CHANNEL"
fi

# Restaging key. Only files cargo reads are hashed: a stray `target/` left in a
# checkout by an older build must not count as a source.
SRC_HASH=$(cd "$HERE" && find "$GUEST/src" "$GUEST/Cargo.toml" "$GUEST/Cargo.lock" \
    abi-guest/src abi-guest/Cargo.toml -type f \
    | sort | xargs shasum -a 256 | shasum -a 256 | cut -c1-16)

STAGE_ROOT=${HOLON_GUEST_STAGE:-/tmp/holon-guest-build}
mkdir -p "$STAGE_ROOT/$GUEST/guests"
STAGE=$(cd "$STAGE_ROOT/$GUEST" && pwd -P)

# Lanes on one machine share the stage root, and a weave runs `just
# guests-verify` in several workspaces at once. One stage dir per guest, so
# hold it exclusively for the whole build.
LOCK=$STAGE/.lock
waited=0
until mkdir "$LOCK" 2>/dev/null; do
    if [ -n "$(find "$LOCK" -maxdepth 0 -mmin +15 2>/dev/null)" ]; then
        rmdir "$LOCK" 2>/dev/null || true
        continue
    fi
    if [ "$waited" -ge 1800 ]; then
        echo "guests/build.sh: $LOCK still held after 30 min; remove it if no build is running" >&2
        exit 1
    fi
    sleep 1
    waited=$((waited + 1))
done
trap 'rmdir "$LOCK" 2>/dev/null || true' EXIT

if [ "$(cat "$STAGE/guests/.staged" 2>/dev/null || true)" != "$SRC_HASH" ]; then
    for pkg in "$GUEST" abi-guest; do
        rm -rf "${STAGE:?}/guests/$pkg"
        cp -R "$HERE/$pkg" "$STAGE/guests/$pkg"
        rm -rf "$STAGE/guests/$pkg/target"
    done
    echo "$SRC_HASH" > "$STAGE/guests/.staged"
fi

CARGO_HOME_DIR=$(cd "${CARGO_HOME:-$HOME/.cargo}" && pwd)
export RUSTFLAGS="--remap-path-prefix=$STAGE=/holon --remap-path-prefix=$CARGO_HOME_DIR=/cargo"

cd "$STAGE/guests/$GUEST"
cargo build --locked --release --target wasm32-unknown-unknown
BUILT=$(ls target/wasm32-unknown-unknown/release/*.wasm | head -1)

# No wasm-opt. A hash gate may not vary with what is installed, and the release
# profile already carries opt-level="z", LTO, codegen-units=1, panic=abort and
# strip.
cp "$BUILT" "$OUT"

echo "guest $GUEST: $(wc -c < "$OUT") bytes raw, $(gzip -c "$OUT" | wc -c) bytes gzipped"
