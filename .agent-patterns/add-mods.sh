#!/usr/bin/env bash
# add-mods.sh <lanes dir> <workspace> <crate> : appends each `mod` line of <lanes dir>/tester-mods.txt (`<crate> <mod line>`) to the crate's single test binary <tests dir>/main.rs when it is missing; the tests dir is the hook's `layout` answer.
set -euo pipefail
R=$1 ws=$2 c=$3 here=$(cd "${0%/*}" && pwd)
[ -s "$R/tester-mods.txt" ] || exit 0
# A crate with no mod line (one test binary per file) needs no tests/main.rs.
grep -q "^$c " "$R/tester-mods.txt" || exit 0
d=$(cd "$here/.." && "$here/hook" layout "$c" | sed -n 's/^tests: //p')
m=$ws/$d/main.rs
[ -f "$m" ] || { echo "$m missing: the crate has no single test binary" >&2; exit 1; }
sed -n "s/^$c //p" "$R/tester-mods.txt" | while IFS= read -r l; do grep -qxF "$l" "$m" || echo "$l" >> "$m"; done
