# Keystone known reds — the full-depth registry

The per-weave land gate is `just keystone-smoke` (ONE proptest case). The
full-depth sweep (`just pbt general`, default 64 cases) reaches sequences the
smoke never draws, and today it fails roughly half its runs from the
pre-existing signatures below. That is why full depth runs as a NIGHTLY tier
(`just keystone-nightly`) instead of per-weave, and why the nightly is judged
against this registry rather than against "zero failures".

**The discipline:**

- A failure whose signature matches a row below is a **pass-with-note**: the
  nightly prints it as a `WARN` line and still exits 0.
- **ANY other signature is a regression to triage** — the nightly exits
  non-zero and prints the novel signature verbatim. Do not add a row to silence
  it; triage it first (`bug-gap-triage`), and only register it here if Martin
  ratifies it as a known red.
- A row is **removed** when its fix lands with a green soak (repeated
  full-depth runs no longer producing that signature). Rows are not archived
  in place — the registry describes what is red NOW.

**How the matching works:** `scripts/keystone-known-reds.sh` parses THIS FILE.
The `Match pattern` column is the single source of truth for classification —
an extended-regex (`grep -E`) applied to each extracted failure signature line.
Editing a pattern here changes the nightly's verdict; there is no second copy
in the script or the justfile. A pattern may not contain `|` — that is the
markdown table separator; use character classes instead of alternation.

## Registry

| Key | Status | Match pattern | Signature | Evidence | Task |
| --- | --- | --- | --- | --- | --- |
| `syn-real-mint` | known-red | `per-tick reconcile: one synthetic per minted real id` | `harness.rs` assertion `per-tick reconcile: one synthetic per minted real id (syn=[], real=[...])` — the per-tick synthetic→real id reconcile finds a real block minted with no synthetic counterpart. | Pre-existing since ≤2026-07-25; also fires in the windowed (GPUI) keystone, so it is not headless-only. | #62 |
| `org-render-echo-loop` | known-red | `diverged from the oracle: .*"inv-org-render-fixed-point"` | `[inv-org-render-fixed-point] render != disk PERSISTED for <budget> — … a real echo-loop / oscillation`. Reproduces around empty-title headings. | Task #66 family; ledger entry 8. | #66 |
| `org-blocks-ref-diverge` | known-red | `diverged from the oracle: .*"inv-blocks-match-ref/[a-z_]+".*fields diverge from reference` | `[inv-blocks-match-ref/org]` (or a sibling projection) reports `fields diverge from reference` on empty-content blocks. | Task #66 family, NEW 9. | #66 |
| `editor-caret-mirror` | known-red | `diverged from the oracle: .*"inv-editor-caret/mirror".*Caret mismatch` | `[inv-editor-caret/mirror] Caret mismatch on <block>: reference model cursor_byte=…, SUT tracked caret=…`. | Task #66 family, NEW 10. | #66 |

## Running the tier

```
just keystone-nightly            # 2 serialized full-depth runs, judged against this file
just keystone-nightly 1 8        # 1 run at 8 cases — for exercising the plumbing, not a gate
```

Keystone runs must be serialized against every other keystone lane:

```
/opt/homebrew/opt/parallel/bin/parallel --semaphore --id holon-keystone -j1 --fg -- just keystone-nightly
```
