# Mobile native file provider

## Problem

Home Space files can only be reached from inside the app. To attach one to an email, the user has
to save it elsewhere first, and an email attachment can't be saved straight to Home Space. Apps
should be able to use Home Space as a storage location, as they do other cloud drives.

## Wanted

Home Space appears as a location in the system file pickers (the Android picker, and Files on iOS):

- Pick a file from it, for example to attach to an email.
- Save, copy or move a file into it, for example to save an email attachment.

Moving a file out deletes it from Home Space, which goes to the recycle bin as usual.

## What exists

Checked in the repo on 2026-11-06:

- **Single client.** Refresh tokens are single use, and a replayed one ends the whole session
  (`AuthService::refresh`, `SessionListener` in `clients/mobile-core/src/lib.rs`). Rotation is only
  serialised inside one client (`refresh_gate` in `clients/api-client/src/client.rs`).
- **Client owner.** `ClientHolder` on each platform owns the only `CoreClient`, which covers one
  process only.
- **Android start-up.** `HomeSpaceApp` runs `PlatformVerifier.init`, so a provider gets it too.
  `MainActivity` asks for `ACCESS_LOCAL_NETWORK`, and a provider can start without that activity.
- **iOS sharing.** There are no entitlements files. `SessionStore.swift` sets no Keychain access
  group and keeps the device id in `UserDefaults.standard`. Copies are in Application Support. An
  extension can reach none of these.
- **Exports.** `mobile-core` has no upload, `changes` or create-folder call yet. `open_copy` refuses
  a second download of the same file (`already_downloading`).
- **Downloads.** The server has no Range support, so a file can only be read from the start.
- **Uploads.** `POST /uploads` needs the total `size` up front, and a node exists only after the
  last chunk (`docs/chunked-upload.md`).
- **Ids and changes.** Moves keep node ids. `GET /changes` pages with a cursor, returns the current
  cursor without `since`, and answers `410 Gone` once the cursor is pruned. A move to the bin is
  reported as `Deleted`.
- **Sessions.** A session only starts from password or SSO sign-in. `sign_out_device` ends every
  session of a device, while `logout` ends only the session of its token.

## Proposal

Both platforms use existing server APIs. Only the iOS session option below needs a server change.

### Android: `DocumentsProvider`

- **Manifest:** a `<provider>` with `exported` and `grantUriPermissions` set, protected by
  `MANAGE_DOCUMENTS`, with the `android.content.action.DOCUMENTS_PROVIDER` intent filter. It needs
  its own authority, separate from the `FileProvider` at `${applicationId}.files`.
- **Client:** the provider runs in the app's process, since the manifest sets no `android:process`.
  It shares the process-wide `CoreClient` holder, so there is no token problem.
- **Roots:** one root when a session is saved, none when signed out. `queryRoots` should avoid the
  network, so it reads `SessionStore` only.
- **Document ids:** the node id, plus a fixed id for Home. Android requires durable ids, and node
  ids survive moves and renames. `queryDocument` should also avoid the network, so this needs a
  small metadata cache, filled by `queryChildDocuments`.
- **Reading:** download first, then return a descriptor on the finished file. This is the simple
  choice and suits a server without Range. `openDocument` blocks meanwhile and checks its
  `CancellationSignal`. Alternatives, for later:
  - A pipe, for mode `r` only: it starts at once but can't seek.
  - `StorageManager.openProxyFileDescriptor` (API 26, our `minSdk`): it can seek, but a read beyond
    what has arrived must wait, or the server must add Range.
- **Writing:** spool to a local file, whatever the mode, since the upload needs the size. Mode `rw`
  downloads first. Upload starts when the descriptor closes
  (`ParcelFileDescriptor.OnCloseListener`), as WorkManager work, so it outlives the caller.
- **New files:** `createDocument` must return an id that never changes, before any bytes or node
  exist. Hand out a provider-made id and persist its mapping to the node once the upload finishes.

### iOS: File Provider extension

- **Type:** `NSFileProviderReplicatedExtension` (iOS 16+, the app targets 17). The system keeps the
  local copies, as dataless or materialized items, and calls the extension to fill or sync them.
- **Target:** a new app-extension target in `iosApp/project.yml`, embedded in the app. It links its
  own copy of `HomeSpaceCore.xcframework` (static libraries), about 5 MB.
- **Sharing:** an App Group and a shared Keychain access group for both targets. The Keychain item
  must move to the group, which signs existing users out once unless it is migrated.
- **Domain:** the app adds an `NSFileProviderDomain` per signed-in user. Apple's sample says a new
  domain starts disabled on iOS, and the user enables it in Files.
- **Content:** `fetchContents` downloads to a file, and `createItem` and `modifyItem` receive a
  complete file, so both fit the server without Range.
- **Changes:** the working-set enumerator maps `enumerateChanges` to `GET /changes`. The cursor is
  the sync anchor, and `410 Gone` becomes `syncAnchorExpired`. `Upserted` and `Deleted` map to
  updated and deleted items. The extension can stay free of a database for this, since the system
  holds the anchor.
- **Remote changes:** the system must be told (`signalEnumerator`, from the app, or a PushKit push).
  The server has no push, so changes appear when the app signals or the system enumerates.

### iOS: the second process

The extension is a separate process, so the process-wide holder doesn't help. Two clients sharing a
token will eventually replay one, which ends the session and signs the user out of the app.

Options for the session:

1. **A session of its own (preferred).** While signed in, the app asks the server for a second
   session on the same device and stores its token for the extension. Nothing is coordinated, and
   a failure in one process can't end the other's session. It needs a new authenticated server
   endpoint. Sign-out must end both, and an unused session expires after 30 days by default, so
   the app must renew it.
2. **One shared token with a lock.** Each process takes a cross-process lock, re-reads the Keychain
   and only then rotates. This needs a hook in `api_client`, and the lock is held across a network
   call. A process suspended with a lock in the App Group container is killed by iOS (`0xdead10cc`).
3. **The app hands out access tokens.** Rejected: they last 15 minutes by default, and the extension
   must work when the app isn't running.

The same applies to auto-upload's SQLite database (`mobile-core/src/autoupload`). Options:

1. **Separate stores (preferred).** The extension doesn't open the app's database, and keeps any
   upload ids in its own small store.
2. **A shared database in the App Group.** It has the same `0xdead10cc` risk, reported for open
   transactions during suspension, and needs care in every code path.

### Uploads

- Use the resumable `/uploads` API (`Client::continue_upload`), for every size.
- Writes go by name in a folder: `keep_both` and `auto_upload` stay off. A new
  name creates a file, an existing name adds a version, and identical content changes nothing.
- Leave `modified_at` out for these uploads. Content dated before the current file's `updated_at` is
  archived as a version rather than made current (`apply_content`), so saving an old attachment over
  a newer file would look as if nothing happened.
- `POST /folders` backs creating folders, and the existing rename, move and delete calls back the
  rest. Delete goes to the bin, even though Apple's `deleteItem` is described as forever.

### Order

This builds on the mobile data sync work (auto-upload and two-way offline sync, see
`docs/specs/mobile-sync.feature`), which leaves the iOS File Provider as a follow-up to two-way sync.

1. **Done:** the upload calls in `mobile-core`, the process-wide client holder, WorkManager, and
   two-way offline sync.
2. **Android provider, open and save:** done (2026-11-12), see below.
3. **iOS extension:** the storage it sits beside is now settled.

### Android: what shipped

`HomeSpaceDocuments` (`clients/mobile/androidApp`) at `${applicationId}.documents`. Checked on an
emulator with the system picker: browse, open, new folder, save, and saving a name again.

- **Open:** `saveCopy` into `cacheDir/documents`, then the descriptor is opened and the file
  deleted, so nothing is pinned or left behind. Online only. Blocks with no size limit; the
  `CancellationSignal` calls `cancel_download`. While the app downloads the same file
  (`already_downloading`), it waits and tries again.
- **Save:** `createDocument` adds to the app's upload queue (`UploadQueue.add`) and returns
  `upload:<queue key>`. The caller writes the queue's copy; on close it is made ready, started, and
  `scheduleUploads` carries it on. It shows in Transfers. A name that exists becomes a new version;
  the system picker doesn't ask first. Each key's folder, name and type are kept for 7 days in
  `provider_uploads` preferences, so the id still resolves after the upload.
- **Folders:** `createDocument` with the directory type calls `create_folder`.
- **Sign-out:** `SessionStore` notifies the roots URI when a session is saved or forgotten, and
  clears the cache and the saved-file records.
- **Known:** the upload worker's foreground notification is refused when it starts with another app
  in front. WorkManager logs a warning, and the upload still runs as plain work.
- **Still open:** editing an existing file in place (`FLAG_SUPPORTS_WRITE` on files), delete,
  rename and move, offline reads, thumbnails, search, recents and tree access
  (`FLAG_SUPPORTS_IS_CHILD`).

Editing by other apps:

- Two-way sync covers edits by viewers (Android `ACTION_EDIT`, iOS Quick Look). The provider adds the
  general case, where any app opens a Home Space file and saves it back.
- In two-way sync, opening a file pins it. A provider read must not pin, or attaching a file to an email
  would keep it synced forever. Provider reads use a cache the OS may clear, not the offline folder.
- On iOS the system holds its own copies, apart from the app's offline copies, so a file can be
  stored twice. A provider write and an unsynced offline edit meet on the server, and follow two-way
  sync's conflict rule: the later change is current, the other a version.

### Sign-out

Includes a session ended from elsewhere, when the listener gets `None`.

- **Android:** return no roots and call `notifyChange` on the roots URI, as the docs require. Revoke
  grants where possible, and delete the spool and read cache with the `logout` wipe.
- **iOS:** remove the domain, end the extension's session and delete its Keychain item. Per Apple's
  sample, removing a domain on iOS deletes all its files, including any not yet uploaded.
- **Both:** warn about writes not yet uploaded, as two-way sync does for offline edits.

## Platform details not yet confirmed

From Apple and Android reference pages, but not tried on a device:

- That common email apps accept a slow `openDocument`, and don't require a seekable descriptor or
  time out. A long block may freeze a caller that opens on its main thread.
- Whether the Android system picker asks before reusing an existing name in
  `ACTION_CREATE_DOCUMENT`.
- How long Android keeps a process alive after a descriptor closes, hence WorkManager.
- The memory limit of the iOS extension. Apple gives no figure that I found, and a forum post
  mentions 20 MB. This matters for a Rust client with its own runtime, and for chunk sizes.
- How long an iOS extension may run during an upload, and whether reqwest works there at all.
- Whether `0xdead10cc` applies to extensions as it does to apps. The sources are Apple forum posts
  quoting TN2151, not the note itself.

## Open questions

- **Offline:** is the provider online only at first? Android could serve existing copies when the
  server can't be reached, as `open_copy` does. iOS serves materialized items without the extension.
- **Large downloads:** what happens when an email app waits on a large file? Proposed: block with no
  size limit and rely on cancelling, then test it. A limit with a clear error is the fallback.
- **Mobile data:** do "Sync only on Wi-Fi" and "Sync in background" apply? Proposed: reads and
  writes the user starts here ignore them, like tapping a file. Uploads queued for later obey them.
- **Existing names on Android:** version the file, or keep both by picking a new name in
  `createDocument`? Proposed: version, to match the server's behaviour and the rest of this page.
- **Replacing an open document:** upload by name, or by `node_id` so that a file moved in the
  meantime is followed? By name matches auto-upload, but can create a second file.
- **iOS session:** is option 1's new server endpoint acceptable?
- **Scope:** thumbnails, search and the trash can wait. Does the first version need any of them?
