"""The rows of the known-red registry tables in docs/Testing/*KnownReds*.md.

A table is a run of lines that start with `|`. A row whose first cell is `Key` is a registry header; the rows below it,
up to the end of the table, are registry rows. A cell holds no literal `|`: a pattern writes alternation as `&#124;`,
decoded after the split. The prose columns (Signature, Evidence) do carry stray `|`s, so the columns before them are
read from the left and the columns after them from the right; the surplus cells belong to the last prose column.
"""
import re

STATUSES = ("known-red", "fixed-pending-soak", "no-longer-fires", "proposed")
PROSE = ("Signature", "Evidence")
SEPARATOR = re.compile(r":?-+:?")


class RowError(Exception):
    def __init__(self, key, message):
        super().__init__(message)
        self.key = key


def unticked(cell):
    return cell.strip().removeprefix("`").removesuffix("`")


def split(line):
    return [cell.strip() for cell in line.strip().split("|")[1:-1]]


def table_rows(text):
    """Yield (lineno, header, cells) per data row of every table; `header` is None outside a registry table."""
    header = None
    for lineno, line in enumerate(text.splitlines(), 1):
        if not line.startswith("|"):
            header = None
            continue
        cells = split(line)
        if cells and cells[0] == "Key":
            header = cells
        elif cells and not all(SEPARATOR.fullmatch(c) for c in cells):
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
