---
id: 2026-09-14-gatekeeper-verifying-windows-stall-test-runs
date: 2026-09-14
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  Running the suites makes macOS stack up Gatekeeper "Verifying ..." windows,
  one per freshly linked test binary, because the app responsible for the test
  run holds no Developer Tools grant and every binary is only ad-hoc signed.
---

## Bug
Found by Martin while several lanes ran nextest gates concurrently: macOS put
up a cascade of progress windows titled `Verifying "<test binary>-<hash>"`
(four stacked at once in the screenshot, names such as
`settings_seeded_secret_windowed-f3d92cc1eef6ad60`) and the machine became
barely usable. No test failed; the host, not the product, degraded.

Measured on macOS 27.0 (26A5425a) during a live five-lane run:

| Signal (10 min window)                                    | Count |
| --------------------------------------------------------- | ----- |
| `CSUICodeEvaluationController startProgressForInfo:`        | 136   |
| `GatekeeperPolicyScanError "Code did not match ... policy"` | 252   |
| `kTCCServiceDeveloperTool` queries from `syspolicyd`        | 362   |

Per-binary cost, from `scripts/gatekeeper-assessment-cost.sh`:

| binary size | first exec | second exec |
| ----------- | ---------- | ----------- |
| 1.09 MB     | 262 ms     | 63 ms       |
| 33.8 MB     | 579 ms     | 139 ms      |
| 135.3 MB    | 1763 ms    | 71 ms       |
| 405.8 MB    | 4751 ms    | 66 ms       |

## Root cause
Cargo links every test binary ad-hoc (`flags=0x20002(adhoc,linker-signed)`,
`TeamIdentifier=not set`), so it matches no Gatekeeper policy. macOS exempts
such binaries when the app *responsible* for the process chain holds the
Developer Tools privacy grant. Here it does not, and the whole chain is
unredacted in the unified log:

```
syspolicyd  GK evaluateScanResult: 2, PST: (team: (null)),
            (id: action_bar_windowed-caf19069fbfe470b), (bundle_id: NOT_A_BUNDLE)
tccd        AUTHREQ_ATTRIBUTION: responsible={identifier=com.stablyai.orca,
            responsible_path=/Applications/Orca.app/Contents/MacOS/Orca},
            accessing={identifier=action_bar_windowed-caf19069fbfe470b},
            requesting={identifier=com.apple.syspolicyd}
tccd        Service kTCCServiceDeveloperTool does not allow prompting; returning denied.
syspolicyd  Error Domain=GatekeeperPolicyScanError "Code did not match any
            currently allowed policy"
```

The assessment hashes the whole file (`holon-gpui` is 505,814,936 bytes, of
which 307,544,064 is `__LINKEDIT`; its code directory carries 122,533 page
hashes), so cost tracks size. The verdict is cached per code-directory hash,
so each binary pays once and pays again after every relink.

Only windowed GPUI tests produce a *visible* window: they attach to the window
server, so `CoreServicesUIAgent` shows the progress UI. Headless binaries pay
the same stall silently, which is why the lanes felt slow beyond the popups.

The earlier explanation on file (a 178k-entry `deps` enumeration) does not
apply: the lanes' `target/debug/deps` hold 1028 entries, and the test binaries
are not in `deps` at all but under `target/debug/build/holon-gpui/<hash>/out/`.

## Gap
ENVIRONMENT. No product code is wrong and no invariant could fire. The host's
security policy is part of the test environment and diverges from what an
unattended gate run assumes.

## Fix
Documented in DEVELOPMENT.md, "Gatekeeper 'Verifying ...' windows during test
runs", with `scripts/gatekeeper-assessment-cost.sh` as the before/after
measurement.

The fix itself is a one-time macOS setting only Martin can apply: System
Settings > Privacy & Security > Developer Tools, add and enable
`/Applications/Orca.app`, then restart it. The grant is scoped to that app and
does not disable Gatekeeper. Agents must not apply it; changing the system
security posture is the owner's decision.

STATUS is FIXED: with the grant in place the `Verifying` windows and the
policy-scan storm are gone (see "After the grant" below). The first exec of a
large binary does not collapse onto the second exec, and that residual cost is
tracked as parity work.

## After the grant (2026-09-15)

Martin granted Developer Tools to `/Applications/Orca.app` and restarted it.
The orchestrator then re-ran `scripts/gatekeeper-assessment-cost.sh` from the
integration workspace (01:37, 6 s window).

| Signal                             | Before grant | After grant |
| ---------------------------------- | ------------ | ----------- |
| `Verifying` windows shown          | 5            | 0           |
| Gatekeeper policy scans            | 56           | 1           |
| `kTCCServiceDeveloperTool` queries | 54           | 1           |
| measurement window                 | 15 s         | 6 s         |
| 405.8 MB binary, first exec        | 4674 ms      | 2296 ms     |
| 405.8 MB binary, second exec       | 399 ms       | 25 ms       |

The reported symptom is gone. The machine no longer stalls under a cascade of
`Verifying` windows, and the policy scans fell from 56 to 1 in a comparable
window. The fix therefore confirms the root cause: the missing Developer Tools
grant on the responsible app produced both the windows and the scan storm.

The earlier FIXED criterion in this entry, that first exec collapses onto
second exec, is not met. A fresh 405.8 MB binary still costs about 2.3 s on
first exec against about 25 ms when its verdict is cached. The measurement does
not separate an assessment cost from the page-in cost of a 405.8 MB file, so
that residual cost is recorded as parity work below and stays unexplained
here.

Raw log: `scratchpad gatekeeper-after-1789426647.log`, the orchestrator's
scratchpad copy of the run, not tracked in the repository.

## Parity work
Three levers would cut the residual cost even with the grant, none taken here:

- **Binary count.** 315 integration-test source files under `crates/*/tests`
  each link their own binary, so each relink re-triggers the per-binary cost.
- **Binary size.** `__LINKEDIT` symbols are 58% of `holon-gpui`. Stripping them
  would shrink the hashed region, but it would also cost symbolicated
  backtraces in a PBT-heavy suite, so it needs Martin's ruling.
- **Separate assessment cost from page-in cost.** The after-grant run leaves the
  first exec of a 405.8 MB binary at about 2.3 s against about 25 ms cached, and
  it does not say how much of that is Gatekeeper. Run the probe with the file
  cache dropped, or compare against a notarised binary of the same size.
