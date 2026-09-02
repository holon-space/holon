---
id: 2026-09-02-the-degraded-toast-is-stale-and-calls-cook-files-org
date: 2026-09-02
gap: PERCEPTION
secondary: null
status: OPEN
summary: >-
  The file-sync degraded toast says "bad org file" about `.cook` files and
  never clears after the files ingest successfully, so the app reports a
  failure that has been fixed.
---

## Bug

Found dogfooding the kitchen feature on a copy of Martin's real vault (lane
`kitchen-dogfood`). Three `.cook` recipes failed to parse at boot, and the app
showed:

```
⚠ File sync degraded (bad org file) — OrgMode initial scan failed for 3 file(s)
```

Two things are wrong with that sentence. It calls three cooklang recipes "org
files", naming a format none of them is, which sends the reader to look for a
malformed headline in a file that has no headlines. And it survives the fix:
all three recipes were repaired on disk and re-ingested cleanly — the log
records `write-back quarantine CLEARED … (ingest fully succeeded)` for each —
and the toast still read the same twelve minutes later.

Screenshots: `kd-shots/01-boot.png` and `kd-shots/03-recipe-retry.png` in the
lane scratchpad
(`/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/1d3fdfe9-af2d-42a8-aecb-fbc009830160/scratchpad/kd-shots/`),
bottom right of both.

## Root cause

The degraded banner is raised from the initial-scan result in
`crates/holon-app/src/wiring.rs` (`OrgMode initial scan degraded — some vault
files were not ingested`) and carries a fixed "bad org file" label, because
when it was written org was the only vault format. `FormatRegistry` now admits
`.cook` as a second format, and the label was not made format-aware.

The staleness is the more costly half. The banner is a one-shot report of a
boot-time outcome, and the un-quarantine event that supersedes it is never
routed back to it, so the app's headline claim about its own health is a
snapshot of a state that no longer exists. That is the "degrades to look fine"
failure inverted — it degrades to look broken — and it trains the user to
disbelieve the banner, which is the one thing that must stay believable.

## Missing piece

No windowed test asserts that the degraded banner CLEARS when the condition
behind it clears. `crates/holon-integration-tests/tests/cook_vault_ingest.rs`
covers the ingest but not the banner, and the GPUI windowed suite has no
degraded-banner lifecycle case at all. Nothing checks the banner's wording
against the format that actually failed either.

## Remedy

OPEN. Two changes: the message names the failing file's format from the
registry rather than saying "org", and the banner is driven by live degraded
state so an un-quarantine retracts it. The closing test is a windowed GPUI case
— boot a vault with one unparseable `.cook`, assert the banner names cooklang,
repair the file, assert the banner is gone.
