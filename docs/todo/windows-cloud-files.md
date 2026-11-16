# Windows Cloud Files: on-demand files on the desktop

## Problem

On Windows the desktop client syncs chosen remote folders into chosen local folders
(`clients/desktop/src/sync.rs` on `sync-core`). Every file in a mapped folder is downloaded, so a
large library needs the same space on the PC as on the server, and anything outside a mapping can
only be reached through the app's browser.

This is the Windows counterpart of [`macos-file-provider.md`](macos-file-provider.md), with the
same requirements. Linux gets its own plan.

## Wanted

Home Space appears in File Explorer's navigation pane, like OneDrive or other cloud drives:

- The whole Home tree is visible, but a file takes no space until it is opened.
- Opening a file downloads it. The user can choose "Free up space" (dehydrate) or "Always keep on
  this device" (pin) per file or folder, and Storage Sense may free up files that weren't opened
  for a while.
- Any app can open, save, rename, move and delete files there, and changes from other devices show
  up without a manual refresh.
- Explorer's status column shows what is online only, downloaded, pinned and not uploaded yet, and
  failures (offline, signed out, storage full) are shown to the user.

## What exists

Checked in the repo on 2026-11-16. The server side is the same as in the macOS plan (see "What
exists" there): `GET /changes?since=` with cursor and `410 Gone`, `Range` on
`GET /files/{id}/content`, resumable `/uploads` with `node_id`, `keep_both` and `modified_at`,
`Node.version` and `Node.sha256`, the recycle bin API, and `507 quota_exceeded`. On the Windows
side:

- **Desktop app.** iced 0.14, a plain Rust binary, `windows_subsystem = "windows"` in release
  (`clients/desktop/src/main.rs`). There is no Windows-specific code besides the platform id, the
  theme and the "open in Explorer" call; no `windows` crate dependency yet.
- **Packaging.** `build-release.sh windows` zips `HomeSpace.exe`. There is no installer, no code
  signing, and `release.yml` has no Windows job (it builds macOS, Linux and Android).
- **Data directory.** `session::data_dir()` is `$HOME/.local/share/home-space`. `HOME` is usually
  not set on Windows, so this needs `%LOCALAPPDATA%\HomeSpace` before anything here.
- **Session.** One refresh token, in the Credential Manager with the `keyring` feature or in a file
  without it. Refresh tokens are single use.
- **Live updates.** The websocket (`clients/desktop/src/events.rs`) delivers `NodeCreated`,
  `NodeUpdated`, `NodeDeleted` and `TrashChanged`.
- **Folder sync.** `sync-core` keeps a SQLite mirror of the remote tree (`remote` table, from the
  journal), a `base` table (node id → local path, last synced state), saved uploads for resume,
  and plans actions (`Download`, `Upload`, `MoveLocal`, `MoveRemote`, `TrashLocal`, …) through a
  `Platform` trait. The desktop's `Platform` watches with `notify` and deletes to the system trash.

## How Windows does it

The Cloud Files API (cfapi, Windows 10 1709+, complete from 1809) is the system part behind
OneDrive's Files On-Demand:

- A **sync root** is an ordinary NTFS folder registered with `StorageProviderSyncRootManager`
  (WinRT, `Windows.Storage.Provider`). Explorer shows it in the navigation pane with the provider's
  name and icon.
- Files and folders in it are **placeholders**: real directory entries with name, size, times and
  a provider blob (the *file identity*), but no data. A placeholder takes about 1 KB on disk.
- The provider process **connects** to the sync root (`CfConnectSyncRoot`) and registers
  callbacks. When an app reads a placeholder that has no data, the `cldflt.sys` minifilter calls
  `FETCH_DATA`, and the provider pushes bytes with `CfExecute(TRANSFER_DATA)`. That is
  **hydration**. `CfDehydratePlaceholder` (or "Free up space") drops the data again.
- Folders can be filled on demand: when a not yet populated folder is listed, the filter calls
  `FETCH_PLACEHOLDERS`.
- The filter reports deletes and renames to the provider **before** they happen
  (`NOTIFY_DELETE`, `NOTIFY_RENAME`), and the provider may refuse them.
- It does **not** report new files or writes. A new file is an ordinary file until the provider
  converts it (`CfConvertToPlaceholder`); a written placeholder loses its *in-sync* state. The
  provider must watch the folder to find both, as the folder sync does today.
- Explorer draws the status icons (cloud, green tick, solid green for pinned, sync arrows for not
  in sync) from the placeholder's state and the `FILE_ATTRIBUTE_PINNED` / `UNPINNED` attributes.
  Nothing has to be drawn by the provider.

The key difference to macOS: there is no system-run extension. **Hydration only works while the
provider process runs and is connected.** Otherwise opening an online-only file fails with "The
cloud file provider is not running" (`ERROR_CLOUD_FILE_PROVIDER_NOT_RUNNING`). And the system
keeps no anchor or item list for us: the provider owns the tree state and the change cursor, much
like the folder sync already does.

## Proposal

### Architecture

```
HomeSpace.exe                  iced app (Rust), unchanged UI, now also:
  ├─ tray icon, runs without a window, starts at login
  └─ vfs/windows.rs, connected to the sync root
       ├─ callbacks (cfapi threads)  ── fetch data, fetch placeholders, delete, rename
       ├─ watcher (notify)           ── new files, writes, pin / unpin
       └─ vfs-core (shared with Linux) ── mirror, /changes, Range, uploads, conflict rule
```

- **One process, in-process provider.** The iced app connects to the sync root itself and keeps
  running when its window is closed (iced's `daemon` mode plus a tray icon via `tray-icon`), and
  starts at login (`HKCU\...\Run`, or a startup task with a package). One process means one
  session and one websocket, so the second-session endpoint of the macOS and iOS plans is not
  needed on Windows.
- **The logic is a shared `vfs-core` crate.** The Linux plan
  ([`linux-on-demand-files.md`](linux-on-demand-files.md)) needs the same platform-neutral parts,
  so both use a new `clients/vfs-core` crate: the metadata mirror in SQLite keyed by node id,
  applying `GET /changes` with its cursor, `Range` downloads, the upload queue (`keep_both` for new
  files, `node_id` + `modified_at` for edits, so the later change wins and the earlier becomes a
  version), pinning and eviction state, and error mapping. It takes what fits from `sync-core`
  (journal follow-up, saved uploads, the conflict rule) rather than duplicating it. The OS adapters
  live in the desktop app: `clients/desktop/src/vfs/windows.rs` here (Cloud Files) and
  `clients/desktop/src/vfs/linux.rs` there (FUSE).
- **What Windows adds on top.** Placeholders are real NTFS entries, so the Windows adapter also keeps
  per-node local state in `vfs-core`'s store (local path, placeholder identity and version, in sync
  or not) and scans the sync root for new files and writes, which FUSE gets as calls instead. That
  state goes behind a small adapter trait (`create_placeholder`, `update_placeholder`,
  `mark_in_sync`, `placeholder_state`), implemented with cfapi on Windows and by a fake in tests,
  so the logic runs on Linux CI with `wiremock`, like the rest.
- **The callbacks are thin.** They run on cfapi's thread pool and must answer quickly. They look
  the node up in the store, hand the work to the tokio runtime and complete the operation with
  `CfExecute`.

Crates:

- **`windows`** (windows-rs) for `Win32_Storage_CloudFilters`, `Storage_Provider` (WinRT sync root
  registration), `Win32_Storage_FileSystem` and `Win32_System_Com`. Microsoft maintained, complete.
- **`cloud-filter`** (ho-229/cloud-filter-rs, 0.0.7 from 2026-11-05) wraps both: a `SyncFilter`
  trait for the callbacks, `Session::connect`, placeholder builders and sync root registration.
  It would save the unsafe callback plumbing, but it is pre-1.0 with one maintainer. Proposed: try
  it in phase 1 behind our own small trait, and fall back to `windows` directly (or vendor what we
  use) if it gets in the way.

Rejected options:

| Option | Why not |
|---|---|
| A separate background agent process | Needs a second session (or IPC to the app) and a second binary to package; the in-process provider with a tray icon covers it. Reconsider if the app must quit while files stay reachable. |
| WinFsp / Dokany (FUSE-like drive) | Kernel driver to install, a drive letter instead of a folder, no Explorer status, pinning or "Free up space". |
| ProjFS (Projected File System) | Built for projecting a read-mostly tree (VFS for Git); a written file becomes a full file and there is no dehydration, pinning or status UI. |
| An "on-demand" mode inside `sync-core` | Placeholders are real files, so it would fit Windows, but not FUSE on Linux, and `sync-core`'s planner is built around full local copies. `vfs-core` shares the parts that fit. |
| A Windows-only engine | Duplicates what the Linux plan needs too. |

### Sync root

- **Folder:** `%USERPROFILE%\Home Space` by default, chosen in settings. It must be on a local NTFS
  volume (cfapi doesn't work on FAT, exFAT, or network drives); the app refuses others.
- **Id:** cfapi expects `{provider}!{user SID}!{account}`, here
  `HomeSpace!{SID}!{hash of server URL and user id}`, so one per signed-in account.
- **Registration** (`StorageProviderSyncRootInfo`):
  - `DisplayNameResource` "Home Space" (or "Home Space – {server host}" with more than one
    account), `IconResource` the exe's icon, `Version` the app version.
  - `HydrationPolicy = Progressive` with `ValidationRequired` off, so an app can read the start of
    a big file while the rest arrives. `HydrationPolicyModifier = AutoDehydrationAllowed` lets
    Storage Sense free up files.
  - `PopulationPolicy = Full`: folders are listed on demand through `FETCH_PLACEHOLDERS`, so a big
    tree costs nothing until it is opened.
  - `InSyncPolicy` tracking file attributes and last write time, `HardlinkPolicy = None`.
  - `AllowPinning = true`, `ShowSiblingsAsGroup = false`, `ProtectionMode = Personal`.
  - `RecycleBinUri` the web client's bin page, so Explorer can offer "View online recycle bin".
  - `ProviderId` a fixed GUID for Home Space.
- Registered after sign-in when the user turns on "Show in File Explorer" (off by default in the
  first release, see Phases). On sign-out the app disconnects and unregisters
  (`StorageProviderSyncRootManager.Unregister`); files not yet uploaded are moved to
  `%USERPROFILE%\Home Space (not uploaded)` first, and placeholders without data are deleted.
- A session ended elsewhere (the listener gets `None`): the provider stays connected but fails
  fetches with `STATUS_CLOUD_FILE_AUTHENTICATION_FAILED`, and the app shows "Sign in again". The
  sync root and pinned files stay.

### Items

| Home Space | Cloud Files |
|---|---|
| The user's root folder | the sync root folder |
| A node | a placeholder; file identity = node id (16 bytes) + `version` (8 bytes) |
| `.recycle-bin-{id}` | not created as a placeholder (hidden, as in the browsers) |
| `name` | the file name; names Windows can't hold (`CON`, `a:b`, trailing dot) are mapped as folder sync already does, or skipped with a fault |
| `size`, `updated_at`, `created_at` | `FileSize`, `LastWriteTime`, `CreationTime` in `CF_FS_METADATA` |
| `version` | the identity blob; a different version on the server dehydrates or refetches |
| Lost & Found | placeholders created read-only; deletes and renames refused |

New files made on the PC have no node id until they are uploaded. They stay ordinary files (Explorer
shows "sync pending") and are converted to placeholders with the server's id once the upload
finishes.

### Enumeration and changes

- **Listing a folder:** `FETCH_PLACEHOLDERS` pages `GET /nodes?parent=` (or reads the store when the
  mirror has the folder), answers with `TRANSFER_PLACEHOLDERS` and the
  `DISABLE_ON_DEMAND_POPULATION` flag, so the folder is filled once and later kept current by the
  journal.
- **Remote changes:** `vfs-core` follows `GET /changes?since={cursor}`, with the cursor in its
  store. In on-demand mode, for folders that are already populated:
  - a new node: `CfCreatePlaceholders` in its parent;
  - a content change: `CfUpdatePlaceholder` with the new size, times and identity and the
    `DEHYDRATE` flag; if the file is pinned it is hydrated again right after;
  - a rename or move: a local rename of the placeholder;
  - a delete (including a move to the bin): the placeholder is deleted;
  - a node in a folder that isn't populated yet is only stored; the folder gets it when listed.
  `410 Gone` starts a full rescan of the populated folders.
- **Own changes:** renames and deletes the provider makes itself also reach its `NOTIFY_*`
  callbacks. The callback carries the calling process id, so those from our own process are
  acknowledged without a server call. The journal entries tagged with this device id are
  reconciled against the stored local state.
- **Push:** the websocket events trigger a `/changes` pass (debounced to about one second), as
  folder sync does today. There is no "app not running" path as on macOS: if the app
  isn't running, nothing is fetched or updated until it starts and catches up from the cursor.

### Hydration and dehydration

- **Open:** `FETCH_DATA` gives an offset and length (rounded to 4 KB). The provider requests
  `GET /files/{id}/content` with `Range: bytes={offset}-` and streams it with
  `CfExecute(TRANSFER_DATA)` in 4 KB-aligned chunks (the last one may be short), reporting
  progress with `CfReportProviderProgress` so Explorer shows it. It checks the sha256 when the
  whole file was fetched.
- **Version moved on:** if the server's `version` differs from the placeholder's identity, the
  fetch fails with `STATUS_CLOUD_FILE_NOT_IN_SYNC` and the placeholder is updated first; the app
  retries the open. (Serving the newer bytes into the old size would corrupt the file.)
- **Cancel:** `CANCEL_FETCH_DATA` aborts the request. A dropped connection resumes with `Range`
  from what has arrived.
- **Progressive hydration** maps directly onto `Range`: a video player reading the middle of a file
  gets that range without the whole download.
- **Free up space:** Explorer's menu item and Storage Sense dehydrate on their own, after
  `NOTIFY_DEHYDRATE`, which the provider allows unless the file is not uploaded yet.
- **Always keep on this device:** Explorer sets `FILE_ATTRIBUTE_PINNED`. The watcher sees the
  attribute change and hydrates the file (or every file under the folder) with
  `CfHydratePlaceholder`. "Free up space" sets `UNPINNED` and the provider dehydrates. New
  placeholders in a pinned folder inherit the pin and are hydrated.
- **Thumbnails:** Explorer thumbnails only hydrated files until a thumbnail handler
  (`IThumbnailProvider`, registered for the sync root) uses a server thumbnail endpoint. Later.

### Uploads and conflicts

- **New files:** the watcher finds a non-placeholder file and uploads it, as folder sync does, with
  `parent_id` + `name` and `keep_both: true`; after the upload the file is converted with
  `CfConvertToPlaceholder` (identity = node id + version) and marked in sync. If the server picked
  `name (1).ext`, the local file is renamed.
- **Edits:** a written placeholder is no longer in sync. The scan uploads it with `node_id` and
  `modified_at` = its last write time, so the later change is current and the earlier becomes a
  version, the same rule as folder sync and the Linux plan. Then `CfUpdatePlaceholder` with the new identity and `MARK_IN_SYNC`.
  Both sides survive, so there are no conflict copies.
- **Edit and remote change at once:** the local file is not in sync and the server's version moved
  on. The upload still runs (later wins); if the server's copy stayed current, the local
  placeholder is updated to it and dehydrated, and the local edit is a version on the server.
- **Files in use:** an upload waits until the file can be opened for reading with sharing
  (Office keeps files locked while open); the watcher's next event retries it.
- **Stale base:** the optional `if_version` precondition proposed in the macOS plan helps here too.
- **Rename and move:** `NOTIFY_RENAME` calls `PATCH /nodes/{id}` or `POST /nodes/move` and
  acknowledges with `CfExecute(ACK_RENAME)`; a name that exists on the server fails the rename
  with `STATUS_CLOUD_FILE_IN_USE`-style refusal and Explorer shows an error. A rename of a file not
  uploaded yet is local only.
- **Folders:** a new folder is created with `POST /folders` and converted to a placeholder.
- **Big files:** chunked `/uploads`, with the upload id in `vfs-core`'s store, so a restart
  resumes.
- **Ignored files:** Office lock files (`~$*`), `desktop.ini`, `Thumbs.db` and `*.tmp` are not
  uploaded, as folder sync should already skip them.

### Recycle bin

- **Delete in Explorer:** for a placeholder the move to the Windows Recycle Bin is reported as a
  rename out of the sync root (`NOTIFY_RENAME` with the target out of scope), and a permanent
  delete as `NOTIFY_DELETE`. Both call `DELETE /nodes/{id}`, so the file goes to the server's bin.
  The provider acknowledges, so Explorer goes on.
- **Restore from the Windows Recycle Bin** puts the file back in the sync root as a rename into
  scope. If its identity is still there, the provider calls `POST /trash/restore` for the node;
  otherwise it is a new file and uploaded. (How the identity survives the Recycle Bin is to be
  confirmed.)
- **Online bin:** `RecycleBinUri` opens the web client's bin. The app's own trash view keeps
  working as today.
- A delete that can't reach the server is refused (`STATUS_CLOUD_FILE_UNSUCCESSFUL`), so files
  never disappear locally while they stay on the server, or the other way around.

### Storage quota

- An upload answered with `507 quota_exceeded` leaves the file not in sync (Explorer keeps the sync
  arrows), records a fault, and the app shows "Storage full" with the usage from `GET /storage`
  and a Windows notification once.
- After `TrashChanged` or a purge, the scan retries uploads that failed for quota.
- Later (phase 4): a custom state "Not uploaded: storage full" in Explorer's status column via
  `StorageProviderItemProperties` and a custom state handler, and the quota in Explorer's cloud
  flyout via `IStorageProviderStatusUISource` (Windows 11). Both need package identity.

### Errors

| Server | Cloud Files |
|---|---|
| unreachable, 502/503/504 | `STATUS_CLOUD_FILE_NETWORK_UNAVAILABLE` |
| 401 after refresh fails, session ended | `STATUS_CLOUD_FILE_AUTHENTICATION_FAILED` |
| 404 | `STATUS_CLOUD_FILE_NOT_IN_SYNC` (and the placeholder is removed by the next pass) |
| 409 `conflict` on a name | rename refused, Explorer shows the error |
| 507 `quota_exceeded` | upload fault, file not in sync (see above) |
| 410 on `/changes` | full rescan |
| anything else | `STATUS_CLOUD_FILE_UNSUCCESSFUL`, retried on the next pass |

### Relation to folder sync

As on macOS, both stay:

- The sync root is for browsing everything with little disk use; folder mappings stay full copies
  in folders the user chooses.
- A mapping whose local folder is inside the sync root, or the other way round, is refused, as
  `layout::conflict` refuses nested mappings today.
- An existing full mapping of Home can be turned into the sync root: its files are converted to
  placeholders in place (`CfConvertToPlaceholder`, marked in sync and kept hydrated) instead of
  downloaded again. That is a phase 4 nicety, not needed at first.

### Packaging and signing

Cloud Files itself needs no package identity: an unpackaged exe can register a sync root and
connect (OneDrive did for years, and cloud-filter-rs's examples run unpackaged). What needs
identity is the Explorer extras (custom states, the status flyout, thumbnail and context-menu
handlers registered the modern way). Proposed:

- **Installer:** replace the zip with a per-user installer (no admin rights), WiX via
  `cargo-wix` or Inno Setup, which installs to `%LOCALAPPDATA%\Programs\Home Space`, adds the
  Start menu entry and the "start at login" entry, and on uninstall unregisters the sync root
  (`HomeSpace.exe --unregister`) before removing files. A zip stays for portable use without the
  Explorer integration.
- **Package identity later (phase 4):** a **sparse package** (MSIX with
  `AllowExternalContent`, Windows 10 2004+) registered by the installer gives the unpackaged exe
  identity for the `windows.cloudFiles` manifest extensions (custom states, status UI, thumbnail
  handler). A full MSIX is the alternative, but its file system and registry virtualisation and the
  Store-style install don't suit a sync app that writes anywhere.
- **Code signing:** without a signature SmartScreen warns on every download until the file has
  reputation, and a sparse package can't be registered at all. Options:
  - Azure Artifact Signing (formerly Trusted Signing), a monthly fee, signs from CI without a
    hardware token; availability for individuals outside the US and Canada is to be checked.
  - An OV code signing certificate from a CA; since 2023 the key must live in an HSM or on a token,
    so CI signing needs a cloud HSM.
  - Unsigned at first, like the macOS app today: works, with SmartScreen's "More info → Run anyway".
    Fine for phase 2, not for default on.
- **Data and settings:** `%LOCALAPPDATA%\HomeSpace` for the store and logs (the `data_dir` fix
  above), the session in the Credential Manager (`keyring` on in Windows builds).

### CI

- **Pull requests:** a `windows-latest` job runs `cargo build -p desktop` and `cargo test -p desktop
  -p vfs-core` so the cfapi code (behind `cfg(windows)`) and its bindings compile. The on-demand
  logic in `vfs-core` runs its tests on Linux with the fake platform, like the rest.
- **Integration test (Windows only):** registers a sync root in a temp folder against a
  `wiremock` server, lists it, reads a file (hydration), writes one (upload) and deletes one, then
  unregisters. GitHub's Windows runners have NTFS and the Cloud Files filter, so this can run in CI;
  to be confirmed in phase 2.
- **Release:** a new Windows job in `release.yml` runs `build-release.sh windows`, builds the
  installer, and signs exe and installer when the secrets are set (`AZURE_SIGNING_*` or
  `WINDOWS_CERTIFICATE_*`); otherwise it ships unsigned, like the unsigned APK fallback.
- **Arch:** x86_64 first; aarch64 (`aarch64-pc-windows-msvc`) is a matrix entry once x86_64 works.
- None of this can be run in the cloud container; it needs a Windows machine or the runner.

### Phases

1. **Groundwork.** `data_dir` on Windows, a Windows CI job, `vfs-core` (shared with the Linux plan)
   with the placeholder adapter trait, a fake and `wiremock` tests, the choice between
   `cloud-filter` and `windows`.
2. **Read-only sync root.** Register and connect, on-demand folder listing, hydration with `Range`,
   remote changes as placeholder updates, free up space and pinning. Local writes, renames and
   deletes are refused. Tray icon, start at login. Behind a "Show in File Explorer" setting, off by
   default. Unsigned zip and installer.
3. **Writes.** New files, edits, folders, renames, moves, deletes to the server bin, restore from
   the Windows Recycle Bin, quota faults, progress. Windows job in `release.yml` with signing.
4. **Polish.** Sparse package with custom states and the status flyout, thumbnails, converting an
   existing full mapping in place, the `if_version` precondition, then default on.

## Platform details not yet confirmed

From Microsoft's documentation, the Cloud Mirror sample and other providers, not tried on a PC:

- That `StorageProviderSyncRootManager.Register` works from an unpackaged, unsigned exe on current
  Windows 10 and 11 builds, and which shell features silently need identity.
- How a placeholder moved to the Windows Recycle Bin is reported (rename out of scope or delete),
  and whether its file identity survives a restore.
- How `FETCH_DATA` behaves when the provider process is busy at login (Explorer opening recent
  files before the app has connected), and whether the app should connect before the window opens.
- Whether pinning a large folder should hydrate everything at once or by a queue with limits.
- How `cloud-filter` handles callback threads and panics (a panic across the FFI boundary must not
  take Explorer's request down with it).
- That GitHub's Windows runners allow sync root registration for the integration test.
- How antivirus scanners and Windows Search indexing interact with placeholders (they should skip
  offline files, `FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS`, but some trigger hydration).

## Open questions

- **Background app:** may the app keep running with a tray icon after its window closes and start
  at login? Proposed: yes, on by default when the sync root is on, since files can't be opened
  otherwise.
- **Engine:** a shared `vfs-core` for Windows and Linux (proposed, aligned with the Linux plan), or
  an on-demand mode inside `sync-core`, which suits Windows placeholders but not FUSE?
- **Crate:** start with `cloud-filter` (pre-1.0) or `windows` directly?
- **Signing:** Azure Artifact Signing, an OV certificate, or unsigned until default on?
- **Installer:** WiX (MSI, `cargo-wix`) or Inno Setup? And keep the portable zip next to it?
- **Minimum Windows:** Windows 10 22H2 and Windows 11 (proposed; Windows 10 is out of mainstream
  support since 2025-10-14), or anything from 1809?
- **Folder location:** fixed `%USERPROFILE%\Home Space`, or chosen by the user (proposed: default
  with a "Change" button, NTFS only)?
- **Folder sync:** keep folder mappings next to the sync root on Windows (proposed), or replace them
  there?
- **Shared with me:** a "Shared with me" folder in the sync root, or left out for now? Proposed:
  left out, as on macOS.
- **Order:** Windows before, after, or with macOS? Windows needs no developer program fee and no
  second session, so it could come first, together with Linux, and prove `vfs-core`.
