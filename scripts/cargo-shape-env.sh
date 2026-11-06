# Sourced, never run: the one source of the build-script env every gate step,
# lane check and rerun builds with. A cargo call with another value of a
# variable a build script reruns on rebuilds that crate and all above it.
#   source scripts/cargo-shape-env.sh gate   the integration checkout: also pins
#                                            HOLON_BUILD_SHA to @- (needs an empty @)
#   source scripts/cargo-shape-env.sh lane   a lane: HOLON_BUILD_SHA stays unset
# Both put scripts/cargo-shape first on PATH, so every cargo call is recorded in
# $HOLON_CARGO_RECORD (default target/cargo-shape/calls.jsonl) for
# scripts/check-compile-shape.py. `shape_env_header` prints the values for a log.
cargo_shape_env() {
  local root mode=${1:-}
  root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
  case $mode in gate | lane) ;; *) echo "shape env: mode must be gate or lane, got '$mode'" >&2; return 2 ;; esac
  RUST_FONTCONFIG_DLOPEN=$(just --justfile "$root/justfile" --evaluate RUST_FONTCONFIG_DLOPEN) \
    || { echo "shape env: just --evaluate RUST_FONTCONFIG_DLOPEN failed" >&2; return 1; }
  [ -n "$RUST_FONTCONFIG_DLOPEN" ] || { echo "shape env: RUST_FONTCONFIG_DLOPEN is empty in the justfile" >&2; return 1; }
  export RUST_FONTCONFIG_DLOPEN
  [ -z "${HOLON_BUILD_TIME+x}" ] || { echo "shape env: HOLON_BUILD_TIME is set; test builds leave it unset" >&2; return 1; }
  if [ "$mode" = gate ]; then
    [ -z "$(jj diff -r @ --summary)" ] || { echo "shape env: @ carries edits; the gate shape needs the integrated tip" >&2; return 1; }
    HOLON_BUILD_SHA=$(jj log -r @- --no-graph -T 'commit_id.short(12)') \
      && [ -n "$HOLON_BUILD_SHA" ] || { echo "shape env: HOLON_BUILD_SHA: jj log -r @- failed" >&2; return 1; }
    export HOLON_BUILD_SHA
  fi
  export HOLON_CARGO_RECORD=${HOLON_CARGO_RECORD:-$root/target/cargo-shape/calls.jsonl}
  case ":$PATH:" in *":$root/scripts/cargo-shape:"*) ;; *) export PATH="$root/scripts/cargo-shape:$PATH" ;; esac
}

shape_env_header() {
  local v names
  for v in RUST_FONTCONFIG_DLOPEN HOLON_BUILD_SHA HOLON_BUILD_TIME RUSTFLAGS; do
    if [ -n "${!v+x}" ]; then echo "shape env: $v=${!v}"; else echo "shape env: $v unset"; fi
  done
  # Names only: a CARGO_* value can be a registry token.
  names=$(compgen -e | grep '^CARGO_' | tr '\n' ' ' || true)
  echo "shape env: CARGO_* set: ${names:-none}"
}

cargo_shape_env "$@"
