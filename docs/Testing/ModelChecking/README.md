# Model checking: write-back and settle

TLA+ specs of the org write-back pipeline and of the settle waits that tests
use, checked with TLC. One model is also written in Quint.

## Files

| File | What it holds |
|---|---|
| `SettleProperties.tla` | The user-level properties every spec checks |
| `Writeback.tla` | Org write-back of one block, a test that settles and reads the file |
| `SettleWake.tla` | A windowed settle that waits on `UiWatermark::changed` |
| `Writeback.qnt` | The pre-fix settle of `Writeback.tla` in Quint |
| `*.cfg` | One design of a spec each: constants only |
| `run.py` | Checks every spec once per property and compares with the expected outcome |
| `windows.py` | Counts the violating settle returns by the stage that holds the newest edit |

Each action in a spec names the code it models (`Models: <file>:<function>`)
or says why it has no anchor (`Modelling choice: ...`).

## Run

From the repository root:

```sh
python3 docs/Testing/ModelChecking/run.py --out target/model-checking/logs
```

The script exits 0 when every outcome is the expected one, 1 when an outcome
differs, and 2 when a tool fails. It writes one log per run and
numbers.txt into the `--out` folder.

## Tools

- Java 21 or newer on `PATH`.
- TLC: `tla2tools.jar` of TLA+ release v1.7.4. `run.py` downloads it to
  `target/model-checking/` and checks its sha256, or takes the path in
  `TLA2TOOLS_JAR`.
- Quint 0.34.0 (npm package @informalsystems/quint). `run.py` installs it
  under `target/model-checking/`, or takes the binary in `QUINT`. On its first
  run Quint downloads its Rust evaluator to `~/.quint/`.
- Nothing is installed into the tracked tree.
