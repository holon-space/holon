#!/usr/bin/env bash
# test-command-scope.sh : the hook says `scope` in `capabilities`, and for fixed and drawn scope files of real workspace paths, `test-command --scope` selects a package whole when a listed file of it lies outside its tests folder, else only test targets of it that the listed test files concern; files outside the units change nothing.
set -uo pipefail
hook=$(cd "${0%/*}/.." && pwd)/hook
RANDOM=${SEED:-1080}; draws=${DRAWS:-40}
"$hook" capabilities 2>/dev/null | grep -qx scope || { echo "FAIL capabilities: no scope"; exit 1; }
meta=$(cargo metadata --no-deps --format-version 1 --offline)
root=$(pwd -P)/
dir_of() { jq -r --arg n "$1" --arg w "$root" '.packages[] | select(.name == $n) | .manifest_path | ltrimstr($w) | rtrimstr("/Cargo.toml")' <<<"$meta"; }
targets_of() { jq -r --arg n "$1" '.packages[] | select(.name == $n) | .targets[] | select(.kind == ["test"]) | .name' <<<"$meta"; }
# shellcheck disable=SC2016
target_at() { jq -r --arg n "$1" --arg f "$root$2" '.packages[] | select(.name == $n) | .targets[] | select(.kind == ["test"] and .src_path == $f) | .name' <<<"$meta"; }
mapfile -t tested < <(jq -r '.packages[] | select(any(.targets[]; .kind == ["test"])) | .name' <<<"$meta")
files_of() { (cd "$(dir_of "$1")" && find "$2" -name '*.rs' 2>/dev/null | sort | sed "s#^#$(dir_of "$1")/#"); }
s=$(mktemp)
fail=0
check() { # check <label> <unit>... -- <scope path>...
  local label=$1; shift; local units=() paths=()
  while [ "$1" != -- ]; do units+=("$1"); shift; done; shift; paths=("$@")
  printf '%s\n' "${paths[@]}" > "$s"
  local out; out=$("$hook" test-command --scope "$s" /nonexistent-build-ws "${units[@]}" 2>&1) || { echo "FAIL $label: exit non-zero: $out"; fail=1; return; }
  local expr; expr=$(grep -A1 -x -- -E <<<"$out" | tail -1)
  local ps; ps=$(grep -A1 -x -- -p <<<"$out" | grep -vx -- -p | grep -vx -- -- | sort | paste -sd' ' -)
  local want_ps=() u
  for u in "${units[@]}"; do
    local d; d=$(dir_of "$u"); local mine=() p
    for p in "${paths[@]}"; do
      [[ $p == "$d/"* ]] || continue
      # a path of a nested package belongs to that package
      local o other=""; for o in "${units[@]}"; do local od; od=$(dir_of "$o"); [ "$o" != "$u" ] && [[ $od == "$d/"* && $p == "$od/"* ]] && other=1; done
      [ -z "$other" ] && mine+=("$p")
    done
    [ ${#mine[@]} -gt 0 ] || continue
    want_ps+=("$u")
    local whole=""; for p in "${mine[@]}"; do [[ $p == "$d/tests/"* ]] || whole=1; done
    if [ -n "$whole" ]; then
      grep -qF "package(=$u)" <<<"$expr" && ! grep -qF "binary_id(=$u::" <<<"$expr" \
        || { echo "FAIL $label: $u has a file outside tests/, the filter is '$expr'"; fail=1; }
    else
      grep -qF "package(=$u)" <<<"$expr" && { echo "FAIL $label: $u has only test files, the filter runs it whole: '$expr'"; fail=1; }
      local t; for t in $(grep -oE "binary_id\(=$u::[^)]*\)" <<<"$expr" | sed -E 's/.*::(.*)\)/\1/'); do
        targets_of "$u" | grep -qx "$t" || { echo "FAIL $label: $u has no test target $t"; fail=1; }
      done
      for p in "${mine[@]}"; do
        t=$(target_at "$u" "$p"); [ -z "$t" ] || grep -qF "binary_id(=$u::$t)" <<<"$expr" || { echo "FAIL $label: $p is the root of $u::$t, the filter is '$expr'"; fail=1; }
      done
      grep -qF "binary_id(=$u::" <<<"$expr" || { echo "FAIL $label: no target of $u in '$expr'"; fail=1; }
    fi
  done
  [ "$ps" = "$(printf '%s\n' "${want_ps[@]}" | sort | paste -sd' ' -)" ] || { echo "FAIL $label: packages '$ps', want '${want_ps[*]}'"; fail=1; }
  echo "ok $label"
}
check "one test file of holon-gpui" holon-gpui -- frontends/gpui/tests/accordion_bounded_pbt.rs
[ "$(grep -c binary_id <<<"$("$hook" test-command --scope "$s" /x holon-gpui)")" = 1 ] || { echo "FAIL one test file: not exactly one target"; fail=1; }
check "one module of a single test binary" holon-frontend -- crates/holon-frontend/tests/accordion.rs
check "src file" holon-api -- crates/holon-api/src/lib.rs
check "src and test file of two units" holon-api holon-gpui -- crates/holon-api/src/lib.rs frontends/gpui/tests/accordion_bounded_pbt.rs
check "outside file only next to a test file" holon-gpui -- docs/Architecture.md frontends/gpui/tests/accordion_bounded_pbt.rs
for ((i = 0; i < draws; i++)); do
  n=$((RANDOM % 3 + 1)) units=() paths=()
  for ((j = 0; j < n; j++)); do units+=("${tested[RANDOM % ${#tested[@]}]}"); done
  mapfile -t units < <(printf '%s\n' "${units[@]}" | sort -u)
  for u in "${units[@]}"; do
    mapfile -t pool < <(files_of "$u" tests; ((RANDOM % 3)) || files_of "$u" src)
    [ ${#pool[@]} -gt 0 ] || continue
    for ((j = RANDOM % 3; j >= 0; j--)); do paths+=("${pool[RANDOM % ${#pool[@]}]}"); done
  done
  ((RANDOM % 2)) && paths+=(docs/Architecture.md)
  check "draw $i: ${units[*]} :: ${paths[*]}" "${units[@]}" -- "${paths[@]}"
done
exit $fail
