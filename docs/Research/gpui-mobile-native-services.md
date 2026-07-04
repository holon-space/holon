# Native services (location, notifications) from a GPUI app on iOS and Android

Status: research memo, 2026-07-05. Question: can Holon's GPUI mobile builds
use platform location services and local/push notifications, and what is the
right integration architecture?

Short answer: **yes on both platforms for location and local notifications,
with well-trodden Rust FFI routes (objc2 on iOS, jni on Android) that this
codebase already uses for the soft keyboard. Push is straightforward on iOS
(APNs) and the one structurally hard case on Android (FCM requires Java
bytecode; our APK is currently `hasCode="false"`).**

---

## 1. What our GPUI platform layer exposes today

Holon does not use upstream Zed GPUI directly on mobile:

- `gpui` is pinned to zed rev `56104fb17e6c` via the `holon-space/zed` fork
  (branch `holon`, workspace `[patch]` in the root `Cargo.toml`).
- Mobile platform support comes from **`gpui-mobile`**
  ([itsbalamurali/gpui-mobile](https://github.com/itsbalamurali/gpui-mobile),
  patched to the `holon-space/gpui-mobile` fork, branch `holon`, lock rev
  `f701952`). It implements the full `gpui::Platform` trait for iOS
  (wgpu/Metal) and Android (wgpu/Vulkan) — the same seam `gpui_linux` fills
  on Linux.

What the fork already provides that matters for native services:

| Capability | iOS | Android |
|---|---|---|
| App entry / lifecycle bridge | ObjC `main.m` + `UIApplicationDelegate` calling Rust FFI (`gpui_ios_register_app`, `ios::ffi::run_app`); hooks documented for `didFinishLaunchingWithOptions:` and `applicationDidBecomeActive:` | `android_main(AndroidApp)` via `android-activity` 0.6 (`native-activity` feature) |
| FFI substrate | `objc2` 0.6, `block2` 0.6, `core-foundation` 0.10 | `jni` 0.22, `ndk` 0.9, `ndk-context`, `android-activity` 0.6 |
| JNI attach points | n/a | `android::jni::vm_as_ptr()` / `activity_as_ptr()` (global refs from the stored `AndroidApp`) |
| Precedent for calling platform services | hidden `UITextView` first-responder keyboard (`ios/text_input.rs`), safe-area insets, `UIKeyboardWillShow/Hide` observers | `show_keyboard_android()` drives `InputConnection`/IME entirely over JNI (`android/jni.rs:1215`) |

**Key architectural fact: GPUI does not own the OS event loop on either
platform.** On iOS, `UIApplicationMain` (UIKit) owns the main run loop and
gpui-mobile bridges frame callbacks into it; the app delegate is ours
(`frontends/gpui/ios/main.m`), so every UIKit lifecycle/notification callback
is reachable. On Android, `android-activity` owns the `ALooper` integration
and forwards lifecycle events to `android_main`'s poll loop. Holon
additionally keeps a dedicated tokio runtime alive on a background thread
(`frontends/gpui/src/mobile.rs`), which is the natural place to land async
service events.

There is **no location or notification API in GPUI or gpui-mobile today** —
this is expected; upstream GPUI has none either (it is a UI framework).
Services must come in beside the platform layer, not through it.

## 2. iOS routes

### Location (CoreLocation)

- Crate: [`objc2-core-location`](https://docs.rs/objc2-core-location/latest/objc2_core_location/)
  (maintained as part of [madsmtm/objc2](https://github.com/madsmtm/objc2),
  which generates bindings for all Apple frameworks; actively released
  through 2025–2026).
- Pattern: create a `CLLocationManager`, implement `CLLocationManagerDelegate`
  in Rust with [`objc2::define_class!`](https://docs.rs/objc2/latest/objc2/macro.define_class.html)
  (the documented way to implement ObjC delegate protocols from Rust), call
  `requestWhenInUseAuthorization` / `startUpdatingLocation`.
- Permissions: `NSLocationWhenInUseUsageDescription` (and
  `NSLocationAlwaysAndWhenInUseUsageDescription` if ever needed) go into
  `frontends/gpui/ios/Info.plist` (generated project via `ios/project.yml`,
  so the key belongs in the xcodegen spec). Authorization prompts are
  triggered by the runtime call; results arrive on the delegate.
- Threading: delegate callbacks arrive on the main run loop. Forward them
  into the app via a channel into the mobile tokio runtime; never block the
  main thread.
- Background location would additionally need `UIBackgroundModes: location`
  in Info.plist — out of scope for a first cut.

### Notifications (UserNotifications, local + push)

- Crate: [`objc2-user-notifications`](https://docs.rs/objc2-user-notifications/latest/objc2_user_notifications/)
  (same generator; on crates.io, 0.3.x current).
- Local: `UNUserNotificationCenter::currentNotificationCenter()`,
  `requestAuthorizationWithOptions:completionHandler:` (completion handler is
  a `block2` block — same mechanics gpui-mobile already links), then
  `UNMutableNotificationContent` + `UNTimeIntervalNotificationTrigger` /
  `UNCalendarNotificationTrigger` and `addNotificationRequest:`.
- Push (APNs): `registerForRemoteNotifications` on `UIApplication`, receive
  the device token in the app delegate
  (`application:didRegisterForRemoteNotificationsWithDeviceToken:`). Because
  `main.m` is our file, the cleanest route is to add these delegate methods
  there and forward to `extern "C"` Rust functions — exactly the existing
  `gpui_ios_register_app` pattern. Requires the `aps-environment` entitlement
  and a paid Apple developer profile; simulator supports APNs sandbox pushes
  since Xcode 14 (`simctl push`).
- Foreground presentation + tap-handling go through
  `UNUserNotificationCenterDelegate`, again implementable with
  `define_class!` or in `main.m`.

Verdict: iOS is fully unblocked. Everything is a plain-Rust (or 20 lines of
ObjC in `main.m`) addition; no fork changes to gpui itself are needed.

## 3. Android routes

Current manifest facts (`frontends/gpui/android/AndroidManifest.xml`):
pure `android.app.NativeActivity`, **`android:hasCode="false"`** (the APK
ships no DEX at all — the justfile packages only `libholon_gpui.so` +
`libc++_shared.so`), `minSdk 33`, `targetSdk 36`.

### Location (LocationManager over JNI)

- Route: JNI against `android.location.LocationManager` obtained from the
  activity context (`getSystemService`). `gpui-mobile`'s
  `vm_as_ptr()`/`activity_as_ptr()` give us the `JavaVM` and `Activity`
  global refs; attach a `JNIEnv` on our own thread (the fork's keyboard code
  is a complete worked example of this pattern).
- `requestLocationUpdates` needs a `LocationListener` **callback object**,
  which normally means a Java class. With `hasCode="false"` we cannot ship
  one. Two viable options:
  1. **Polling**: `getLastKnownLocation` / `getCurrentLocation` on a timer —
     no callback class needed; fine for journal-geotagging-grade needs.
  2. **`Proxy`-based listener**: build a `java.lang.reflect.Proxy`
     implementing `LocationListener` whose `InvocationHandler` is… also a
     Java class. Not possible bytecode-free. So real streaming updates
     ultimately want a small DEX (see push, below).
- Permissions: declare `ACCESS_COARSE_LOCATION` / `ACCESS_FINE_LOCATION` in
  the manifest, then request at runtime. From native code:
  - Checking works natively:
    [`APermissionManager_checkPermission`](https://developer.android.com/ndk/reference/group/permission)
    (API 31+) or JNI `Context.checkSelfPermission`.
  - **Requesting** must go through the activity:
    JNI call to `Activity.requestPermissions(String[], int)` (API 23+, no
    appcompat needed). The grant *result* callback
    (`onRequestPermissionsResult`) is not surfaced by NativeActivity /
    `android-activity` (long-standing gap, cf.
    [ndk-samples #114](https://github.com/googlesamples/android-ndk/issues/114));
    the standard native workaround is to re-check `checkSelfPermission`
    when the activity regains focus (we get `GainedFocus`/`Resume` events
    from `android-activity`). Crates like `android-permissions` wrap this
    same dance; the JNI is small enough to own directly.

### Notifications

- **Local notifications: fully feasible bytecode-free.** JNI to
  `NotificationManager` + `NotificationChannel` (required since API 26) +
  `Notification.Builder` — all framework classes, constructed and posted from
  native code. Runtime permission `POST_NOTIFICATIONS` (API 33+ — and our
  minSdk is 33, so it is mandatory) via the same `requestPermissions` dance
  ([Android notification runtime permission docs](https://developer.android.com/develop/ui/views/notifications/notification-permission)).
  One caveat: notification *tap* intents can only target our own activity
  (`PendingIntent.getActivity` with the NativeActivity component) — deep-link
  payloads arrive through the relaunch intent, readable over JNI
  (`Activity.getIntent()`).
- **Push (FCM): the hard case.** Firebase Cloud Messaging requires a
  `FirebaseMessagingService` subclass — Java bytecode, Google Play services
  client libraries, and a Gradle-style resource pipeline. That means:
  `hasCode="true"`, a small DEX (either a minimal Gradle module or `d8` on a
  handful of `.java` files in the `apk` recipe), plus `google-services.json`
  wiring. It is doable while keeping the Rust-first build (the service
  forwards messages to native via a tiny JNI shim), but it is a build-system
  project, not a Rust project.
  - Pragmatic alternatives, in Holon's spirit (local-first, self-hosted):
    **UnifiedPush** (still needs a receiver class → same DEX problem),
    or **piggyback on Holon's own sync channel** — the app already maintains
    an iroh/Loro sync connection; a foreground-service + local-notification
    combination ("sync poked me, post a local notification") avoids Google
    infrastructure entirely and reuses code we own. Recommended first step.

## 4. Recommended integration architecture for Holon

Keep native services **out of the gpui-mobile fork** (it is a UI platform
shim; every fork line is rebase debt) and **out of holon-gpui's render code**.
Introduce a platform-services seam following the existing DI conventions
(fluxdi modules, insert-only CapMap, fail-loud stubs):

```
crates/holon-native-services/          (new, no gpui dependency)
  src/lib.rs        — traits: LocationService, NotificationService
  src/ios.rs        — objc2-core-location / objc2-user-notifications impls
  src/android.rs    — JNI impls (attach via gpui_mobile::android::jni ptrs*)
  src/unsupported.rs— fail-loud impl: every call returns
                      Err("no native <service> backend on this platform")
```

\* to avoid a gpui-mobile dependency in the services crate, `holon-gpui`'s
`mobile.rs` entry points hand the raw `JavaVM`/`Activity` pointers (Android)
into the module at startup — the same way `db_path`/`orgmode_root` flow today.

- **Registration**: a fluxdi module registers `Arc<dyn LocationService>` /
  `Arc<dyn NotificationService>` per platform at session bootstrap
  (`holon_app::new_from_config` path), exactly like `LoroModule` opt-in in
  `mobile.rs`. Desktop gets the `unsupported` impl — visible failure, not a
  silent no-op, per the fail-loud rule.
- **Consumption**: services surface as *operations/entities*, not widget
  code: e.g. a `notify` operation (op registry → reachable from the mobile
  action bar and MCP for free) and a `location` provider that writes a
  properties update through the normal op-dispatch path. This keeps the ONE
  composed PBT able to drive them with a stub service.
- **Event flow**: platform callbacks (delegate methods, JNI callbacks,
  focus-regain permission re-checks) → `tokio::sync::mpsc` into the mobile
  runtime → dispatched as ops. No direct UI mutation from platform threads.
- **Permission state as data**: expose `PermissionState`
  (`Granted | Denied | NotDetermined | Unsupported`) from the trait, parse —
  don't validate — at the FFI boundary, and render it (grant prompts are
  user-visible state, not hidden retries).

### Sequencing proposal

1. Local notifications, both platforms (pure additive; Android needs the
   `POST_NOTIFICATIONS` manifest + request dance; iOS needs nothing but code).
2. Location one-shot (`getCurrentLocation` / `requestLocation`) both
   platforms + permission plumbing.
3. iOS APNs push (delegate methods in `main.m` + token → sync backend).
4. Android push: start with sync-connection + foreground-service local
   notifications; revisit FCM/UnifiedPush only if real background push is
   required (accepting the DEX/Gradle cost).

## 5. Sources

- [gpui-mobile README (itsbalamurali/gpui-mobile)](https://github.com/itsbalamurali/gpui-mobile) — platform trait coverage, iOS Metal / Android Vulkan.
- [GPUI Mobile on HN (2026)](https://hn.nuxt.dev/item/47244184) — current state of GPUI-on-mobile; upstream Zed still desktop-only ([gpui.rs](https://www.gpui.rs/)).
- [madsmtm/objc2](https://github.com/madsmtm/objc2), [objc2-core-location](https://docs.rs/objc2-core-location/latest/objc2_core_location/), [objc2-user-notifications](https://docs.rs/objc2-user-notifications/latest/objc2_user_notifications/), [define_class!](https://docs.rs/objc2/latest/objc2/macro.define_class.html) — iOS FFI route.
- [rust-mobile/android-activity](https://github.com/rust-mobile/android-activity), [android_activity docs](https://docs.rs/android-activity) — NativeActivity glue; docs note most complex apps eventually need a small Activity subclass (i.e. bytecode).
- [Android notification runtime permission](https://developer.android.com/develop/ui/views/notifications/notification-permission) — POST_NOTIFICATIONS, API 33+.
- [NDK permission API](https://developer.android.com/ndk/reference/group/permission) — `APermissionManager_checkPermission` (check-only, API 31+).
- [ndk-samples #114](https://github.com/googlesamples/android-ndk/issues/114) — the native-activity runtime-permission-request gap and workaround.
- [CLLocationManagerDelegate](https://developer.apple.com/documentation/corelocation/cllocationmanagerdelegate), [CLLocationManager](https://developer.apple.com/documentation/corelocation/cllocationmanager) — iOS location API shape.
- In-repo ground truth: `frontends/gpui/src/mobile.rs`, `frontends/gpui/ios/main.m`, `frontends/gpui/android/AndroidManifest.xml`, `frontends/gpui/justfile`, gpui-mobile fork checkout (`android/jni.rs` keyboard JNI, `ios/text_input.rs` first-responder pattern).
