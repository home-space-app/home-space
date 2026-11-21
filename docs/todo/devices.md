# Registered devices

## Problem

The server does not know which device does what. Several clients of one user (two desktops, the
web client, later the Android app) look the same to it:

- An unfinished chunked upload belongs to a user, not to a device. Only the client that started it
  knows its id. A device that is signed out or lost leaves its uploads to the 24 h expiry.
- The change journal does not say which device made a change. A client re-reads its own uploads
  when it catches up, and the UI cannot say "edited on Laptop".
- `sessions` (migration `0007`) is "one signed-in device", but its id changes with every sign-in.
  It cannot name a device over time, and there is no device list for the user.

## Done

Steps 1 and 2 below (migration `0010_devices.sql`):

- **Identity.**
  - Each client makes up a UUID once and keeps it: `~/.local/share/home-space/device-id` on the
    desktop, `homespace.device_id` in localStorage on the web.
  - The client sends `device: {id, name, platform}` on `POST /auth/login` and
    `POST /auth/oidc/exchange`. The name is the host name on the desktop and the browser's name
    on the web. Older clients leave the field out and sign in as before, without a device.
- **Schema.**
  - `devices` is keyed by `(user_id, id)`, so several users of one install each get their own
    row and nobody can claim another user's device. (The first design refused an id already
    registered to another user with `409`; that would have broken switching users on one desktop.)
  - `sessions.device_id` references it. `uploads.device_id` has no foreign key: devices are only
    deleted with their user.
- **Sign-in.**
  - The device is registered, or marked as seen. A known device keeps the name the user gave it.
  - `last_seen_at` is updated on token refresh, at most once an hour.
  - `AuthUser` carries `device_id`, read from the session row the token check already loads.
- **Uploads.** `POST /uploads` records the device.
- **API.**
  - `GET /devices` lists the user's devices with `signed_in`, `current` and their number of
    unfinished `uploads`.
  - `PATCH /devices/{id}` renames one.
  - `DELETE /devices/{id}/sessions` ends its sessions and cancels its unfinished uploads. The row
    stays, so the device is still named in history.
  - `DELETE /devices/{id}` does the same, declines the files waiting for it and deletes the row.
  - `GET /uploads?this_device=true` lists the uploads of the calling device, to clean up after
    its local database is lost.
- **Signing out.** Signing out on the device itself (`/auth/logout`) does not cancel its
  uploads. The desktop keeps its saved upload sessions and continues them after signing in again.
- **API client.** `Client::with_device`, `devices`, `rename_device`, `sign_out_device`, `uploads`.

Step 3 (migration `0011_change_devices.sql`):

- **Tagged changes.** `node_changes.device_id` records the device behind each change.
  - API routes write through `state.files.on_device(user.device_id)`. Its transactions set
    `homespace.device_id` with `set_config(..., true)`, which lasts only for that transaction.
  - `record_node_change()` reads the setting. Writes without it (no device, background jobs)
    record `NULL`. The setting reads as `''` once a transaction that set it has ended, so the
    trigger maps `''` to `NULL`.
- **`GET /changes`** returns `device_id` on each entry: the device of the node's latest change,
  since only that one is kept after compaction.
- **Echo skipping does not use it.** Sync skips a live event when it already holds the node as
  the event shows it. Skipping by device would be wrong: the desktop's file browser also writes
  as this device, and sync has not applied those writes yet.

Step 4, the device list in the clients:

- **Desktop.** A Devices page, from the toolbar next to the recycle bin. It lists each device
  with its platform, when it was last seen, whether it is signed in and its unfinished uploads.
  A device can be renamed in place; signing one out asks first and says how many uploads it
  cancels. This device is marked and signs out with the header's Sign out instead.
- **Web.** The same list at `/devices`, opened from the username in the app bar.
- **Shared labels.** `client_core::labels` names the platforms and words the device state and
  the sign-out question for both clients.
- **macOS name.** The desktop names itself after `scutil --get ComputerName`, since macOS has
  no `/etc/hostname` and does not export `HOSTNAME`.

"Edited on" (migration `0012_edited_on.sql`):

- **Schema.** `nodes.edited_on` and `file_versions.edited_on` hold the device that wrote the
  content. A `BEFORE INSERT OR UPDATE OF sha256` trigger sets it on the node from the
  transaction's `homespace.device_id` when the node is created or its content changes, so
  renames, moves and trashing keep it. Archiving copies it to the version; content kept as a
  version because it was older than the current one is stamped with the sending device.
- **API.** `Node` and `FileVersion` carry `edited_on` (left out when unknown, so older clients
  are unaffected).
- **Clients.** The details panel shows "Edited on Laptop" ("Created on" for folders, "this
  device" for the current one) and the device after each previous version. Names come from
  `GET /devices`, loaded with the panel, so a rename shows everywhere.

## Left

- ~~**Android.**~~ Done (2026-11-16): Android and iOS register on sign-in, and their uploads show
  where they came from. Renaming and signing out devices from the phones is tracked in
  `mobile-feature-parity.md`.
- Conflict copies named after the other device do not apply: sync keeps no conflict copies,
  the server keeps the losing content as a version.

## Not in scope

- Per-device permissions or limits.
- Pushing changes from one device to a chosen other device. Every device still syncs from the
  journal. One-off sending of files to a device is planned separately in
  [send-to-device.md](send-to-device.md).
