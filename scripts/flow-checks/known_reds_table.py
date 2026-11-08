"""The rows of the known-red registry tables in docs/Testing/*KnownReds*.md.

A table is a header line, a separator line directly under it, and the `|` lines directly under that. A header with
any of the columns `Key`, `Status`, `Task`, `Owner` (compared without emphasis and case) opens a registry table and
must be the exact registry `HEADER`, else `HeaderError`: a table under a changed header would hide its rows. A table
whose header has none of these columns is prose, whatever its cells hold. A `|` line that belongs to no table (after
a blank line, above a header) is an orphan. A cell holds no literal `|`: a pattern writes alternation as `&#124;`,
decoded after the split. The prose columns (Signature, Evidence) do carry stray `|`s, so the columns before them are
read from the left and the columns after them from the right; the surplus cells belong to the last prose column.
"""
import re

HEADER = ["Key", "Status", "Match pattern", "Site", "Signature", "Evidence", "Task", "Owner", "Companions"]
REGISTRY_COLUMNS = {"Key", "Status", "Task", "Owner"}
EMPHASIS = re.compile(r"[*_`]+")
STATUSES = ("known-red", "fixed-pending-soak", "no-longer-fires", "proposed")
PROSE = ("Signature", "Evidence")
SEPARATOR = re.compile(r":?-+:?")


class RowError(Exception):
    def __init__(self, key, message):
        super().__init__(message)
        self.key = key


class HeaderError(Exception):
    def __init__(self, lineno, cells):
        missing = [c for c in HEADER if c not in cells]
        extra = list(dict.fromkeys(c for c in cells if c not in HEADER))
        doubled = [c for c in HEADER if cells.count(c) > 1]
        why = "; ".join(f"{name} {', '.join(columns)}" for name, columns in
                        [("missing", missing), ("extra", extra), ("doubled", doubled)] if columns)
        super().__init__(f"the table header names registry columns but is not the registry header "
                         f"({why or 'the columns stand in another order'})")
        self.lineno = lineno


def unticked(cell):
    return cell.strip().removeprefix("`").removesuffix("`")


def split(line):
    return [cell.strip() for cell in line.strip().split("|")[1:-1]]


def is_separator(cells):
    return all(SEPARATOR.fullmatch(c) for c in cells)


def names_registry_column(cells):
    return bool({c.lower() for c in REGISTRY_COLUMNS} & {EMPHASIS.sub("", c).strip().lower() for c in cells})


def scan(text):
    """Yield (lineno, header, cells, in_table) per data line; `header` is None outside a registry table and
    `in_table` is False for an orphan. Raises HeaderError at a header that names a registry column but is not
    the registry header."""
    lines = text.splitlines()
    header, in_table = None, False
    for lineno, line in enumerate(lines, 1):
        if not line.startswith("|"):
            header, in_table = None, False
            continue
        cells = split(line)
        if is_separator(cells):
            continue
        under = lines[lineno].startswith("|") and is_separator(split(lines[lineno])) if lineno < len(lines) else False
        if not in_table and under:
            in_table = True
            if names_registry_column(cells):
                if cells != HEADER:
                    raise HeaderError(lineno, cells)
                header = cells
            continue
        yield lineno, header, cells, in_table


def table_rows(text):
    """Yield (lineno, header, cells) per data line of every `|` line; `header` is None outside a registry table."""
    for lineno, header, cells, _ in scan(text):
        yield lineno, header, cells


def registry_row(header, cells):
    """The cells of a registry row by column name; raises RowError for a row the registry grammar does not admit."""
    key = unticked(cells[0])
    if not (cells[0].startswith("`") and cells[0].endswith("`") and key):
        raise RowError(cells[0], f"the Key cell {cells[0]!r} is not a code span")
    if len(cells) < len(header):
        raise RowError(key, f"has {len(cells)} cells, its table header has {len(header)}")
    prose = [i for i, column in enumerate(header) if column in PROSE]
    if len(cells) > len(header) and not prose:
        raise RowError(key, f"has {len(cells)} cells, its table header has {len(header)} and no prose column")
    if prose:
        right = len(header) - prose[-1] - 1
        cut = len(cells) - right
        cells = cells[:prose[-1]] + [" | ".join(cells[prose[-1]:cut])] + cells[cut:]
    row = dict(zip(header, cells))
    row["Key"] = key
    if row.get("Status") not in STATUSES:
        raise RowError(key, f"has the Status {row.get('Status')!r}, none of {', '.join(STATUSES)}")
    for column in ("Match pattern", "Site"):
        if column in row:
            row[column] = unticked(row[column]).replace("&#124;", "|")
    return row
