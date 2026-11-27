"""Count the settle returns of Writeback.tla that break SettledReadsNewest, by
the pipeline stage that holds the newest edit at the return. Reads the full
state graph that `tlc -dump dot,actionlabels` writes; a return is a SettlePoll
edge into a state with phase "returned", and equal return states count once.

Usage: python3 windows.py <graph.dot> <label>
Prints `<label>.<stage>: <count>` lines and `<label>.returns: <total>`.
Exits 1, and prints no counts, when the graph holds no return at all, a return
it cannot classify, or the script crashes (an unreadable graph, a missing
argument).
"""

import re
import sys

STAGES = [
    # furthest-along stage first: the newest edit is reported where it is now
    ("net-behind", lambda s: s["authority"] == s["newest"] != s["base"]),
    ("bulk-armed", lambda s: s["bulk"] and s["feed"] == s["newest"]),
    ("rerender-channel", lambda s: s["newest"] in s["rrChan"]),
    ("supervisor-channel", lambda s: s["newest"] in s["supChan"]),
    ("fold", lambda s: s["folding"] and s["foldVer"] == s["newest"]),
    ("fold-source", lambda s: s["srcPending"] and s["feed"] == s["newest"]),
    ("cdc-hop", lambda s: s["feed"] < s["newest"] == s["store"]),
    ("lost-edit", lambda s: s["store"] != s["newest"]),
]

NODE = re.compile(r'(-?\d+) \[label="(.*)"(?:,style = filled)?\];?$')
EDGE = re.compile(r'(-?\d+) -> (-?\d+) \[label="(\w+)"')


def parse_value(raw):
    raw = raw.strip()
    if raw in ("TRUE", "FALSE"):
        return raw == "TRUE"
    if re.fullmatch(r"-?\d+", raw):
        return int(raw)
    if raw.startswith("<<"):
        if "|->" in raw:
            return [int(v) for v in re.findall(r"v \|-> (\d+)", raw)]
        return [int(v) for v in re.findall(r"-?\d+", raw)]
    return raw.strip('"')


def parse_state(label):
    text = re.sub(r"\\(.)", lambda m: "\n" if m.group(1) == "n" else m.group(1), label)
    state = {}
    for line in text.splitlines():
        m = re.match(r"/\\ (\w+) = (.*)", line)
        if m:
            state[m.group(1)] = parse_value(m.group(2))
    return state


def main():
    graph_path, label = sys.argv[1], sys.argv[2]
    nodes, returns = {}, set()
    for line in open(graph_path, encoding="utf-8"):
        edge = EDGE.match(line)
        if edge:
            if edge.group(3) == "SettlePoll":
                returns.add(edge.group(2))
            continue
        node = NODE.match(line)
        if node:
            nodes[node.group(1)] = parse_state(node.group(2))
    returned = [nodes[n] for n in returns if nodes[n]["phase"] == "returned"]
    if not returned:
        sys.exit(f"{graph_path}: no settle return in the state graph")
    counts = {name: 0 for name, _ in STAGES}
    violating = 0
    for state in returned:
        if state["disk"] == state["newest"]:
            continue
        stage = next((name for name, holds in STAGES if holds(state)), None)
        if stage is None:
            sys.exit(f"{graph_path}: unclassified return state {state}")
        counts[stage] += 1
        violating += 1
    for name, count in counts.items():
        print(f"{label}.{name}: {count}")
    print(f"{label}.returns: {violating}")


if __name__ == "__main__":
    main()
