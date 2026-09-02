---
id: 2026-09-02-just-apk-recipe-cannot-build-a-launchable-apk
date: 2026-09-02
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  `just apk` (and therefore `just deploy`) packaged neither the launcher-icon
  resources nor `classes.dex`, so the Android dev loop could not produce an
  installable, launchable APK; `just build` in the same file had rotted the
  same way, missing the `aarch64-linux-android` target install.
---

## Bug

Found while setting up the `double-dogfood` lane (Mac desktop + Android
emulator sharing one vault, 2026-09-02). The lane brief names
`just -f frontends/gpui/justfile deploy` as the Android deploy path, so it was
the first thing driven.

`frontends/gpui/justfile`'s `apk` recipe built an APK from the manifest and the
two native `.so` files only. It cannot succeed, and if it did the artifact
would not launch:

- `frontends/gpui/android/AndroidManifest.xml` declares
  `android:icon="@mipmap/ic_launcher"` and `android:roundIcon=`, but the recipe
  ran `aapt2 link` with no compiled resources at all, so the link fails on the
  unresolved references.
- The same manifest sets `android:hasCode="true"` and names the activity
  `dev.gpui.mobile.GpuiNativeActivity`, but the recipe packaged no
  `classes.dex`. That activity is the class that `System.loadLibrary`s the
  native code so `GpuiTextInputView`'s native methods resolve, so a dex-less
  APK cannot start, and the soft keyboard could never surface.

## Root cause

Three separate Android packagers existed in the tree, and only the two that a
gate or a workflow executes stayed correct:

- `frontends/gpui/android/build-apk.sh` — the dev packager. Compiles the two
  vendored Java sources with `javac`, runs `d8`, compiles
  `assets/icons/app/android/res` with `aapt2 compile`, links, aligns, signs,
  and then asserts `classes.dex` is present in the finished APK.
- `frontends/gpui/android/build-release-apk.sh` + `build-release-aab.sh` — what
  `.github/workflows/release.yml:754-755` runs.
- `frontends/gpui/justfile`'s `apk` recipe — a hand-copied duplicate of the
  first, executed by no gate and no workflow.

`.github/workflows/ci.yml` has no Android packaging job; the only Android CI
coverage is the `check-android` justfile gate, which runs
`cargo ndk … check -p holon-turso`, a compile-rot guard that never packages an
APK. So when the icon resources and the dex step were added to `build-apk.sh`
and the manifest, nothing forced the justfile copy to follow, and it silently
rotted.

Evidence: `lane-logs/build-android.log` in the lane workspace, and the
side-by-side of the two recipes.

## Missing piece

No gate executes the Android packaging path. `check-android` proves the crate
graph still cross-compiles; nothing proves a launchable APK can still be
produced, so a packager that no one runs can drift from the manifest it is
supposed to satisfy without any test going red.

The deeper cause is duplication rather than absence of a test: the dev loop had
its own copy of logic that already existed in a script, so correctness had to
be maintained in two places by hand.

## Second escape from the same gap: `just build`

The recipe one line above `apk` had rotted the same way, and is recorded here
rather than in its own entry because it is the same escape: the Android dev
loop is on no gate, so every recipe in it drifts independently.

`just build` failed on a fresh workspace with:

```
error[E0463]: can't find crate for `core`
  = note: the `aarch64-linux-android` target may not be installed
error: could not compile `cfg-if` (lib) due to 1 previous error
```

The headline blames `cfg-if` and `memchr`, which have nothing to do with it;
the cause appears only in a trailing `note:`. `rust-toolchain.toml` pins
`nightly-2026-08-16` and declares no `targets`, so a freshly materialized
toolchain has only the host target. Two other places already compensated —
`justfile:705-715` (`check-android`) and
`.github/workflows/release.yml:720-722`, whose comment cites this exact
`E0463` — and the dev-loop recipe was the one path that skipped it.

## Remedy

Fixed by deletion, not by a second test. `frontends/gpui/justfile`'s `apk`
recipe now delegates to `android/build-apk.sh`, so there is one dev packager
and the divergence is structurally impossible. `build-apk.sh` already carries
the invariant that matters — it refuses to finish if the APK has no
`classes.dex` — and that assertion now protects `just deploy` too. The
build-tools, platform, NDK and manifest path variables were deleted from the
justfile in the same pass, since duplicating them there is what let the two
implementations drift apart.

`just build` now performs the same guarded `rustup target add` as
`check-android`, gated on `rustup target list --installed` so it is not a
network touch on every build, with a comment recording why the raw failure
names `cfg-if`.

Still open, two items:

- No gate builds an APK, so a future edit to `build-apk.sh` itself is
  unguarded. A cheap CI job that runs it and asserts the artifact contains
  `classes.dex` and a resolved launcher icon would close that.
- `rust-toolchain.toml` still declares no `targets`. Adding
  `aarch64-linux-android` there would make every consumer correct by
  construction and let all three guarded installs be deleted, at the cost of
  downloading the target on every host that materializes the toolchain,
  including CI jobs that never cross-compile. That trade is Martin's call.

## Also required, and undocumented

`frontends/gpui/justfile` opens with `set dotenv-path := "android/.env"` plus
`set dotenv-load`, so every recipe in the file fails when that file is absent —
and it is untracked, so it is absent in every fresh clone and every fresh jj
workspace. It needs `ANDROID_SDK_HOME` and `ANDROID_NDK_HOME`. Not a bug in
itself, but it is the first wall a new Android contributor hits, and nothing
in the tree says so.
