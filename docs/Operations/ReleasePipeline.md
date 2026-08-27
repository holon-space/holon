# Release Pipeline

**The workflow file is the specification.** What builds, in what order, with
which tools, into which artifact names, lives in
`.github/workflows/release.yml` (tag `v*.*.*`). Read that for "what happens".

This document holds only what that file cannot say: *why* the pipeline is
shaped this way, the one-time setup that happens outside the repo, where each
secret comes from, the human steps, and what to do when a release job fails.

## Why one workflow

One tag builds every platform: macOS, Linux, Windows, iOS and Android.

Desktop and mobile used to be split by tag prefix (`v*` vs `mobile-v*`) so a
desktop fix could ship without consuming an Apple/Google build number. The
split cost more than it saved, because both halves needed the same
`aarch64-linux-android` build and each kept its own copy of it. The copies
diverged: a fix to strip the release `.so` landed in the mobile half only, so
the same commit shipped a 93 MB APK under `mobile-v0.0.16` and a 759 MB one
under `v0.0.17`. One recipe per platform makes that class of drift
unrepresentable.

The accepted cost: **every `v*` tag now consumes an Apple build number and a
Play versionCode**, so a desktop-only fix still burns a store submission slot.
Both stores reject reused build numbers, so a re-release always means a new
patch version and a new tag.

## Why attestation, not code signing, is the integrity mechanism

The workflow attaches a GitHub **Artifact Attestation** (SLSA build
provenance) to every artifact: a publicly verifiable statement that the file
was built by this repo's release workflow from the exact tagged commit. OS
code-signing (Apple notarization, Play signing) only proves the *publisher's
identity* to the OS — it says nothing about which source produced the bits.
The attestation is what makes "no injected code" checkable by anyone:

```
gh attestation verify <downloaded-file> --repo <owner>/holon
```

Per-OS `SHA256SUMS-*.txt` files are attached as a plain-checksum fallback.

## Why the Apple/Android paths are gated on repo *variables*

Apple Developer Program enrollment and the Android keystore are external
prerequisites that may not exist yet. Rather than let a release fail on
missing credentials, the workflow branches on repo **variables**
`APPLE_RELEASE_ENABLED` and `ANDROID_RELEASE_ENABLED`; when a variable is not
`"true"` the gated job is replaced by a stub job (`ios-skipped`,
`android-skipped`) that emits a `::warning::` saying exactly which variable
and secrets are missing, and the generated release notes label the degraded
output (e.g. an `-unsigned` macOS zip). This is the "fall back visibly" rule
applied to CI: a release still ships, but nobody can mistake it for a signed
one.

**The variable is not the same thing as the secrets.** Both variables are
currently `true`, so the stub jobs never run and the real jobs always attempt
the signed path. `APPLE_RELEASE_ENABLED=true` with the `MACOS_DEVELOPER_ID_*`
secrets unset does **not** produce the unsigned fallback — it produces a
*failing* macOS job (`security: SecKeychainItemImport: One or more parameters
passed to a function were not valid`), and the release ships with no macOS
artifact at all. The unsigned fallback is reached only by setting the
variable to `false`. Turning a variable on is therefore a commitment to
having its secrets in place.

Variables (not secrets) because they must be readable in `if:` conditions and
carry nothing sensitive. Set them under Settings → Secrets and variables →
Actions → **Variables**.

Windows is unsigned by decision (no OV/EV certificate purchased for v1). Users
see one SmartScreen prompt; the release notes say so. Attestation still
applies.

## One-time manual setup, in order

All of this happens outside the repo and cannot be automated.

### 1. Apple Developer Program (unblocks macOS signing + iOS)

1. Enroll at <https://developer.apple.com/programs/enroll/> — **US$99/year**,
   as an individual or organization (org needs a D-U-N-S number). Approval
   usually takes a day or two.
2. Note your **Team ID** (Membership page, 10-char alphanumeric) → repo
   secret `APPLE_TEAM_ID`.

### 2. App Store Connect API key (used for notarization AND TestFlight)

1. <https://appstoreconnect.apple.com> → **Users and Access** →
   **Integrations** → **App Store Connect API** → Team Keys → **+**.
2. Role: **App Manager** (enough for uploads; Admin also works).
3. Download the `.p8` file — **you can only download it once**.
4. Store as repo secrets:
   - `APPLE_API_KEY_ID` — the Key ID shown in the list (e.g. `2X9R4HXF34`)
   - `APPLE_API_ISSUER_ID` — the Issuer ID shown at the top of the page (a UUID)
   - `APPLE_API_KEY_P8_BASE64` — `base64 -i AuthKey_XXXX.p8 | pbcopy`

### 3. macOS: Developer ID Application certificate

1. On a Mac: Keychain Access → Certificate Assistant → **Request a
   Certificate From a Certificate Authority** → save the `.certSigningRequest`.
2. <https://developer.apple.com/account/resources/certificates> → **+** →
   **Developer ID Application** → upload the CSR → download the `.cer` →
   double-click to install into your keychain.
3. In Keychain Access, find "Developer ID Application: …", expand it, select
   cert **and** private key → File → Export Items → `.p12` with a strong
   password.
4. Store as repo secrets:
   - `MACOS_DEVELOPER_ID_CERT_P12_BASE64` — `base64 -i cert.p12 | pbcopy`
   - `MACOS_DEVELOPER_ID_CERT_PASSWORD` — the export password

### 4. iOS: Distribution certificate + provisioning profile + app registration

1. Same CSR flow as above, but choose **Apple Distribution** as the
   certificate type. Export as `.p12` the same way:
   - `IOS_DISTRIBUTION_CERT_P12_BASE64`
   - `IOS_DISTRIBUTION_CERT_PASSWORD`
2. Register the App ID: developer.apple.com → Identifiers → **+** → App ID →
   explicit bundle ID **`space.holon.gpui`** (no special capabilities needed).
3. Create the provisioning profile: Profiles → **+** → **App Store Connect**
   (distribution) → select the `space.holon.gpui` App ID and your
   Distribution certificate → name it (e.g. `Holon AppStore`) → download.
   - `IOS_PROVISIONING_PROFILE_BASE64` — `base64 -i Holon_AppStore.mobileprovision | pbcopy`

   The profile's Name/UUID is read out of the file itself, so no extra secret
   is needed for it.
4. Register the app in App Store Connect: **My Apps** → **+** → New App →
   platform iOS, bundle ID `space.holon.gpui`, pick a globally-unique app
   name and an SKU (e.g. `holon-ios`).
5. Finally set repo variable `APPLE_RELEASE_ENABLED` = `true`.

### 5. Google Play: service account (Android upload auth)

Prereq: a Play Console developer account and the app created in Play Console
(**Create app**, package name `space.holon.gpui`).

1. Play Console → **Setup** → **API access** → link a Google Cloud project.
2. In that Cloud project: IAM & Admin → Service Accounts → **Create service
   account** (e.g. `play-publisher`) → Keys → **Add key** → JSON → download.
3. Back in Play Console → API access → grant the service account access to
   the app with the **Release manager** role (or a custom role with
   release-to-testing-tracks permission).
4. Store as repo secret `GOOGLE_PLAY_SERVICE_ACCOUNT_JSON` — the **raw JSON
   file contents** (paste as-is, not base64).

### 6. Android release keystore

Generate once and back it up somewhere safe. Losing it means you can never
update the app again under the same signature — unless you enroll in Play App
Signing (recommended: enroll during app creation, then this keystore is
"only" your upload key and is resettable).

```
keytool -genkeypair -v \
  -keystore holon-release.jks \
  -alias holon \
  -keyalg RSA -keysize 4096 -validity 10000
```

Store as repo secrets `ANDROID_RELEASE_KEYSTORE_BASE64`
(`base64 -i holon-release.jks`), `ANDROID_RELEASE_KEYSTORE_PASSWORD`,
`ANDROID_RELEASE_KEY_ALIAS` (`holon`, or whatever you chose), and
`ANDROID_RELEASE_KEY_PASSWORD` (same as the store password if you pressed
Enter at the prompt). Then set `ANDROID_RELEASE_ENABLED` = `true`.

There is **no debug-keystore fallback** by design:
`frontends/gpui/android/build-release-aab.sh` and `build-release-apk.sh` both
require `KEYSTORE_FILE` / `KEYSTORE_PASSWORD` / `KEY_ALIAS` and abort if any
is unset, so a misconfigured secret produces a failed release, never a
debug-signed one that Play would reject later.

## Where each secret comes from

The workflows say which secret they consume; this table says where you obtain
it. Names not listed here do not exist.

| Secret | Where it comes from |
|---|---|
| `APPLE_TEAM_ID` | developer.apple.com → Membership |
| `APPLE_API_KEY_ID` | App Store Connect → Integrations → API key list |
| `APPLE_API_ISSUER_ID` | Same page, Issuer ID |
| `APPLE_API_KEY_P8_BASE64` | base64 of the downloaded `.p8` |
| `MACOS_DEVELOPER_ID_CERT_P12_BASE64` | base64 of exported Developer ID Application `.p12` |
| `MACOS_DEVELOPER_ID_CERT_PASSWORD` | your `.p12` export password |
| `IOS_DISTRIBUTION_CERT_P12_BASE64` | base64 of exported Apple Distribution `.p12` |
| `IOS_DISTRIBUTION_CERT_PASSWORD` | your `.p12` export password |
| `IOS_PROVISIONING_PROFILE_BASE64` | base64 of the App Store provisioning profile |
| `GOOGLE_PLAY_SERVICE_ACCOUNT_JSON` | raw JSON key of the Play service account |
| `ANDROID_RELEASE_KEYSTORE_BASE64` | base64 of `holon-release.jks` (keytool, above) |
| `ANDROID_RELEASE_KEYSTORE_PASSWORD` / `_KEY_ALIAS` / `_KEY_PASSWORD` | chosen at keytool time |

## Cutting a release — the human steps

1. **Bump the macOS bundle version by hand.**
   `frontends/gpui/macos/Info.plist` (`CFBundleVersion`,
   `CFBundleShortVersionString`) is copied verbatim into the `.app` by
   `scripts/bundle-macos.sh`; nothing injects it. Everything else derives from
   the one tag: desktop artifact names, the iOS build number, and the Android
   `versionName`/`versionCode` (`code = major*10000 + minor*100 + patch`).
   Because that derivation is deterministic, **re-submitting to a store
   requires bumping the patch version and tagging again** — stores reject
   reused build numbers. There is no automated version bumping or changelog
   generation.

   Build numbers must stay monotone per store, and the highest each has
   accepted is what constrains the next tag: Play is at versionCode **16**
   (`mobile-v0.0.16`), TestFlight at build **14** (`mobile-v0.0.14`), and tag
   `v0.0.17` already exists. So the next tag is **≥ `v0.0.18`** → versionCode
   and build number 18, clear of both.

2. **Tag and push.** Tags are a git-level concept and jj has no tag command;
   this repo is jj/git-colocated, so plain git is correct here — the one place
   git commands are right in this repo. From a clean, pushed `main`:

   ```
   git tag v1.2.0 && git push origin v1.2.0
   ```

3. **Finish in the stores by hand.** Nothing is auto-promoted. iOS lands in
   TestFlight and needs processing plus (for external testers) review; Android
   lands as a **draft on the internal testing track** and must be promoted in
   Play Console.

## Dry run — exercising the pipeline without spending a version

A tag is a one-shot, irreversible test: it creates a public Release and burns
an Apple build number and a Play versionCode even when the run fails. To test
a pipeline change instead, run the workflow manually.

Actions → **Release** → *Run workflow*, give it a `version` (e.g. `0.0.18`,
no leading `v`). Every platform builds, signs, and packages exactly as it
would for a tag; what is suppressed is only the publication:

| | tag push | manual run |
|---|---|---|
| GitHub Release created | yes | no |
| Artifacts | Release assets | run artifacts (`dryrun-*`) |
| TestFlight submission | yes | no |
| Play internal upload | yes | no |
| Build, sign, package, attest | yes | yes |

**A manual run can never publish** — there is no option to make it do so.
Publishing is inseparable from the tag: the Release, the artifact names, and
both store build numbers all derive from it, and a manual run has no tag to
derive them from. To publish, push a tag.

So a dry run proves the build and packaging work; it cannot prove the upload
credentials work, since it never uses them.

One value decides this: `create-release` resolves `publish` once, from the
event alone, and every gated step reads it — so a step can only be in one
mode, and a trigger added later is a dry run rather than an accidental store
submission.

## Failure-mode playbook

- **A gated job "did nothing".** Look for the `*-skipped` stub job in the run;
  its warning names the missing variable/secret. This is expected before
  setup, not a pipeline bug.
- **First Play upload rejected by the API.** A brand-new Play app may require
  the very first AAB to be uploaded by hand in Play Console before the API
  accepts uploads. Download `holon-release.aab` off the GitHub Release the run
  created and upload it manually, once.
- **Play rejects the build as debuggable.** Should be impossible — both
  packaging scripts strip `android:debuggable` and fail loud if the strip did
  not change the manifest. If it happens, the checked-in dev manifest changed
  shape; fix the script, don't work around it in Play.
- **Store rejects a reused build number.** Bump patch, tag again (see above).
  Never retag an existing tag.
- **macOS job fails at `security import`** (`SecKeychainItemImport: One or
  more parameters passed to a function were not valid`). The
  `MACOS_DEVELOPER_ID_CERT_P12_BASE64` / `_PASSWORD` secrets are empty while
  `APPLE_RELEASE_ENABLED` is `true`. Add the secrets, or set the variable to
  `false` to ship the unsigned zip deliberately. In a step log an unset secret
  prints as blank where a set one prints `***`.
- **Windows job fails compiling `windows-core`** (`the trait bound
  IWbemObjectSink: windows_core::Interface is not satisfied`). A dependency
  version skew in the tree, not a pipeline defect — fix it in `Cargo.toml`.
- **iOS job fails with `linker command failed` / fastlane `Exit status: 65`.**
  A real build failure. The `ios-build-logs` artifact (uploaded on failure,
  7-day retention) carries the raw `gym` logs. Read them for the signature
  below before assuming anything else — the failure is inside the cargo
  script phase, not the app link, so `xcbeautify` buries it.
- **…and the logs show `___chkstk_darwin` undefined plus `was built for newer
  'iOS' version (26.5) than being linked (10.0)`.** A deployment-target split:
  the cargo invocation ran without `IPHONEOS_DEPLOYMENT_TARGET`, so rustc
  linked at its own 10.0 default (visible as `-target arm64-apple-ios10.0.0`
  on the link line) while cc-rs compiled the C deps at the SDK default.
  `___chkstk_darwin` only exists in libSystem from iOS 12 up, so the 10.0 link
  cannot resolve it. It surfaces on `turso_sdk_kit` because that crate builds a
  `cdylib` and therefore links during `cargo build`. The fix lives in the
  `Build Rust Static Library` script phase of `frontends/gpui/ios/project.yml`,
  which passes Xcode's `IPHONEOS_DEPLOYMENT_TARGET` through to cargo under
  `set -u` — so a missing value fails loud instead of silently reverting to
  10.0. `frontends/gpui/justfile`'s `ios-build` pins the same value.
- **A run produced no Release and no store upload.** Check the event that
  started it. Every manual run is a dry run by design; `create-release` logs a
  `DRY RUN` warning naming the version, and the artifacts are attached to the
  run as `dryrun-*` instead of to a Release. Publishing requires a tag push.
- **A run fails immediately in `create-release`.** The version must be bare
  `MAJOR.MINOR.PATCH` with no leading zero on any field — `v0.0.18`, `0.0`,
  `1.2.3-rc1` and `0.0.010` are all rejected there, before any job can turn
  them into a store build number. (A leading zero is read as octal: `010`
  would become build number 80000, and `08` is not a number at all.)
- **A packaging script aborts on the `.so` size cap.** The packaged
  `libholon_gpui.so` came out over 150 MB, which means stripping did not
  happen — `llvm-strip` was missing, or the NDK layout moved. Fix
  `frontends/gpui/android/lib-release-so.sh`; do not raise the cap to get past
  it. An unstripped `.so` is ~760 MB and the stores reject it.
- **A macOS/Windows/Linux artifact fails to launch on a specific machine.**
  The AppImage and Windows artifacts are freshly minted; a runtime failure on
  one machine is a finding to triage, not automatically a pipeline defect.
  Check the glibc floor and Vulkan driver requirement first (below).

## Design decisions & known limitations

- **macOS universal2**: one lipo'd arm64+x86_64 binary in a single
  `Holon.app` — one download, no arch confusion, at the cost of a second
  cargo build (~2× mac build time). This is why macOS builds in its own
  per-arch matrix plus a finalize job rather than the main build matrix.
- **Assets placement**: the binary resolves `assets/` next to the executable.
  Linux/Windows archives place `assets/` beside the binary; the macOS bundle
  puts them in `Contents/Resources/assets` with a symlink from
  `Contents/MacOS/assets` (codesign forbids non-code files in `MacOS/`).
  Packaging copies assets via `scripts/stage-assets.sh` (real files copied,
  symlinks dereferenced, dangling links dropped with a loud warning).
  `assets/queries/*.prql` are vestigial links into `crates/*/queries` that
  resolve only through the `frontends/gpui/assets` → `../../assets`
  indirection and never in a flat package, so they are dropped. A plain
  `cp -R` would ship them as dangling links (broken at runtime) and fails the
  Windows build outright (`cp: cannot create symbolic link …`).
- **Linux glibc floor**: built on Ubuntu 22.04 so the floor is glibc 2.35
  (covers 22.04 / Debian 12 / etc.) rather than 24.04's 2.39. glibc cannot be
  bundled, so that floor is the hard minimum; a Vulkan-capable GPU driver is
  also required at runtime. Two artifacts ship: a portable AppImage that
  bundles the GUI shared libs, and a plain `.tar.gz` fallback that needs the
  system GUI libs present. `.deb`/Flatpak, or an older floor via a manylinux
  container, are follow-ups.
- **Windows CRT**: built with `-C target-feature=+crt-static`, so the `.exe`
  has no VC++ redistributable (`vcruntime140.dll`) dependency — it runs on a
  clean Windows install. x86_64 only; no ARM64 Windows build.
- **AAB for Play, APK for sideload**: Google Play rejects APK uploads for this
  app ("APKs are not allowed for this application", observed on
  `mobile-v0.0.13`) — a new Play app requires an AAB, with no opt-out. There
  is no Gradle project, so the AAB is produced by a direct `aapt2
  --proto-format` + bundletool pipeline in
  `frontends/gpui/android/build-release-aab.sh`, which ends in a bundletool
  `validate`. The direct-APK pipeline (`build-release-apk.sh`) still runs to
  produce a sideloadable `holon-release.apk` for the GitHub Release; only the
  AAB goes to Play.
- **Stripping the Android `.so`**: the release profile keeps debuginfo and
  cargo-ndk strips only when copying via `-o`, which these scripts do not use,
  so an unstripped `libholon_gpui.so` is ~760 MB against ~92 MB stripped. Both
  packaging scripts stage the library through `stage_release_so`
  (`frontends/gpui/android/lib-release-so.sh`), which strips a **copy** in the
  packaging tree — the cargo artifact under `target/` stays debuggable — and
  then refuses to package anything over a 150 MB cap. The cap sits ~1.6× above
  the real stripped size and ~5× below an unstripped one, so it catches a
  broken strip without tripping on genuine binary growth. It lives in the
  scripts rather than in workflow YAML so a local packaging run is bound by it
  too.
- **No symbols in mobile builds**: the iOS and Android release jobs set
  `CARGO_PROFILE_RELEASE_DEBUG=false`, dropping the workspace profile's
  `line-tables-only` debuginfo. Neither shipped artifact carries it anyway —
  the packaging script strips the `.so` and the IPA has none — so it was
  compile time spent on bytes that were discarded. A Play/TestFlight crash
  therefore symbolicates no further than addresses; wanting real stack traces
  means reversing this and uploading the symbols to the store instead.
- **Play track**: uploads go to `internal` as `draft`, never auto-promoted.
- **Windows signing**: skipped in v1 by decision. Follow-up if SmartScreen
  friction matters: an OV/EV cert or Azure Trusted Signing.
- **Windows build**: `gpui_windows` (holon-space/zed fork) compiles in CI
  (proven by the `v0.0.1` run, ~48 min).
