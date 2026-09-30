#!/usr/bin/env python3
"""Write an anonymised org vault shaped like a real long-lived one.

The shape follows a measured personal vault: pages spread over nested
directories, about 45 headlines per page, headline depth falling off from 1 to
8, nearly every headline carrying an :ID:, some properties, TODO keywords and
`[[id:...]]` links between pages. The text is synthetic.

    gen_deep_vault.py OUT_DIR [--pages 400] [--seed 7]
"""

import argparse
import random
from pathlib import Path

# Headlines per depth in the measured vault, levels 1..8.
DEPTH_WEIGHTS = [2979, 6861, 4138, 1832, 1156, 1023, 372, 176]
WORDS = (
    "alpha bravo cedar delta ember fjord grove harbor island juniper kestrel "
    "lantern meadow nectar orbit pine quartz river summit timber umber valley "
    "willow yarrow zephyr"
).split()
KEYWORDS = ["TODO", "DOING", "DONE", "WAITING", None, None, None, None]


def words(rng, n):
    return " ".join(rng.choice(WORDS) for _ in range(n))


def page_path(rng, index):
    depth = rng.choice([0, 1, 1, 2, 2, 3, 3, 4, 5])
    dirs = [f"area{rng.randrange(6)}"] + [f"topic{rng.randrange(4)}" for _ in range(depth)]
    return Path(*dirs[: depth + 1]) / f"Page {index:04d}.org"


def headline_levels(rng, count):
    levels = [1]
    for _ in range(count - 1):
        want = rng.choices(range(1, 9), weights=DEPTH_WEIGHTS)[0]
        levels.append(min(want, levels[-1] + 1))
    return levels


def page(rng, index, all_ids):
    lines = [
        f"#+TITLE: Page {index:04d} {words(rng, 2)}",
        f"#+ID: dv-page-{index:04d}",
        "",
    ]
    count = max(1, int(rng.gauss(45, 20)))
    ids = []
    for n, level in enumerate(headline_levels(rng, count)):
        block_id = f"dv-{index:04d}-{n:03d}"
        ids.append(block_id)
        keyword = rng.choice(KEYWORDS)
        title = words(rng, rng.randint(2, 7))
        if all_ids and rng.random() < 0.05:
            title += f" [[id:{rng.choice(all_ids)}][see {words(rng, 1)}]]"
        head = "*" * level + (f" {keyword}" if keyword else "") + f" {title}"
        lines.append(head)
        lines.append(":PROPERTIES:")
        lines.append(f":ID: {block_id}")
        if rng.random() < 0.15:
            lines.append(f":CATEGORY: {rng.choice(WORDS)}")
        if rng.random() < 0.05:
            lines.append(f":EFFORT: {rng.randint(1, 8)}h")
        lines.append(":END:")
        if rng.random() < 0.3:
            lines.append(words(rng, rng.randint(5, 25)))
    return "\n".join(lines) + "\n", ids


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("out", type=Path)
    parser.add_argument("--pages", type=int, default=400)
    parser.add_argument("--seed", type=int, default=7)
    args = parser.parse_args()
    if args.out.exists() and any(args.out.iterdir()):
        raise SystemExit(f"{args.out} is not empty")
    rng = random.Random(args.seed)
    all_ids = []
    headlines = 0
    for index in range(args.pages):
        text, ids = page(rng, index, all_ids)
        path = args.out / page_path(rng, index)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        all_ids.extend(ids)
        headlines += len(ids)
    print(f"pages={args.pages} headlines={headlines} out={args.out}")


if __name__ == "__main__":
    main()
