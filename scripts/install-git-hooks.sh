#!/usr/bin/env bash
# Install git pre-commit / pre-push hooks that run the two-tier quality gate.
#
# NOTE: this repo is a colocated jj+git repo. Git hooks fire ONLY for plain
# `git commit` / `git push`; they do NOT fire for `jj` operations. jj users get
# the gate by running `just precommit` / `just prepush` by hand (or via their own
# tooling). This installer is a convenience for contributors on plain git.

set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOOKS_DIR="$REPO_ROOT/.git/hooks"

if [ ! -d "$REPO_ROOT/.git" ]; then
    echo "No .git directory at $REPO_ROOT (jj-only checkout?). Nothing to install." >&2
    exit 1
fi
mkdir -p "$HOOKS_DIR"

cat > "$HOOKS_DIR/pre-commit" <<'HOOK'
#!/usr/bin/env bash
# Tier 1 quality gate. Bypass with `git commit --no-verify`.
exec just precommit
HOOK
chmod +x "$HOOKS_DIR/pre-commit"

cat > "$HOOKS_DIR/pre-push" <<'HOOK'
#!/usr/bin/env bash
# Tier 2 quality gate. Bypass with `git push --no-verify`.
exec just prepush
HOOK
chmod +x "$HOOKS_DIR/pre-push"

echo "Installed pre-commit (Tier 1) and pre-push (Tier 2) hooks in $HOOKS_DIR"
echo "Bypass a single run with --no-verify. jj users: run 'just precommit' / 'just prepush' manually."
