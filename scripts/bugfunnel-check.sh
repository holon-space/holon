#!/usr/bin/env bash
# Recomputes each BugFunnel category total as archived-baseline + sum(increment
# log) and diffs it against the header. Only the LEADING "(+N CATEGORY" token
# of each `- (+N …)` line counts — a `secondary CATEGORY: …` mention inside the
# same line is prose, not a second increment, and must not be double-counted.
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
  # increment log line: "- (+N CATEGORY ..." — only the leading token counts.
  /^- \(\+[0-9]+ [A-Z]+/ {
    line = $0
    sub(/^- \(\+/, "", line)
    split(line, parts, " ")
    n = parts[1] + 0
    cat = parts[2]
    sub(/[^A-Z].*$/, "", cat)
    if (!(cat in norm)) {
      print "UNKNOWN CATEGORY TOKEN: " cat " in line: " $0 > "/dev/stderr"
      exit 2
    }
    sum[norm[cat]] += n
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
      if (actual != expected) {
        printf "MISMATCH %s: header=%s baseline+log=%d (baseline=%d log_sum=%d)\n", cat, actual, expected, baseline[cat], sum[cat]
        status = 1
      } else {
        printf "OK %s: header=%s == baseline+log=%d\n", cat, actual, expected
      }
    }
    exit status
  }
' "$DOC"
