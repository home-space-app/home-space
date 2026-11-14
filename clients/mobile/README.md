# Mobile clients

Native UI on each platform, with the code that talks to the server in Rust:

- `../mobile-core/`: the Rust client for mobile. It wraps `api-client` and `client-core` (the
  code the desktop and web clients run) and exports them with
  [UniFFI](https://mozilla.github.io/uniffi-rs/). Rust `async fn` becomes a Kotlin `suspend fun`
  and a Swift `async throws` func, and Rust errors become typed exceptions (`CoreException` in
  Kotlin, `CoreError` in Swift). The bindings are generated on every build and are not committed.
  Every async call runs on a runtime in `mobile-core`, never on the app's UI thread.
- `androidApp/`: Android app in Kotlin with Jetpack Compose.
- `iosApp/`: iOS app in SwiftUI.

Both apps sign in with a password or single sign-on, whichever the server offers, stay signed in
between runs, browse folders, and select several rows. They also manage files: details, rename,
move, delete (to the recycle bin), save to a chosen location, create folders, upload files and search
by name. On Android, tapping a text file shows it in the built-in text viewer, where it can be
edited and saved as a new version. Tapping an image shows it in the image viewer, which swipes
through the other images of its folder (see `docs/viewers.md`). Tapping any other file, or choosing
**Open in another app**, downloads it and opens it in the system's viewer.

Routes, JSON, session refresh, error mapping, sorting and size labels all come from the Rust crates,
so nothing is mirrored by hand. Dates are the exception: Rust exports the time, and each app formats
it with the platform's formatter for the device's locale and time zone. `mobile-core/src/lib.rs` is a thin mapping to FFI-friendly
records (ids as strings, times as `SystemTime`). Add a call there, and both apps get it on their
next build.

## Requirements

- Rust targets and tools:
  ```sh
  rustup target add aarch64-apple-ios aarch64-apple-ios-sim aarch64-linux-android x86_64-linux-android
  cargo install cargo-ndk
  ```
- JDK 17 or newer
- Android: Android Studio (or the Android SDK with `ANDROID_HOME` set), API 37, and the NDK
  (`sdkmanager "ndk;30.0.16248370"`, or SDK Manager > SDK Tools > NDK)
- iOS: a Mac with Xcode 16 or newer, and [XcodeGen](https://github.com/yonaskolb/XcodeGen)
  (`brew install xcodegen`)

## Android

Open `clients/mobile` in Android Studio and run `androidApp`, or:

```sh
./gradlew :androidApp:installDebug
./gradlew :androidApp:testDebugUnitTest   # the bindings on the JVM, against the host library
```

`androidApp/build.gradle.kts` runs `mobile-core/scripts/build-android.sh`. The Kotlin bindings
come from a host build, and the `.so` files (arm64-v8a, x86_64) from `cargo ndk`. Debug builds
use Cargo's debug profile, and release builds use `mobile`. The bindings load the library through
JNA.

The app defaults to `http://10.0.2.2:8080`, which is the host machine as seen from the emulator.

The server must listen on the host's loopback (`127.0.0.1` or `0.0.0.0`), since `10.0.2.2` maps
there. A server bound only to a LAN address is unreachable from the emulator.

On Android 17 (API 37) and later, connections to private addresses (the LAN, `10.0.2.2`) need
the runtime permission `ACCESS_LOCAL_NETWORK` ("nearby devices"), which the app asks for at
launch. Without it, Android silently drops the connection, which looks like a timeout, while
`adb shell` can still reach the server.

## iOS

The Xcode project is generated from `iosApp/project.yml` and is not committed. XcodeGen needs the
generated bindings folder to exist, so build the Rust client once first:

```sh
../mobile-core/scripts/build-ios.sh debug
cd iosApp
xcodegen generate
open iosApp.xcodeproj
```

The `RustCore` aggregate target runs `mobile-core/scripts/build-ios.sh` before the app builds
(`debug` for the Debug configuration, `mobile` otherwise). It writes `HomeSpaceCore.xcframework`
(device and simulator static libraries) and the Swift bindings to `build/rust-core/ios`, so
building in Xcode is enough. Run `xcodegen generate` again after adding or removing Swift files.

The app defaults to `http://localhost:8080`. `iosAppUITests` checks that the app offers sign-in
once the server answers, so it needs the server running on `localhost:8080` with password sign-in
on, and nobody signed in on the simulator.

## Offline copies

A file that is tapped, or a file or folder made available offline, stays on the device and syncs both
ways. `clients/sync-core` is the desktop client's engine, and `mobile-core/src/offline.rs` runs it for
the signed-in user. Copies live in `filesDir/offline` on Android and Application Support `/offline`
on iOS (excluded from backups):

```
<offline>/<user id>/sync.db                      what the engine remembers
<offline>/<user id>/<node id>/<name>             a file kept for its own sake
<offline>/<user id>/<node id>/...                a folder kept for its own sake, with what is in it
<offline>/<user id>/.parts/<node id>             a download started by tapping
```

What the engine does is in `docs/specs/mobile-sync.feature`. On phones it differs from the desktop:

- **A missing local file comes back, and nothing is deleted on the server.** The copies belong to the
  app, so nobody deleted them on purpose. Deleting happens through the API.
- There is no system trash, so local files are deleted directly.
- Something kept that is deleted on the server loses its copy. Edits in it that were not sent go to
  Lost & Found.
- Nobody watches the folder for changes. Passes run at launch, on resume, every minute while open, on
  network changes, after Quick Look closes and in the background job.
- `Gate` holds back automatic transfers. It starts closed, and the app opens it only when the settings
  and network allow, so that closing it stops transfers at their next chunk. Tapping a file ignores
  it, since the user asked.
- Removing a copy and signing out warn first when edits have not been uploaded.
- Copies are never evicted. They stay until the user removes them, or signs out.

Conflicts follow the desktop client: the later change is current, the other one becomes a version.
There is no conflict state to show. An edit made before a rename on the server still counts as the
later change, since a rename leaves the content alone.

A tap asks the server for the current node. If the copy is current nothing downloads, and if the
server can't be reached any copy will do. A copy with edits opens as it is and is never overwritten
by a tap. Names the engine ignores (such as `*.tmp`) are never synced, so a tap leaves a plain copy.

Android hands files to viewers through a `FileProvider` scoped to `offline/`. A tap grants read-only
access, and **Edit in another app** sends `ACTION_EDIT` with a write grant. iOS uses `QLPreviewController`
in `.updateContents` mode, whose markup tools save into the file. Editing in other iOS apps needs the
File Provider: see `docs/todo/mobile-file-provider.md`. Save to… picks the location first on Android,
but on iOS only after the download, because the system's export picker needs a file that already exists.

The server has no Range support, so an interrupted download starts over. Uploads over the chunk size
continue from what the server has. Requests are never aborted, because a token refresh in flight must
finish or the rotated token is lost.

## Search

Search covers all of the user's files and folders, whichever folder is open. The query goes out once
typing pauses for 250 ms, as in the desktop client. Tapping a file opens it, tapping a folder goes
there, and **Show in folder** goes to the folder holding a file.

## Uploading picked files

The add button offers **New folder** and **Upload files**, plus **Upload Photos** on iOS, where the
file picker doesn't list the photo library. The photo picker needs no permission, and uploads the
original file.

Files go into the folder that was open when they were picked, one at a time, as a resumable upload
with up to 8 MiB per chunk. A row at the top of every folder sums them up, so a failure can't go
unseen, and opens **Transfers**.

The queue lives in `mobile-core` (`uploads/`), in a SQLite file next to its copies under the app's
private `sync/uploads-<user>/` folder. It runs on the Rust runtime, so no screen owns it, and it
carries on whatever ended the app's last run: the next start continues from what the server has.

- **Each file goes up from a copy of its own**, removed once it is on the server, because a picked
  document has no lasting path. Storage must hold that second copy meanwhile.
  - Android makes the copy when the file's turn comes, so only one exists at a time. It keeps access
    to the original until then, and a provider that doesn't offer lasting access works only until
    the app ends.
  - iOS copies when picking: the document picker does so anyway, and a picked photo can't be reached
    in the app's next run. So everything picked at once takes space at once. A photo the app was
    still copying when it ended asks to be picked again.
- **A name clash makes a new version**, as on desktop and web, because the user chose this file and
  this folder. Identical content changes nothing. Auto-upload keeps both instead, since no one is
  there to decide.
- **Trying again is automatic when the server can't be reached:** on opening the app, every minute
  while it is open and when the network changes. A file the server refuses waits for Retry or Cancel,
  and the others go on. Cancelling ends the upload on the server too. The server drops an abandoned
  upload on its own, after which the file starts over.
- **Mobile data is allowed.** "Sync only on Wi-Fi" and "Sync in background" are for automatic
  transfers; this was asked for.
- **Signing out, or a session that ended, drops the queue** with its copies. The originals stay.

### After leaving the app

- **Android** runs `UploadWorker`, a WorkManager job with a `dataSync` foreground service and a
  notification showing progress, scheduled while the app is in front. It comes back by itself when
  Android ends the app, with up to a minute or more of backoff, and when the network returns. A
  failure shows as a notification of its own. Force-stopping the app stops it until the app is opened.
  Android limits `dataSync` services to 6 hours a day from Android 15; what is left then waits.
  Without the notification permission, asked for on the first upload, it runs the same but doesn't show.
- **iOS 26** runs a `BGContinuedProcessingTask` when the user picks files or taps Retry, with progress
  shown by the system. iOS may end it, or the user may from there. Rows then wait for the app to open.
  A queue that resumed by itself on opening the app doesn't get one, since it must start from something
  the user did.
- **iOS 17 to 25**, or when iOS won't run the task, gets only the short time any app gets (about 30
  seconds). Uploads then wait for the app to open. A background `URLSession` isn't used: it can't
  chunk through Rust, and it would outlive the short-lived access token.

The task doesn't run in the simulator (`BGTaskSchedulerErrorDomain` 1), so it needs a real iPhone.

## Transfers

**Transfers** (a navigation bar tab on Android, in the header's menu on iOS) has each picked file
with its progress, Retry and Cancel; each
auto-upload source, with the file on its way and how far along it is; and how offline copies stand.
Downloads aren't listed, since they show on their rows and run only while the app is open.

### Send to device (Android)

On Android, **Transfers** also lists what was sent to the user's devices (**Received**) and what the
user sent (**Sent**); see [`docs/send-to-device.md`](../../docs/send-to-device.md). iOS does not
have this yet. `mobile-core/src/transfers.rs` holds the calls, with the same words as the desktop
and web clients.

- **Receiving**: a waiting transfer's menu offers **Save** (the server moves it into a folder of
  the user's tree), **Download** (to `Download/Home Space` on the phone, through MediaStore; each
  file is checked against its hash before the server is told to drop its copy) and **Decline**.
  Before Android 10 the download goes to the app's own folder on shared storage, since the public
  one needs a permission the app does not ask for.
- **Sending**: **Send to device…** in a row's menu and in the selection's More menu sends files and
  folders of the server, which copies them itself. **New > Send files to a device** sends files
  of the phone.
- **Share sheet**: `ShareActivity` takes `SEND` and `SEND_MULTIPLE` from other apps, such as a mail
  attachment, and offers **Save** (a folder picker; the files then go through the upload queue) and
  **Send** (the device picker). The files are copied into the app first, because access to a shared
  file ends with the screen it was shared to. Only `content:` URIs are accepted.
- A send from the phone runs in the process, kept alive by `SendWorker`'s notification. It does
  not resume if Android ends the process: the transfer then stays `uploading` until it is
  cancelled from **Sent** or expires.
- The server pushes `transfer_received` only while the app is open. A notification for a transfer
  sent to this phone is shown then, after each reconnect, and from background sync when that is on.
  There is no push service, so with the app closed and background sync off nothing is shown until
  the app is next opened.

## Auto-upload

Settings > Sync marks local folders that upload to a folder on the server. New and changed files go
up, and nothing on the server ever changes the local folder. The rules are in
`docs/specs/mobile-sync.feature`, and `mobile-core/src/autoupload/` owns them, along with what was
sent (SQLite in `<files>/sync/autoupload-<user id>.db`, excluded from backups).

- The apps list and read files through the `LocalSources` trait, because Rust can't reach them by
  path: a folder on Android is a document tree, and a photo on iOS is a PhotoKit asset.
- A file goes up by name through `/uploads`, always, even a small one. The first time it asks the
  server to keep both (`keep_both`) if a different file has the name. It saves the id and name the
  server gave it, and an edit goes to that name with the id (`version_of`). That makes a new version
  if that file still has the name, keeps both if a different file took it, and makes a new file if
  the name is free. A record from before the id was saved always keeps both.
- An edit is noticed by size and modification time. Nothing is hashed on the phone.
- Chunks are at most 8 MiB, and stopping takes effect between them. The upload session is saved, so
  the next pass continues from what the server has.
- A file that changed in the last 5 seconds waits, and one that changes before its last chunk is
  cancelled and sent again later.
- A deleted target folder pauses its source until the user picks another. It isn't made again.

**Sources.** Android: a folder through the system's picker (which refuses the storage root and the
`Download` root), or the camera, which is MediaStore under `DCIM/`. A photo in `DCIM/Camera` goes to
`<target>/Camera`. iOS: a folder through the document picker, or the photo library, which all goes
straight into the target. A Live Photo uploads as its still photo only.

**The one client.** Refresh tokens are single use, so `ClientHolder` on each platform owns the only
`CoreClient` in the process, shared by the screens and background work.

**Wi-Fi only.** The app decides, since a pass only starts if the network allows it and is stopped
when that changes: `NET_CAPABILITY_NOT_METERED` on Android, `!isExpensive` on iOS. So Wi-Fi from a
metered hotspot doesn't count, on either platform. Tapping a file to open it isn't held back.

**In the background (off by default).** Android: `SyncWorker` runs through WorkManager every 15
minutes and when MediaStore changes, so a new photo goes up within seconds, even with the app
killed. It shows a notification while there are files to send. iOS: `BackgroundSync` asks for
`BGAppRefreshTask` and `BGProcessingTask`, and iOS decides when they run, often while charging, so a
new photo may wait until then or until the app is opened. Background `URLSession` isn't used: it
would send requests long after the short-lived bearer token was set. iOS has no permission to ask
for, so the setting points to Background App Refresh when that is off.

To force the Android job: `adb shell cmd jobscheduler run -f -n androidx.work.systemjobscheduler
app.homespace.android <job id>`.

`iosAppUITests/AutoUploadUITests` runs the iOS flow against a dev server, given
`TEST_RUNNER_HOMESPACE_E2E_USER` and `TEST_RUNNER_HOMESPACE_E2E_PASSWORD`.

Known limits:

- The photo source lists everything until a pass has dealt with all of it, then asks only what
  changed (MediaStore generations on Android 11+, PhotoKit's persistent change token on iOS), with a
  listing every 7 days in case the platform missed something. It lists on every pass before Android
  11 and with access to selected photos only. Picked folders have no way to tell, so they are always
  listed.
- iOS changes an asset's modification date for reasons other than edits. Such a photo is sent again,
  and the server finds it identical and changes nothing, so this costs only the transfer.
- After a reinstall nothing is remembered. Files still in place on the server are recognised as
  identical, but ones moved or deleted there go up again, if the source uploads existing files.

## Single sign-on

The browser returns to the app at `app.homespace://oidc/callback`
(`protocol::paths::MOBILE_OIDC_REDIRECT`), which the server allows along with PKCE. It isn't
registered at the identity provider, which only ever sees the server's own callback.

To try it, start Keycloak and the server as in the root `README.md`. `HOMESPACE_PUBLIC_URL` must be
https or loopback, so `10.0.2.2` won't do on the Android emulator. Forward the ports instead, and
use `http://127.0.0.1:8080` as the server in the app:

```sh
adb reverse tcp:8080 tcp:8080
adb reverse tcp:8180 tcp:8180
```

The iOS simulator shares the host's network, so it needs no forwarding.

On a real iPhone, a server on the local network needs the local network permission. Two things made
that fail on iOS 18.7, both with `No route to host` and no prompt:

- A second Debug build next to an installed one. Debug builds can share a stub executable, and iOS
  tracks this permission by the executable's UUID. Build with `ENABLE_DEBUG_DYLIB=NO`.
- Opening the app from a UI test. Have the test attach to an app opened another way, such as
  `xcrun devicectl device process launch`, and call `activate()` rather than `launch()`.

## Notes

- Size: Rust adds about 5 MB to the stripped iOS binary with the `mobile` Cargo profile
  (`opt-level = "s"`, fat LTO), and about 14 MB with the plain `release` profile.
- Builds: the first Rust build per target takes about 30 s. The iOS script clears Xcode's
  environment, because cargo's C builds (aws-lc-sys) break under it. The Rust build also has its
  own Xcode target, so Xcode does not read the xcframework while it is being replaced.
  Xcode copies the xcframework's headers before that target runs, so the first build after a
  change to the exported API can fail with missing `uniffi_homespace_core_*` symbols. Build again.
- UniFFI: no error field may be named `message`, because it clashes with Kotlin's
  `Throwable.message`.
- HTTPS on Android: reqwest verifies certificates with `rustls-platform-verifier`, whose Kotlin
  half comes from upstream's Maven repository (see `settings.gradle.kts`), at the version in
  `Cargo.lock`. `PlatformVerifier` initialises it at startup. Don't add a `networkSecurityConfig`
  to the app: it would replace the library's allow-list of revocation servers, and certificates
  without an OCSP responder, such as all of Let's Encrypt's, would be reported as revoked.
- Plain HTTP: the Rust client's sockets aren't subject to Android's network security config or
  iOS's App Transport Security, so the apps will sign in to any `http://` address they are given.
- Size: bundled SQLite and auto-upload add about 1.9 MB to the arm64 Android library (6.7 MB to
  8.6 MB with the `mobile` profile).
- reqwest cannot upload while an iOS app is suspended, only during the background tasks iOS grants.
