# Send files and folders to a device

> **Status:** the server part is done and documented in
> [../send-to-device.md](../send-to-device.md). It differs from the proposal below where the
> decisions at the end say so. Still to do: the shared client code (`api-client`, `client-core`),
> then the desktop, web and mobile clients, and the zip route for the web client.

## Problem

There is no quick way to hand a file to a specific device: "put this PDF on the laptop", "send
these photos to my partner's phone". Today the sender has to upload into their own tree, and the
other device has to find it, or the sender has to make a share link
([sharing.md](sharing.md)) and pass it on through some other channel.

## How it differs from sharing

| | Sharing ([sharing.md](sharing.md)) | Send to device (this item) |
|---|---|---|
| Target | A user, a group, or anyone with the link | One device, of the sender or of another user |
| Source | A node in the owner's tree | A node on the server **or** files from the local disk that are not on the server |
| Data | A live view: the share follows the node's current contents | A copy taken at send time: later edits or deletes of the source do not change it |
| Delivery | Pull: the recipient opens the link or "Shared with me" | Push: the target device gets a notification |
| Lifetime | Until revoked or `expires_at` (optional, may be never) | Until downloaded, at most 24 hours |
| Storage | No extra bytes, it points at the node | Its own temporary copy, deleted after download or expiry |

The two features share no tables. Sending does not need sharing to exist first.

## Flow

1. The user picks data: files or a folder from the local disk, or nodes from their tree on the
   server.
2. The user picks a target device. The picker lists their own other devices first (signed-in ones
   on top), and offers "Another user's device…", which lists other users and their devices.
3. The user presses Send. The data is copied to a temporary place on the server.
4. When the copy is complete, the target device gets a notification: "Laptop (Dana) sent you
   3 files, 12 MB".
5. The recipient downloads. After the download has been confirmed, the server deletes the copy.
6. Anything not downloaded within 24 hours is deleted automatically.

## Proposal

### Schema

```sql
CREATE TABLE transfers (
    id                UUID PRIMARY KEY,
    sender_id         UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    sender_device_id  UUID,                 -- NULL for clients without a device
    recipient_id      UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    recipient_device  UUID NOT NULL,        -- devices is keyed (user_id, id)
    state             TEXT NOT NULL CHECK (state IN ('uploading', 'ready', 'downloaded',
                                                     'cancelled', 'expired')),
    total_bytes       BIGINT NOT NULL,
    item_count        INT NOT NULL,
    message           TEXT,                 -- optional note from the sender, later
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    ready_at          TIMESTAMPTZ,
    expires_at        TIMESTAMPTZ NOT NULL, -- created_at + retention
    finished_at       TIMESTAMPTZ,          -- downloaded, cancelled or expired
    FOREIGN KEY (recipient_id, recipient_device) REFERENCES devices (user_id, id)
);

CREATE TABLE transfer_items (
    id           UUID PRIMARY KEY,
    transfer_id  UUID NOT NULL REFERENCES transfers (id) ON DELETE CASCADE,
    path         TEXT NOT NULL,             -- relative, '/'-separated: 'Photos/2026/a.jpg'
    kind         TEXT NOT NULL CHECK (kind IN ('file', 'folder')),
    size         BIGINT NOT NULL DEFAULT 0,
    sha256       BYTEA,                     -- set once the bytes are in
    mime         TEXT,
    modified_at  TIMESTAMPTZ,
    UNIQUE (transfer_id, path)
);
```

- A folder becomes one item per file plus one per folder, so empty folders survive. The sender
  sends the whole manifest up front, so `total_bytes` and `item_count` are known before any byte.
- `path` is checked on the server: no `..`, no leading `/`, no empty or `.` segments, no `\`, and
  the usual name rules of nodes. The recipient checks it again before writing to its disk.
- The finished rows stay for 30 days (like expired shares) so the sender can see "downloaded"
  and when. A background task removes older rows.

### Storage

- Bytes live in `<storage_root>/transfers/<transfer_id>/<item_id>`, outside `blobs/`, so they
  never mix with the users' trees, versions or the recycle bin, and a whole transfer is removed
  with one `remove_dir_all`.
- **Local data** goes up with the existing chunked upload protocol (`docs/chunked-upload.md`).
  `POST /uploads` gets an optional `transfer_item_id` in place of `parent_id`/`node_id`. The
  last chunk then moves the partial file into the transfer directory instead of calling
  `FileService::finish_store`. Resume, retry and the 24 h idle expiry of uploads work unchanged.
- **Server data** is copied on the server, without a round trip through the client. Blobs are
  replaced by `rename` (`BlobStore::commit`), never written in place, so a hard link
  (`std::fs::hard_link`) to `blobs/<owner>/<node>` is already a stable snapshot that costs no
  space. Fall back to a real copy when the link fails (e.g. `transfers/` on another filesystem).
  Only nodes the sender owns can be sent; binned nodes cannot.
- Mixed selections (some local, some server) are fine: server items are linked at once, local
  items are uploaded, and the transfer turns `ready` when every file item has its `sha256`.
- Transfer bytes do not count towards the sender's or the recipient's storage. They do count
  towards the free-space reserve of [storage-limits.md](storage-limits.md): `POST /transfers`
  refuses with 507 when `total_bytes` of local items would go past it.
- New setting `HOMESPACE_MAX_TRANSFER_BYTES` (unset means no limit), checked against
  `total_bytes` when the transfer is created.

### Retention

- `HOMESPACE_TRANSFER_RETENTION_HOURS`, 24 by default. `expires_at` is set when the transfer is
  created, so a slow upload eats into the recipient's time. (Alternative: count from `ready_at`.
  Counting from creation keeps a stuck upload from holding space for longer than 24 hours.)
- A task next to `expire_uploads` in `server/crates/app/src/main.rs` runs hourly. It marks
  transfers past `expires_at` as `expired`, removes their directory and cancels their unfinished
  uploads. It also removes directories under `transfers/` that have no row (crash leftovers),
  like `BlobStore::sweep` does for `tmp/`.
- Download routes answer 410 Gone for an expired, cancelled or downloaded transfer.

### API

| Method | Path | Purpose |
|---|---|---|
| GET | `/transfer-targets` | Devices that can receive: the caller's own other devices, then other users with their devices (`{user: {id, username}, devices: [{id, name, platform, signed_in}]}`) |
| POST | `/transfers` | `{recipient_id, recipient_device, items: [{path, kind, size?, node_id?, mime?, modified_at?}]}` → `{id, items: [{id, path, upload_needed}]}`. Items with `node_id` are linked at once. |
| POST | `/uploads` | Existing, plus `transfer_item_id` for each local file |
| GET | `/transfers/outgoing` | The caller's sent transfers with their state |
| DELETE | `/transfers/{id}` | Sender cancels (also deletes the bytes) or recipient declines |
| GET | `/transfers/incoming?this_device=true` | Ready transfers for the calling device (also catches up after being offline) |
| GET | `/transfers/{id}` | Metadata and item list (sender or recipient) |
| GET | `/transfers/{id}/items/{item}/content` | Download one file; supports `Range` |
| GET | `/transfers/{id}/archive` | The whole transfer as a streamed zip (for the web client) |
| POST | `/transfers/{id}/received` | Recipient confirms; the server deletes the bytes and marks it `downloaded` |

- **Who may download**: the recipient user, from the recipient device. The device is known from
  `AuthUser.device_id`. Other devices of the recipient see the transfer as "sent to Phone" and
  cannot fetch it. (See open questions.)
- **Deleting only after a confirmed download**: the server does not delete when a response body
  ends, because a dropped connection looks the same as a finished one. The client verifies each
  file's SHA-256 (returned in the item list), then calls `/received`. Downloads can resume with
  `Range` until then. For the zip route, the web client calls `/received` after the browser
  reports the download as finished; if it cannot tell, it asks "Did the download finish?" first.
- Every lookup checks that the caller is the sender or the recipient. A transfer of another pair
  answers 404, like nodes of another owner.
- The device sign-out (`DELETE /devices/{id}`) cancels transfers still waiting for that device,
  and cancels transfers it was still uploading.

### Notifications

- `Hub` is per user, so the event goes to the recipient user's sockets and carries the device:
  `Event::TransferReceived { transfer: TransferSummary }`, with `recipient_device` in the summary.
  A client that is not that device ignores it, or only refreshes an "incoming for other devices"
  count.
- `Event::TransferChanged { id, state }` goes to the sender (downloaded, declined, expired) and to
  the recipient (cancelled by the sender, expired), so lists update live.
- A device that was offline asks `GET /transfers/incoming?this_device=true` after connecting and
  after each `Resync`, so it does not depend on having been connected when the event went out.
- These are not node changes, so nothing is written to `node_changes` and sync is unaffected.

### Clients

- **Shared (`client-core`, `api-client`)**: `Client::create_transfer`, `upload_transfer_item`
  (wraps `upload_resumable`), `incoming_transfers`, `download_transfer_item`,
  `confirm_transfer`, `cancel_transfer`, `transfer_targets`. A shared state machine for a
  receive (items → download each → verify → confirm), and shared labels ("Sent from Laptop by
  Dana · expires in 5 h").
- **Desktop (iced)**:
  - "Send to device…" in the file browser's context menu (server data), and a "Send files…" /
    "Send folder…" button that opens a native picker (local data), plus drag and drop onto the
    Send dialog.
  - The Send dialog: target picker (own devices, then "Another user's device…"), the size, Send.
    Progress lives in a Transfers page next to Devices, with Cancel.
  - Incoming: an OS notification (e.g. `notify-rust`) and a badge on the Transfers page. Download
    goes to `~/Downloads/Home Space/<sender device> <date>/`, keeping the folder structure, never
    overwriting (adds ` (2)`). Optionally "Download automatically from my own devices" in the
    settings. A "Save to my files" action that uploads into the recipient's own tree can come
    later.
- **Web (Leptos)**:
  - "Send to device" in the node menu and on multi-selection, and a "Send from this computer"
    button using a file input (`webkitdirectory` for folders) and drag and drop.
  - The web client is a device too, so it can receive: a toast plus an "Incoming" entry in the app
    bar. One file downloads directly, several or a folder as the zip.
- **Android**: receive and send later, together with auto-upload (see `clients/mobile/README.md`). Sending
  from the Android share sheet is the natural entry point there.

### Steps

1. Migration (`transfers`, `transfer_items`), `db::transfers`, path validation, and
   `BlobStore` support for `transfers/<id>/<item>` (link, copy, open, remove dir, sweep).
2. `POST /transfers` with server items only, download routes, `/received`, `DELETE`, expiry
   task, and the events. Tests for the authorisation checks (sender, recipient, recipient device,
   other users), 410 after expiry and after download, and that a later edit of the source node
   does not change a sent file.
3. Local items: `transfer_item_id` on `/uploads`, `ready` when the last item arrives, upload
   expiry cancels a stuck transfer.
4. `/transfer-targets`, the API client, `client-core` state and labels.
5. Desktop: send dialog, Transfers page, notifications, download into Downloads.
6. Web: send dialog, incoming list, zip route.
7. Docs: a `docs/send-to-device.md` like `docs/chunked-upload.md`, settings in the README,
   `compose.yaml` and `server/.env.example`.

## Decisions

- **Sending to other users**: each device has a visibility: `owner` (the default: only its
  owner), `hidden` (nobody, the owner included), `everyone`, or `users` (the owner plus chosen
  users). A transfer is not accepted or declined before it is sent: the recipient downloads,
  saves or declines it once it arrives.
- **Other devices of the recipient**: any signed-in session of the recipient user may download,
  save or decline. The target device is the one notified.
- **After download**: the bytes are deleted once the recipient confirms (`/received`), saves,
  or declines. The row stays until it expires.
- **Saving**: `POST /transfers/{id}/save` moves the files into the recipient's tree on the
  server, with no round trip through a client.
- **Retention**: fixed, `HOMESPACE_TRANSFER_TTL_HOURS` (24 by default), counted from creation.
  Expiry removes the row as well; there is no 30-day history.

## Open questions

- **Several targets at once**: one transfer per device (bytes linked, not copied, between them),
  or leave it out at first?
- **Quota**: transfer bytes count against nobody now. With quotas from
  [admin-panel.md](admin-panel.md), should a pending transfer count against the sender?
- **Free-space reserve**: refuse a transfer with 507 once [storage-limits.md](storage-limits.md)
  adds a reserve.
