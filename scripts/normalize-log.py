#!/usr/bin/env python3
"""Coarse GROUP BY normalizer for /tmp/holon.log.

Strips timestamps, ANSI codes, UUIDs, ULIDs, `block:` URIs, paths, large
integers and inline JSON / SQL-param blobs, then `sort | uniq -c`. Collapses
~16k raw lines into ~1k unique normalized lines — the highest-count rows
are almost always the bottleneck.

Pure stdlib, no deps.

Usage:
    python3 scripts/normalize-log.py /tmp/holon.log | head -80
"""
import re
import sys
from collections import Counter

ANSI = re.compile(r'\x1b\[[0-9;]*m')
TS = re.compile(r'^\d{4}-\d{2}-\d{2}T[\d:.]+Z\s*')
ULID = re.compile(r'\b01[A-HJ-NP-Z0-9]{24}\b')
UUID = re.compile(r'\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b')
BLOCK_URI = re.compile(r'block:[A-Za-z0-9_:-]+')
DOC_URI = re.compile(r'doc:[A-Za-z0-9_:-]+')
PATH = re.compile(r'/[A-Za-z0-9_./-]+\.(?:org|rs|md|json|sql|yaml|toml|loro|db)')
LARGE_INT = re.compile(r'\b\d{6,}\b')
HEX = re.compile(r'\b0x[0-9a-f]+\b')
PARAMS = re.compile(r'-- params: \[.*$', re.DOTALL)
JSON_BLOB = re.compile(r'Text\("\{.*?\}"\)')
SPAN_KV = re.compile(r'\{[a-z_]+=[^}]*\}')


def normalize(line: str) -> str:
    line = ANSI.sub('', line)
    line = TS.sub('', line)
    line = PARAMS.sub('-- params: [...]', line)
    line = JSON_BLOB.sub('Text("{...}")', line)
    line = ULID.sub('<ULID>', line)
    line = UUID.sub('<UUID>', line)
    line = BLOCK_URI.sub('block:<ID>', line)
    line = DOC_URI.sub('doc:<ID>', line)
    line = PATH.sub('<PATH>', line)
    line = HEX.sub('<HEX>', line)
    line = LARGE_INT.sub('<N>', line)
    line = SPAN_KV.sub('{...}', line)
    # collapse repeated VALUES (?, ?, ?), (?, ?, ?)... groups
    line = re.sub(r'(\(\?(?:,\s*\?)*\)),\s*(?:\(\?(?:,\s*\?)*\),?\s*)+', r'\1,...', line)
    line = re.sub(r'\s+', ' ', line).strip()
    return line


def main() -> None:
    if len(sys.argv) != 2:
        sys.stderr.write(f'usage: {sys.argv[0]} <log-file>\n')
        sys.exit(2)
    counts: Counter[str] = Counter()
    with open(sys.argv[1]) as f:
        for line in f:
            if not line.strip():
                continue
            counts[normalize(line)] += 1
    for line, n in counts.most_common():
        print(f'{n:6d}  {line}')


if __name__ == '__main__':
    main()
