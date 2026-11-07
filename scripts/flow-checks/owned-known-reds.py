#!/usr/bin/env python3
"""Stub: passes every lane summary. The self-check scripts/flow-checks/test-owned-known-reds.py is red on it."""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import known_reds_table  # noqa: E402,F401


def registry(texts, where):
    return {}, []


def owners(row):
    return set()


def lane_cards(summary_text, root_name):
    return None


def problems(base_texts, tip_texts, summary_text, cards, is_log):
    return []


if __name__ == "__main__":
    sys.exit(0)
