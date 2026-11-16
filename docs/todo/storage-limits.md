# Optional upload limit, free-space check and storage usage

**Status (2026-11-16):** half done. `GET /storage` returns the user's usage split into files,
versions and the recycle bin, and the disk's total and available bytes (`statvfs`, Unix only). The
web client shows it in the sidebar and in Settings. Quotas from `admin-panel.md` are enforced.

Still open: the upload limit is still on by default (`HOMESPACE_MAX_UPLOAD_BYTES`, 10 GiB), there
is no free-space reserve and no disk check on upload, the desktop and mobile clients don't show
usage, and no client warns before an upload that will not fit.


## Problem

`HOMESPACE_MAX_UPLOAD_BYTES` defaults to 10 GiB (`server/crates/app/src/config.rs`), so a file over
10 GiB is refused even when the disk has room. The limit dates from single-request uploads. Chunked
uploads (`docs/chunked-upload.md`) no longer need it to keep requests small. It also protects the
disk badly: many files below the limit can still fill it, and nobody can see how much space is
left.

## Proposal

### Optional upload limit

- `max_upload_bytes: Option<u64>` in the config and in `FileService`. When it is unset, there is no
  per-file limit.
- When it is set, it is still checked against `size` when a chunked upload starts, and while a
  single-request upload streams.
- Remove the default from `compose.yaml`, `server/.env.example`, the README and
  `docs/chunked-upload.md`, and document "unset means no limit".

### Free-space check

- Read the free space of the filesystem that holds `HOMESPACE_STORAGE_ROOT` (`statvfs`, e.g. through
  the `fs4`/`sysinfo` crate).
- Add `HOMESPACE_MIN_FREE_BYTES` (e.g. 1 GiB) as a reserve that uploads may not use, so the
  database and the OS keep some room.
- `POST /uploads`: refuse with 413 (or a new `InsufficientStorage` / 507 code) when
  `size - offset` would go past the reserve. Unfinished uploads have already claimed space for
  their remaining bytes, so subtract it too. Clients then fail at once, not at 95%.
- Single request (`POST /files`, `PUT /files/{id}/content`): the size is unknown up front, so
  `BlobStore::stage` checks free space every few MiB while writing and stops at the reserve.
- `PATCH /uploads/{id}`: check before appending, in case the disk filled up since the upload
  started.

### Storage usage in the UI

- New `GET /api/v1/storage` (or fields on `/me`):
  - `used`: the user's bytes, meaning current files, versions and the recycle bin, each shown
    separately so a user can see what emptying the bin or deleting versions would free.
  - `free`: the bytes still available on disk after the reserve.
  - `total`: the filesystem size.
  - `max_upload`: the limit when one is set.
- `used` comes from a `SUM(size)` over `nodes` and `file_versions` for the owner. Keep it cheap
  with an index, or with a per-user counter updated in the same transactions as uploads and
  deletes.
- Web (`clients/web`): a usage bar in the sidebar or the user menu ("12.4 GB used · 1.8 TB
  free"), with the breakdown on hover. Refresh it on `node_created` / `node_deleted` /
  `trash_changed` events.
- Desktop (`clients/desktop`): the same figures in the browser view and the settings.
- Before uploading, both clients compare the selected files' total size with `free` and warn
  before sending anything.

## Open questions

- Should there be per-user quotas (`users.quota_bytes`) on top of the disk check? Not needed for
  one household, but the usage endpoint would already provide the numbers.
- Blobs are not deduplicated across users today. If they ever are, "used" per user and the space
  actually taken on disk will differ.
