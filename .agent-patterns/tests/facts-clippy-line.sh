#!/usr/bin/env bash
# facts-clippy-line.sh : for fixed and drawn sets of cargo packages, the clippy command of the `prompt-facts` lint rule selects the same packages and features as `test-command` and passes the justfile's CLIPPY_LINTS.
set -uo pipefail
hook=$(cd "${0%/*}/.." && pwd)/hook
RANDOM=${SEED:-1}; draws=${DRAWS:-15}
mapfile -t all < <(cargo metadata --no-deps --format-version 1 --offline | jq -r '.packages[].name')
lints=$(sed -n 's/^CLIPPY_LINTS := "\(.*\)"$/\1/p' justfile)
[ -n "$lints" ] || { echo "FAIL: the justfile holds no CLIPPY_LINTS"; exit 1; }
cases=("holon-gpui" "holon-api" "holon-gpui holon-integration-tests")
for ((i = 0; i < draws; i++)); do
  set=(); for ((j = RANDOM % 3; j >= 0; j--)); do set+=("${all[RANDOM % ${#all[@]}]}"); done
  cases+=("$(printf '%s\n' "${set[@]}" | sort -u | paste -sd' ' -)")
done
fail=0
for c in "${cases[@]}"; do
  read -ra units <<<"$c"
  # the selection words of the test run: every -p and --features pair, in order
  want=$("$hook" test-command /x "${units[@]}" | paste -sd' ' - | grep -oE '(-p|--features) [^ ]+' | paste -sd' ' -)
  line=$("$hook" prompt-facts "${units[@]}" | awk '/^@@ LINT_RULE$/{f=1;next} /^@@ /{f=0} f' | grep -oE 'cargo clippy [^>]*')
  sel=$(grep -oE '(-p|--features) [^ ]+' <<<"${line%% -- *}" | paste -sd' ' -)
  [ "$sel" = "$want" ] || { echo "FAIL $c: clippy selects '$sel', the test run '$want'"; fail=1; }
  [ "$(sed 's/ *$//' <<<"${line#* -- }")" = "$lints" ] || { echo "FAIL $c: clippy lints '${line#* -- }', the justfile '$lints'"; fail=1; }
  echo "ok $c"
done
exit $fail
