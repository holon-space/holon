The blocks of prompt-facts.md that differ for the unit `scripts`; `hook prompt-facts scripts` puts each in place of the block of the same name.

@@ UNIT
scripts folder

@@ UNITS
units

@@ CODE
`scripts/`

@@ BUILD_TOOL
scripts test

@@ BUILD_ENV
PYTHONDONTWRITEBYTECODE=1

@@ TEST_CMD
@HOOK_DIR@/hook run-scripts-tests

@@ NARROW_RUN
`python3 scripts/<file>.py` or `bash scripts/<file>.sh`

@@ PBT_LIB
Python's `random` (the scripts tests' house style: fixed examples first, then drawn cases with the seed in the failure message)

@@ RETURNED_ERROR
exited non-zero with a message

@@ LINT_TOOL
a syntax check

@@ FORMAT_CMD
true # the scripts folder has no formatter:

@@ TEST_DEPS_RULE
The scripts tests use only Python's standard library and bash; add no dependency.

@@ TEST_REGISTRATION_RULE
- Name each new test file `test-<behaviour>.py` or `test-<behaviour>.sh`, under `scripts/` or a folder below it. The scripts test run finds every `test-*` file, so nothing registers it; leave tester-mods.txt empty.

@@ LINT_RULE
- Your last tester-run script run must also pass a syntax check of your files: `bash -n <file>` for each .sh file and `python3 -c 'import ast, sys; ast.parse(open(sys.argv[1]).read())' <file>` for each .py file. Fix each error, then freeze again.

@@ BEFORE_LANE_CHECKS
A diff of scripts alone selects no clippy and no nextest step: run every step line that `python3 scripts/select-tests.py --base <base>` prints, as printed.
