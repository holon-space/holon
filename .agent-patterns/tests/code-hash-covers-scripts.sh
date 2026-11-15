#!/usr/bin/env bash
# code-hash-covers-scripts.sh : in a fixture tree, `code-hash` changes when a file under scripts/, crates/*/src or frontends/*/src changes, is added or is removed, and stays the same when a file elsewhere or a Python bytecode cache under scripts/ changes.
set -uo pipefail
hook=$(cd "${0%/*}/.." && pwd)/hook
t=$(mktemp -d)
mkdir -p "$t/crates/a/src" "$t/crates/a/tests" "$t/frontends/b/src" "$t/scripts/flow-checks" "$t/docs"
for f in crates/a/src/lib.rs crates/a/tests/t.rs frontends/b/src/main.rs scripts/x.py scripts/flow-checks/test-y.sh docs/d.md justfile; do echo base > "$t/$f"; done
hash() { (cd "$t" && "$hook" code-hash) | awk '{print $1}'; }
fail=0
expect() { # expect same|changed <label> <shell edit in the fixture>
  local before after; before=$(hash); (cd "$t" && eval "$3"); after=$(hash)
  if [ "$1" = same ] && [ "$before" != "$after" ]; then echo "FAIL $2: code-hash changed"; fail=1; fi
  if [ "$1" = changed ] && [ "$before" = "$after" ]; then echo "FAIL $2: code-hash unchanged"; fail=1; fi
}
expect changed "edit under scripts/" 'echo edit >> scripts/x.py'
expect changed "edit in a folder below scripts/" 'echo edit >> scripts/flow-checks/test-y.sh'
expect changed "new file under scripts/" 'echo new > scripts/test-new.py'
expect changed "removed file under scripts/" 'rm scripts/test-new.py'
expect changed "edit under crates/*/src" 'echo edit >> crates/a/src/lib.rs'
expect changed "edit under frontends/*/src" 'echo edit >> frontends/b/src/main.rs'
expect same "edit under docs/" 'echo edit >> docs/d.md'
expect same "edit of a root file" 'echo edit >> justfile'
expect same "edit under crates/*/tests" 'echo edit >> crates/a/tests/t.rs'
expect same "bytecode cache under scripts/" 'mkdir -p scripts/__pycache__ && echo pyc > scripts/__pycache__/x.cpython-313.pyc'
rm -rf "$t"
exit $fail
