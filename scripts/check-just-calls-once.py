#!/usr/bin/python3
"""Fails when one `just` call (same recipe, same args) ran twice in a gate.

    scripts/check-just-calls-once.py <record>   the JSON lines scripts/shims/just wrote

The key of a call is its argv without the options that only pick the justfile or directory. Calls that run no
recipe (`--evaluate`, `--dump`, ...) are not counted. Env prefixes are not in the record: `X=1 just r` and
`just r` are the same key.
"""
import collections
import json
import sys

OPTS_WITH_VALUE = {"--justfile", "-f", "--working-directory", "-d"}
NO_RECIPE = {"--evaluate", "--dump", "--show", "--list", "--summary", "--variables"}


def key(argv):
    out, i = [], 0
    while i < len(argv):
        a = argv[i]
        if a in OPTS_WITH_VALUE:
            i += 2
            continue
        if a.split("=", 1)[0] in OPTS_WITH_VALUE and "=" in a:
            i += 1
            continue
        if a in NO_RECIPE or a.split("=", 1)[0] in NO_RECIPE:
            return None
        out.append(a)
        i += 1
    return tuple(out)


def repeated(argvs):
    counts = collections.Counter(k for k in map(key, argvs) if k is not None)
    return counts, {k: n for k, n in counts.items() if n > 1}


def main(path):
    with open(path) as f:
        argvs = [json.loads(line)["argv"] for line in f if line.strip()]
    counts, dup = repeated(argvs)
    for k, n in dup.items():
        print("FAIL: just %s ran %d times" % (" ".join(k), n))
    if dup:
        return 1
    print("just-calls-once: %d calls, 0 repeated" % sum(counts.values()))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
