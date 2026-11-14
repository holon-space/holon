The holon text of the lane briefs: `hook prompt-facts` prints this file with @PKGS@ (packages and their test features), @CLIPPY_LINTS@, @HOOK_DIR@ and @TESTS_DIRS@ filled in.
Block format and the names the kit's prompts use: the agent-patterns kit's hook-interface.md.

@@ STEWARD
holon-steward

@@ STEWARD_ALIASES
 (offline Remote Control sessions are also named "Holon Steward")

@@ USER
Martin

@@ CARD_VIEW
gh issue view @CARD@ -R holon-space/holon-work

@@ UNIT
crate

@@ UNITS
crates

@@ CODE
`src/`

@@ TESTS_DIR
@TESTS_DIRS@

@@ BUILD_OUTPUT
`target/`

@@ BUILD_TOOL
cargo

@@ BUILD_ENV
CARGO_TARGET_DIR=@W@/target

@@ TEST_CMD
cargo nextest run @PKGS@

@@ NARROW_RUN
`-p`

@@ PBT_LIB
proptest

@@ RETURNED_ERROR
returned Err

@@ LINT_TOOL
clippy

@@ FORMAT_CMD
rustfmt --edition 2024 --config-path @T@/rustfmt.toml

@@ PRIVATE_DATA
~/.config/holon* (the user's live database)

@@ PRIVATE_PATHS
~/.config/holon*

@@ EVIDENCE_PLACE
 (a report and failing probes in the lane's target folder)

@@ CLASS_EXAMPLE
 (e.g. a group's leader: alive, gone, a foreign process; a pid file: absent, empty, foreign lines)

@@ OWNED_STATE_EXAMPLE
 (for "a group the shell did not start": its leader alive AND its leader gone)

@@ NAME_EXAMPLE
 ("netd asserts on such a pgid")

@@ CONFIG_EXAMPLE
 (for a net: every kind of transition, arc and logic the format allows, on every place the change touches)

@@ TEST_DEPS_RULE
You may add dev-dependencies to the crate's `Cargo.toml` before the first freeze only, so the implementer knows the Cargo.lock change before its lane checks (`proptest.workspace = true` and `proptest-state-machine.workspace = true` exist in the workspace).

@@ TEST_REGISTRATION_RULE
- The crate may have ONE test binary (`autotests = false`, `tests/main.rs` with `mod` lines). Then list each module line you need in @R@/tester-mods.txt as `<crate> mod <name>;`; the kit's tester-run script and the landing append it, and `@HOOK_DIR@/add-mods.sh @R@ @T@ <crate>` does it for your own runs. Never edit `tests/main.rs` itself.

@@ LINT_RULE
- Your last tester-run script run must also be clippy-clean for your files: run `cd @T@ && CARGO_TARGET_DIR=@W@/target FLOW_CARD=@CARD@ @BIN@/nosecrets.sh @SLOT@ cargo clippy @PKGS@ --tests -- @CLIPPY_LINTS@ > <log> 2>&1` and fix each warning in your files (for example `type_complexity`: a type alias), then freeze again. The lane checks run clippy with `-D warnings`.

@@ BEFORE_LANE_CHECKS
Before the lane checks, run `jj diff --git --from <base> -- Cargo.lock`. A changed Cargo.lock (e.g. the tester's dev-dependency) makes the test selection the whole workspace: run that full selection ONCE, never a narrow run first. Also run the cheap generated-row checks first: `just arch-validate` (strict already; a new test dir needs `just arch-baseline`, keep only your crate's hunk) and `scripts/timing-split.py check` plus `assertions` (a new wait or a new timing-critical test needs its rows). Each such row is an edit outside Owns: ask holon-steward in questions.md and get the answer BEFORE the long lane-check run.
