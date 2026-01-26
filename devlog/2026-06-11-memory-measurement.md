# Memory measurement: Phases 0–3(partial) state, virtualized vs eager

Date: 2026-06-11. Binary: `holon-gpui --release --features heap-profile`
(dhat + MemoryMonitor RSS @30s), corpus = synthetic vault with one
1000-block page (`bigpage.org`, ~86KB) + default assets, Loro enabled,
fresh DB per run. Workload: boot → click bigpage in left sidebar (main
panel renders 1000 rows) → settle ~100s → 10×scroll-down + 3×scroll-up →
settle ~130s → SIGINT. No concurrent builds.

Run dirs (logs, dhat-heap.json, dhat_summary_*.txt):
`~/.claude/jobs/f9fe225e/tmp/runA` (virtualized default),
`~/.claude/jobs/f9fe225e/tmp/runB` (`HOLON_EAGER_PANEL_RENDER=1`).

| metric | A: virtualized | B: eager rollback |
|---|---|---|
| dhat live at global max | 162MB | 135MB |
| dhat live at end | 158MB | 135MB |
| dhat total churn | 12.5GB * | 5.0GB |
| RSS settle after render | ~373MB | ~383MB |
| RSS after scroll workload | ~539MB | ~496MB |

\* Run A is contaminated: ad-hoc MCP SQL probing during the run inflated
Turso-side churn (1.4GB actor vs 0.3GB in B). Render-side numbers are
comparable.

## Conclusions

1. **Panel virtualization is a paint/CPU win, not an RSS win** at 1000
   blocks: per-row `ReactiveViewModel` trees exist in both modes; gpui's
   `list` only skips element building for off-screen rows (element churn
   returns to the frame arena anyway).
2. **Top retained render-side allocations (both runs)**: render
   interpreter `build` (~28MB live at peak) + `OperationDescriptor::clone`
   (~16MB live) — per-row op-descriptor clones. This is the strongest
   argument for the remaining Phase 3 items (ElementInfo `Arc<str>`,
   op-descriptor sharing/dedup).
3. **Post-scroll RSS jump (~+100MB in both) is NOT live-heap growth**
   (dhat at-gmax ≈ at-end); under heap-profile it's dominated by dhat
   bookkeeping of scroll churn. Scrolling does cause heavy transient
   allocation (bumpalo chunks, relayout, splice) but it's reclaimed.
4. **Churn hotspots** (12.5GB/5GB per ~6min session!): Turso actor
   command processing, `Vec<u8>` capacity allocs (I/O buffers, 2.4GB in
   A), DBSP `HashableRow` clones, `render_entity_tree`/org re-render
   (set_property/push_str ~330MB). live_query streaming (in flight) and
   Phase 4 LiveData dedup target parts of this.
5. dhat dump on SIGINT races the tokio ctrl_c flush-then-exit handler in
   `frontends/gpui/src/main.rs` — both runs did write the file but B took
   ~45s. If a future run lacks dhat-heap.json, that race is why.

Baseline-vs-now for Phases 0/1 wasn't measurable: the work is already
absorbed into main and no clean pre-phase commit builds with the same
measurement plumbing. These numbers stand as the go-forward baseline.
