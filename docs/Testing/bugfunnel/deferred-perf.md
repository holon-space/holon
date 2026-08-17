- **Partitioned (per-anchor) top-K in Turso IVM.** The advice weaver applies its
  per-anchor top-K in Rust because Turso IVM has global `ORDER BY ... LIMIT` but
  no PARTITION-BY top-K operator, so the watched `advice_rule_{slug}` outer matview
  is per-anchor UNBOUNDED and each recompute is O(all-candidate-advice). Fine while
  advice sets are small. If it becomes a latency dominator (p95<200ms), push a
  partitioned top-K operator into the Turso IVM fork (`core/incremental/`) so
  K-per-anchor is maintained incrementally and the matview is bounded. Referenced
  from `holon_frontend::advice_weaver::recompute_sidecar` (TODO(partitioned-top-K)).
