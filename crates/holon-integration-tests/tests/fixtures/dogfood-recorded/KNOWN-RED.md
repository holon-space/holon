# Parked recordings (`*.feature.known-red`)

The replay suite discovers `*.feature` only; these are real dogfood recordings
whose deterministic replay is red on a KNOWN cause. Re-activate by renaming
back to `.feature` once the blocking item is fixed.

- `compass_item_authoring.feature.known-red` — replay trips the registered
  `editor-caret-mirror` known red deterministically (reference cursor_byte=40,
  SUT caret=0 after TypeChars). That makes it the family's second deterministic
  reproducer; it is referenced in that row in docs/Testing/KeystoneKnownReds.md.
- `compass_slash_menu.feature.known-red` — step `TriggerSlashCommand` fails its
  preconditions on the composed headless SUT (the recording pins the WINDOWED
  MCP-driver behavior of BugFunnel row "template picker unreachable"; the
  headless slash flow needs different setup). Re-shape or re-activate with the
  F1 fix.
