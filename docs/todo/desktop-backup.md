# Desktop backup with point-in-time restore

> **Status (2026-11-27):** plan only. Nothing is built yet. The open questions at the end need
> answers before phase 1 starts.

## Problem

Sync keeps a local folder and a server folder equal in both directions, so a file deleted or
damaged on the laptop is deleted or damaged on the server too (the recycle bin catches deletes,
but not a bad edit, a ransomware run or a folder emptied by mistake weeks ago). The phones already
have auto-upload (`clients/mobile-core/src/autoupload`), which only sends files up and never
touches the phone. The desktop has nothing like it.

Backup gives the desktop a one-way copy of chosen local folders, kept per computer, with history,
so the user can get back a folder or the whole computer **as it was at a chosen time**.

## What the user asked for

- A new **Backup** screen in the desktop client (iced) only. Mobile keeps its own auto-upload.
- The user picks one or more local folders to back up. A backup has **no server destination**:
  it belongs to the computer (hostname) and the local path, not to a folder in the user's tree.
- Changes are watched and uploaded, like sync.
- The screen lists the user's computers that run a backup. Expanding one shows its folders with
  the local path (so two folders with the same name can be told apart) and when each was last
  updated.
- **Restore**: pick a folder or a whole computer, optionally a **time**, and a local target
  folder. The target must be **empty**, so nothing is ever overwritten.
- Built on what exists: the folder watcher and the sync engine, file versions for history, file
  download for restore.
- New: a **retention period** set by the user. Older versions, and deleted files, are removed from
  the server once they are older than it.
- **Case 1**: a file deleted locally is not moved to the recycle bin. It is marked deleted on the
  server and removed for good once the retention period has passed.
- **Case 2**: a file deleted and then created again with the same name is a **new** file. The
  deleted one stays, detached, with its own history. A restore at a time between the delete and
  the new file restores no file at that path.

## Summary of the proposal

| Piece | Choice |
|---|---|
| Where the data lives | Ordinary `nodes`, `file_versions` and blobs, under a hidden per-user folder, like the recycle bin. Reuses uploads (also chunked), versions, download, quota and the object storage plan |
| Identity | `backup_machines` (one per user and desktop device, shows the hostname) and `backup_sources` (one per machine and local path) |
| Deletes | A **tombstone** (`nodes.deleted_at`), never the recycle bin. Tombstones leave the unique-name index, so a new file with the same name is a new node: Case 2 comes for free |
| Moves and renames | A tombstone at the old path plus a server-side copy (a hard link of the blob) at the new path. No re-upload. A node's path never changes, which keeps "what was where at time T" a simple query |
| Time | Server time of each change (`content_at`, `archived_at`, `deleted_at`), never the computer's clock or the file's mtime |
| Retention | Per user, in days, or forever. A daily job removes versions replaced, and tombstones deleted, before `now - retention`. The current content of a live file never expires |
| Desktop engine | A one-way **backup mode** in `sync-core`: same scan, hashing, ignore rules, watcher, queue, retries, outage handling and upload store. A planner that compares local with base only, as nothing on the server is ever applied locally |
| Restore | `GET /backups/sources/{id}/tree?at=` lists what was alive at a time. The desktop downloads each file's version into an empty folder, verifying the hash and keeping mtimes |

## Why nodes, not a separate backup store

A separate table of path rows pointing at blobs (`valid_from`, `valid_to`) would make restore
queries slightly simpler, but it would need its own uploads, chunked uploads, quota counting,
blob removal and object storage support. Nodes already have all of that. The cost is that every
query over a user's nodes must leave backup nodes out, which the recycle bin already forced for
`trashed_at` (search, storage, duplicates, transfers, accounts). Backup adds one more filter in
the same places, kept in one helper so a new query cannot forget it (see
[Hiding backups](#hiding-backups-from-the-rest-of-the-app)).

## Server

### Schema

```sql
-- Hidden folder holding every backup of a user, like trash_id. Named `.backups-{id}`.
ALTER TABLE user_folders ADD COLUMN backups_id UUID UNIQUE REFERENCES nodes (id);

CREATE TABLE backup_machines (
    id          UUID PRIMARY KEY,
    user_id     UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    device_id   UUID,                         -- NULL once the device is forgotten
    name        TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100), -- hostname, renamable
    platform    TEXT NOT NULL,
    folder_id   UUID NOT NULL UNIQUE REFERENCES nodes (id), -- under user_folders.backups_id
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, device_id)
);

CREATE TABLE backup_sources (
    id             UUID PRIMARY KEY,
    machine_id     UUID NOT NULL REFERENCES backup_machines (id) ON DELETE CASCADE,
    local_path     TEXT NOT NULL,             -- as the OS spells it, shown to the user
    root_id        UUID NOT NULL UNIQUE REFERENCES nodes (id), -- under the machine's folder
    state          TEXT NOT NULL CHECK (state IN ('active', 'paused', 'removed')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    first_complete_at TIMESTAMPTZ,            -- first pass that left nothing to upload
    last_complete_at  TIMESTAMPTZ,            -- "Up to date as of"
    last_change_at    TIMESTAMPTZ,            -- last upload, copy or tombstone
    removed_at     TIMESTAMPTZ,
    UNIQUE (machine_id, local_path)
);

-- Per-user retention; NULL keeps everything.
ALTER TABLE users ADD COLUMN backup_retention_days INTEGER CHECK (backup_retention_days > 0);

-- Every node of a backup carries its source, so other queries can leave them out cheaply and
-- the retention job can find them.
ALTER TABLE nodes ADD COLUMN backup_source_id UUID REFERENCES backup_sources (id);
ALTER TABLE nodes ADD COLUMN deleted_at TIMESTAMPTZ;     -- tombstone, backup nodes only
ALTER TABLE nodes ADD COLUMN content_at TIMESTAMPTZ;     -- when the current content arrived
ALTER TABLE nodes ADD CONSTRAINT nodes_deleted_backup
    CHECK (deleted_at IS NULL OR backup_source_id IS NOT NULL);
ALTER TABLE file_versions ADD COLUMN valid_from TIMESTAMPTZ; -- the node's content_at when archived

-- Tombstones leave the unique-name index too, so a new file can take the name (Case 2).
DROP INDEX nodes_unique_name;
CREATE UNIQUE INDEX nodes_unique_name
    ON nodes (owner_id, parent_id, kind, name) NULLS NOT DISTINCT
    WHERE trashed_at IS NULL AND deleted_at IS NULL;

CREATE INDEX nodes_backup_source_idx ON nodes (backup_source_id) WHERE backup_source_id IS NOT NULL;
CREATE INDEX nodes_backup_deleted_idx ON nodes (deleted_at) WHERE deleted_at IS NOT NULL;
```

`content_at` and `valid_from` are set for every node and version from now on (cheap, and it
gives normal files the same timeline later), but only backup nodes rely on them. For a version,
`valid_from .. archived_at` is the time it was the file's content; for the node itself it is
`content_at .. deleted_at` (open-ended while live).

Machine and source folder nodes are named by their ids (`machine-{id}`, `source-{id}`), so
renaming a machine or two folders called `Documents` never clash. The local path lives in
`backup_sources.local_path`, not in node names.

### What "alive at T" means

```
folder or file n is alive at T  ⇔  n.created_at ≤ T  and  (n.deleted_at IS NULL or T < n.deleted_at)
content of file n at T          ⇔  the version with valid_from ≤ T < archived_at,
                                    else the current content when content_at ≤ T
```

A restore walks from the source root with a recursive query that only follows nodes alive at T.
As nodes never move, the path of each one is its chain of names. With Case 2:

| Time | Event | Nodes |
|---|---|---|
| 10:00 | `a.txt` created | A (alive from 10:00) |
| 11:00 | `a.txt` deleted | A gets `deleted_at = 11:00` |
| 12:00 | `a.txt` created again | B (alive from 12:00), unrelated to A |

Restore at 10:30 gives A, at 11:30 gives nothing, at 12:30 gives B. A is removed once
`11:00 + retention` has passed; B is untouched by that.

### Times come from the server

Every timestamp is the server's commit time. A change made while the computer was offline counts
from when it was uploaded, and a delete from when the scan noticed it. The restore screen says
"as backed up at", and a point in time means "what the server had then". The file's own mtime is
kept as the version's `modified_at` and is restored on the file, but it never decides what is
restored.

### API

All under `/backups`, behind the session like other file routes. Only the owner sees their
backups (no shares, no admin access to contents).

| Method and path | Purpose |
|---|---|
| `GET /backups/machines` | Machines with their sources: name, platform, local path, state, `last_complete_at`, `last_change_at`, file count and bytes (live only, and with history) |
| `POST /backups/machines` | Register this device as a machine (idempotent per device), or adopt an existing machine after a reinstall (`adopt: <machine id>`) |
| `PATCH /backups/machines/{id}` | Rename |
| `DELETE /backups/machines/{id}` | Remove a machine and every backup on it, now (asks to re-authenticate) |
| `POST /backups/sources` | Add `{machine, local_path}`; returns the source and its root node id. Idempotent on the pair; a removed source comes back with its history |
| `PATCH /backups/sources/{id}` | Pause, resume, report a completed pass (`last_complete_at`) |
| `DELETE /backups/sources/{id}?purge=` | Stop backing up. Without `purge` everything is tombstoned now and expires with retention; with `purge` it is deleted now |
| `GET /backups/sources/{id}/tree?at=&parent=&cursor=` | Live entries at `at` (default now), paged: node id, kind, name, size, sha256, mtime, version to download |
| `GET /backups/sources/{id}/timeline?from=&to=` | Days (or hours) with changes and their count, for the time picker |
| `GET /backups/settings`, `PUT /backups/settings` | `{retention_days}`; lowering it asks to re-authenticate |
| `POST /backups/nodes/{id}/delete` | Tombstone a file, or a folder with everything alive inside it, in one transaction |
| `POST /backups/nodes/{id}/copy` | `{parent, name}`: a new node with the same content (hard link), for local moves and renames |

Uploads and folder creation reuse `POST /uploads` (single and chunked) and folder creation with a
backup parent; the server stamps `backup_source_id` from the parent and refuses a parent from
another machine than the caller's device. A write with a parent in the backup tree **never**
renames or moves an existing node and never puts anything in the recycle bin: `DELETE /nodes/{id}`
on a backup node answers 409 and points at `/backups/nodes/{id}/delete`.

Download reuses `GET /files/{id}/content` and `GET /files/{id}/versions/{v}/content`, and their
stream tickets, which must accept backup nodes (and tombstoned ones, for restores in the past).

`last_change_at` and `last_complete_at` changes send one new event, `BackupsChanged`, so an open
Backup screen on another computer refreshes. Per `docs/versioning.md`, `features.rs` gains
`BACKUPS` ("Desktop backup") in `ALL`. The desktop does **not** add it to `REQUIRES`: on an older
server it hides the Backup screen instead.

### Hiding backups from the rest of the app

Backup nodes must not show up in the tree, `/changes`, websocket node events, search, duplicates,
"Edited on", shares, transfers or sync. The hidden root keeps them out of listings; every query
that scans a user's nodes adds `backup_source_id IS NULL` next to its existing
`trashed_at IS NULL`, through one shared SQL fragment. The change journal trigger skips rows with
a `backup_source_id`, as the backup engine never reads `/changes` (it is the only writer of its
source, so its own store knows the state). A test lists every node query and fails when a new one
neither filters nor is on an allow-list.

### Retention job

A daily job in `server/crates/app/src/jobs.rs`, next to the change-journal prune. For each user
with a retention period, `cutoff = now - retention`:

1. Delete versions of backup nodes with `archived_at < cutoff`, then their blobs.
2. Delete backup nodes (files, then folders) with `deleted_at < cutoff`, with their versions and
   blobs.
3. Delete `removed` sources whose nodes are all gone, and machines with no sources left and no
   device.

Blobs are removed after the transaction commits, as `remove_version_blob` does today. Work goes in
batches so a big purge does not hold a long transaction.

The current content of a live file never expires, however old. So the earliest point a restore
can promise is `max(first_complete_at, now - retention)`, and the picker does not go earlier.
Lowering the retention takes effect at the next run, which the settings dialog warns about.

Normal (non-backup) file versions keep today's behaviour. Retention for them is a separate
decision (see the open questions).

### Quota and storage

Backup content and history count toward `users.quota_bytes` like files, versions and the bin
(storage-quota-feature): an upload that would not fit answers 507 `quota_exceeded`, which the
backup shows as "Backup paused: storage full". The storage page gets a **Backups** line per
machine. Copies made for moves are hard links, but quota counts their size each time (the same
rule as today's shared copies); counting them once is an open question.

## Desktop

### Backup engine (`sync-core`, backup mode)

The two-way engine compares remote, local and base. Backup only needs local against base, as
nothing on the server is ever brought down. The plan adds a `Mode::Backup` (or a small
`backup` module) that reuses:

- `scan` (hashing, `is_ignored`, unreadable files), the `notify` watcher behind `Platform::watch`,
  the settle and five-minute rescan;
- `queue` (four transfers, retries with backoff, outage after three unreachable failures);
- `store` (bases and saved chunked uploads, so a big file resumes after a restart) in its own
  database, `backup-{user}.db`;
- `apply` for uploads.

Its planner turns differences into:

| Local change since the base | Action |
|---|---|
| New file or folder | Create folder / upload into the matching parent |
| File content changed | Upload as a new version of the same node |
| File or folder gone | `POST /backups/nodes/{id}/delete` |
| Moved or renamed (same hash and size, as the sync move detection finds it) | `copy` to the new path, then delete at the old one |
| Only mtime changed | Nothing on the server; the base is updated |

After a pass that leaves nothing queued, the engine reports `last_complete_at`. A source whose
folder is missing (an unplugged disk) pauses with "Folder not found" instead of tombstoning
everything; the same guard that sync uses for a missing root. A pass that would delete more
than a threshold (say half the files or 1,000) asks first, which protects against a folder
emptied by mistake.

Rules for sources: a source may not contain or be inside another source of the same machine, nor
the app's data dir (`protected_dir`). It may overlap a synced folder; the two run independently.
Symlinks are skipped, as in sync. Permissions and extended attributes are not kept.

The desktop runs it as a second iced subscription next to `sync::subscription`, with its own
status (queued, uploading, last error) for the screen and the tray.

### Backup screen

A new sidebar entry **Backup**, between Devices and Trash.

- **This computer** at the top: its folders, each with the local path, state (up to date,
  backing up 1,204 files, paused, storage full, folder not found), "Up to date as of 14:02",
  size, and buttons Pause, Remove and Restore. **Add folder** opens the system folder picker.
- **Other computers** below, one collapsible row per machine (name, platform, last activity).
  Expanded, it lists folders with name, full local path, last updated and size, and Restore on
  the machine and on each folder.
- **Settings** (a small dialog from the header): "Keep deleted files and old versions for"
  7, 30, 90, 365 days or Forever.
- Removing a folder asks: "Keep its history for the retention period" (default) or "Delete it
  now". Removing the last folder does not remove the machine.

### Restore dialog

1. **What**: the machine (all its folders) or one folder, preselected from where Restore was
   pressed. An optional "Choose items…" browser of the tree at the chosen time restores a
   subfolder or single files (later phase).
2. **When**: "Latest" (default) or "At a time": a date field and a time field in local time,
   limited to the earliest guaranteed point and now. The timeline endpoint marks days that had
   changes. iced has no date picker, so this is a small calendar widget of our own.
3. **Where**: a folder picker. The dialog checks the folder is empty (or does not exist yet and
   is created) and refuses otherwise, with "Restore only into an empty folder".

A machine restore puts each source in its own subfolder named after the local path with
separators replaced (`C_Users_ana_Documents`, `home_ana_Documents`), so two `Documents` folders
never meet. A single folder restore writes its contents straight into the target.

The restore downloads through the transfers code: files go to `name.homespace-part` and are
renamed when the hash matches, with the version's `modified_at` set as mtime. Files are opened
with `create_new`, so even if something appears in the target meanwhile it is never overwritten:
that file is reported as skipped. Progress and a final report (restored, skipped, failed, with
retry) show in the transfer list. A restore that stops (app closed) can continue into the same
folder, as it only fills in what is missing (later phase; until then, start again in a new
folder).

Restoring onto another computer works the same way, as nothing ties a restore to the machine
it came from.

## Phases

1. **Server**: schema, hidden folder, machines and sources, tombstones, copy, tree at T,
   timeline, settings, filters on every node query, retention job, `BACKUPS` feature, tests.
2. **Desktop backup**: backup mode in `sync-core`, subscription, Backup screen (this computer and
   other computers), settings, quota and missing-folder states.
3. **Desktop restore**: restore dialog, time picker, empty-target rule, download with progress
   and report.
4. **Later**: choose items to restore, resume a stopped restore, exclude patterns, Windows Volume
   Shadow Copy for files open in other apps, read-only backup view on the web, moving mobile
   auto-upload onto the same machines and sources.

## Testing

- Server (`server/crates/api/tests/backups.rs`): Case 1 (delete then purge after retention, with a
  fake clock), Case 2 (tombstone and new node, restore at the three times), move as copy plus
  tombstone, tree at T across versions, lowering retention, quota 507, a device writing into
  another machine's source is refused, backup nodes absent from listings, `/changes`, search and
  duplicates.
- `sync-core` e2e for backup mode: create, edit, delete, rename, missing root, mass delete guard,
  resume of a chunked upload.
- Desktop: restore into an empty folder, refusal of a non-empty one, a file that appears during
  restore is skipped, mtimes kept.
- A `docs/specs/desktop-backup.feature` describing the screen, like `mobile-sync.feature`.

## Open questions

1. **Retention scope.** Per user (as asked) or also per machine or folder? Per user is simpler;
   per folder would let `Downloads` keep 7 days and `Documents` a year.
2. **Default retention.** 30 days, 90 days or Forever for a new user? And a minimum (1 day)?
3. **Normal files.** Should the same retention period also clean old versions of normal (synced)
   files, which today are kept forever? The plan leaves them alone.
4. **Quota.** Do backups count toward the quota (the plan says yes), and should a moved file's
   hard-linked copy count once or twice?
5. **Machine identity.** Machines are keyed by the desktop's device id, shown by hostname. After a
   reinstall the device id is new: is "Continue an existing backup" (adopt a machine) on first
   use enough, or should the hostname alone match automatically? Hostname matching is easier but
   wrong when two computers share a name.
6. **Removing a folder.** Keep history until retention by default (the plan) or delete at once?
7. **Mass delete guard.** Ask before a pass that deletes many files: which threshold, and should
   it be on by default?
8. **Restore layout.** For a whole machine, is one subfolder per source named after its path
   right, or should it rebuild the full path (`C/Users/ana/Documents`)?
9. **Restore source.** Desktop only, or should the web also be able to browse backups and
   download a folder as a zip (the archive endpoint exists)?
10. **Time picker.** Is a date and time field enough, or do you want a list of "backup points"
    (each completed pass) to pick from?
11. **Mobile.** Should mobile auto-upload later move onto machines and sources, so the phone shows
    up in the same list, or stay separate as you said?
12. **Server-side cleanup of a forgotten device.** When a device is removed from Devices, should
    its machine's backups stay until removed by hand (the plan) or expire with retention?
