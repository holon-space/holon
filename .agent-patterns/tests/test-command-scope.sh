#!/usr/bin/env bash
# test-command-scope.sh : the hook says `scope` in `capabilities`, and for fixed and drawn scope files of real workspace paths, `test-command --scope` selects a package whole when a listed file of it lies outside its tests folder, else only test targets of it that the listed test files concern; files outside the units change nothing.
set -uo pipefail
hook=$(cd "${0%/*}/.." && pwd)/hook
RANDOM=${SEED:-1}; draws=${DRAWS:-40}
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
# Where the package lists its targets itself, register-tests declares a new file in the single test binary tests/main.rs.
single_binary_check() { # single_binary_check <unit> <new file name>: the filter names the target rooted at tests/main.rs
  local u=$1 p t; p=$(dir_of "$u")/tests/$2; t=$(target_at "$u" "$(dir_of "$u")/tests/main.rs"); printf '%s\n' "$p" > "$s"
  local out; out=$("$hook" test-command --scope "$s" /x "$u" 2>&1)
  if grep -qF "binary_id(=$u::$t)" <<<"$out"; then echo "ok single binary $p"; else echo "FAIL $p: the new file joins $u::$t, the run is: $out"; fail=1; fi
}
mapfile -t single < <(for u in "${tested[@]}"; do grep -q 'autotests *= *false' "$(dir_of "$u")/Cargo.toml" && [ -n "$(target_at "$u" "$(dir_of "$u")/tests/main.rs")" ] && echo "$u"; done)
single_binary_check holon-integration-tests added_by_tester_pbt.rs
for ((i = 0; i < draws; i++)); do single_binary_check "${single[RANDOM % ${#single[@]}]}" "$(printf 'added_%s_pbt.rs' "$RANDOM")"; done
# Files under tests/ that are no Rust source (fixtures, snapshots, feature files) are read by test targets, never declared.
src_of() { jq -r --arg n "$1" --arg t "$2" '.packages[] | select(.name == $n) | .targets[] | select(.kind == ["test"] and .name == $t) | .src_path' <<<"$meta"; }
data_check() { # data_check <unit> <path>: the filter names only real test targets, and each target whose root names the file's tests/ subfolder
  local u=$1 p=$2 d; d=$(dir_of "$u"); printf '%s\n' "$p" > "$s"
  local out; out=$("$hook" test-command --scope "$s" /x "$u" 2>&1) || { echo "FAIL $p: exit non-zero: $out"; fail=1; return; }
  local expr; expr=$(grep -A1 -x -- -E <<<"$out" | tail -1)
  grep -qF "package(=$u)" <<<"$expr" && { echo "ok $p"; return; }
  local t; for t in $(grep -oE "binary_id\(=$u::[^)]*\)" <<<"$expr" | sed -E 's/.*::(.*)\)/\1/'); do
    targets_of "$u" | grep -qx "$t" || { echo "FAIL $p: $u has no test target $t, the filter is '$expr'"; fail=1; }
  done
  local sub=${p#"$d/tests/"}; sub=${sub%%/*}
  [ "$sub" != "${p##*/}" ] || { echo "ok $p"; return; }
  for t in $(targets_of "$u"); do
    grep -qF "$sub/" "$(src_of "$u" "$t")" && ! grep -qF "binary_id(=$u::$t)" <<<"$expr" && { echo "FAIL $p: $u::$t reads $sub/, the filter is '$expr'"; fail=1; }
  done
  echo "ok $p"
}
mapfile -t data_files < <(for u in "${tested[@]}"; do d=$(dir_of "$u"); [ -d "$d/tests" ] && find "$d/tests" -type f ! -name '*.rs' | sed "s#^#$u #"; done | sort)
data_check holon-filesystem crates/holon-filesystem/tests/fixtures/sync_base_legacy_sample.json
data_check holon-gpui frontends/gpui/tests/features/ordinary_block_interaction.feature
for ((i = 0; i < draws; i++)); do e=${data_files[RANDOM % ${#data_files[@]}]}; data_check "${e%% *}" "${e#* }"; done
# A new test file of the tester is in no target of the build workspace yet; register-tests adds its `mod` line later.
new_module_check() { # new_module_check <unit> <new file name>: the filter names only real test targets when the package lists its targets itself
  local u=$1 p; p=$(dir_of "$u")/tests/$2; printf '%s\n' "$p" > "$s"
  local out; out=$("$hook" test-command --scope "$s" /x "$u" 2>&1) || { echo "FAIL $p: exit non-zero: $out"; fail=1; return; }
  local expr; expr=$(grep -A1 -x -- -E <<<"$out" | tail -1)
  if [ "$(jq -r --arg n "$u" '.packages[] | select(.name == $n) | .targets | map(select(.kind == ["test"])) | length' <<<"$meta")" -gt 0 ] \
    && grep -q 'autotests *= *false' "$(dir_of "$u")/Cargo.toml"; then
    local t; for t in $(grep -oE "binary_id\(=$u::[^)]*\)" <<<"$expr" | sed -E 's/.*::(.*)\)/\1/'); do
      targets_of "$u" | grep -qx "$t" || { echo "FAIL $p: $u lists its test targets and has no $t, the filter is '$expr'"; fail=1; }
    done
  fi
  echo "ok $p"
}
new_module_check holon-integration-tests added_by_tester_pbt.rs
for ((i = 0; i < draws; i++)); do new_module_check "${tested[RANDOM % ${#tested[@]}]}" "$(printf 'added_%s_pbt.rs' "$RANDOM")"; done
# A path under tests/ that is no test file name and no target root (a data file next to the targets, a source file in a folder no target declares) concerns no new target.
odd_path_check() { # odd_path_check <unit> <path>: the filter names the package whole or only real test targets
  local u=$1 p=$2; printf '%s\n' "$p" > "$s"
  local out; out=$("$hook" test-command --scope "$s" /x "$u" 2>&1) || { echo "FAIL $p: exit non-zero: $out"; fail=1; return; }
  local expr t; expr=$(grep -A1 -x -- -E <<<"$out" | tail -1)
  for t in $(grep -oE "binary_id\(=$u::[^)]*\)" <<<"$expr" | sed -E 's/.*::(.*)\)/\1/'); do
    targets_of "$u" | grep -qx "$t" || { echo "FAIL $p: $u has no test target $t, the filter is '$expr'"; fail=1; return; }
  done
  echo "ok odd $p"
}
odd_path_check holon-api crates/holon-api/tests/added_snapshot.json
odd_path_check holon-api crates/holon-api/tests/added_support/helper.rs
for ((i = 0; i < draws; i++)); do
  u=${tested[RANDOM % ${#tested[@]}]}; exts=(json snap txt feature yaml)
  odd_path_check "$u" "$(dir_of "$u")/tests/added_$RANDOM.${exts[RANDOM % ${#exts[@]}]}"
  odd_path_check "$u" "$(dir_of "$u")/tests/added_$RANDOM/helper_$RANDOM.rs"
done
exit $fail
