#!/usr/bin/env bash
# Usage: check-target-leftovers.sh MARKER
# Lists every file under target/ created or changed after MARKER that cargo,
# nextest, the job and gate tooling and lane logs do not own, and fails when
# there is one: a test run that started after MARKER left it behind. It compares
# the inode change time, because a copy (fs::copy, cp -p) keeps the source
# mtime but cannot keep the ctime. Tests write scratch files
# to a temp dir. Lane logs are top-level files and the `c<card>`/`verify-*`
# folders, so a test that writes one of those shapes is not seen.
set -euo pipefail
marker=$1
[ -f "$marker" ] || { echo "check-target-leftovers: no marker file '$marker'" >&2; exit 2; }
cd "$(dirname "$0")/.."

prune=()
for d in target/*/; do
    d=${d%/}
    case ${d#target/} in
        nextest | gate-logs | jobs | c[0-9]* | verify-*) prune+=(-path "$d" -o) ;;
        *) if [ -e "$d/.cargo-lock" ] || compgen -G "$d/*/.cargo-lock" >/dev/null; then
               prune+=(-path "$d" -o)
           fi ;;
    esac
done

left=$(find target \( "${prune[@]}" -false \) -prune -o -type f -path "target/*/*" -newercm "$marker" -print)
if [ -n "$left" ]; then
    echo "FAIL: the test run left these files under target/ (write them to a temp dir instead):"
    echo "$left"
    exit 1
fi
echo "check-target-leftovers: no file left under target/"
