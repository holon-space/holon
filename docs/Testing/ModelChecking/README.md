# Model checking: write-back and settle

TLA+ specs of the org write-back pipeline and of the settle waits that tests
use, checked with TLC. One model is also written in Quint.

## Files

| File | What it holds |
|---|---|
| `SettleProperties.tla` | The user-level properties every spec checks |
| `Writeback.tla` | Org write-back of one block, a test that settles and reads the file |
| `SettleWake.tla` | A windowed settle that waits on `UiWatermark::changed` |
| `HomeBySettle.tla` | The `home_by` fold, its write-back consumer and the delivery cursor; the action alphabet of the PBT in `crates/holon-api/src/live_data/home_by/settle_spec.rs` |
| `HomeBySettle_trace.cfg` | The design that PBT replays its recorded runs against, in process (tla-checker) |
| `Writeback.qnt` | The pre-fix settle of `Writeback.tla` in Quint |
| `*.cfg` | One design of a spec each: constants only |
| `Writeback_through_oracle.cfg` | The oracle of a red test, checked on the design that closes its race |
| `run.py` | Checks every spec once per property and compares with the expected outcome |
| `test_run_exit_codes.py` | Pins that every crash of `run.py` exits 2 |
| `windows.py` | Counts the violating settle returns by the stage that holds the newest edit |

Each action in a spec names the code it models (`Models: <file>:<function>`)
or says why it has no anchor (`Modelling choice: ...`).

## Run

From the repository root:

```sh
python3 docs/Testing/ModelChecking/run.py --out target/model-checking/logs
```

The script exits 0 when every outcome is the expected one, 1 when an outcome
differs, and 2 when it has no verdict: a tool fails, the command line is
wrong, or the script crashes. It writes one log per run and
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

## Rules

- A replay names the commit whose code has the replayed shape; a design that
  no commit held (a hybrid or a mutant) is reported as such, never as a
  replay.
- A spec bound to a PBT shares its action names with the PBT's action enum;
  a test compares the two.
- A red test of a race that TLC finds is a red-first test only if its oracle
  holds on the design that closes the race. `run.py` checks the oracle as an
  invariant of that design (`RED_TEST_ORACLES`, with the expected verdict).
