#!/usr/bin/env bash
# RED probe for rev-2 items 2 and 3: revert both guards in place, run the two
# tests, restore byte-identically, prove it with sha256.
set -uo pipefail
W=/Users/martin/Workspaces/pkm/holon/.claude/worktrees/plaintext-layer
cd "$W" || exit 90
F=crates/holon-filesystem/src/file_sync_controller.rs
mkdir -p lane-logs
cp "$F" lane-logs/probe-backup.rs
shasum -a 256 "$F" | tee lane-logs/r2-sha-before.txt

python3 - <<'PY'
p='crates/holon-filesystem/src/file_sync_controller.rs'
s=open(p).read()
a="""        if disk_content.is_empty() {"""
b="""        if disk_content.trim().is_empty() { // RED-PROBE"""
assert s.count(a)==1
s=s.replace(a,b,1)
a2="""        if let Some(disclosure) = &self.writeback_disclosure {
            disclosure.vault_file_emptied(path);
        }"""
b2="""        // RED-PROBE: disclosure removed"""
assert s.count(a2)==1
s=s.replace(a2,b2,1)
open(p,'w').write(s)
PY

bash "$HOME/.claude/skills/orchestrator/scripts/with-build-slot.sh" \
  "cargo nextest run -p holon-orgmode --features di --no-fail-fast -E 'binary(empty_file_is_not_a_document)'" \
  >lane-logs/r2-red23.log 2>&1
echo "PROBE_EXIT=$?"

cp lane-logs/probe-backup.rs "$F"
rm -f lane-logs/probe-backup.rs
shasum -a 256 "$F" | tee lane-logs/r2-sha-after.txt
echo "RED-PROBE occurrences after restore: $(grep -c RED-PROBE "$F")"
grep -E 'PASS|FAIL|Summary|assertion|panicked|left|right|emptied|titles' lane-logs/r2-red23.log | tail -30
