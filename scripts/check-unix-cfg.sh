#!/usr/bin/env bash
# Guard: `std::os::unix` in non-test code of crates/ must sit under `#[cfg(unix)]`
# (within the 50 lines above, which covers an
# enclosing module), or the crate stops compiling on Windows, which
# CI's `windows-check` job builds. Lines after a file's first `#[cfg(test)]`
# are test code and exempt.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

violations="$(find crates -name '*.rs' -not -path '*/tests/*' -not -path '*/target/*' -print0 |
  xargs -0 awk '
    FNR == 1 { in_tests = 0; delete recent }
    /#\[cfg\((all\()?test/ { in_tests = 1 }
    !in_tests && /std::os::unix/ {
      guarded = 0
      for (i = 1; i <= 50; i++) if (recent[FNR - i] ~ /cfg\(unix\)/) guarded = 1
      if (!guarded) print FILENAME ":" FNR ": " $0
    }
    { recent[FNR] = $0 }
  ')"

if [ -n "$violations" ]; then
  echo "std::os::unix outside #[cfg(unix)] (breaks the Windows build):" >&2
  echo "$violations" >&2
  exit 1
fi
