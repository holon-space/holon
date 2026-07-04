# HANDOFF: iOS-simulator pixel proof for holon-gpui (WS-IOS-SIM)

Self-contained instructions to finish the ONE remaining step: prove (or disprove)
that the already-built holon GPUI iOS-sim app actually PRESENTS FRAMES, with
screenshots. Everything else (build, upstream-issue draft) is DONE.

## 1. Goal and current state

- **Goal:** boot an iOS simulator, install + launch the app, screenshot it at
  launch and after a tap, and paste the evidence into the Android upstream issue
  draft. The load-bearing claim is "works on iOS sim vs never presents on Android
  emulator".
- **Workspace (ALL paths relative to this; never touch the main checkout):**
  `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/ws-mobile-gpui`
- **Already built (2026-07-05, `just ios-xcbuild Debug`, BUILD SUCCEEDED, log
  `logs/ios-build.log`):**
  - App bundle: `frontends/gpui/ios/build/Debug-iphonesimulator/Holon.app`
    (614 MB binary, arm64, links Metal/MetalKit/UIKit; bundle id `space.holon.gpui`)
  - Staticlib: `target/aarch64-apple-ios-sim/debug/libholon_gpui.a`
- **Revisions (from Cargo.lock, cite in evidence):**
  - gpui-mobile: `fa778004e88b85f6f4e1a31b5da5bd8d0c7fd0f2`
    (git+https://github.com/holon-space/gpui-mobile.git?branch=holon)
  - gpui (zed fork): `e4cf5b1570923b93fdec76bcfd3db207fad373ff`
    (git+https://github.com/holon-space/zed.git?branch=holon)
- If the .app is missing or stale, rebuild with:
  `cd frontends/gpui && just ios-xcbuild Debug` (cap cargo with `CARGO_BUILD_JOBS=4`).

## 2. The blocking gate

Every `xcrun simctl` call HANGS on this machine: system CoreSimulator.framework is
1051.54 vs Xcode 26.6's 1051.55, so the simctl wrapper blockingly runs
`xcodebuild -runFirstLaunch`, which needs admin auth.

- **Check:** `bash -c 'xcodebuild -checkFirstLaunchStatus > logs/firstlaunch-check.log 2>&1; echo EXIT=$? >> logs/firstlaunch-check.log'`
  - exit **69** = still blocked. Do NOT proceed; do NOT run sudo yourself.
    Report that the USER must run `sudo xcodebuild -runFirstLaunch` once.
  - exit **0** = gate cleared, proceed.
- ALWAYS wrap simctl calls in `timeout 30 ...` so a hang can't wedge the session.
  Hung first-launch child processes linger — kill them by PID (see §5).

## 3. Proof steps (once gate is cleared)

Run each via `bash -c '<cmd> > logs/<name>.log 2>&1'` (nushell false-greens plain
redirects) and READ the log afterward. From the workspace root:

1. List sims: `timeout 30 xcrun simctl list devices available > logs/sim-list.log 2>&1`
   Pick an iPhone UDID (justfile recipe greps `iPhone 1[5-9]|iPhone 2`). Call it $SIM.
2. Boot: `xcrun simctl boot $SIM` (ok if "already booted"), then optionally
   `open -a Simulator`. Wait for `xcrun simctl bootstatus $SIM` to return.
3. Install: `xcrun simctl install $SIM frontends/gpui/ios/build/Debug-iphonesimulator/Holon.app`
4. Launch: `xcrun simctl launch $SIM space.holon.gpui` (note the printed PID).
   Optionally set env first: `xcrun simctl launch --console-pty $SIM space.holon.gpui`
   to capture app stdout, or export `SIMCTL_CHILD_RUST_FONTCONFIG_DLOPEN=on` if
   font loading fails.
5. Screenshot at launch (give it ~5-10 s to settle):
   `xcrun simctl io $SIM screenshot logs/ios-sim-01-launch.png`
6. Tap interaction (simctl has no native tap; use one of):
   - `xcrun simctl io $SIM sendkey` is NOT a thing — instead use
     AppleScript/cliclick on the Simulator window, or simply launch, screenshot,
     then use `xcrun simctl openurl` / app-visible state changes; if no tap path
     works, capture a second screenshot after ~15 s (`logs/ios-sim-02-later.png`)
     and DISCLOSE that no tap was exercised.
7. Read the .png files with the Read tool to VERIFY content honestly:
   - **Frame presents** = non-black image with visible holon UI (sidebar/journals
     text, buttons). Screenshot IS the claim.
   - **Broken** = black/blank/crashed — screenshot it anyway and grab the app log:
     `xcrun simctl spawn $SIM log show --last 5m --predicate 'process == "Holon"' > logs/ios-app.log 2>&1`
   Never claim success from exit codes alone; only from looking at the pixels.

## 4. Where the evidence goes

Update `logs/android-upstream-issue-draft.md` — it has TWO clearly marked
"NOT CAPTURED" slots (lines ~70-78, "Evidence status (iOS)" section):
1. The runtime-pixel-proof bullet: replace with PASS/FAIL + what the screenshots show.
2. The screenshot-paths bullet: replace with the actual `logs/ios-sim-*.png` paths
   + one-line description each.
Keep the draft honest (if the frame did NOT present, say so — that changes the
issue's framing). **Do NOT file anything on GitHub** — the supervisor/user files it.

## 5. Cleanup

- `xcrun simctl terminate $SIM space.holon.gpui` (ignore errors)
- `xcrun simctl shutdown $SIM`
- Kill ONLY processes you started, by PID (never pkill by name — the user runs
  their own `holon-gpui` desktop instances that must not be touched).

## 6. Protocol constraints

- NO jj/git commands that write; read-only only (`jj --ignore-working-copy log|show|file`).
- Never touch the main checkout `/Users/martin/Workspaces/pkm/holon` or other
  workspaces; all reads/writes inside the ws-mobile-gpui workspace.
- All command output redirected into `logs/` via `bash -c '... > logs/x.log 2>&1'`.
- Cap cargo with `-j 4` / `CARGO_BUILD_JOBS=4` if rebuilding.
- Long waits: background + poll; produce activity at least every 50 minutes.
- Final report: simulator UDID, screenshot paths + honest per-image description,
  draft path, cleanup confirmation, disclosed limitations (tap/keyboard/rotation
  not exercised, etc.).
