# Release Pipeline

How Holon releases are built, signed, attested, and shipped — and every piece
of one-time manual setup needed outside this repo before the gated parts turn
on. Written for someone who has never done any of this before.

## Overview

Two workflows, decoupled by tag prefix:

| Tag | Workflow | Ships |
|---|---|---|
| `v1.2.0` (`v*.*.*`) | `.github/workflows/release-desktop.yml` | macOS `.app` (universal2 zip), Linux `.tar.gz`, Windows `.zip` → attached to a GitHub Release |
| `mobile-v1.2.0` (`mobile-v*.*.*`) | `.github/workflows/release-mobile.yml` | iOS → TestFlight, Android → Google Play internal testing track (+ both artifacts mirrored on a GitHub Release) |

**Source-integrity guarantee**: every artifact gets a GitHub **Artifact
Attestation** (SLSA build provenance) — a cryptographic, publicly verifiable
statement that the file was built by this repo's release workflow from the
exact tagged commit. This — not OS code-signing — is the "no injected code"
mechanism. OS signing (Apple notarization, Play signing) only proves the
*publisher's identity* to the OS; the attestation proves the *source* of the
bits. End users verify with:

```
gh attestation verify <downloaded-file> --repo <owner>/holon
```

### Gating: what runs today vs. after setup

Apple Developer Program enrollment hasn't happened yet, so all
Apple-dependent steps are gated behind repo **variables** (not secrets). The
pipeline never hard-fails on missing Apple credentials — it skips with a loud
warning and labels the output:

| Repo variable | When unset / not `"true"` | When `"true"` |
|---|---|---|
| `APPLE_RELEASE_ENABLED` | Desktop: macOS artifact is built but **unsigned**, named `…-unsigned.zip`, release notes say so. Mobile: iOS job is skipped entirely (a stub job logs why). | macOS is codesigned + notarized; iOS builds a signed IPA and uploads to TestFlight. |
| `ANDROID_RELEASE_ENABLED` | Android jobs are skipped entirely (stub jobs log why). | Mobile (`mobile-v*`): builds a **release-keystore-signed** APK and uploads to the Play internal track. Desktop (`v*`): additionally attaches the same-shaped sideloadable `holon-release.apk` to every desktop GitHub Release (no store involvement). |

Windows is intentionally unsigned in v1 (decision: no paid or free signing
cert yet). Users see one SmartScreen prompt; the release notes say so.
Provenance attestation still applies.

## Current blocker: non-reproducible `gpui-component` patch

The workspace `Cargo.toml` patches `gpui-component` to a **local path**
(`/Users/martin/Workspaces/rust/gpui-component/crates/ui` — see the
"NON-REPRODUCIBLE BUILD" comment block in the root `Cargo.toml`). **No GitHub
runner can build `holon-gpui` until that patch is converted to a pushed git
pin** (steps are spelled out in that comment). Fixing this is a prerequisite
for the first real release; the workflows are otherwise complete.

## One-time manual setup, in order

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

### 3. macOS: Developer ID Application certificate (for notarized desktop builds)

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
   (The workflow reads the profile's Name/UUID out of the file itself; the
   name is passed to xcodebuild automatically.)
4. Register the app in App Store Connect: **My Apps** → **+** → New App →
   platform iOS, bundle ID `space.holon.gpui`, pick a globally-unique app
   name and an SKU (e.g. `holon-ios`).
5. Finally set repo **variable** `APPLE_RELEASE_ENABLED` = `true`
   (Settings → Secrets and variables → Actions → **Variables** tab).

### 5. Google Play: service account (Android upload auth)

Prereq: a Play Console developer account (exists already) and the app created
in Play Console (**Create app**, package name `space.holon.gpui`). The very
first APK upload to the internal track may need to be done by hand in Play
Console before API uploads are accepted — Play quirk, do it once with the
CI-built APK off the GitHub Release if the first `supply` run complains.

1. Play Console → **Setup** → **API access** → link a Google Cloud project.
2. In that Cloud project: IAM & Admin → Service Accounts → **Create service
   account** (e.g. `play-publisher`) → Keys → **Add key** → JSON → download.
3. Back in Play Console → API access → grant the service account access to
   the app with the **Release manager** role (or a custom role with
   release-to-testing-tracks permission).
4. Store as repo secret:
   - `GOOGLE_PLAY_SERVICE_ACCOUNT_JSON` — the **raw JSON file contents**
     (paste as-is, not base64).

### 6. Android release keystore

Generate once, back it up somewhere safe (losing it means you can never
update the app again under the same signature, unless you enroll in Play App
Signing — recommended: enroll during app creation, then this keystore is
"only" your upload key and is resettable):

```
keytool -genkeypair -v \
  -keystore holon-release.jks \
  -alias holon \
  -keyalg RSA -keysize 4096 -validity 10000
```

Store as repo secrets:

- `ANDROID_RELEASE_KEYSTORE_BASE64` — `base64 -i holon-release.jks | pbcopy`
- `ANDROID_RELEASE_KEYSTORE_PASSWORD` — the keystore password
- `ANDROID_RELEASE_KEY_ALIAS` — `holon` (or whatever you chose)
- `ANDROID_RELEASE_KEY_PASSWORD` — the key password (same as store password
  if you pressed Enter at the prompt)

Then set repo **variable** `ANDROID_RELEASE_ENABLED` = `true`.

The release job **never** falls back to the debug keystore — the packaging
script (`frontends/gpui/android/build-release-apk.sh`) requires every
keystore variable and fails loud otherwise. It also strips the dev manifest's
`android:debuggable="true"` (Play rejects debuggable APKs) and injects
`versionCode`/`versionName` at link time.

## Complete secrets & variables reference

Repo **variables** (Settings → Secrets and variables → Actions → Variables):

| Variable | Purpose |
|---|---|
| `APPLE_RELEASE_ENABLED` | `true` ⇒ macOS signing+notarization runs and the iOS TestFlight job runs. Anything else ⇒ unsigned macOS build, iOS skipped loudly. |
| `ANDROID_RELEASE_ENABLED` | `true` ⇒ Android build+Play upload runs. Anything else ⇒ skipped loudly. |

Repo **secrets**:

| Secret | Used by | Where it comes from |
|---|---|---|
| `APPLE_TEAM_ID` | iOS build | developer.apple.com → Membership |
| `APPLE_API_KEY_ID` | macOS notarization, TestFlight upload | App Store Connect → Integrations → API key list |
| `APPLE_API_ISSUER_ID` | macOS notarization, TestFlight upload | Same page, Issuer ID |
| `APPLE_API_KEY_P8_BASE64` | macOS notarization, TestFlight upload | base64 of the downloaded `.p8` |
| `MACOS_DEVELOPER_ID_CERT_P12_BASE64` | macOS codesign | base64 of exported Developer ID Application `.p12` |
| `MACOS_DEVELOPER_ID_CERT_PASSWORD` | macOS codesign | your `.p12` export password |
| `IOS_DISTRIBUTION_CERT_P12_BASE64` | iOS codesign | base64 of exported Apple Distribution `.p12` |
| `IOS_DISTRIBUTION_CERT_PASSWORD` | iOS codesign | your `.p12` export password |
| `IOS_PROVISIONING_PROFILE_BASE64` | iOS codesign | base64 of the App Store provisioning profile |
| `GOOGLE_PLAY_SERVICE_ACCOUNT_JSON` | Play upload | raw JSON key of the Play service account |
| `ANDROID_RELEASE_KEYSTORE_BASE64` | APK signing | base64 of `holon-release.jks` (keytool, above) |
| `ANDROID_RELEASE_KEYSTORE_PASSWORD` | APK signing | chosen at keytool time |
| `ANDROID_RELEASE_KEY_ALIAS` | APK signing | chosen at keytool time |
| `ANDROID_RELEASE_KEY_PASSWORD` | APK signing | chosen at keytool time |

## How to cut a release

1. **Bump versions manually** (no automation in v1):
   - `frontends/gpui/Cargo.toml` — `version`
   - `frontends/gpui/macos/Info.plist` — `CFBundleVersion` + `CFBundleShortVersionString`
   - iOS `frontends/gpui/ios/Info.plist` and the Android manifest do **not**
     need manual bumps: the mobile workflow injects the version from the tag
     (Info.plist via fastlane, APK via `aapt2 --version-code/--version-name`).
     Mobile version/build numbers are derived deterministically from the tag
     (`code = major*10000 + minor*100 + patch`), so **re-submitting to a
     store requires bumping patch and tagging again** — stores reject reused
     build numbers.
2. **Tag and push.** Tags are a git-level concept; jj has no tag command, and
   this repo is jj/git-colocated, so plain git is correct here (the one place
   git commands are right in this repo). From a clean, pushed `main`:

   ```
   git tag v1.2.0 && git push origin v1.2.0            # desktop release
   git tag mobile-v1.2.0 && git push origin mobile-v1.2.0  # mobile release
   ```

3. Watch the run under Actions. Desktop: artifacts + `SHA256SUMS-<os>.txt`
   appear on the GitHub Release for the tag. Mobile: TestFlight processes the
   build (test in the TestFlight app); the Android build lands as a **draft
   release on the internal testing track** — promote manually in Play Console.

## How users verify a download

```
gh attestation verify Holon-1.2.0-linux-x86_64.tar.gz --repo <owner>/holon
```

Prints the workflow, repository, and commit the artifact was built from,
verified against GitHub's Sigstore instance. Also compare
`sha256sum <file>` against the attached `SHA256SUMS-<os>.txt`.

## Design decisions & known limitations

- **macOS universal2**: one lipo'd arm64+x86_64 binary in a single
  `Holon.app` — one download, no arch confusion, at the cost of a second
  cargo build (~2× mac build time).
- **Assets placement**: the binary resolves `assets/` next to the executable.
  Linux/Windows archives place `assets/` beside the binary; the macOS bundle
  puts them in `Contents/Resources/assets` with a symlink from
  `Contents/MacOS/assets` (codesign forbids non-code files in `MacOS/`).
  Packaging copies assets with `cp -RL` (**dereference**): `assets/queries/*.prql`
  are symlinks into `crates/*/queries`, so a plain `cp -R` would ship dangling
  links (broken queries at runtime; on Windows it fails the build outright).
- **Linux packaging**: two artifacts per release — a portable **AppImage**
  (bundles the GUI shared libs so users need no `-dev`/runtime packages) and a
  plain **`.tar.gz`** (kept as a fallback; needs the system GUI libs present).
  Built on **Ubuntu 22.04** so the glibc floor is 2.35 (covers 22.04 / Debian
  12 / etc.) rather than 24.04's 2.39. glibc itself can't be bundled, so that
  floor is the hard minimum; a Vulkan-capable GPU driver is also required at
  runtime. Further options (`.deb`/Flatpak, or an even older glibc via a
  manylinux container) are follow-ups.
- **Windows CRT**: built with `-C target-feature=+crt-static`, so the `.exe`
  has no VC++ redistributable (`vcruntime140.dll`) dependency — it runs on a
  clean Windows install. x86_64 only; no ARM64 Windows build.
- **APK, not AAB**: the Android build is a direct aapt2/zipalign/apksigner
  pipeline (no Gradle). Google Play requires an **AAB** for a new app's
  *production* track; the internal testing track accepts APKs, which is all
  this pipeline targets. **Follow-up before any production rollout**: produce
  an AAB (bundletool or a minimal Gradle wrapper).
- **Play track**: uploads go to `internal` as `draft`, never auto-promoted.
- **Windows signing**: skipped in v1 by decision. Follow-up if SmartScreen
  friction matters: an OV/EV cert or Azure Trusted Signing.
- **Windows build**: `gpui_windows` (holon-space/zed fork) compiles in CI
  (proven by the `v0.0.1` run, ~48 min). The AppImage and (first-run) Windows
  artifact are still freshly minted — a runtime failure on a specific machine
  is a finding, not a pipeline bug.
- **No automated version bumping** and no changelog generation in v1.
- **Nightly is unpinned**: `rust-toolchain.toml` says `channel = "nightly"`
  (no date). Releases build with whatever nightly is current that day; pin a
  dated nightly there if a release ever breaks on a fresh toolchain.
