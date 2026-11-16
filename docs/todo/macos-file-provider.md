# macOS File Provider: on-demand files on the desktop

## Problem

On macOS the desktop client syncs chosen remote folders into chosen local folders
(`clients/desktop/src/sync.rs` on `sync-core`). Every file in a mapped folder is downloaded, so a
large library needs the same space on the Mac as on the server, and anything outside a mapping can
only be reached through the app's browser.

## Wanted

Home Space appears in Finder's sidebar, under Locations, like iCloud Drive or other cloud drives:

- The whole Home tree is visible, but a file takes no space until it is opened.
- Opening a file downloads it. macOS may remove the download again when space runs low, and the
  user can choose "Remove Download" or "Keep Downloaded" (pin) per file or folder.
- Any app can open, save, rename, move and delete files there, and changes from other devices show
  up without a manual refresh.
- Badges show what is not uploaded yet, and failures (offline, signed out, storage full) are shown
  by Finder.

## What exists

Checked in the repo on 2026-11-16:

- **Desktop app.** iced 0.14, a plain Rust binary. It already uses `objc2`, `objc2-foundation` and
  `objc2-app-kit` on macOS. Bundle id `io.github.indomitable.home-space`. `build-release.sh` builds
  the `.app` by hand and signs it ad hoc (`codesign --deep --sign -`); `release.yml` builds it with
  `cargo bundle` and makes an unsigned `.dmg` for arm64.
- **Session.** One refresh token, in the Keychain with the `keyring` feature or in a file without
  it (`clients/desktop/src/session.rs`). Refresh tokens are single use, and a replayed one ends the
  whole session.
- **Live updates.** The desktop keeps a websocket open (`clients/desktop/src/events.rs`) and gets
  `NodeCreated`, `NodeUpdated`, `NodeDeleted` and `TrashChanged`. The server pings every 15 s.
- **Change journal.** `GET /changes?since=` pages with a cursor, returns the current cursor without
  `since`, answers `410 Gone` once the cursor is pruned, and reports a move to the bin as `Deleted`.
  Node ids survive moves and renames.
- **Content.** `GET /files/{id}/content` honours a single `Range`. Uploads go through the resumable
  `/uploads` API, which needs the total `size` up front. `node_id` replaces a file's content,
  `keep_both` picks a free name instead of adding a version, and `modified_at` older than the
  current file's `updated_at` archives the upload as a version instead of making it current.
- **Versions.** `Node.version` goes up on every content change only. `Node.sha256` is the content
  hash. There is no precondition on uploads or renames (no `If-Match` / expected version).
- **Recycle bin.** A per-user hidden folder `.recycle-bin-{id}`. `DELETE /nodes/{id}` moves to the
  bin, `POST /trash/restore` brings back (with an `on_conflict`), `DELETE /trash/{id}` purges.
- **Quota.** Uploads over the quota answer `507` with `quota_exceeded` (PR #65, merged).
  `GET /storage` reports usage.
- **Swift bindings.** `clients/mobile-core` exports `api-client`, `client-core` and `sync-core` to
  Swift through UniFFI. `scripts/build-ios.sh` builds `HomeSpaceCore.xcframework` for iOS device
  and simulator only. The iOS app is generated with XcodeGen (`clients/mobile/iosApp/project.yml`).
- **iOS File Provider.** Planned in [`mobile-file-provider.md`](mobile-file-provider.md), not built.
  Its findings on the second process, sessions and uploads apply here unchanged, and the extension
  API (`NSFileProviderReplicatedExtension`) is the same on macOS.

## Proposal

### Architecture

```
Home Space.app/
  Contents/MacOS/HomeSpace                      iced app (Rust), unchanged UI
  Contents/PlugIns/HomeSpaceFiles.appex/        File Provider extension (Swift, sandboxed)
    Contents/MacOS/HomeSpaceFiles               links HomeSpaceCore (mobile-core, static)
```

- **The extension is Swift.** macOS loads File Provider extensions only as `.appex` bundles with an
  `NSExtension` Info.plist and `NSExtensionPrincipalClass`, run by the system on its own. Writing
  it in Rust through `objc2` is possible in theory but means defining Objective-C classes by hand,
  with no Xcode support for the appex target, so it is not worth it.
- **The logic is Rust, in-process.** The extension links `mobile-core` (UniFFI, static library) and
  stays a thin adapter: item mapping, change translation, upload state and error mapping live in a
  new `provider` module in `mobile-core` (or a small `provider-core` crate it re-exports). That code
  is tested with `wiremock` on Linux CI, like the rest of the clients, and is shared with the
  iOS extension later.
- **No XPC to the main app.** The extension must work when the app is not running (Finder opens a
  file at login, Spotlight, another app saves). XPC would make the iced app a required server, and
  it would need an `NSXPCListener` in Rust. The app only talks to the system's
  `NSFileProviderManager`, which it can do from Rust through `objc2-file-provider`.
- **The main app stays Rust.** It adds and removes the domain, signals changes, and shows status. All
  of that is a handful of `NSFileProviderManager` calls through `objc2-file-provider` from a new
  `clients/desktop/src/file_provider.rs` (macOS only).

Rejected options:

| Option | Why not |
|---|---|
| `swift-bridge` instead of UniFFI | UniFFI is already set up for iOS, with async and generated Swift. |
| Extension calls the app over XPC | The app must be running; two IPC layers; no gain. |
| The whole desktop app in Swift | Rewrites the iced UI, against the decision to stay on iced. |
| macFUSE / FSKit | macFUSE needs a kernel or system extension and user approval; FSKit (macOS 15.4+) is for file systems on block devices, not cloud files. |

### Sessions and storage

The same second-process problem as on iOS (see "iOS: the second process" in
[`mobile-file-provider.md`](mobile-file-provider.md)), with the same answer:

- **A session of its own.** While signed in, the app asks the server for a second session for the
  same device and hands its refresh token to the extension. This is the new server endpoint already
  proposed for iOS; build it once for both. Sign-out ends both sessions. The app renews the
  extension's session before its 30 days run out.
- **Hand-over.** The token goes into a Keychain item in a shared access group
  (`$(TeamID).io.github.indomitable.home-space.shared`). The extension rotates it itself and writes
  it back; the app only writes it once and deletes it on sign-out.
- **App Group container** `$(TeamID).io.github.indomitable.home-space` for what both read: the
  server URL, the user id, the device id, and the extension's own small SQLite (pending uploads and
  their upload ids, provider-made ids for items created before upload). The team-id prefix avoids
  the macOS 15 prompt that `group.`-prefixed containers get outside the App Store.
- **No shared sync database.** The extension doesn't open `sync-core`'s store, as the iOS plan also
  chose.

### Domain

- One `NSFileProviderDomain` per signed-in account. Identifier: a hash of server URL and user id, so
  it is stable and has no slashes. Display name: "Home Space", or "Home Space – {server host}" when
  more than one account is signed in. Finder shows it at `~/Library/CloudStorage/HomeSpace-…`.
- The app adds it after sign-in when the user turns on "Show in Finder" (off by default in the first
  release, see Phases), and removes it on sign-out with `removeDomain(_:mode: .preserveDirtyUserData)`,
  which keeps unsynced files in a folder rather than deleting them.
- A session ended elsewhere (the listener gets `None`): the extension answers `notAuthenticated` and
  the app shows "Sign in again". The domain stays, so pinned files are not lost.

### Items

| Home Space | File Provider |
|---|---|
| The user's root folder | `.rootContainer` |
| A node | `itemIdentifier` = node id (UUID string) |
| `.recycle-bin-{id}` | hidden from enumeration in phase 1; `.trashContainer` in phase 4 |
| `version` | `itemVersion.contentVersion` (8-byte big-endian) |
| `parent_id`, `name` | `itemVersion.metadataVersion` = short hash of both |
| `size`, `updated_at`, `created_at`, `mime` | `documentSize`, `contentModificationDate`, `creationDate`, `contentType` (`UTType(mimeType:)`) |
| Lost & Found | capabilities without delete; no create/upload into it (as the clients already do) |

Identifiers for items created on the Mac: `createItem` must return an item at once. It returns
the server's node after the upload, so the identifier is the node id from the start. A create that
can't reach the server fails with `serverUnreachable`, and the system retries it later.

### Enumeration and changes

- **Folders:** `enumerateItems` pages `GET /nodes?parent=` with the page token as the server's
  cursor or offset. `enumerateChanges` on a folder enumerator just returns the current anchor;
  all changes go through the working set.
- **Working set:** `enumerateChanges(from anchor)` calls `GET /changes?since={anchor}`. `Upserted`
  becomes `didUpdate`, `Deleted` becomes `didDeleteItems`, `more` maps to `moreComing`, and the
  cursor is the new anchor. `410 Gone` becomes `syncAnchorExpired`, and the system re-enumerates.
  `currentSyncAnchor` calls `GET /changes` without `since`. The extension keeps no tree of its own:
  the system holds the items and the anchor.
- **Changes made by the extension itself** come back in the journal tagged with this device's id.
  They are still reported (the system compares versions and ignores no-ops), so nothing has to be
  filtered.
- **Push:** the server has no APNs, and a self-hosted server can't send Apple pushes without a
  relay we would have to run, so remote changes reach the extension in two ways:
  1. **The app is running:** on `NodeCreated`, `NodeUpdated`, `NodeDeleted` or `TrashChanged` from
     its websocket, it calls `signalEnumerator(for: .workingSet)`, debounced to about one second.
     This is the main path, so the app should keep running in the background (see the open
     question on a menu-bar mode and login item).
  2. **The app is not running:** the system enumerates the working set when Finder shows the domain
     and from time to time. Changes are then late, never lost, because the journal is the source.

### Materialization and eviction

- **Open:** `fetchContents` downloads `GET /files/{id}/content` to a temporary file in the
  extension's container, reports `Progress`, checks the sha256, and returns it with the item. A
  `version` other than the one asked for is fine; the returned item carries the new version.
- **Resume:** a dropped download continues with `Range` from what has arrived.
- **Large media (later):** `NSFileProviderPartialContentFetching` (macOS 12.3+) lets QuickTime read
  ranges without the whole file. It maps directly onto `Range`.
- **Eviction:** the system evicts on its own when space is low. "Remove Download" works with no
  code. Pinning ("Keep Downloaded") sets `contentPolicy = .downloadEagerlyAndKeepDownloaded` on
  the pinned item (macOS 14+); whether Finder offers the menu item itself or the extension adds it
  as a custom action is to be confirmed.
- **Thumbnails:** a `NSFileProviderThumbnailing` implementation can use the server's thumbnails once
  there is an endpoint for them; until then Finder thumbnails only downloaded files.

### Uploads and conflicts

- **New files:** `createItem` gets the whole file. Upload with `parent_id` + `name` and
  `keep_both: true`, so a file another device created with the same name meanwhile is not turned
  into a version. If the server picks `name (1).ext`, the returned item has that name and the
  system renames the local copy.
- **Edits:** `modifyItem` with `.contents` uploads with `node_id`. `modified_at` is the file's
  modification date, so the existing two-way sync rule holds: the later change is current, the
  earlier one becomes a version. Both survive, so there are no conflict copies.
- **Stale base:** without a precondition, an edit made on a stale base still wins if it is later.
  That matches the folder sync, but a precondition makes it explicit: add an optional `if_version`
  to `CreateUpload` (with `node_id`), answering `409 conflict` when the file moved on. The
  extension would then upload as a version (`modified_at` from the base) and return the server's
  current item so the system fetches it. Proposed as an optional server change, not needed for
  phase 1.
- **Metadata:** rename and move map to `PATCH /nodes/{id}` and `POST /nodes/move`. A name that
  exists answers `filenameCollision`, and the system asks the user.
- **Folders:** `createItem` for a folder calls `POST /folders`.
- **Packages and bundles** (`.app`, `.pages`, `.photoslibrary`) are folders to the server. They work
  as trees but are uploaded file by file; a half-uploaded package can be seen on other devices.
  Exclude `.photoslibrary` and similar through `excludedFromSync` in the first version.
- **Big files:** chunked `/uploads`, session kept in the extension's SQLite so a killed extension
  resumes. The extension reports progress on the `Progress` the call returns.

### Recycle bin

- **Phase 1:** `deleteItem` calls `DELETE /nodes/{id}`, so a delete in Finder goes to the server's
  bin, not away for good. The domain doesn't declare trashing, so Finder deletes "immediately"
  locally and says so; the server still keeps it.
- **Phase 4:** declare `.allowsTrashing` and map `.trashContainer` to the server bin. Moving to the
  Finder trash becomes `modifyItem` with the trash as parent: `DELETE /nodes/{id}`. Put Back is
  `POST /trash/restore`. Emptying the trash calls `deleteItem` on trashed items: `DELETE /trash/{id}`.
  The journal reports a move to the bin as `Deleted`; for this phase it should report it as an
  update with the bin as parent (a protocol change, behind a capability flag) so the system moves
  it into the trash instead of deleting it.

### Storage quota

- An upload answered with `507 quota_exceeded` fails with `NSFileProviderError.insufficientQuota`.
  Finder shows its own message and the item is badged; the system retries when signalled.
- After `TrashChanged` or a purge, the app signals the working set so failed uploads are retried.
- The app shows usage from `GET /storage`, as today.

### Errors

| Server | File Provider |
|---|---|
| unreachable, 502/503/504 | `serverUnreachable` |
| 401 after refresh fails, session ended | `notAuthenticated` |
| 404 | `noSuchItem` |
| 409 `conflict` on a name | `filenameCollision` |
| 507 `quota_exceeded` | `insufficientQuota` |
| 410 on `/changes` | `syncAnchorExpired` |
| anything else | `NSError` with the server message, retried by the system |

### Relation to folder sync

The File Provider and `sync-core` folder mappings do the same job in different ways. Both stay:

- The File Provider is for browsing everything with little disk use; folder sync is for folders that
  must always be complete on disk in a place the user chooses (for tools that dislike
  `~/Library/CloudStorage`, such as some backup or build tools).
- A mapping whose local folder is inside the domain's folder is refused, the same way
  `layout::conflict` refuses nested mappings today.
- Linux and Windows keep folder sync only. (Windows Cloud Files API would be the counterpart there,
  a separate plan.)

### Packaging, signing and notarization

The extension needs real signing; an ad-hoc signed `.appex` is not loaded by the system.

- **Apple Developer Program** membership (paid) is needed, for a Developer ID Application
  certificate and a team id.
- **Build:** a new XcodeGen spec `clients/desktop/macos/project.yml` with one target, the
  `HomeSpaceFiles` app extension (`com.apple.fileprovider-nonui`), deployment target macOS 13. The
  Rust app stays outside Xcode. `build-release.sh macos` then:
  1. builds `mobile-core` for `aarch64-apple-darwin` and `x86_64-apple-darwin` and adds a macOS slice
     to `HomeSpaceCore.xcframework` (`build-ios.sh` gains a `--macos` option or a sibling
     `build-macos.sh`),
  2. builds the `.appex` with `xcodebuild`,
  3. assembles the `.app` as today and copies the `.appex` into `Contents/PlugIns/`,
  4. signs inside-out, without `--deep`: the `.appex` with its entitlements, then the app with its
     own, both with `--options runtime` (hardened runtime) and a timestamp,
  5. makes the `.dmg`, signs it, notarizes it with `xcrun notarytool submit --wait`, and staples it.
- `release.yml` switches from `cargo bundle` to `build-release.sh`, since `cargo bundle` can't add
  the extension.
- **Entitlements, app:** `com.apple.security.application-groups`, `keychain-access-groups`. The app
  itself stays unsandboxed (folder sync needs any path).
- **Entitlements, extension:** `com.apple.security.app-sandbox`, `com.apple.security.network.client`,
  the same application group and keychain access group.
- **Provisioning profiles:** `keychain-access-groups` is a restricted entitlement, so both targets
  need a Developer ID provisioning profile embedded (`embedded.provisionprofile`). Profiles are made
  in the developer portal for the two bundle ids, with App Groups enabled.
- **Bundle ids:** `io.github.indomitable.home-space` and
  `io.github.indomitable.home-space.files`. The Flatpak id is `app.home_space.HomeSpace`; changing
  the macOS id now is cheaper than after users have a domain (see open questions).

### CI

- **Pull requests:** a macOS job builds the extension unsigned (`CODE_SIGNING_ALLOWED=NO`) to catch
  Swift and binding breaks. The Rust `provider` module's tests run on Linux with the rest.
- **Release:** the macOS job imports the certificate into a temporary keychain, installs the
  profiles, signs, notarizes and staples. Notarization adds a few minutes.
- **Secrets:** `MACOS_CERTIFICATE_P12_BASE64`, `MACOS_CERTIFICATE_PASSWORD`, `APPLE_TEAM_ID`,
  `MACOS_PROFILE_APP_BASE64`, `MACOS_PROFILE_EXTENSION_BASE64`, and an App Store Connect API key for
  `notarytool` (`APPLE_API_KEY_ID`, `APPLE_API_ISSUER_ID`, `APPLE_API_KEY_P8_BASE64`). Without them
  the release builds the app as today, without the extension, like unsigned APKs.
- **Universal:** the release only builds arm64. The extension and `mobile-core` slice follow the
  same matrix; `--universal` keeps working through `lipo` for both binaries.
- Nothing of this can be built or tested in the cloud container; it needs a Mac or the macOS
  runner.

### Phases

1. **Server and core.** The second-session endpoint (shared with iOS). The `provider` module in
   `mobile-core`: item mapping, change translation, error mapping, upload and download with resume,
   with `wiremock` tests. The macOS slice of the xcframework.
2. **Read-only domain.** Extension with enumeration, working set, `fetchContents`; the app adds and
   removes the domain and signals on websocket events. Writes answer "not supported". Behind a
   "Show in Finder" setting, off by default. Signed builds by hand on a Mac.
3. **Writes.** `createItem`, `modifyItem`, `deleteItem` (to the server bin), folders, quota and
   error mapping, progress. Signing and notarization in `release.yml`.
4. **Polish.** Trash container and Put Back, pinning, partial fetches for media, thumbnails, the
   `if_version` precondition, menu-bar mode and login item, then default on.
5. **iOS.** The same Swift extension source set, built for iOS in `iosApp/project.yml`, per
   [`mobile-file-provider.md`](mobile-file-provider.md).

## Platform details not yet confirmed

From Apple's reference pages and other cloud drives, not tried on a Mac:

- That a Developer ID (non-App Store) app can ship a File Provider extension with a Developer ID
  provisioning profile for `keychain-access-groups`. Dropbox, Google Drive and OneDrive do, but the
  exact profile steps should be tried before phase 2.
- That the team-id-prefixed App Group avoids the macOS 15 container prompt for both an unsandboxed
  app and a sandboxed extension.
- How often macOS enumerates the working set on its own when the app is not running.
- Which macOS version first offers "Keep Downloaded" for third-party domains, and how.
- Whether macOS 15 asks the user to allow the extension in System Settings (Login Items &
  Extensions) before the domain appears, and how the app should guide them.
- Whether the Rust tokio runtime and reqwest inside the extension behave under the extension's
  lifecycle (the system may suspend or kill it between calls). Upload state must survive a kill.
- That `objc2-file-provider` covers `addDomain`, `removeDomain(mode:)` and `signalEnumerator`
  completely enough for the app.

## Open questions

- **Developer account:** is there (or will there be) an Apple Developer Program membership? Without
  it the extension can't be signed and won't load, even for local testing beyond Xcode's
  development signing.
- **Bundle id:** keep `io.github.indomitable.home-space` on macOS, or move to `app.home-space…` to
  match the Flatpak id and the domain, before the first release with the extension?
- **Folder sync:** keep folder mappings next to the File Provider on macOS (proposed), or replace
  them there?
- **Background app:** may the app keep running after its window closes (menu-bar icon) and start at
  login, so remote changes appear at once? Proposed: yes, as an option, on by default when the
  domain is on.
- **Minimum macOS:** 13 for the extension (proposed; pinning needs 14), while the app keeps 11/12?
- **Order:** macOS before iOS (proposed, since a Mac is easier to debug and the logic is shared), or
  iOS first as in the mobile plan?
- **Precondition:** add `if_version` to uploads, or keep "later change wins" everywhere?
- **Trash:** is phase 1's "delete in Finder goes to the server bin, Finder says immediately"
  acceptable until the trash container lands?
- **Shared with me:** show files shared with the user as a folder in the domain, or leave them out
  of Finder for now? Proposed: leave out.
