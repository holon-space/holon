#!/usr/bin/env bash
# Fails when a hand-authored regression case's `description` or a comment line
# of the corpus names a ticket number (`#<digit>`). A reader of the corpus
# cannot open one; name the bugfunnel entry path (docs/Testing/bugfunnel/entries/)
# instead. Text inside backticks is a quote (a log line, a signature) and is
# exempt; a backtick span may continue over consecutive comment lines.
#
# Usage: scripts/check-regression-descriptions.sh               # self-check, then the corpus
#        scripts/check-regression-descriptions.sh --self-check  # the self-check only
set -euo pipefail
cd "$(dirname "$0")/.."

/usr/bin/python3 - "${1-}" crates/holon-integration-tests/hand-authored-regressions/*.jsonl <<'PY'
import json, os, re, sys, tempfile

def unquoted(text, quoted):
    parts = text.split("`")
    outside = " ".join(part for i, part in enumerate(parts) if (i % 2 == 1) == quoted)
    return outside, quoted != ((len(parts) - 1) % 2 == 1)

def scan(paths):
    hits = []
    for path in paths:
        with open(path) as f:
            quoted = False
            for lineno, raw in enumerate(f, 1):
                # Classified as crates/holon-integration-tests/src/pbt/hand_authored.rs:195-196 (load_cases) does.
                line = raw.strip()
                if line.startswith("#"):
                    where = "comment"
                    text, quoted = unquoted(line, quoted)
                else:
                    quoted = False
                    if not line:
                        continue
                    case = json.loads(line)
                    where = case["name"]
                    text, _ = unquoted(case.get("description", ""), False)
                hits += [(path, lineno, where, m.group()) for m in re.finditer(r"#[0-9]+", text)]
    return hits

def self_check():
    fixture = "\n".join([
        '{"name": "clean", "description": "no number here", "transitions": []}',
        '{"name": "no-description", "transitions": []}',
        '   # an indented comment',
        '#41 a number at the start of a comment',
        '{"name": "quoted", "description": "fails with `click #1 failed`", "transitions": []}',
        '# signature: `[toggle_state] click #1 failed for',
        '# block:parent ... region main` and #42 after the quote',
    ]) + "\n"
    with tempfile.TemporaryDirectory() as d:
        path = os.path.join(d, "fixture.jsonl")
        with open(path, "w") as f:
            f.write(fixture)
        got = [(lineno, where, m) for _, lineno, where, m in scan([path])]
    want = [(4, "comment", "#41"), (7, "comment", "#42")]
    assert got == want, f"self-check: scan found {got}, expected {want}"
    print("regression descriptions self-check: ok")

mode, paths = sys.argv[1], sys.argv[2:]
assert mode in ("", "--self-check"), f"unknown argument {mode!r}"
self_check()
if mode == "--self-check":
    sys.exit(0)
hits = scan(paths)
for path, lineno, where, m in hits:
    print(f"{path}:{lineno}: {where}: {m}")
if hits:
    sys.exit(f"{len(hits)} ticket number(s) in case descriptions or comments; name the bugfunnel entry path or drop them")
print("regression descriptions and comments: no ticket numbers")
PY
