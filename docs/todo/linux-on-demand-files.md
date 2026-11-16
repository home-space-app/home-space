# Linux on-demand files: the whole Home tree in the file manager, downloaded when opened

## Problem

On Linux the desktop client syncs chosen remote folders into chosen local folders
(`clients/desktop/src/sync.rs` on `sync-core`). Every file in a mapped folder is downloaded, so a
large library needs the same space on the PC as on the server, and anything outside a mapping can
only be reached through the app's browser.

The macOS plan ([`macos-file-provider.md`](macos-file-provider.md)) solves this with Apple's File
Provider. This plan has the same requirements for Linux. Windows gets its own plan with the Cloud
Files API.

## Wanted

The same as on macOS:

- Home Space appears in the file manager's sidebar (Nautilus, Dolphin, Thunar, Nemo, ...) as a
  folder, and every other app can open and save files in it like in any folder.
- The whole Home tree is visible, but a file takes no space until it is opened.
- Opening a file downloads it. Downloads are removed again when the cache is full, and the user can
  choose "Free up space" or "Keep on this device" (pin) per file or folder.
- Changes from other devices show up without a manual refresh.
- Badges show what is not downloaded, not uploaded yet, or failed, and failures (offline, signed
  out, storage full) are shown by the app.

## What exists

Checked in the repo on 2026-11-16:

- **Desktop app.** iced 0.14 with tokio, a plain Rust binary. Linux packages: a `.tar.gz` with the
  binary and a `.desktop` file (`release.yml`, job `desktop-linux`), and a Flatpak (PR #66, app id
  `app.home_space.HomeSpace`, `org.freedesktop.Platform` 26.08, `--filesystem=home`,
  `--talk-name=org.freedesktop.secrets`). The Flatpak launcher points `XDG_DATA_HOME` at the host's
  `~/.local/share` so sync can use the user's trash.
- **Session.** One refresh token, in the Secret Service with the `keyring` feature (both Linux
  packages enable it) or in a file without it (`clients/desktop/src/session.rs`). Refresh tokens
  are single use, and a replayed one ends the whole session.
- **Live updates.** The desktop keeps a websocket open (`clients/desktop/src/events.rs`) and gets
  `NodeCreated`, `NodeUpdated`, `NodeDeleted` and `TrashChanged`. The server pings every 15 s.
- **Change journal.** `GET /changes?since=` pages with a cursor, returns the current cursor without
  `since`, answers `410 Gone` once the cursor is pruned, and reports a move to the bin as `Deleted`.
  Node ids survive moves and renames.
- **Listing.** `GET /nodes?parent_id=` returns a folder's children in one answer, without paging.
- **Content.** `GET /files/{id}/content` honours a single `Range` (`server/crates/api/src/routes/content.rs`).
  Uploads go through the resumable `/uploads` API, which needs the total `size` up front. `node_id`
  replaces a file's content, `keep_both` picks a free name instead of adding a version, and
  `modified_at` older than the current file's `updated_at` archives the upload as a version instead
  of making it current.
- **Versions.** `Node.version` goes up on every content change only. `Node.sha256` is the content
  hash. There is no precondition on uploads or renames (no `If-Match` / expected version).
- **Recycle bin.** A per-user hidden folder `.recycle-bin-{id}`. `DELETE /nodes/{id}` moves to the
  bin, `POST /trash/restore` brings back (with an `on_conflict`), `DELETE /trash/{id}` purges.
- **Quota.** Uploads over the quota answer `507` with `quota_exceeded` (PR #65, merged).
  `GET /storage` reports usage.
- **Thumbnails.** The server has no thumbnail endpoint yet.
- **sync-core** already has resumable downloads into `.part` files (`apply::part_path`,
  `apply::read_range`), the upload queue with saved upload sessions, and a SQLite store
  (`rusqlite`, bundled). Its tree logic is for full mirrors and is not reused as is.
- **CI.** `flatpak.yml` builds the Flatpak on PRs that touch the desktop client; `release.yml`
  builds the packages on tags. There is no workflow that runs the Rust tests on PRs.

## Options

Linux has no system API for cloud placeholders like the File Provider or the Cloud Files API, so the
client has to supply a file system itself or plug into one file manager.

| Option | What it is | Verdict |
|---|---|---|
| **FUSE (`fuser` crate)** | A user-space file system mounted at a folder, served by the app. Every app and file manager sees it as a normal folder. | **Recommended.** Works everywhere, all in Rust, full control over caching. |
| GVfs backend | A `gvfsd-*` daemon for GIO (GNOME). | No stable API for third-party backends (they live in the GVfs tree, in C). Only GIO apps see it natively; others get the slow `~/.gvfs` bridge. |
| KIO worker | A `kio-*` plugin for KDE. | C++/Qt, KDE apps only; non-KDE apps get a temporary copy, so edits are lost or uploaded late. |
| `rclone mount` | rclone's FUSE mount with its VFS cache (`--vfs-cache-mode full`). | Needs a Home Space backend in rclone (Go, upstream) or a WebDAV server we don't have, plus a second login. Its cache design is worth copying. |
| Placeholder files + fanotify pre-content events | Sparse files on the real disk, filled when a reader is blocked by fanotify (Linux 6.14+). | Needs `CAP_SYS_ADMIN`, so a privileged daemon; too new for current LTS distros. Worth a look again in a few years. |
| Folder sync only | What exists. | Doesn't meet "no space until opened". |

Both GVfs and KIO would also need to be written twice, once per desktop, and would still leave
terminals, IDEs and Electron apps outside. FUSE covers them all with one implementation.

## Proposal

### Architecture

```
clients/vfs-core/            new crate, no OS code: metadata mirror, cache, fetch, upload queue,
                             change journal, errors; shared with the Windows plan
clients/desktop/src/vfs/
  mod.rs                     starts and stops the mount, wires websocket events, status for the UI
  linux.rs                   the FUSE adapter (fuser::Filesystem), cfg(target_os = "linux")
```

- **One process, the app's own session.** The FUSE server runs inside the iced app, on its own
  thread (fuser's session loop) and answers through the app's tokio runtime. fuser's reply objects
  can be sent to another task, so slow calls (a download, a listing) don't block the loop. Unlike
  macOS, nothing else is started by the system, so there is no second process, no second session
  and no token hand-over.
- **The app keeps running in the background** while the mount is on. Closing the window hides it;
  a tray icon (StatusNotifierItem through `ksni`) offers Open, Pause and Quit. Quitting unmounts.
  An optional autostart entry (`~/.config/autostart`, or the Background portal in the Flatpak)
  mounts at login. This is the same open question as the macOS menu-bar mode.
- **`vfs-core` does the work and is tested without FUSE**, with `wiremock` like the other clients.
  The FUSE adapter only translates inodes, file handles and errors. The Windows Cloud Files adapter
  uses the same core.
- **Mount point:** `~/Home Space` by default, changeable in settings. Mount options
  `fsname=homespace,subtype=homespace,default_permissions`, so `mount` and `df` show
  `fuse.homespace`. The app adds a bookmark to `~/.config/gtk-3.0/bookmarks` (Nautilus, Thunar,
  Nemo, the GTK file chooser) and `~/.local/share/user-places.xbel` (Dolphin, KDE file dialogs), so
  it shows in the sidebar, and removes them on sign-out.
- **Runtime need:** `fusermount3` (package `fuse3`, installed on most desktops). fuser is built
  without its `libfuse` feature, so nothing else has to be linked. The app checks for `/dev/fuse`
  and `fusermount3` and explains what to install when they are missing.

Rejected inside the FUSE option:

| Option | Why not |
|---|---|
| A separate `home-space-vfs` daemon (systemd user service) | Needs the second-session endpoint and a token hand-over like macOS, and an IPC channel to the app for status. Keep it for later if users want the mount without the app. |
| The high-level libfuse API in C, or `fuse3` / `fuse-backend-rs` crates | fuser is the maintained pure-Rust choice, has notify support (cache invalidation), and is used by other Rust file systems. |
| Mounting inside the folder-sync root or reusing `sync-core`'s tree | Different jobs: sync mirrors whole folders on disk; the mount mirrors metadata only. Sharing the store would couple two state machines. |

### Sessions and storage

- The mount uses the app's `Client`, so it shares the refresh logic and the Secret Service entry.
  A session ended elsewhere (the websocket listener gets `None`) puts the mount in "signed out":
  cached files stay readable, everything that needs the server answers `EACCES`, and the app shows
  "Sign in again". Sign-out unmounts after pending uploads are finished or the user confirms
  dropping them.
- **State** lives under `$XDG_DATA_HOME/home-space/vfs/{account}/` (the account is a hash of the
  server URL and user id, as for the macOS domain):
  - `meta.sqlite`: the mirror of nodes (id, parent, name, kind, size, mtime, version, sha256), the
    inode table, the `/changes` cursor, pins, and pending uploads with their upload ids.
  - `cache/`: content, one sparse file per node id (`{node id}.{version}`), plus a bitmap of the
    ranges that have arrived.
- The cache goes under the data folder rather than `$XDG_CACHE_HOME`, because it holds unsynced
  edits that cleaning tools must not delete. Clean, unpinned content is still evicted by the app.

### Items

| Home Space | FUSE |
|---|---|
| The user's root folder | inode 1, the mount point |
| A node | an inode from the inode table (stable across mounts), mapped to the node id |
| `.recycle-bin-{id}` | hidden in phase 1; the trash folder `.Trash-{uid}` in phase 4 |
| `version` | `generation` of the inode and part of the cache file name |
| `size`, `updated_at`, `created_at` | `st_size`, `st_mtime`, `st_ctime` (and `st_birthtime` via statx when fuser offers it) |
| downloaded bytes | `st_blocks`, so `du` and file managers' "size on disk" show 0 for files not downloaded |
| `GET /storage` | `statfs`: total = quota (or server free space), free = what is left |
| Lost & Found | read-only (mode without `w`), no create inside, as the clients already do |
| state for badges | extended attribute `user.homespace.state` (`cloud`, `partial`, `local`, `pinned`, `uploading`, `error`) |

Permissions are the user's own (`uid`/`gid` of the app, `0644`/`0755`, `0444`/`0555` for read-only
nodes). No `allow_other`, so other users and root can't read the mount.

Items created on the PC get a local inode at once and a node id once the server has them; the
inode table maps both, so open handles keep working across the switch.

### Enumeration and changes

- **Listing:** `readdir` and `lookup` are answered from `meta.sqlite`. A folder never listed is
  fetched with `GET /nodes?parent_id=` the first time it is opened, and marked as known. With a
  server that answers without paging, a very large folder is one big request; paging is a server
  change to consider if it shows up.
- **Journal:** the cursor is taken with `GET /changes` (no `since`) before the first listing, so
  nothing between listing and journal is missed. Changes are applied with `GET /changes?since=`:
  `Upserted` updates or inserts the row, `Deleted` removes it (and its cached content, if clean).
  Rows for folders never listed are only stored when their parent is known. `410 Gone` drops the
  known-folder marks and lists again from the root; cached content is kept where `version` and
  `sha256` still match.
- **Kernel cache:** attribute and entry timeouts of 1 s, plus explicit invalidation through fuser's
  `Notifier` (`inval_entry`, `inval_inode`) when the journal changes something, so a Nautilus window
  that is open updates at once.
- **Changes made by this device** come back in the journal tagged with its device id. They are
  applied like any other; versions already match, so nothing changes.
- **Triggers:** the journal is read when the websocket reports `NodeCreated`, `NodeUpdated`,
  `NodeDeleted` or `TrashChanged` (debounced to about one second), after a reconnect, and every
  five minutes as a fallback. No push service is needed, because the app is the server's websocket
  client.

### Materialization and eviction

- **Reads by range.** A `read` fetches only the blocks it needs (1 MiB blocks, with read-ahead that
  grows on sequential reads up to 16 MiB) with `Range` into the sparse cache file. A video player or
  `file` reading a header doesn't download the whole file. A file read from start to end is
  completed in the background and its sha256 checked; a mismatch drops the cache and reads again.
- **Opening for writing** (`O_WRONLY`/`O_RDWR` without `O_TRUNC`) downloads the whole file first,
  so the upload later has all of it. `O_TRUNC` skips the download.
- **A newer version** on the server while a file is open for reading: open handles keep reading the
  old version's cache file (it stays until they close); new opens see the new one. Opens are
  answered without `keep_cache`, so the kernel's page cache doesn't serve the old bytes to a new
  open.
- **Eviction:** a cache limit (default 10 % of the disk, at most 20 GiB, changeable) is enforced by
  least recently used, only for clean, unpinned files with no open handle. "Free up space" on a file
  or folder evicts at once. Dirty files are never evicted.
- **Pinning** ("Keep on this device") downloads the file or the whole folder and keeps new files in
  it downloaded as they arrive. It is set from the file manager (see below), with
  `setfattr -n user.homespace.pinned -v 1`, or from the app's browser.
- **Thumbnails:** file managers make thumbnails by reading files, which would download every image in
  a folder that is opened. Two defences: range reads limit what is fetched for formats with embedded
  thumbnails (JPEG EXIF), and once the server has a thumbnail endpoint the app writes them into the
  shared thumbnail cache (`~/.cache/thumbnails`, freedesktop spec, keyed by the file's URI), so the
  file manager finds them and doesn't read the file. Whether GIO treats `fuse.homespace` as remote
  (Nautilus' "thumbnails for local files only") is to be checked.

### Uploads and conflicts

- **When:** a file is uploaded after the last writable handle is closed (`release`) and it has not
  changed for two seconds, so an app that closes and reopens doesn't upload twice. `fsync` uploads
  at once.
- **New files:** upload with `parent_id` + `name` and `keep_both: true`, as on macOS. If the server
  picks `name (1).ext`, the local entry is renamed to match and the kernel is told
  (`inval_entry`).
- **Edits:** upload with `node_id`. `modified_at` is the file's mtime, so the existing two-way sync
  rule holds: the later change is current, the earlier one becomes a version. Both survive, so there
  are no conflict copies.
- **Safe-save pattern.** Most editors (gedit, LibreOffice, Kate, vim with `backupcopy=no`) write a
  temporary file and rename it over the original. A rename of a new local file over an existing
  node is turned into a content upload for that node (`node_id`), not a delete and a create, so the
  file keeps its id, shares and versions.
- **Temporary files** are kept local and never uploaded: `.~lock.*#`, `*.swp`, `*~`, `.goutputstream-*`,
  `.#*`, `*.part`, `*.crdownload`, `.directory`, `.Trash-*` (until phase 4). The list lives in
  `vfs-core`, next to the one Windows will need.
- **Stale base:** same as macOS. An optional `if_version` on `CreateUpload` would make it explicit;
  proposed as a server change shared with the macOS plan, not needed for phase 1.
- **Metadata:** `rename` maps to `PATCH /nodes/{id}` (same folder) or `POST /nodes/move`. A name
  that exists on the server answers `EEXIST` unless the rename is the safe-save pattern above.
  `mkdir` calls `POST /folders` and waits for the answer, so the inode has a node id at once.
- **Offline:** writes and creates go to the cache and are queued; listings and cached files keep
  working; opening a file that is not cached answers `EIO` (and the app shows "offline"). The queue
  is in `meta.sqlite`, so it survives a restart, and uses the resumable `/uploads` with the saved
  upload id.
- **Errors after close.** An upload fails after the app that wrote the file got its `close()`.
  Most apps ignore `close()` errors anyway, so failures are shown by the app (a desktop
  notification through the Notification portal, and the `error` badge), and the file stays dirty and
  is retried.

### Recycle bin

- **Phase 1:** `unlink` and `rmdir` call `DELETE /nodes/{id}`, so a delete always goes to the
  server's bin, never away for good. The mount refuses `mkdir .Trash-{uid}` at its root, so GIO and
  KIO can't make a trash folder there and the file manager asks "Delete permanently?". That message
  is wrong for Home Space (the server still keeps it), the same compromise as phase 1 on macOS.
- **Phase 4:** the mount serves `.Trash-{uid}` itself, as the freedesktop trash spec's per-volume
  trash, backed by the server bin:
  - `files/` lists the bin (`GET /trash`), `info/` holds `.trashinfo` files made from each item's
    original path and deletion date.
  - Moving an item into `files/` is `DELETE /nodes/{id}`; the `.trashinfo` the file manager writes
    is accepted and kept in `meta.sqlite`.
  - Restore (moving it out) is `POST /trash/restore`, and emptying the trash is `DELETE /trash/{id}`.
  - Nautilus and Dolphin then show these items in their Trash and offer Restore, with no changes on
    the server. The journal's `Deleted` for a move to the bin is fine here, because the trash
    folder is listed from `/trash`, not from the journal.

### Storage quota

- `statfs` reports the space left from `GET /storage`, so file managers warn before copying too
  much.
- A `write` that would take the user over the quota known from `GET /storage` answers `ENOSPC` at
  once, so the app writing gets a real error.
- An upload answered with `507 quota_exceeded` keeps the file dirty with the `error` badge and shows
  "Storage full" in the app and as a notification. After `TrashChanged` or a purge, uploads are
  retried.

### Errors

| Server | FUSE (errno) |
|---|---|
| unreachable, 502/503/504, on a read that is not cached | `EIO` (the app shows offline) |
| 401 after refresh fails, session ended | `EACCES` |
| 404 | `ENOENT` (and the row is removed) |
| 409 `conflict` on a name | `EEXIST` |
| 507 `quota_exceeded` (known before the write) | `ENOSPC` |
| 413 over the upload limit | `EFBIG` |
| 410 on `/changes` | not an error: list again |
| write in Lost & Found or read-only node | `EROFS` / `EACCES` |
| anything else | `EIO`, logged with the server message |

### File manager integration

- **Sidebar:** the bookmarks above. The mount also shows in Nautilus "Other Locations" and in
  Dolphin's "Devices" as a FUSE mount; the bookmark gives it the name and icon.
- **Badges (phase 4).** File managers don't show FUSE state on their own. Both plugins only read the
  `user.homespace.state` xattr, so they need no connection to the app:
  - **Nautilus:** a small extension with `libnautilus-extension-4` (`InfoProvider` for emblems,
    `MenuProvider` for "Keep on this device" and "Free up space", which set the xattr). Written in C
    and shipped as `libnautilus-homespace.so`, or in Python with `nautilus-python` if that package is
    acceptable as a dependency. Recent Nautilus versions show fewer emblems than before; which
    views still show them is to be checked.
  - **Dolphin:** a `KOverlayIconPlugin` (C++/Qt, KIO) for the badges, and a service menu
    (`~/.local/share/kio/servicemenus/homespace.desktop`, no code) running
    `home-space --pin %F` / `--free-up %F` for the actions.
  - **Others (Thunar, Nemo, Caja):** the actions through their custom-action files later; no badges.
- **The app's own browser** shows the same states for every file, since it reads `meta.sqlite`.
- **Command line:** `home-space status|pin|free-up <path>` talk to the running app through the
  xattrs, so scripts and file managers without plugins can use them.

### Relation to folder sync

The mount and `sync-core` folder mappings do the same job in different ways. Both stay:

- The mount is for browsing everything with little disk use; folder sync is for folders that must
  always be complete on disk in a place the user chooses (backup tools, builds, things that dislike
  network-like file systems).
- A mapping whose local folder is inside the mount point, or a mount point inside a mapping, is
  refused, the same way `layout::conflict` refuses nested mappings today.

### Packaging and the Flatpak sandbox

- **`.tar.gz` (and later `.deb`/`.rpm`):** the app mounts directly; it needs `fuse3` installed. A
  `.deb`/`.rpm` can declare that dependency, which is a reason to add them (open question).
- **Flatpak:** a FUSE mount from inside the sandbox is not visible to the host. The app runs in its
  own mount namespace, so even with `/dev/fuse` (`--device=all`) the mount would exist only inside
  the sandbox, and `fusermount3` (setuid) is not available there. The options are:
  1. **No mount in the Flatpak** (proposed for the first release): the setting is hidden and the
     Flatpak keeps folder sync. Users who want the mount use the `.tar.gz` or a distro package.
  2. **Mount on the host through `flatpak-spawn --host`**: needs
     `--talk-name=org.freedesktop.Flatpak`, which is a full sandbox escape; Flathub accepts it only
     with a strong reason. It would run a small host-side helper, which must exist on the host, so it
     is not simpler than option 1.
  3. **A portal:** there is no cloud-files portal in xdg-desktop-portal. If one appears, the
     Flatpak would use it.
  The macOS plan's extension and this plan's mount both need the app outside a strict sandbox; on
  Linux that is the native package.
- **Signing:** nothing extra on Linux. The Flatpak bundle and tarball stay as they are.

### CI

- **New workflow `ci.yml`** for PRs that touch `clients/`: `cargo test -p vfs-core` (pure, with
  `wiremock`), and `cargo test -p desktop --features vfs-mount-tests` which installs `fuse3`, mounts
  in a temporary folder against a `wiremock` server or a real server started from `server/`, and
  checks `ls`, `cat`, `cp`, safe-save, `rm`, `du` and `df` through the kernel. GitHub's Ubuntu
  runners offer `/dev/fuse`; the job checks for it and skips with a warning otherwise.
- The mount tests can run in the cloud container only if it has `/dev/fuse`; otherwise they run in
  CI or on the PC.
- `flatpak.yml` is unchanged (the Flatpak doesn't get the mount).
- **Plugins:** the Nautilus extension and Dolphin plugin are built in a container job
  (`fedora:latest`, which has both development packages) on tags, and packaged with the `.deb`/`.rpm`.

### Phases

1. **Core.** `vfs-core`: metadata mirror, inode table, journal, range cache, eviction, upload queue,
   error mapping, all with `wiremock` tests. `ci.yml` with these tests. Shared with Windows.
2. **Read-only mount.** FUSE adapter for lookup, readdir, getattr, open, read, statfs; mount and
   unmount from the app; websocket-driven updates; bookmarks. Writes answer `EROFS`. Behind a
   "Show Home Space as a folder" setting, off by default, not in the Flatpak.
3. **Writes.** create, write, safe-save rename, rename, mkdir, unlink/rmdir to the server bin,
   offline queue, quota and errors, the tray icon and background mode, notifications.
4. **Polish.** The `.Trash-{uid}` trash backed by the server bin, pinning, Nautilus and Dolphin
   plugins, `home-space` CLI actions, thumbnails once the server has them, the `if_version`
   precondition, autostart, then default on for native packages.
5. **Packages.** `.deb`/`.rpm` with `fuse3` and the plugins; decide on the Flatpak (see open
   questions).

## Platform details not yet confirmed

From documentation and other projects, not tried yet:

- That fuser without `libfuse` mounts and unmounts cleanly through `fusermount3` on current
  distros (Ubuntu 24.04/26.04, Fedora, Arch), and recovers from a crashed app's stale mount
  ("Transport endpoint is not connected") with a lazy unmount at start.
- That a FUSE mount can't be made visible to the host from the Flatpak sandbox without
  `flatpak-spawn --host`, on the current Flatpak and with `--device=all`.
- Whether GIO treats `fuse.homespace` as a remote file system for Nautilus' thumbnail setting, and
  whether tumbler (Thunar) does.
- Which emblems Nautilus 47+ still shows from an `InfoProvider`, and in which views.
- That GitHub's `ubuntu-latest` runners provide `/dev/fuse` for an unprivileged mount.
- How GIO and KIO behave when `mkdir .Trash-{uid}` is refused (they should fall back to "Delete
  permanently?"), and that they accept a `.Trash-{uid}` served by the file system in phase 4.
- The fanotify pre-content events (Linux 6.14) as a later alternative to FUSE for local placeholders.

## Open questions

- **Flatpak:** leave the mount out of the Flatpak (proposed), or try `flatpak-spawn --host` and
  accept that Flathub may refuse it?
- **Distro packages:** add `.deb`/`.rpm` (and an AUR recipe) so `fuse3` and the file manager plugins
  come as dependencies, or keep the `.tar.gz` and document `fuse3`?
- **Background app:** may the app keep running after its window closes, with a tray icon and an
  autostart entry? Needed for the mount to stay; the same question as the macOS menu-bar mode.
- **Mount point:** `~/Home Space` (with a space, like the app name), `~/HomeSpace`, or ask at
  first start?
- **Cache size:** 10 % of the disk, at most 20 GiB, as default?
- **Folder sync:** keep folder mappings next to the mount on Linux (proposed), or replace them?
- **Plugin language:** Nautilus extension in C (no extra dependency) or Python (`nautilus-python`,
  less code)?
- **Precondition:** add `if_version` to uploads, or keep "later change wins" everywhere? (Same
  question as macOS; decide once.)
- **Trash:** is phase 1's "Delete permanently?" prompt acceptable until the trash folder lands?
- **Shared with me:** show files shared with the user as a folder in the mount, or leave them out
  for now? Proposed: leave out, as on macOS.
- **Order:** Linux before Windows and macOS (it can be built and tested in CI and on the
  developer's PC without accounts or signing), or follow the macOS plan's order?
