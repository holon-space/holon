#!/usr/bin/env python3
"""Property tests for scripts/check-nextest-no-tests.py.

    scripts/test-check-nextest-no-tests.py [cases]

SEED=<n> replays a run. CHECK=<path> runs the properties against another copy of the check. Each case writes a tree
of random call sites (justfile recipes, shell and Python scripts) and runs the check on it. The calls come wrapped
(`cd x &&`, `timeout`, `env`, `if !`, `bash -c '...'`, `"$CARGO"`, `cargo +nightly`, multi-line or single-quoted
lists), with comments that name the flag, and in shapes the check cannot read. Properties:
- the check fails iff some readable test-running call lacks `--no-tests=fail` before a bare `--`, has it twice, or
  has another `--no-tests` value or the space form, or some call is unreadable; comments never count; a call with
  `--no-run` never fails it;
- `--list` names exactly the sites with a readable test-running call and no unreadable one;
- a tree with no such call is an error, not a pass.
"""
import json
import os
import random
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CHECK = Path(os.environ.get("CHECK", REPO / "scripts" / "check-nextest-no-tests.py"))
ARGS = ["--workspace", "--no-fail-fast", "--profile load-tolerant", "-p holon-net", "--features naive-twins",
        "--test integration", "-E 'test(/^loro_suite::/)'", "-E 'binary_id(holon::main) & test(=x)'", "--lib",
        "--no-capture", "--test-threads 1", '"$@"', "{{FLAGS}}"]
FLAG = {"once": ["--no-tests=fail"], "absent": [], "twice": ["--no-tests=fail", "--no-tests=fail"],
        "pass": ["--no-tests=pass"], "warn": ["--no-tests=warn"], "space": ["--no-tests", "fail"],
        "after-dashes": ["--", "--no-tests=fail"]}
# Fixed calls, run before the random ones: (form, body lines, fails).
EXAMPLES = [
    ("py", ['["cargo", "nextest", "run", *select, "--no-tests=fail", *args]'], False),
    ("py", ['["cargo", "nextest", "run" if args else "list", "--workspace",',
            '        *(["--no-tests=fail"] if args else []), *args]'], True, False),
    ("sh", ["bash scripts/steps.sh 'latency' 'cargo nextest run --profile timing --no-tests=fail 2>&1 | tee \"$L\"'"],
     False),
    ("py", ['["cargo", "nextest", "run", *reuse, "-E", filt, "--no-fail-fast", "--test-threads", "1"]'], True),
    ("sh", ['HOLON_PBT_TIMING=measure cargo nextest run --profile load-tolerant --no-tests=fail "$@" --workspace'],
     False),
    ("sh", ["cargo nextest run --workspace --no-run"], False),
    ("sh", ["cargo nextest run --workspace -- --no-tests=fail"], True),
]


SH_WRAPS = [("", ""), ("exec ", ""), ('UFT_LOWER_TWIN="$twin" ', ""), ("cd crates && ", ""), ("timeout 600 ", ""),
            ("env A=1 ", ""), ("if ! ", "; then exit 1; fi"), ("( ", " )"), ("bash -c '", "'")]
CARGO = ["cargo", '"$CARGO"', "cargo +nightly"]
NOTES = ["--no-tests=fail", "--no-run", "cargo nextest run --workspace --no-tests=fail", "TODO"]


def call(rng):
    """(form, body lines, fails) of one random call the check must read."""
    kind = rng.choice(list(FLAG))
    no_run = rng.random() < 0.15
    args = rng.sample(ARGS, rng.randint(0, 4)) + (["--no-run"] if no_run else [])
    tokens = list(args)
    flag = FLAG[kind]
    if kind == "after-dashes":
        tokens += flag
    else:
        i = rng.randint(0, len(tokens))
        tokens[i:i] = flag
    fails = kind != "once" and not no_run
    form = rng.choice(["sh", "py"])
    if form == "py":
        quote = rng.choice([json.dumps, repr])
        words = ["cargo", "nextest", "run"] + [t for a in tokens for t in a.split(" ", 1)]
        body = "[" + rng.choice([", ", ",\n    "]).join(quote(w) for w in words) + "]"
        if rng.random() < 0.3:
            body = "subprocess.run(" + body + ", check=True)"
        return form, body.split("\n"), fails
    head, tail = rng.choice(SH_WRAPS)
    if tail == "'" and any(q in t for t in tokens for q in "'\""):
        head, tail = "", ""
    lines = [head + rng.choice(CARGO) + " nextest run"]
    for t in tokens:
        if rng.random() < 0.3 and tail != "'":
            lines[-1] += " \\"
            lines.append("    " + t)
        else:
            lines[-1] += " " + t
    lines[-1] += rng.choice(["", " 2>&1 | tee target/x.log"]) + tail
    return form, lines, fails


SHELL_WRAPS = ["cd crates && ", "timeout 600 ", "env A=1 ", "nice ", "if ! ", "rust_log=debug ", "( "]
PY_FORMS = ["subprocess.run(%r, shell=True)", "['cargo', 'nextest', 'run', %s]", "subprocess.run([\n    \"cargo\", \"nextest\", \"run\", %s])"]


def wrapped_call(rng):
    """(name, body text) of a script whose one test-running nextest call lacks the flag, written in a common form
    that does not start the line with `cargo nextest run` or the list with `"cargo", "nextest", "run"`."""
    args = rng.sample(["--workspace", "--lib", "-p holon-net", "--no-fail-fast"], rng.randint(1, 3))
    if rng.random() < 0.5:
        wrap = rng.choice(SHELL_WRAPS)
        tail = {"if ! ": "; then exit 1; fi", "( ": " )"}.get(wrap, "")
        return "scripts/w.sh", "#!/usr/bin/env bash\n" + wrap + "cargo nextest run " + " ".join(args) + tail + "\n"
    form = rng.choice(PY_FORMS)
    if "shell=True" in form:
        body = form % ("cargo nextest run " + " ".join(args))
    else:
        q = "'" if form.startswith("[") else '"'
        body = form % ", ".join(q + t + q for a in args for t in a.split(" ", 1))
    return "scripts/w.py", "import subprocess\n" + body + "\n"


def check_wrapped(name, text):
    """A call the check cannot read is a miss, not a pass: the check fails or names the call."""
    with tempfile.TemporaryDirectory() as d:
        tree = Path(d)
        write_tree(tree, [("recipe", "ok", [("sh", ["cargo nextest run --workspace --no-tests=fail"], False)])])
        (tree / name).write_text(text)
        r = subprocess.run([sys.executable, str(CHECK), str(tree)], capture_output=True, text=True)
        assert r.returncode == 1 and name in r.stdout, (text, r.stdout, r.stderr)


def commented_call(rng):
    """(form, body lines) of a test-running call without the flag whose comment names `--no-tests=fail` or `--no-run`."""
    form, lines, _ = call(rng)
    while "--no-tests" in " ".join(lines) or "--no-run" in " ".join(lines):
        form, lines, _ = call(rng)
    note = rng.choice(["# TODO pass --no-tests=fail", "# --no-run is not wanted here", "# see --no-tests=fail"])
    if form == "py":
        head, _, tail = lines[0].partition('"run",')
        return form, [head + '"run",  ' + note, "    " + tail.lstrip()] if tail.strip() != "]" else [
            lines[0][:-1] + ",  " + note, "    ]"]
    return form, lines[:-1] + [lines[-1] + "  " + note]


def check_commented(form, lines):
    """A comment is no argument: the call fails the check whatever its comment says."""
    with tempfile.TemporaryDirectory() as d:
        tree = Path(d)
        write_tree(tree, [("script", "scripts/c." + form, [(form, lines, True)])])
        r = subprocess.run([sys.executable, str(CHECK), str(tree)], capture_output=True, text=True)
        assert r.returncode == 1 and "NO-TESTS: script scripts/c." + form in r.stdout, (lines, r.stdout, r.stderr)


def runs(lines):
    return "--no-run" not in " ".join(lines)


def commented(rng, form, lines):
    """The call with a comment that names a flag; inside a multi-line Python list, also as a line of its own."""
    note = "  # " + rng.choice(NOTES)
    if form == "py" and len(lines) > 1 and rng.random() < 0.5:
        return lines[:1] + ["    " + note.strip()] + lines[1:]
    return lines[:-1] + [lines[-1] + note]


def odd_call(rng):
    """(form, body lines) of a test-running call in a shape the check cannot read; it fails even with the flag."""
    args = rng.sample(["--workspace", "--lib", "--no-tests=fail", "-p holon-net"], rng.randint(0, 3))
    listed = "".join(", " + json.dumps(a) for a in args)
    return rng.choice([
        ("sh", ["cargo nextest --profile ci run " + " ".join(args)]),
        ("py", ['subprocess.run("cargo nextest run %s", shell=True)' % " ".join(args)]),
        ("py", ['["cargo", "nextest", "run" if args else "list"%s]' % listed]),
        ("py", ['["cargo", "nextest", *rest%s]' % listed]),
    ])


def judged(c):
    """Whether the check must read and judge the call c = (form, lines, fails[, judged])."""
    return c[3] if len(c) == 4 else runs(c[1])


def write_tree(root, sites):
    """sites: [(kind, name, [(form, lines, fails)])]; a recipe holds sh calls only."""
    (root / "scripts").mkdir()
    just = ["FLAGS := ''", ""]
    for kind, name, cs in sites:
        if kind == "recipe":
            just += [name + ":", "    #!/usr/bin/env bash", "    set -euo pipefail", "    # cargo nextest run --workspace"]
            just += ["    " + l for c in cs for l in c[1]] + [""]
        elif name.endswith(".py"):
            body = ["#!/usr/bin/env python3", '"""Wraps cargo nextest run."""', "x = 1"] + [l for c in cs for l in c[1]]
            (root / name).write_text("\n".join(body) + "\n")
        else:
            body = ["#!/usr/bin/env bash", "x=1"] + [l for c in cs for l in c[1]]
            (root / name).write_text("\n".join(body) + "\n")
    (root / "justfile").write_text("\n".join(just) + "\n")


def case(rng):
    sites = []
    for n in range(rng.randint(1, 5)):
        kind = rng.choice(["recipe", "sh", "py"])
        want_form = "py" if kind == "py" else "sh"
        cs = []
        for _ in range(rng.randint(1, 3)):
            r = rng.random()
            if r < 0.05:
                form, lines = odd_call(rng)
                while form != want_form:
                    form, lines = odd_call(rng)
                cs.append((form, lines, True, False))
            elif r < 0.15:
                note = "# cargo nextest run --workspace" if kind != "py" else '# ["cargo", "nextest", "run"]'
                cs.append((want_form, [note], False, False))
            else:
                form, lines, fails = call(rng)
                while form != want_form:
                    form, lines, fails = call(rng)
                ran = runs(lines)
                if rng.random() < 0.3:
                    lines = commented(rng, form, lines)
                cs.append((form, lines, fails, ran))
        name = {"recipe": "r%d" % n, "sh": "scripts/s%d.sh" % n, "py": "scripts/p%d.py" % n}[kind]
        sites.append(("recipe" if kind == "recipe" else "script", name, cs))
    return sites


def check(sites):
    with tempfile.TemporaryDirectory() as d:
        tree = Path(d)
        write_tree(tree, sites)
        calls = [c for _, _, cs in sites for c in cs]
        want_fail = any(c[2] for c in calls)
        r = subprocess.run([sys.executable, str(CHECK), str(tree)], capture_output=True, text=True)
        want = sorted("%s %s" % (k, n) for k, n, cs in sites
                      if any(judged(c) for c in cs) and not any(c[2] and not judged(c) for c in cs))
        if not any(judged(c) or c[2] for c in calls):
            assert "no nextest call site" in r.stderr, (sites, r.stdout, r.stderr)
            return
        assert r.returncode == (1 if want_fail else 0), (sites, r.stdout, r.stderr)
        listed = subprocess.run([sys.executable, str(CHECK), "--list", str(tree)], capture_output=True, text=True,
                                check=True).stdout.split("\n")
        assert sorted(filter(None, listed)) == want, (sites, listed)


def main():
    cases = int(sys.argv[1]) if len(sys.argv) > 1 else 40
    seed = int(os.environ.get("SEED", random.randrange(2 ** 32)))
    print("SEED=%d" % seed)
    rng = random.Random(seed)
    for fixed in EXAMPLES:
        check([("script", "scripts/fixed." + fixed[0], [fixed])])
    for _ in range(cases):
        check(case(rng))
    check_wrapped("scripts/w.sh", "cd crates && cargo nextest run --workspace\n")
    for _ in range(cases):
        check_wrapped(*wrapped_call(rng))
    check_commented("sh", ["cargo nextest run --workspace  # TODO pass --no-tests=fail"])
    for _ in range(cases):
        check_commented(*commented_call(rng))
    print("ok: %d examples, %d cases" % (len(EXAMPLES), cases))
    return 0


if __name__ == "__main__":
    sys.exit(main())
