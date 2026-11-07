#!/usr/bin/env python3
"""Self-check for scripts/flow-checks/owned-known-reds.py.

Run: `python3 scripts/flow-checks/test-owned-known-reds.py [cases]`. Exits non-zero on the first violation.
The fixed examples run first, then `cases` random lanes (default 200): a registry at the base, the lane's edit of it
at the tip, the lane's cards, and a summary built from the rows the cards own.
The real-history example reads the registry of two commits of this repository with `jj`; fixtures/owned-known-reds-summary.md
is the summary of that lane in the `## Known-red rows` shape.
"""
import importlib.util
import random
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("owned_known_reds", HERE / "owned-known-reds.py")
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)

REGISTRY = "docs/Testing/KeystoneKnownReds.md"
HEADER = ("| Key | Status | Match pattern | Site | Signature | Evidence | Task | Owner | Companions |\n"
          "| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n")
STATUSES = ["known-red", "fixed-pending-soak", "no-longer-fires", "proposed"]
WORDS = ["boot", "undo", "slot", "load", "timeout", "toast", "focus", "org", "sql", "budget", "drift", "peer", "page"]
OWNER_FORMS = ["#{}", "holon-work#{}", "holon-space/holon-work#{}", "load reds (#{})", "#{} — load-tolerant caps"]
NO_OWNER = ["—", "**UNOWNED**", "**UNOWNED** — resurfaces via its Match pattern", "region-literal fix (unowned)"]
CAUSES = ["wait", "lock", "cpu", "race", "other:unmeasured"]
# The registry at the base and at the tip of a lane that owns these rows; its summary lists every one of them.
HISTORY = ("408f71715456cce6fcc8914082c9b2e4f9fd40e2", "d1558f8ac73c2cae33292830a46e9b9a24044dd7")
HISTORY_CARDS = {676}
LEFT_OUT = "composed-boot-timeout-under-load"
SEVEN_CELL_ROWS = ("advice-dismiss-prod-session-row-missing", "tour-spike-advance-cursor-not-visible",
                   "journal-feed-main-panel-still-loading")


def owner_cell(rng, pool):
    n = rng.choice([0, 1, 1, 2])
    if n == 0:
        return rng.choice(NO_OWNER)
    return ", ".join(rng.choice(OWNER_FORMS).format(c) for c in rng.sample(pool, n))


def row(key, status="known-red", pattern="boom", evidence="measured", task="—", owner="—", companions="—"):
    return (f"| `{key}` | {status} | `{pattern}` | — | `src/x.rs:1:1` the test panics | {evidence} | {task} | {owner} "
            f"| {companions} |\n")


def random_row(rng, key, pool):
    pattern = rng.choice(["boom", "a&#124;b", "x \\(1\\)&#124;y"])
    evidence = rng.choice(["measured", f"see #{rng.choice(pool)}", "`zstd -dcq <log> | sed -n 1p` quotes it"])
    companions = rng.choice(["—", "`inv-a/*`", "`inv-a/*, inv-b/sql`"])
    return row(key, rng.choice(STATUSES), pattern, evidence, owner_cell(rng, pool), owner_cell(rng, pool), companions)


def registry_text(rows, rng):
    """`rows` in two registry tables, with prose and a table that is no registry between them."""
    cut = rng.randint(0, len(rows))
    return ("# Known reds\n\nProse naming `a-key` and #1.\n\n" + HEADER + "".join(rows[:cut])
            + "\n## Families\n\n| Family | Cause |\n| --- | --- |\n| `fam` | see #1, #2 |\n\n## More\n\n"
            + HEADER + "".join(rows[cut:]) + "\nTrailing prose.\n")


def summary_row(key, outcome, cause="wait"):
    if outcome == "deleted":
        return f"| `{key}` | deleted | target/lane/{key}.log |\n"
    return f"| `{key}` | kept | {cause} | target/lane/{key}.log | 81.5 | 12.0 |\n"


def summary_for(rows, cards, extra=(), gaps=()):
    """A summary whose `## Known-red rows` table has `rows` ({key: outcome}) and the raw `extra` rows."""
    return (f"# lane summary\n\nCards: {', '.join(f'#{c}' for c in sorted(cards))}\n\n## How tested\n\n- logs\n\n"
            "## Known-red rows\n\n| Key | Outcome | Cause or log | Log | Load | Seconds |\n| --- | --- | --- | --- | --- | --- |\n"
            + "".join(summary_row(k, o) for k, o in rows.items()) + "".join(extra)
            + "\n## Known gaps\n\n| Row | Note |\n| --- | --- |\n" + "".join(f"| `{k}` | left alone |\n" for k in gaps)
            + "\nConfidence (1-5) that the verifier will confirm: 4\n")


def is_log(log):
    return log.startswith("target/lane/")


def expect(name, got, want):
    assert got == want, f"{name}: problems\n  {got}\nexpected\n  {want}"


def lane(rng):
    pool = list(range(1, 10))
    cards = set(rng.sample(pool[:6], rng.choice([1, 1, 2, 3])))
    keys = rng.sample(sorted({f"{a}-{b}-{c}" for a in WORDS for b in WORDS for c in WORDS if len({a, b, c}) == 3}),
                      rng.randint(1, 14))
    # The registry at the base always has a row: an empty one fails by design (a static example).
    base_keys = keys[:1] + [k for k in keys[1:] if rng.random() < 0.7]
    added = [k for k in keys if k not in base_keys]
    deleted = {k for k in base_keys if rng.random() < 0.3}
    rows = {k: random_row(rng, k, pool) for k in keys}
    base = {REGISTRY: registry_text([rows[k] for k in base_keys], rng)}
    tip_keys = [k for k in base_keys if k not in deleted] + added
    tip = {REGISTRY: registry_text([rows[k] for k in tip_keys], rng)}
    owned = {k for k in keys if check.owners(registry_row_of(rows[k])) & cards}
    outcomes = {k: "deleted" if k in deleted else "kept" for k in sorted(owned)}
    return cards, base, tip, owned, outcomes, set(keys) - owned


def registry_row_of(line):
    _, header, cells = next(check.known_reds_table.table_rows(HEADER + line))
    return check.known_reds_table.registry_row(header, cells)


def properties(rng, cards, base, tip, owned, outcomes, others):
    keys = list(outcomes)
    rng.shuffle(keys)
    order = {k: outcomes[k] for k in keys}
    run = lambda summary: check.problems(base, tip, summary, cards, is_log)
    # P1: every owned row named with the outcome of the tip passes; other cards' rows outside the section are fine.
    expect("P1 all owned rows named", run(summary_for(order, cards, gaps=sorted(others))), [])
    for k in keys:
        # P2: the problem of a left-out row names exactly that row.
        expect(f"P2 drop {k}", run(summary_for({x: o for x, o in order.items() if x != k}, cards)),
               [f"owned row {k} is not named in the summary"])
        # P4: an outcome against the tip state names the row.
        flipped = {**order, k: "kept" if order[k] == "deleted" else "deleted"}
        want = ("is deleted at the tip but the summary says kept" if order[k] == "deleted"
                else "is kept at the tip but the summary says deleted")
        expect(f"P4 flip {k}", run(summary_for(flipped, cards)), [f"row {k} {want}"])
    for k in sorted(others):
        # P3: a row of other cards inside the section is an error naming it.
        expect(f"P3 foreign {k}", run(summary_for(order, cards, extra=[summary_row(k, "kept")])),
               [f"summary names {k}, which is no row of the lane's cards {sorted(cards)}"])
    expect("P3 no such row", run(summary_for(order, cards, extra=[summary_row("no-such-row", "kept")])),
           ["summary names no-such-row, which is no row of the registry"])
    deleted = [k for k in keys if order[k] == "deleted"]
    # P5: a row the lane deleted from the tip is owned through the base; without the base it would not be.
    empty_tip = [] if check.registry(tip, "the tip")[0] else [f"no registry row in {REGISTRY} at the base"]
    for k in deleted:
        assert k not in check.registry(tip, "the tip")[0]
        expect(f"P5 deleted {k} named without base", check.problems(tip, tip, summary_for(order, cards), cards, is_log),
               empty_tip + [f"summary names {d}, which is no row of the registry" for d in sorted(deleted)])
    for k in keys[:1]:
        expect(f"kept row {k} without a cause", run(summary_for(order, cards).replace(
            summary_row(k, order[k]), f"| `{k}` | kept | slow | target/lane/{k}.log | 1 | 2 |\n")),
            [f"row {k} is kept without a cause/log; write `| {k} | kept | <wait|lock|cpu|race|other:word> | <log> "
             "| <load at start> | <seconds> |`"])
        expect(f"row {k} with a missing log", check.problems(base, tip, summary_for(order, cards), cards, lambda _: False),
               [f"row {x} cites `target/lane/{x}.log`, which is no existing log under target/ from the lane root"
                for x in sorted(keys)])
    return bool(deleted)


def static_examples():
    two = row("two-card-row", owner="#12, holon-work#34", task="—")
    base = {REGISTRY: HEADER + two + row("other-row", owner="#99")}
    for cards in ({12}, {34}, {12, 34}):
        expect(f"two cards in one Owner cell, lane {cards}",
               check.problems(base, base, summary_for({"two-card-row": "kept"}, cards), cards, is_log), [])
        expect(f"two cards in one Owner cell, lane {cards}, left out",
               check.problems(base, base, summary_for({}, cards), cards, is_log),
               ["owned row two-card-row is not named in the summary"])
    piped = row("piped-row", pattern="a&#124;b", evidence="`zstd -dcq <log> | sed -n 1p` quotes it", owner="#5")
    reg = {REGISTRY: HEADER + piped}
    parsed = check.registry(reg, "the tip")[0]["piped-row"]
    assert parsed["Match pattern"] == "a|b" and parsed["Owner"] == "#5" and parsed["Companions"] == "—", parsed
    expect("`&#124;` pattern and a stray `|` in Evidence", check.problems(reg, reg, summary_for({}, {5}), {5}, is_log),
           ["owned row piped-row is not named in the summary"])
    short = {REGISTRY: HEADER + "| `short-row` | known-red | `p` | — | sig | ev | — |\n" + row("ok-row", owner="#5")}
    expect("a row without Task and Owner at the tip", check.problems(short, short, summary_for({}, {5}), {5}, is_log),
           [f"registry row short-row ({REGISTRY}:3 at the tip) has 7 cells, its table header has 9",
            "owned row ok-row is not named in the summary"])
    fixed = {REGISTRY: HEADER + row("short-row") + row("ok-row", owner="#5")}
    expect("a short row at the base that the tip holds whole",
           check.problems(short, fixed, summary_for({"ok-row": "kept"}, {5}), {5}, is_log), [])
    gone = {REGISTRY: HEADER + row("ok-row", owner="#5")}
    expect("a short row at the base that the tip deleted",
           check.problems(short, gone, summary_for({"ok-row": "kept"}, {5}), {5}, is_log),
           [f"registry row short-row ({REGISTRY}:3 at the base) has 7 cells, its table header has 9"])
    odd = {REGISTRY: HEADER + row("odd-row", status="flaky", owner="#5")}
    expect("an unknown Status", check.problems(odd, odd, summary_for({}, {5}), {5}, is_log),
           [f"registry row odd-row ({REGISTRY}:3 at the tip) has the Status 'flaky', none of "
            "known-red, fixed-pending-soak, no-longer-fires, proposed"])
    for name, texts in [("an empty registry", {REGISTRY: ""}), ("no registry file", {}),
                        ("a registry of garbage", {REGISTRY: "| a | b |\n|---|---|\n| c | d |\n"})]:
        got = check.problems(texts, texts, summary_for({}, {5}), {5}, is_log)
        assert got and all("no registry row" in p for p in got), f"{name}: {got}"
    misnamed = {REGISTRY: HEADER + row("ok-row", owner="#5") + "\n" + HEADER.replace("| Key |", "| **Key** |")
                + row("hidden-row", owner="#5")}
    got = check.problems(misnamed, misnamed, summary_for({"ok-row": "kept"}, {5}), {5}, is_log)
    assert got, "an owned row under a table whose header is not `Key` passes unnamed"
    expect("a changed registry header", got,
           [f"{REGISTRY}:5 at the tip: the table header names registry columns but is not the registry header "
            "(missing Key; extra **Key**)"])
    windowed = {**misnamed, REGISTRY: HEADER + row("ok-row", owner="#5"),
                "docs/Testing/WindowedKnownReds.md": (HERE / "../../docs/Testing/WindowedKnownReds.md").read_text()}
    expect("a table without registry columns", check.problems(
        windowed, windowed, summary_for({"ok-row": "kept"}, {5}), {5}, is_log), [])
    expect("Cards line", check.lane_cards("x\nCards: #3, #40\n", "lane-7-slug"), {3, 40})
    expect("lane folder", check.lane_cards("x\n", "lane-7-slug"), {7})
    expect("neither", check.lane_cards("x\n", "_sw_integ"), None)
    print("[owned-known-reds] static examples: PASS")


def history_example():
    """The registry of a real lane's base and tip, and its summary with and without one owned row."""
    def show(rev):
        return {REGISTRY: with_owner_cells(subprocess.run(
            ["jj", "--ignore-working-copy", "file", "show", f"--revision={rev}", "--", f"root:{REGISTRY}"],
            cwd=HERE, capture_output=True, text=True, check=True).stdout)}
    base, tip = (show(rev) for rev in HISTORY)
    final = (HERE / "fixtures/owned-known-reds-summary.md").read_text()
    round_one = "".join(line for line in final.splitlines(True) if LEFT_OUT not in line)
    logs = lambda log: log.startswith("target/cpu-bound-test-headroom/")
    expect("history: the final summary", check.problems(base, tip, final, HISTORY_CARDS, logs), [])
    expect("history: the summary without one owned row", check.problems(base, tip, round_one, HISTORY_CARDS, logs),
           [f"owned row {LEFT_OUT} is not named in the summary"])
    print("[owned-known-reds] history example: PASS")


def with_owner_cells(text):
    """The registry with the three rows that predate their Task and Owner cells given `—` and `**UNOWNED**`."""
    lines = []
    for line in text.splitlines(True):
        if any(line.startswith(f"| `{k}` |") for k in SEVEN_CELL_ROWS) and line.count("|") == 8:
            line = line.rstrip()[: -len("| — |")] + "| — | **UNOWNED** | — |\n"
        lines.append(line)
    return "".join(lines)


def cli_examples():
    """The command end to end in a jj repository: the base comes from a commit, the tip from the working copy."""
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp) / "lane-5-probe"
        (root / "docs/Testing").mkdir(parents=True)
        (root / "target/lane").mkdir(parents=True)
        reg = root / REGISTRY
        subprocess.run(["jj", "git", "init", "--quiet"], cwd=root, check=True, capture_output=True)
        reg.write_text(HEADER + row("kept-row", owner="#5") + row("gone-row", task="#5") + row("theirs", owner="#6"))
        subprocess.run(["jj", "commit", "-m", "base"], cwd=root, check=True, capture_output=True)
        base = subprocess.run(["jj", "log", "--no-graph", "-r", "@-", "-T", "commit_id"], cwd=root, check=True,
                              capture_output=True, text=True).stdout
        reg.write_text(HEADER + row("kept-row", owner="#5") + row("theirs", owner="#6"))
        (root / "docs/Testing/WindowedKnownReds.md").write_text("| Run | Summary line |\n| --- | --- |\n| a | b |\n")
        for k in ("kept-row", "gone-row"):
            (root / f"target/lane/{k}.log").write_text("exit: 0\n")
        def run(text, rev=base, cwd=root):
            summary = cwd / "target/summary.md"
            summary.write_text(text)
            return subprocess.run([sys.executable, str(HERE / "owned-known-reds.py"), f"--base={rev}",
                                   "--summary", str(summary)], cwd=cwd, capture_output=True, text=True)

        both = {"kept-row": "kept", "gone-row": "deleted"}
        no_cards = summary_for(both, {5}).replace("Cards: #5\n", "")
        cases = [
            ("pass, cards from the folder", run(no_cards), 0, ""),
            ("a deleted row left out", run(summary_for({"kept-row": "kept"}, {5})), 1, "owned row gone-row is not named"),
            ("a deleted row said kept", run(summary_for({**both, "gone-row": "kept"}, {5})), 1,
             "row gone-row is deleted at the tip but the summary says kept"),
            ("an unknown base", run(summary_for(both, {5}), rev="no-such-rev"), 2, "jj file list"),
            ("a base read as an option", run(summary_for(both, {5}), rev="--help"), 2, "jj file list"),
        ]
        other = Path(tmp) / "_sw_integ"
        root.rename(other)
        cases.append(("no Cards line and no lane folder", run(no_cards, cwd=other), 2, "add a `Cards:` line"))
        for name, done, code, needle in cases:
            assert done.returncode == code and needle in done.stderr, f"{name}: exit {done.returncode}, stderr:\n{done.stderr}"
            print(f"[owned-known-reds] cli {name}: exit {code} as expected")


def main():
    n = int(sys.argv[1]) if len(sys.argv) > 1 else 200
    static_examples()
    history_example()
    cli_examples()
    rng = random.Random()
    seed = rng.randrange(2**32)
    rng.seed(seed)
    with_deletions = 0
    for case in range(n):
        try:
            with_deletions += properties(rng, *lane(rng))
        except AssertionError as e:
            raise SystemExit(f"[owned-known-reds] case {case} of seed {seed}: {e}")
    assert with_deletions >= n // 10, f"only {with_deletions} of {n} lanes deleted an owned row"
    print(f"[owned-known-reds] {n} random lanes ({with_deletions} delete an owned row), seed {seed}: PASS")


if __name__ == "__main__":
    main()
