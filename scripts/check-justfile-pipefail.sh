#!/usr/bin/env bash
# Guard against never-fail justfile recipes.
#
# A recipe body whose last command is a pipe ending in `tee` exits with tee's
# status, not the real command's. Without a shebang `just` runs each body line
# under `sh -cu`, so such a recipe reports green however red the work was — nine
# recipes did exactly that (BugFunnel 2026-08-14 ORACLE). The fix, and what this
# script enforces: any recipe that pipes into `tee` must be a shebang block that
# sets pipefail.
#
# Usage:
#   scripts/check-justfile-pipefail.sh [justfile]   # default: repo-root justfile

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
JUSTFILE="${1:-$REPO_ROOT/justfile}"

if [ ! -f "$JUSTFILE" ]; then
    echo "ERROR: justfile not found: $JUSTFILE" >&2
    exit 2
fi

awk '
# A recipe header sits at column 0, names the recipe before a colon, and is not
# an assignment (`x := y`) or a setting.
/^[@a-zA-Z_][a-zA-Z0-9_-]*([ \t]+[^:]*)?:/ && !/:=/ {
    flush()
    recipe = $0
    sub(/[ \t]*[:(].*$/, "", recipe)
    sub(/^@/, "", recipe)
    in_recipe = 1; seen_body = 0; is_shebang = 0; has_pipefail = 0; has_tee = 0
    recipes++
    next
}

# Body lines are indented; anything else at column 0 ends the recipe.
in_recipe && /^[ \t]+[^ \t]/ {
    if (!seen_body) { seen_body = 1; if ($0 ~ /^[ \t]*#!/) is_shebang = 1 }
    # Only a real `set` COMMAND counts. The word inside an echo string or a
    # comment enables nothing, and `set +o pipefail` turns it back off — last
    # one wins, so a body that ends with it is unprotected.
    if ($0 ~ /^[ \t]*set[ \t]/) {
        if ($0 ~ /\+o[ \t]+pipefail/) has_pipefail = 0
        else if ($0 ~ /-[a-zA-Z]*o[ \t]+pipefail/) has_pipefail = 1
    }
    if ($0 ~ /\|[ \t]*tee([ \t]|$)/) has_tee = 1
    next
}
in_recipe && /^[ \t]*$/ { next }
in_recipe { flush() }

function flush() {
    if (in_recipe && has_tee && !(is_shebang && has_pipefail)) {
        reason = is_shebang ? "shebang block without pipefail" : "no #! shebang (runs under sh -cu)"
        printf "%s: pipes into tee but %s\n", recipe, reason
        violations++
    }
    in_recipe = 0
}

END {
    flush()
    # A file that parses to zero recipes means the header pattern stopped
    # matching, not that the justfile is clean — reporting OK there would be the
    # same false green this guard exists to prevent.
    if (recipes == 0) {
        print "ERROR: parsed 0 recipes — the guard cannot vouch for this file." > "/dev/stderr"
        exit 2
    }
    if (violations > 0) {
        printf "\n%d never-fail recipe(s). Each must start with:\n", violations
        print "    #!/usr/bin/env bash"
        print "    set -euo pipefail"
        print "so the exit status is the real command'"'"'s, not tee'"'"'s."
        exit 1
    }
    print "justfile pipefail guard OK: every `| tee` recipe is a pipefail shebang block."
}
' "$JUSTFILE"
