# Scale-soak: first measured table + findings ledger

Recovered/continued from the prior (spend-limited) soak workstream. This is the
first CLEAN measured latency table the workstream produced. All prior report
files were empty (runs panicked at boot or tripped scale deadlines before any
`action_total` event); this one has 169 real end-to-end action events. The test binary was killed
mid-case-2 (no `test result` line), but the latency dataset is complete and valid.

## Run under measurement

- Harness: `general_e2e_composed_pbt` keystone, `HOLON_PBT_FORCE_FULL=1`
  (full_headless wiring => **CRDT on**), synthetic soak vault.
- Config: **500 seed blocks**, per_doc=200, **settle=60000ms**, 2 proptest cases,
  ~40 mixed UI actions. `inv-main-panel-rows-match-focus:warn` softened (DISCLOSED).
- Raw log: `/tmp/holon-soak-20260707-160112.log`
- RSS csv: `/tmp/holon-soak-rss-20260707-160112.csv`
- Report:  `docs/Testing/soak/soak-500-blocks-20260707-160112.txt`
- Repro:
  `just soak 500 40 60000 200 inv-main-panel-rows-match-focus:warn`

## MEASURED TABLE (500 blocks, CRDT on, settle 60s)

END-TO-END  action -> visible rows  (stage=action_total), ms:

    action            n     p50     p95     max    mean
    SplitBlock      127   131.0   191.3   208.0   136.7
    NavigateFocus     6   224.5   259.2   268.0   231.5
    BulkExternalAdd   3   467.0   494.0   497.0   443.3   <- worst p95
    PinBlock          4   216.0   217.8   218.0   216.2
    ApplyMutation     2   341.5   350.9   352.0   341.5
    SimulateRestart   2   244.0   244.9   245.0   244.0
    SetEdgeField      4   104.5   110.1   111.0   105.5
    AddPeer           3   111.0   114.6   115.0   108.0
    CreateDocument    2   137.5   146.1   147.0   137.5
    DeleteDocument    1   257.0     -       -     257.0
    NavigateHome      1   230.0     -       -     230.0
    ToggleState       1   153.0     -       -     153.0
    EmitMcpData       5    53.0    53.0    53.0    52.6
    SwitchView        3    54.0    54.0    54.0    53.0
    (SetupWatch/RemoveWatch/ArrowNavigate/Nothing/EpochFlipRejected ~53-70ms)

PIPELINE STAGE COST (global), ms:

    stage                        n     p50     p95     max    mean
    projection (full pass)     189    53.0   495.2   799.0   121.1
    projection (snapshot only) 189    16.0    22.0    38.0    16.2
    rows (CDC batch apply)     609     0.0     1.0     3.0     0.1

    projection doc size: blocks p50=524 max=558  (full-document DFS per commit)
    rows per CDC batch:  p50=1 max=201

DISPATCH stage (action -> op applied), ms:

    action          n     p50     p95     max    mean
    focus          16  2796.5  3397.2  3815.0  1869.2   <- sidebar-bind cost
    split_block   127    11.0    19.0    35.0    10.9
    set_field       1    35.0     -       -      35.0

DOMINATOR: projection = 22879ms across 189 passes vs 24659ms end-to-end action
wall = **~93% of total action time**.

SLO GATE: worst end-to-end p95 = 494.0ms (BulkExternalAdd) > 200ms threshold => FAIL.

RSS: samples=1534  start=401  peak=912  end=490 MB  (csv began after boot+seed;
peak 912MB at 500 blocks under CRDT). Earlier run2 saw a full-lifecycle
start=67 peak=788 end=410 (growth +342MB) at the same scale.

## Findings ledger (classified)

| # | Signal (evidence)                                                                 | Class                | Fix applied                                                                 |
|---|-----------------------------------------------------------------------------------|----------------------|-----------------------------------------------------------------------------|
| 1 | Boot-to-sync-ready >30s at 5k blocks under CRDT (hardcoded 2s Loro poll starved). | **harness tuning** (masks a real boot-sync cost) | builder.rs ~L377-394: boot poll scales with `HOLON_SOAK_SETTLE_MS` (>=2s). CONFIRMED in tree. |
| 2 | Sidebar click-intent bind: 5s fixed deadline tripped at 500 blocks; boot **focus dispatch p50=2796ms, max=3815ms** (was 4135ms). | **PROD-BUG CANDIDATE** — sidebar nested `live_block` watch streaming cost grows with vault size. | frontend_slice/components.rs L49-55: `raise_deadline` scales with `HOLON_SOAK_SETTLE_MS`. Measurement-unblocking only; underlying cost is real. |
| 3 | `inv-blocks-match-ref`/org divergence: soak docs tracked as user docs.            | **harness correctness** | soak-*.org written to disk + excluded from test-side org readers; `soak_seed.rs` present. |
| 4 | **Projection = ~95% of action wall; full-document DFS snapshot per commit; p95=495ms max=799ms at ~524 blocks.** | **PROD-BUG CANDIDATE (primary)** — projection is O(document size) per commit, so per-action latency grows with vault size. This is the real SLO breach cause. | Not fixed (out of soak scope). Candidate: incremental/subtree projection instead of full-doc DFS. |
| 5 | `inv-main-panel-rows-match-focus` stale-row softening fired (stale `block:block-0` lingering after focus nav). | **PROD-BUG CANDIDATE** (pre-existing; cross-frontend stale-row) | Softened to `warn` (DISCLOSED) to let measurement proceed; separate from soak. |
| 6 | `just soak` reports `action_total events: 0` while the run has 169+. | **harness reporting bug** (cosmetic) | recipe greps literal `stage=action_total`, but the event renders as `stage=<ANSI>"action_total"<ANSI>` (ANSI escapes around `=` AND surrounding quotes) so grep misses. Python analyzer strips ANSI and is correct. Fix: strip ANSI + match `stage=.?"action_total`. |

## Disclosed casualties (NOT measured)

- **No CRDT-off baseline** — every run forced CRDT on (`HOLON_PBT_FORCE_FULL=1`);
  no A/B to isolate CRDT overhead from projection cost.
- **2000 / 5000 / 10000 block tables** — not obtained. 5k panicked at boot in the
  prior run; this recovery pinned a clean result at 500 first. 500 is the only
  scale with a real table.
- **GPU paint** — headless; excludes final paint.
- **Multi-peer sync/merge** — single in-process peer (AddPeer is synthetic).
- **Full-lifecycle RSS for this run** — the 160112 RSS csv started at 401MB
  (after boot+seed), so its +87MB growth understates true start->peak; use run2's
  +342MB (67->788->410) for lifecycle growth at 500 blocks.
