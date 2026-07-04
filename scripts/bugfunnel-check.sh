#!/usr/bin/env bash
# Recomputes each BugFunnel category total as archived-baseline + sum(increment
# log) and diffs it against the header AND against the Ledger table rows, which
# are the ground truth. Only the LEADING "(±N CATEGORY" token of each
# `- (±N …)` line counts — a `secondary CATEGORY: …` mention inside the
# same line is prose, not a second increment, and must not be double-counted.
# Reconciliation lines may be negative, so the sign is parsed.
#
# Row-counting rules: one table row = one counted bug; `A→B` in the Primary-gap
# cell counts as the re-triaged class B; the description cell can itself contain
# `|`, so the gap class is the first cell after the date that names one; and the
# table CONTINUES below the `## Deferred perf` heading that interrupts it, so
# every table line to EOF is scanned.
#
# Written in awk rather than bash associative arrays: macOS ships bash 3.2,
# which lacks `declare -A`.
set -euo pipefail

DOC="${1:-docs/Testing/BugFunnel.md}"

awk '
  BEGIN {
    baseline["ENVIRONMENT"] = 87
    baseline["COVERAGE"]    = 37
    baseline["PERCEPTION"]  = 35
    baseline["ORACLE"]      = 18
    norm["ENV"]  = "ENVIRONMENT"; norm["ENVIRONMENT"] = "ENVIRONMENT"
    norm["COV"]  = "COVERAGE";    norm["COVERAGE"]    = "COVERAGE"
    norm["PERC"] = "PERCEPTION";  norm["PERCEPTION"]  = "PERCEPTION"
    norm["ORACLE"] = "ORACLE"
  }
  # increment log line: "- (±N CATEGORY ..." — only the leading token counts.
  /^- \([+-][0-9]+ [A-Z]+/ {
    line = $0
    sign = (substr($0, 4, 1) == "-") ? -1 : 1
    sub(/^- \([+-]/, "", line)
    split(line, parts, " ")
    n = sign * (parts[1] + 0)
    cat = parts[2]
    sub(/[^A-Z].*$/, "", cat)
    if (!(cat in norm)) {
      print "UNKNOWN CATEGORY TOKEN: " cat " in line: " $0 > "/dev/stderr"
      exit 2
    }
    sum[norm[cat]] += n
    next
  }
  # ledger table row: "| YYYY-MM-DD | description | CATEGORY | ..."
  /^\| 20[0-9][0-9]-[0-9][0-9]-[0-9][0-9] \|/ {
    nf = split($0, cell, "|")
    for (i = 3; i <= nf; i++) {
      c = cell[i]
      gsub(/^[ \t]+|[ \t]+$/, "", c)
      sub(/^.*→/, "", c)     # `A→B` — the re-triaged class wins
      if (c in norm) { rowcount[norm[c]]++; rows_total++; break }
      if (i == nf) print "UNCLASSIFIED ROW: " substr($0, 1, 80) > "/dev/stderr"
    }
    next
  }
  # header line: "- CATEGORY: N"
  /^- (ENVIRONMENT|COVERAGE|PERCEPTION|ORACLE): [0-9]+$/ {
    split($0, parts, ": ")
    cat = parts[1]
    sub(/^- /, "", cat)
    header[cat] = parts[2] + 0
  }
  END {
    status = 0
    n = split("ENVIRONMENT COVERAGE PERCEPTION ORACLE", cats, " ")
    for (i = 1; i <= n; i++) {
      cat = cats[i]
      expected = baseline[cat] + sum[cat]
      actual = (cat in header) ? header[cat] : "MISSING"
      if (actual != expected || actual != rowcount[cat]) {
        printf "MISMATCH %s: header=%s baseline+log=%d rows=%d (baseline=%d log_sum=%d)\n", cat, actual, expected, rowcount[cat], baseline[cat], sum[cat]
        status = 1
      } else {
        printf "OK %s: header=%s == baseline+log=%d == rows=%d\n", cat, actual, expected, rowcount[cat]
      }
    }
    printf "ledger rows scanned: %d\n", rows_total
    exit status
  }
' "$DOC"
