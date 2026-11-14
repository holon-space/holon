#!/usr/bin/env bash
# register-tests-sorted.sh : for fixed and drawn rustfmt-clean test binaries (`mod` and `pub mod` lines) and drawn mods files, `register-tests` leaves the binary rustfmt-clean (the workspace's rustfmt.toml) with every old and new module declared exactly once.
set -uo pipefail
hook=$(cd "${0%/*}/.." && pwd)/hook
ws=$(pwd -P)
RANDOM=${SEED:-1}; draws=${DRAWS:-60}
crate=holon-frontend
tests=$("$hook" layout $crate | sed -n 's/^tests: //p')
fmt() { rustfmt --edition 2024 --config-path "$ws/rustfmt.toml" "$@"; }
name() { local parts=(a b z q_r x1 x2 x10 _y ab a_b B) n="" k; for ((k = RANDOM % 3; k >= 0; k--)); do n+=${parts[RANDOM % ${#parts[@]}]}; done; [[ $n == _* ]] && n=m$n; echo "$n"; }
fail=0
run() { # run <label> <main.rs text> <new mod name>...
  local label=$1 text=$2; shift 2
  local t; t=$(mktemp -d); mkdir -p "$t/$tests"; cp "$ws/rustfmt.toml" "$t/"
  printf '%s\n' "$text" | fmt > "$t/$tests/main.rs" || { echo "SKIP $label: the drawn binary is no Rust"; return; }
  local n; for n in "$@"; do echo "$crate mod $n;"; done > "$t/tester-mods.txt"
  (cd "$t" && "$hook" register-tests "$t/tester-mods.txt" $crate) || { echo "FAIL $label: register-tests exit non-zero"; fail=1; return; }
  # rustfmt --check on stdin exits 0 on a diff, so compare its output instead
  fmt < "$t/$tests/main.rs" | diff "$t/$tests/main.rs" - > "$t/diff" || { echo "FAIL $label: not rustfmt-clean:"; cat "$t/diff"; fail=1; }
  local want n; mapfile -t want < <(sed -nE 's/^(pub )?mod ([A-Za-z0-9_]+);$/\2/p' <<<"$text"; printf '%s\n' "$@")
  for n in "${want[@]}"; do
    [ "$(grep -cxE "(pub )?mod $n;" "$t/$tests/main.rs")" = 1 ] || { echo "FAIL $label: module $n is not declared exactly once"; fail=1; }
  done
  echo "ok $label"
  rm -rf "$t"
}
run "new mod before the old ones" $'mod b;\nmod c;' a
run "new mod in a binary with items after the mods" $'use std::fmt;\n\nmod b;\nmod d;\n\nfn helper() {}' c a
run "new mod equal to an old one" $'mod a;\nmod b;' a
run "new mods in an empty binary" '' b a
run "new mod equal to an old pub one" $'pub mod a;\nmod b;' a
run "found by a draw: new mods next to an old pub one of the same name" $'mod m_y;\npub mod x2;' bB Bx2x2 x2
for ((i = 0; i < draws; i++)); do
  old=() new=()
  for ((j = RANDOM % 6; j > 0; j--)); do old+=("$(name)"); done
  for ((j = RANDOM % 3; j >= 0; j--)); do new+=("$(name)"); done
  mapfile -t old < <(printf '%s\n' "${old[@]}" | sort -u | sed '/^$/d'); mapfile -t new < <(printf '%s\n' "${new[@]}" | sort -u)
  text=$( ((RANDOM % 2)) && printf 'use std::fmt;\n\n'; for o in "${old[@]}"; do ((RANDOM % 3)) || printf 'pub '; echo "mod $o;"; done; ((RANDOM % 2)) && printf '\nfn helper() {}\n')
  run "draw $i: old ${old[*]} new ${new[*]}" "$text" "${new[@]}"
done
exit $fail
