---
id: 2026-07-12-martin-live-vault-typing-link-edit
date: 2026-07-12
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  P1 (Martin live vault): typing `[[` link (or any edit) in a block — after
  ~1s the ENTIRE block resets to its pre-typing content; fast typing shows
  link suggestions that then vanish, Enter briefly applies the completion then
  it's undone. Co-occurs with set_field e2e 337–1358ms SLO breaches + a
  quarantine write-back flood on Journals.org
source_line: 835
---

## Bug

P1 (Martin live vault): typing `[[` link (or any edit) in a block — after
~1s the ENTIRE block resets to its pre-typing content; fast typing shows
link suggestions that then vanish, Enter briefly applies the completion then
it's undone. Co-occurs with set_field e2e 337–1358ms SLO breaches + a
quarantine write-back flood on Journals.org

## Missing piece

The gpui **focused-editor convergence** path (`editor_view.rs`
`data_sync`/`remote_delta` closures) is platform-only and absent from the
headless keystone; it converges a FOCUSED editor's InputState to any
external `set_field("content")` echo whenever the editor is momentarily idle
(`prev_synced == current`), with NO causal-ordering guard — so a
delayed/stale CDC echo (or a stale org re-ingest write) of pre-typing
content overwrites in-flight edits. No invariant asserts "a focused editor
is never converged BACKWARD to a stale value"; the p95<200ms SLO oracle
isn't wired for the interaction→editor-visible stage at vault scale

## Remedy

ROOT-CAUSED + REPRODUCED LIVE (loro:false SqlOnly, 599-block seed): direct
proof — external `set_field` on the focused block instantly reset its editor
to the injected value; trace `[data-sync] apply … new="…"` +
`editor.converge_input source="data_sync"`. Churn reproduces the write-back
flood + ~1s/keystroke latency. FIXED: op-versioned echo suppression. New
monotonic `write_seq` token (`holon_api::write_seq`, ordering-only newtype)
— the editor stamps each content keystroke and records `last_local_seq`; the
token round-trips `block_raw.write_seq` → `block` matview → CDC → `DataRow`;
the data-sync guard now converges iff `echo_seq >= last_local_seq` and DROPS
stale/reordered echoes of earlier keystrokes (`split_block` truncation
carries a greater seq so it still converges; missing seq fails loud +
drops). Pinned by pure decision fn `evaluate_data_sync_echo` + 8 directed
tests (`echo_suppression`, incl. red-first
`stale_echo_while_typing_ahead_is_dropped`). Verified: gpui lib tests 8/8,
keystone CASES=8 FORCE_FULL green, `cargo check --features pbt` clean.
Related writer-side variant (org re-ingest full-replace, no Loro-mode merge,
`file_sync_controller.rs:1321`) OUT of scope — filed separately by
orchestrator; guard is correct regardless of it. Loro `remote_delta` path
unchanged (Martin runs loro:false; that path has Loro's own version)
