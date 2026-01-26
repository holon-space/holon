#!/usr/bin/env python3
"""Extract SQL trace from holon HOLON_TRACE_SQL=1 log and produce a .sql replay file.

Usage:
    python3 scripts/extract-sql-trace.py /tmp/flutter-sql-trace-v2.log > /tmp/replay.sql

The output is a plain SQL file where:
  - Parameters are inlined (named $params and positional ? placeholders)
  - Timing between statements is captured as SQL comments
  - Duplicate DDL lines are deduplicated (keeps actor_ddl, skips execute_ddl)
  - CDC callback registration is emitted as `-- !CDC_CALLBACK_SET` directives

Filtering:
  --include TABLE1,TABLE2   Only include statements mentioning these tables
  --exclude TABLE1,TABLE2   Exclude statements mentioning these tables
  --after TIMESTAMP         Only include statements after this ISO timestamp
  --before TIMESTAMP        Only include statements before this ISO timestamp
  --stop-at LINE            Stop reading the log at this line number
  --stop-pattern REGEX      Stop reading when a line matches this regex
"""

import re
import argparse
from datetime import datetime

ANSI_ESCAPE_RE = re.compile(r'\x1b\[[0-9;]*m')


# Matches a traced SQL line from tracing output
# Group 1: timestamp, Group 2: tag (execute_sql, actor_ddl, etc.), Group 3: SQL + optional params
TRACE_RE = re.compile(
    r'^(\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d+)Z\s+'
    r'(?:TRACE|DEBUG|INFO)\s+holon::storage::turso:\s+'
    r'\[TursoBackend\]\s+([\w]+):\s+(.*)',
    re.DOTALL,
)

# Any line that starts with a timestamp or known log prefix — NOT a SQL continuation
ANY_LOG_LINE_RE = re.compile(
    r'^(?:\d{4}-\d{2}-\d{2}T|flutter:|The relevant|\[|thread \')'
)

# Named params: key=Type("value") or key=Type(number)
NAMED_PARAM_RE = re.compile(
    r'(\w+)=(?:'
    r'String\("([^"]*)"\)'       # String("...")
    r'|Integer\((\d+)\)'         # Integer(N)
    r'|Real\(([\d.]+)\)'         # Real(N.N)
    r'|Null'                     # Null
    r')'
)

# Positional params in [...] list
POSITIONAL_PARAM_RE = re.compile(
    r'(?:'
    r'Text\("((?:[^"\\]|\\.)*)"\)'  # Text("...")
    r'|Integer\((-?\d+)\)'          # Integer(N)
    r'|Real\(([\d.eE+-]+)\)'        # Real(N.N)
    r'|Null'                         # Null
    r')'
)


def parse_timestamp(ts_str: str) -> datetime:
    # Truncate sub-microsecond precision
    dot_idx = ts_str.rfind('.')
    if dot_idx >= 0:
        frac = ts_str[dot_idx + 1:]
        if len(frac) > 6:
            ts_str = ts_str[:dot_idx + 7]
    return datetime.strptime(ts_str, '%Y-%m-%dT%H:%M:%S.%f')


def escape_sql_string(s: str) -> str:
    return "'" + s.replace("'", "''") + "'"


def inline_named_params(sql: str, params_str: str) -> str:
    """Replace $name placeholders with actual values from named params."""
    params = {}
    for m in NAMED_PARAM_RE.finditer(params_str):
        key = m.group(1)
        if m.group(2) is not None:  # String
            params[key] = escape_sql_string(m.group(2))
        elif m.group(3) is not None:  # Integer
            params[key] = m.group(3)
        elif m.group(4) is not None:  # Real
            params[key] = m.group(4)
        else:  # Null
            params[key] = 'NULL'

    # Also handle Null which doesn't have a capture group
    for m in re.finditer(r'(\w+)=Null', params_str):
        key = m.group(1)
        if key not in params:
            params[key] = 'NULL'

    for key, value in params.items():
        sql = sql.replace(f'${key}', value)

    return sql


def inline_positional_params(sql: str, params_str: str) -> str:
    """Replace ? placeholders with actual values from positional params list."""
    values = []
    for m in POSITIONAL_PARAM_RE.finditer(params_str):
        if m.group(1) is not None:  # Text
            values.append(escape_sql_string(m.group(1).replace('\\"', '"')))
        elif m.group(2) is not None:  # Integer
            values.append(m.group(2))
        elif m.group(3) is not None:  # Real
            values.append(m.group(3))
        else:  # Null
            values.append('NULL')

    # Also count raw Null tokens not captured by the regex groups
    # We need to walk the params string more carefully
    values_from_raw = []
    pos = 0
    while pos < len(params_str):
        m = POSITIONAL_PARAM_RE.search(params_str, pos)
        # Check for standalone Null before next match
        null_pos = params_str.find('Null', pos)
        if null_pos >= 0 and (m is None or null_pos < m.start()):
            # Check it's not part of a longer word
            before_ok = null_pos == 0 or params_str[null_pos - 1] in ' ,['
            after_ok = null_pos + 4 >= len(params_str) or params_str[null_pos + 4] in ' ,]'
            if before_ok and after_ok:
                values_from_raw.append(('NULL', null_pos))
                pos = null_pos + 4
                continue
        if m is None:
            break
        if m.group(1) is not None:
            values_from_raw.append((escape_sql_string(m.group(1).replace('\\"', '"')), m.start()))
        elif m.group(2) is not None:
            values_from_raw.append((m.group(2), m.start()))
        elif m.group(3) is not None:
            values_from_raw.append((m.group(3), m.start()))
        else:
            values_from_raw.append(('NULL', m.start()))
        pos = m.end()

    # Sort by position to get correct order
    values_from_raw.sort(key=lambda x: x[1])
    ordered_values = [v for v, _ in values_from_raw]

    # Replace ? placeholders left-to-right
    result = []
    val_idx = 0
    for ch in sql:
        if ch == '?' and val_idx < len(ordered_values):
            result.append(ordered_values[val_idx])
            val_idx += 1
        else:
            result.append(ch)

    return ''.join(result)


def should_include(sql: str, include_tables: set, exclude_tables: set) -> bool:
    # Only match against the SQL template part (before any inlined string values)
    # Strip single-quoted strings to avoid matching table names in param values
    sql_stripped = re.sub(r"'[^']*'", "''", sql)
    sql_lower = sql_stripped.lower()
    if include_tables:
        return any(t.lower() in sql_lower for t in include_tables)
    if exclude_tables:
        return not any(t.lower() in sql_lower for t in exclude_tables)
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument('logfile', help='Path to the SQL trace log file')
    parser.add_argument('--include', help='Comma-separated table names to include')
    parser.add_argument('--exclude', help='Comma-separated table names to exclude')
    parser.add_argument('--after', help='Only include statements after this ISO timestamp')
    parser.add_argument('--before', help='Only include statements before this ISO timestamp')
    parser.add_argument('--stop-at', type=int, metavar='LINE',
                        help='Stop reading the log at this line number')
    parser.add_argument('--stop-pattern', metavar='REGEX',
                        help='Stop reading when a line matches this regex')
    parser.add_argument('--dedup-ddl', action='store_true', default=True,
                        help='Deduplicate DDL (keep actor_ddl, skip execute_ddl)')
    args = parser.parse_args()

    include_tables = set(args.include.split(',')) if args.include else set()
    exclude_tables = set(args.exclude.split(',')) if args.exclude else set()
    after_ts = parse_timestamp(args.after) if args.after else None
    before_ts = parse_timestamp(args.before) if args.before else None

    # Tags that represent actual execution (not the outer dispatch)
    ACTOR_TAGS = {'actor_ddl', 'execute_sql', 'execute_via_actor', 'transaction_stmt'}
    # Directive tags (non-SQL events emitted as special comments)
    DIRECTIVE_TAGS = {'set_change_callback'}
    # Tags to skip when deduplicating
    SKIP_TAGS = {'execute_ddl'} if args.dedup_ddl else set()

    statements = []  # list of (timestamp, sql_with_params)

    stop_pattern = re.compile(args.stop_pattern) if args.stop_pattern else None
    with open(args.logfile, 'r') as f:
        lines = []
        for line_num, raw_line in enumerate(f, 1):
            clean = ANSI_ESCAPE_RE.sub('', raw_line)
            if args.stop_at and line_num >= args.stop_at:
                break
            if stop_pattern and stop_pattern.search(clean):
                break
            lines.append(clean)

    i = 0
    while i < len(lines):
        line = lines[i]
        m = TRACE_RE.match(line)
        if not m:
            i += 1
            continue

        timestamp_str = m.group(1)
        tag = m.group(2)
        sql_and_params = m.group(3)

        i += 1

        # Skip non-execution tags
        if tag in SKIP_TAGS:
            # But we need to consume continuation lines
            while i < len(lines) and not ANY_LOG_LINE_RE.match(lines[i]):
                i += 1
            continue

        # Directive tags become special comments (no SQL body)
        if tag in DIRECTIVE_TAGS:
            ts = parse_timestamp(timestamp_str)
            if after_ts and ts < after_ts:
                continue
            if before_ts and ts > before_ts:
                continue
            statements.append((ts, tag, None))
            continue

        if tag not in ACTOR_TAGS:
            continue

        # Collect continuation lines (multiline DDL)
        while i < len(lines):
            next_line = lines[i]
            # A continuation line doesn't start with a timestamp or log prefix
            if ANY_LOG_LINE_RE.match(next_line):
                break
            sql_and_params += '\n' + next_line.rstrip()
            i += 1

        # Parse timestamp
        ts = parse_timestamp(timestamp_str)

        if after_ts and ts < after_ts:
            continue
        if before_ts and ts > before_ts:
            continue

        # Split SQL from params
        params_marker = ' -- params: '
        if params_marker in sql_and_params:
            idx = sql_and_params.index(params_marker)
            sql = sql_and_params[:idx].strip()
            params_str = sql_and_params[idx + len(params_marker):]

            # Determine named vs positional
            if params_str.startswith('['):
                sql = inline_positional_params(sql, params_str)
            else:
                sql = inline_named_params(sql, params_str)
        else:
            sql = sql_and_params.strip()

        # Collapse internal blank lines so multiline DDL doesn't break
        # when fed to tursodb (which treats blank lines as statement separators)
        sql = '\n'.join(line for line in sql.split('\n') if line.strip())

        if not should_include(sql, include_tables, exclude_tables):
            continue

        statements.append((ts, tag, sql))

    # Output
    print(f'-- Extracted from: {args.logfile}')
    print(f'-- Statements: {len(statements)}')
    if statements:
        print(f'-- Time range: {statements[0][0].isoformat()}Z .. {statements[-1][0].isoformat()}Z')
    print()

    prev_ts = None
    for ts, tag, sql in statements:
        if prev_ts is not None:
            delta_ms = (ts - prev_ts).total_seconds() * 1000
            if delta_ms >= 1:
                print(f'-- Wait {delta_ms:.0f}ms')
        if tag in DIRECTIVE_TAGS:
            directive_name = tag.upper()
            print(f'-- !{directive_name} {ts.isoformat()}Z')
        else:
            print(f'-- [{tag}] {ts.isoformat()}Z')
            print(f'{sql};')
        print()
        prev_ts = ts


if __name__ == '__main__':
    main()
