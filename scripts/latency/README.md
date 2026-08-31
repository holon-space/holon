# set_field latency measurement

Measures prod `stage="e2e"` (interaction→projection-visible) against a live GPUI
app over MCP, in two arms that differ only in how fast writes enter the pipeline.

```sh
bash scripts/latency/build-target.sh <tree-root>                 # cargo build -p holon-gpui --features pbt
bash scripts/latency/run_tree.sh <tree-root> 8730 <label> <out-dir>
python3 scripts/latency/analyze_spans.py <app.log> <label> <paced-p50-ms>
```

`run_tree.sh` prints the app log path and writes `<out-dir>/arms-<label>.json`.
For an A/B, run one tree at a time on the same machine — window state and host
load move these numbers more than most code changes do.

JSON fields: `burst` is one `type_text` of 32 characters (dispatches ~5 ms
apart); `paced` is 32 separate `type_text` calls with `await_quiescence` between
each, so one interaction is in flight at a time — that arm is the service time,
the burst arm is service plus queue wait. Each carries `n`/`p50`/`p95`/`max`/`mean`
in ms plus the raw `*_samples`; `*_wall_s` is the arm's driving duration and
`*_window_inactive` counts the app's own `WINDOW-INACTIVE` markers, which
certify whether the render loop was OS-throttled during the arm.

`analyze_spans.py` recovers each sample's dispatch instant as `delivery_ts - ms`
and reports the arrival interval, the drain rate, and queue wait as a share of
the burst p50. `tight_arm.py` is a variant that drives N writes from concurrent
MCP connections instead of one call.
