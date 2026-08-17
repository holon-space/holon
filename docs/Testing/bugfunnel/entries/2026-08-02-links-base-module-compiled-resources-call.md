---
id: 2026-08-02-links-base-module-compiled-resources-call
date: 2026-08-02
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  `build-release-aab.sh` links its base module with NO compiled resources: its
  `aapt2 link --proto-format` call is passed no `res.zip`, while the shared
  `AndroidManifest.xml` references `@mipmap/ic_launcher` and
  `@mipmap/ic_launcher_round`. Presumed consequence (unverified locally — no
  bundletool run): the AAB build fails at `aapt2 link` on the unresolved
  resource references, or, if aapt2 tolerates it, ships an icon-less bundle.
  Both APK paths (`build-apk.sh`, `build-release-apk.sh`) compile
  `assets/icons/app/android/res` into `res.zip` and link it; the AAB path
  never received that step. Pre-existing on main — inherited by, NOT
  introduced by, the 2026-08-02 dex/IME landing that touches the same script.
source_line: 1136
---

## Bug

(agent exploration — the IME-landing verifier, lane report §8)
`build-release-aab.sh` links its base module with NO compiled resources: its
`aapt2 link --proto-format` call is passed no `res.zip`, while the shared
`AndroidManifest.xml` references `@mipmap/ic_launcher` and
`@mipmap/ic_launcher_round`. Presumed consequence (unverified locally — no
bundletool run): the AAB build fails at `aapt2 link` on the unresolved
resource references, or, if aapt2 tolerates it, ships an icon-less bundle.
Both APK paths (`build-apk.sh`, `build-release-apk.sh`) compile
`assets/icons/app/android/res` into `res.zip` and link it; the AAB path
never received that step. Pre-existing on main — inherited by, NOT
introduced by, the 2026-08-02 dex/IME landing that touches the same script.

## Missing piece

No automated layer executes any Android packaging path:
`build-release-aab.sh` runs only in `release-mobile.yml:309`, and the
release workflows never run on PRs, so a packaging defect sits invisibly on
main until a release is cut. Missing piece = a packaging smoke gate (run the
three scripts against a stub `.so` up to their own fail-loud asserts) or a
scheduled release-workflow dry run.

## Remedy

OPEN 2026-08-02 — diagnosis only; needs Martin ruling. Fix direction: mirror
the APK paths — compile `assets/icons/app/android/res` with `aapt2 compile`
and hand the `res.zip` to the AAB `aapt2 link` call; the fix lane touches
the same script as the just-landed dex machinery.
