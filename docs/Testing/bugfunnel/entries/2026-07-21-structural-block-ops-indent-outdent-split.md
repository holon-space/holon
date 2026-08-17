---
id: 2026-07-21-structural-block-ops-indent-outdent-split
date: 2026-07-21
gap: PERCEPTION
secondary: ENVIRONMENT
status: UNCLASSIFIED
summary: >-
  Structural block ops (indent/outdent/split/join/move/delete/cycle…)
  double-advertised under Loro authority (SqlBlockOperations +
  LoroBlockOperations) — second source of persisting slash-menu dupes (dogfood
  B1 live: Delete x2, Cycle Task State x2); SqlBlockOperations wins by
  registration order so structural mutations under Loro route SQL-direct =
  latent authority bypass. Tolerated via debug-only
  STRUCTURAL_BLOCK_OP_DUP_ALLOWLIST (W7).
source_line: 1057
---

## Bug

Structural block ops (indent/outdent/split/join/move/delete/cycle…)
double-advertised under Loro authority (SqlBlockOperations +
LoroBlockOperations) — second source of persisting slash-menu dupes (dogfood
B1 live: Delete x2, Cycle Task State x2); SqlBlockOperations wins by
registration order so structural mutations under Loro route SQL-direct =
latent authority bypass. Tolerated via debug-only
STRUCTURAL_BLOCK_OP_DUP_ALLOWLIST (W7).

## Missing piece

structural-op authority/routing under Loro undecided; menu-level: extend
slashmenu_correspondence to assert entries unique by (label, op id)

## Remedy

MENU-LAYER FIXED+WOVEN 2026-07-21 (cycle 2) — presentation dedup by op id in
command_provider.rs build_command_items (dispatch byte-identical); empirical
dup list = indent/outdent/move_up/move_down/embed_entity (Listed subset of
the allowlist; round-3 Delete/Cycle-Task-State screenshot reading was
imperfect). Locked by slashmenu_correspondence uniqueness test vs
prod-faithful dual-provider engine; round-4 live: each op exactly once.
DISPATCH AUTHORITY still OPEN (SqlBlockOperations wins by registration order
under Loro — ruling queued)
