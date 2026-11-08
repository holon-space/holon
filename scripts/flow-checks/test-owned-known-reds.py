#!/usr/bin/env python3
"""Self-check for scripts/flow-checks/owned-known-reds.py.

Run: `python3 scripts/flow-checks/test-owned-known-reds.py [cases]`. Exits non-zero on the first violation.
The fixed examples run first, then `cases` random lanes (default 200): a registry at the base, the lane's edit of it
at the tip, the lane's cards, and a summary built from the rows the cards own.
The real-history example reads the registry of two commits of this repository with `jj`; fixtures/owned-known-reds-summary.md
is the summary of that lane in the `## Known-red rows` shape.
"""
import re
import importlib.util
import random
import subprocess
import sys
import tempfile
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("owned_known_reds", HERE / "owned-known-reds.py")
check = importlib.util.module_from_spec(spec)
spec.loader.exec_module(check)

REGISTRY = "docs/Testing/KeystoneKnownReds.md"
HEADER = ("| Key | Status | Match pattern | Site | Signature | Evidence | Task | Owner | Companions |\n"
          "| --- | --- | --- | --- | --- | --- | --- | --- | --- |\n")
HEADER_CELLS = [cell.strip() for cell in HEADER.splitlines()[0].split("|")[1:-1]]
FOREIGN_COLUMNS = ["Note", "Test", "Count", "Where", "key", "Owners"]
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


def foreign_rows(rng):
    """Rows of a table that is no registry: any cell count but the registry's, a first cell that is no registry key."""
    count = lambda: rng.choice([1, 2, 2, 3, 5, 8, 10, 12])
    return "".join("| " + " | ".join([rng.choice([f"`fam-{rng.choice(WORDS)}`", "see #1", "plain"])]
                                     + ["x"] * (count() - 1)) + " |\n" for _ in range(rng.randint(1, 3)))


def registry_text(rows, rng, first_header=HEADER):
    """`rows` in two registry tables, with prose and a table that is no registry between them."""
    cut = rng.randint(0, len(rows))
    return ("# Known reds\n\nProse naming `a-key` and #1.\n\n" + first_header + "".join(rows[:cut])
            + "\n## Families\n\n| Family | Cause |\n| --- | --- |\n| `fam` | see #1, #2 |\n" + foreign_rows(rng)
            + "\n## More\n\n" + HEADER + "".join(rows[cut:]) + "\nTrailing prose.\n")


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


def random_header(rng):
    """The cells of a header row: the registry header, an edit of it, or a table of other columns."""
    shape = rng.choice(["exact", "edited", "edited", "edited", "foreign"])
    if shape == "exact":
        return list(HEADER_CELLS)
    if shape == "foreign":
        return rng.sample(FOREIGN_COLUMNS + ["Match pattern", "Site", "Evidence"], rng.randint(1, 4))
    cells = list(HEADER_CELLS)
    for _ in range(rng.randint(1, 3)):
        i, j = rng.randrange(len(cells)), rng.randrange(len(cells) + 1)
        op = rng.choice(["drop", "double", "extra", "markup", "swap"])
        if op == "drop" and len(cells) > 1:
            del cells[i]
        elif op == "double":
            cells.insert(j, cells[i])
        elif op == "extra":
            cells.insert(j, rng.choice(FOREIGN_COLUMNS))
        elif op == "markup":
            cells[i] = rng.choice([f"**{cells[i]}**", cells[i].lower(), f"`{cells[i]}`"])
        elif op == "swap":
            j = rng.randrange(len(cells))
            cells[i], cells[j] = cells[j], cells[i]
    return cells


def header_rule(cells):
    """The table rule as an oracle: the exact header opens a registry table, a header that names a registry column
    but is not exact is an error naming what differs, any other header is ignored. Names compare without emphasis
    and case. Returns the class and the reason."""
    if cells == HEADER_CELLS:
        return "registry", None
    if not {"key", "status", "task", "owner"} & {re.sub(r"[*_`]+", "", c).strip().lower() for c in cells}:
        return "ignored", None
    have, want = Counter(cells), Counter(HEADER_CELLS)
    surplus = have - want
    parts = [("missing", [c for c in HEADER_CELLS if c in want - have]),
             ("extra", [c for c in dict.fromkeys(cells) if c in surplus and c not in want]),
             ("doubled", [c for c in HEADER_CELLS if c in surplus])]
    return "error", "; ".join(f"{name} {', '.join(cs)}" for name, cs in parts if cs) or "the columns stand in another order"


def header_property(cells):
    text = "| " + " | ".join(cells) + " |\n|" + " --- |" * len(cells) + "\n" + row("ok-row", owner="#5")
    want, why = header_rule(cells)
    try:
        headers = [header for _, header, _ in check.known_reds_table.table_rows(text)]
    except check.known_reds_table.HeaderError as e:
        expect(f"header {cells}", (e.lineno, str(e)), (1, "the table header names registry columns but is not the "
                                                          f"registry header ({why})"))
        assert want == "error", f"header {cells}: HeaderError, the rule says {want}"
        assert ("another order" in why) == (sorted(cells) == sorted(HEADER_CELLS)), f"header {cells}: {why}"
        return want
    expect(f"header {cells}", headers, [HEADER_CELLS] if want == "registry" else [None])
    return want


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


def lines_of(lines):
    return "\n".join(lines) + "\n"


def altered_header(rng):
    """The cells of a header that names each registry column bold, lower-case or ticked, so it is not the exact header."""
    alter = [lambda c: f"**{c}**", str.lower, lambda c: f"`{c}`"]
    return [rng.choice(alter)(c) if c in check.known_reds_table.REGISTRY_COLUMNS else c for c in HEADER_CELLS]


def stray_registry(rng, shape, visible, hidden, listed_key):
    """(text, linenos of the orphan `|` lines that are registry-shaped, lineno and message of a header error or None).
    `visible` rows stand in a registry table; `hidden` rows are placed so that the registry grammar cannot see them."""
    lines = ["# Known reds", "", *HEADER.splitlines(), *(r.rstrip("\n") for r in visible), ""]
    stray, header_error = [], None

    def add(line, is_stray=False):
        lines.append(line)
        if is_stray:
            stray.append(len(lines))

    if shape == "blank":
        for r in hidden:
            add(r.rstrip("\n"), True)
    elif shape == "header":
        cells = altered_header(rng)
        add("| " + " | ".join(cells) + " |")
        header_error = (len(lines), f"{REGISTRY}:{len(lines)} at the tip: the table header names registry columns but is not "
                                    f"the registry header ({header_rule(cells)[1]})")
        add("|" + " --- |" * len(HEADER_CELLS))
        for r in hidden:
            add(r.rstrip("\n"))
    elif shape == "prose-table":
        add("| Note | Cause |")
        add("| --- | --- |")
        for r in hidden:
            add(r.rstrip("\n"))
        add(f"| `{listed_key}` | known-red |")
    elif shape == "listed-key":
        add(f"| `{listed_key}` | {rng.choice(['known-red', 'flaky', 'x'])} |", True)
    return lines_of(lines), stray, header_error


STRAY_SHAPES = ["blank", "header", "prose-table", "listed-key"]


def stray_message(n):
    return f"{REGISTRY}:{n} at the tip: registry-shaped row outside a table"


def stray_property(rng, shape):
    """A registry row that the grammar cannot see is an error naming its line; everything else is unaffected."""
    pool = list(range(1, 10))
    card = rng.choice(pool[:6])
    keys = rng.sample(sorted({f"{a}-{b}-{c}" for a in WORDS for b in WORDS for c in WORDS if len({a, b, c}) == 3}), 6)
    visible_keys, hidden_keys, gone_key = keys[:rng.randint(1, 3)], keys[3:5], keys[5]
    visible = [random_row(rng, k, pool) for k in visible_keys]
    hidden = [row(k, owner=f"#{card}") for k in hidden_keys]
    base_rows = visible + [row(gone_key)]
    base = {REGISTRY: registry_text(base_rows, rng)}
    text, stray, header_error = stray_registry(rng, shape, visible, hidden, rng.choice(visible_keys + [gone_key]))
    owned = {k: "kept" for k, r in zip(visible_keys, visible) if check.owners(registry_row_of(r)) & {card}}
    summary = summary_for(owned, {card})
    expect(f"stray {shape}", check.problems(base, {REGISTRY: text}, summary, {card}, is_log), [stray_message(n) for n in stray] + ([header_error[1]] if header_error else []))
    return shape


def broken_header(rng):
    while header_rule(cells := random_header(rng))[0] != "error":
        pass
    return cells, "| " + " | ".join(cells) + " |\n|" + " --- |" * len(cells) + "\n"


def broken_row(rng, key, good):
    """(line, message after `registry row <key> (<where>) `) of a row the registry grammar refuses."""
    if rng.random() < 0.5:
        return "| `{}` | known-red | `p` | — | sig | ev | — |\n".format(key), "has 7 cells, its table header has 9"
    return (row(key, status="flaky", owner=good[key].split("|")[8].strip()),
            "has the Status 'flaky', none of known-red, fixed-pending-soak, no-longer-fires, proposed")


def repair_property(rng):
    """A base error that the tip fixes or deletes passes; one that the tip keeps is reported once, at the tip."""
    pool = list(range(1, 10))
    cards = {rng.choice(pool[:6])}
    keys = rng.sample(sorted({f"{a}-{b}-{c}" for a in WORDS for b in WORDS for c in WORDS if len({a, b, c}) == 3}),
                      rng.randint(3, 8))
    good = {k: random_row(rng, k, pool) for k in keys}
    summary = lambda text: summary_for({k: "kept" for k, r in check.registry({REGISTRY: text}, "the tip")[0].items()
                                        if check.owners(r) & cards}, cards)
    if rng.random() < 0.3:
        fate = rng.choice(["fixed", "kept"])
        cells, header = broken_header(rng)
        base = registry_text(list(good.values()), rng, first_header=header)
        tip = registry_text(list(good.values()), rng, first_header=HEADER if fate == "fixed" else header)
        want = [] if fate == "fixed" else [f"{REGISTRY}:5 at the tip: the table header names registry columns but is not "
                                             f"the registry header ({header_rule(cells)[1]})"]
        if fate == "kept":
            summary_text = summary_for({}, cards)
        else:
            summary_text = summary(tip)
        expect(f"header {fate} {cells}", check.problems({REGISTRY: base}, {REGISTRY: tip}, summary_text, cards, is_log), want)
        expect(f"header {fate} {cells} notes", check.fixed_base_errors({REGISTRY: base}, {REGISTRY: tip}),
               [f"base error at {REGISTRY}:5, fixed by this diff"] if fate == "fixed" else [])
        return "header", fate
    victims = keys[:rng.randint(1, 2)]
    fates = {k: rng.choice(["fixed", "deleted", "kept"]) for k in victims}
    broken = {k: broken_row(rng, k, good) for k in victims}
    base_rows = [broken[k][0] if k in broken else good[k] for k in keys]
    tip_rows = [broken[k][0] if fates.get(k) == "kept" else good[k] for k in keys if fates.get(k) != "deleted"]
    base, tip = registry_text(base_rows, rng), registry_text(tip_rows, rng)
    tip_lines = tip.splitlines()
    want = [f"registry row {k} ({REGISTRY}:{tip_lines.index(broken[k][0].rstrip(chr(10))) + 1} at the tip) {broken[k][1]}"
            for k in victims if fates[k] == "kept"]
    want.sort(key=lambda m: int(m.split(":")[1].split(" ")[0]))
    owned = {k: "kept" for k, r in check.registry({REGISTRY: tip}, "the tip")[0].items() if check.owners(r) & cards}
    expect(f"rows {fates}", check.problems({REGISTRY: base}, {REGISTRY: tip}, summary_for(owned, cards), cards, is_log), want)
    base_lines = base.splitlines()
    notes = sorted(base_lines.index(broken[k][0].rstrip("\n")) + 1 for k in victims if fates[k] != "kept")
    expect(f"rows {fates} notes", check.fixed_base_errors({REGISTRY: base}, {REGISTRY: tip}),
           [f"base error at {REGISTRY}:{n}, fixed by this diff" for n in notes])
    return "rows", tuple(sorted(fates.values()))


def stray_examples():
    ok, hidden = row("ok-row", owner="#5"), row("hidden-row", owner="#5")
    bold = "| " + " | ".join(f"**{c}**" if c in check.known_reds_table.REGISTRY_COLUMNS else c for c in HEADER_CELLS) + " |\n"
    lower = "| " + " | ".join(c.lower() if c in check.known_reds_table.REGISTRY_COLUMNS else c for c in HEADER_CELLS) + " |\n"
    sep = HEADER.splitlines(True)[1]
    shapes = [
        ("a blank line inside the table", HEADER + ok + "\n" + hidden, [5]),
        ("a one-cell line above the header", HEADER + ok + "\n| note |\n" + HEADER + hidden, []),
        ("a registry key and a Status in a two-cell line", HEADER + ok + "\n| `ok-row` | known-red |\n", [5]),
        ("a registry key and a bad Status in a two-cell line", HEADER + ok + "\n| `ok-row` | flaky |\n", [5]),
        ("a registry key in a three-cell orphan", HEADER + ok + "\n| `ok-row` | a | b |\n", [5]),
        ("a registry key, short, in a prose table", HEADER + ok + "\n| Note | Cause |\n| --- | --- |\n| `hidden-row` | proposed |\n"
         + hidden, []),
    ]
    for name, header in (("an all-bold header", bold), ("an all-lower-case header", lower)):
        reg = {REGISTRY: HEADER + ok + "\n" + header + sep + hidden}
        got = check.problems({REGISTRY: HEADER + ok}, reg, summary_for({"ok-row": "kept"}, {5}), {5}, is_log)
        assert len(got) == 1 and got[0].startswith(f"{REGISTRY}:5 at the tip: the table header names registry columns"), (name, got)
    prose = {REGISTRY: HEADER + ok + "\n| Note | Cause |\n| --- | --- |\n| `other` | x |\n| `ok-row` | see above |\n| plain | y | z |\n"}
    expect("a prose table that names a registry key without a Status", check.problems(prose, prose, summary_for({"ok-row": "kept"}, {5}), {5}, is_log), [])
    base = {REGISTRY: HEADER + ok + "\n" + bold + sep + hidden}
    tip = {REGISTRY: HEADER + ok + hidden}
    expect("an all-bold table at the base that the tip repairs",
           check.problems(base, tip, summary_for({"ok-row": "kept", "hidden-row": "kept"}, {5}), {5}, is_log), [])
    tip = {REGISTRY: HEADER + ok}
    base = {REGISTRY: HEADER.replace("| Key |", "| **Key** |", 1) + ok}
    expect("a changed header at the base that the tip repairs", check.problems(base, tip, summary_for({"ok-row": "kept"}, {5}), {5}, is_log), [])
    expect("a changed header at the base that the tip repairs, note", check.fixed_base_errors(base, tip),
           [f"base error at {REGISTRY}:1, fixed by this diff"])
    print("[owned-known-reds] stray and repair static examples: PASS")


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
           check.problems(short, gone, summary_for({"ok-row": "kept"}, {5}), {5}, is_log), [])
    expect("a short row at the base that the tip deleted, note", check.fixed_base_errors(short, gone),
           [f"base error at {REGISTRY}:3, fixed by this diff"])
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
    doubled = {REGISTRY: HEADER.replace("| Companions |", "| Companions | Companions |", 1) + row("ok-row", owner="#5")}
    got = check.problems(doubled, doubled, summary_for({"ok-row": "kept"}, {5}), {5}, is_log)
    assert got and not any("another order" in p for p in got), f"a header with a doubled column, in the registry order: {got}"
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
    stray_examples()
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
    classes = Counter()
    for case in range(n):
        try:
            classes[header_property(random_header(rng))] += 1
        except AssertionError as e:
            raise SystemExit(f"[owned-known-reds] header case {case} of seed {seed}: {e}")
    assert all(classes[c] >= n // 20 for c in ("registry", "error", "ignored")), f"header classes {dict(classes)}"
    print(f"[owned-known-reds] {n} random headers {dict(classes)}, seed {seed}: PASS")
    shapes, repairs = Counter(), Counter()
    for case in range(n):
        try:
            shapes[stray_property(rng, rng.choice(STRAY_SHAPES))] += 1
            repairs[repair_property(rng)[0]] += 1
        except AssertionError as e:
            raise SystemExit(f"[owned-known-reds] stray/repair case {case} of seed {seed}: {e}")
    assert all(shapes[s] >= n // 10 for s in STRAY_SHAPES) and all(repairs[r] >= n // 10 for r in ("header", "rows")), \
        f"stray shapes {dict(shapes)}, repairs {dict(repairs)}"
    print(f"[owned-known-reds] {n} stray cases {dict(shapes)}, {n} repair cases {dict(repairs)}, seed {seed}: PASS")


if __name__ == "__main__":
    main()
