#!/usr/bin/env bash
# test-command-features.sh : for fixed and drawn sets of cargo packages, `test-command` enables exactly the features of select-tests.py's canonical list that belong to the selected packages, and cargo accepts the package and feature selection it prints.
set -euo pipefail
hook=$(cd "${0%/*}/.." && pwd)/hook
RANDOM=${SEED:-1080}; draws=${DRAWS:-25}
mapfile -t all < <(cargo metadata --no-deps --format-version 1 --offline | jq -r '.packages[].name')
mapfile -t canon < <(python3 -c 'import importlib.util as u, pathlib, sys
s = u.spec_from_file_location("s", "scripts/select-tests.py"); m = u.module_from_spec(s); s.loader.exec_module(m)
print("\n".join(m.canon_features(pathlib.Path("."))))')
cases=("holon-gpui" "holon-api" "holon-api holon-integration-tests" "holon-gpui holon-integration-tests holon-api")
for ((i = 0; i < draws; i++)); do
  n=$((RANDOM % 4 + 1)) set=()
  for ((j = 0; j < n; j++)); do set+=("${all[RANDOM % ${#all[@]}]}"); done
  cases+=("$(printf '%s\n' "${set[@]}" | sort -u | paste -sd' ' -)")
done
fail=0 err=$(mktemp)
for c in "${cases[@]}"; do
  read -ra units <<<"$c"
  want=$(for f in "${canon[@]}"; do for u in "${units[@]}"; do if [ "${f%%/*}" = "$u" ]; then echo "$f"; fi; done; done | paste -sd, -)
  mapfile -t words < <("$hook" test-command /nonexistent-build-ws "${units[@]}")
  got="" args=()
  for ((k = 0; k < ${#words[@]}; k++)); do
    case ${words[k]} in
      --features) got=${words[k + 1]}; args+=(--features "$got") ;;
      -p) args+=(-p "${words[k + 1]}") ;;
    esac
  done
  [ "$got" = "$want" ] || { echo "FAIL $c: features '$got', want '$want'"; fail=1; continue; }
  cargo tree --offline -e normal --depth 0 "${args[@]}" >/dev/null 2>"$err" \
    || { echo "FAIL $c: cargo refuses ${args[*]}: $(tail -1 "$err")"; fail=1; continue; }
  echo "ok $c: features '$got'"
done
exit $fail
