#!/usr/bin/env bash
# The admission-site count sees call expressions only: an entry name inside a
# string literal, a doc comment or a trailing comment is not a call site.
set -euo pipefail
cd "$(dirname "$0")/.."

root=$(mktemp -d)
trap 'rm -rf "$root"' EXIT
mkdir -p "$root/crates/fixture/src"
cat >"$root/crates/fixture/src/lib.rs" <<'RUST'
/// Doc comment naming execute_operation(op) and dispatch_intent(intent).
pub fn describe() -> &'static str {
    let _ = "execute_operation(ask, answer, ...)"; // execute_intent(parts) in a comment
    // commit_keystroke(key) in a line comment
    "dispatch_intent_sync(intent)"
}
RUST

out=$(ADMISSION_SCAN_ROOTS="$root/crates" scripts/check-admission-sites.sh --print)
if [ -n "$out" ]; then
    echo "test-admission-sites-count: expected no call sites in the fixture, got:" >&2
    echo "$out" >&2
    exit 1
fi

cat >>"$root/crates/fixture/src/lib.rs" <<'RUST'

pub async fn real_site(engine: &Engine) {
    engine.execute_operation(op()).await;
}
RUST
out=$(ADMISSION_SCAN_ROOTS="$root/crates" scripts/check-admission-sites.sh --print)
expected="1 $root/crates/fixture/src/lib.rs"
if [ "$out" != "$expected" ]; then
    echo "test-admission-sites-count: expected '$expected', got '$out'" >&2
    exit 1
fi

echo "test-admission-sites-count: OK — text hits count 0, a real call counts 1"
