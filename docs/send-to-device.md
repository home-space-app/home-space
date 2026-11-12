# Send to device

A user can send files and folders to one device: one of their own, or one of another user that
is visible to them. The server keeps its own copy of the bytes until the recipient has dealt with
it, and for at most `HOMESPACE_TRANSFER_TTL_HOURS` (24 by default).

Unlike sharing, a transfer is a copy taken when it is sent. Later edits or deletes of the source
do not change it, and it uses no space in either user's tree until the recipient saves it.

## Device visibility

Each device has a visibility that decides who sees it in `GET /devices/targets` and may send to it:

| Visibility | Who can send to it |
|---|---|
| `owner` (default) | Only the user it belongs to. |
| `hidden` | Nobody, its owner included. |
| `everyone` | Its owner and every other user. |
| `users` | Its owner and the users listed with it. |

`PUT /devices/{id}/visibility` takes `{visibility, users?}`. `users` holds user ids, from
`GET /users`, and is only kept for `users`. Naming the owner or an unknown user is a 400. The
owner's sockets get `devices_changed`, and so do the sockets of every user who could see the
device before the change or can see it now.

`GET /devices` reports each device's `visibility` and, for `users`, the users in `visible_to`.

## Lifecycle

```
            all files in            download + POST /received
uploading ───────────────▶ ready ─────────────────────────────▶ downloaded
    │                        │  POST /save
    │                        ├─────────────────────────────────▶ saved
    │                        │  POST /decline
    ├────────────────────────┼─────────────────────────────────▶ declined
    │  DELETE (sender)       │
    └────────────────────────┴─────────────────────────────────▶ cancelled
```

- A transfer whose files all come from the sender's tree is `ready` at once.
- Leaving `ready` or `uploading` removes the bytes and any unfinished uploads. The row stays,
  so both sides can still see what happened.
- At `expires_at` (creation time plus the TTL) the row is removed whatever its state. An hourly
  task does this, and it also sweeps directories under `<storage>/transfers/` that have no row.
  Once `expires_at` has passed, the transfer answers 410 even before the task has run.

## Making a transfer

`POST /transfers` with the whole manifest:

```json
{
  "recipient_id": "…",
  "recipient_device": "…",
  "message": "optional, up to 1000 characters",
  "items": [
    { "path": "Photos", "kind": "folder" },
    { "path": "Photos/a.jpg", "kind": "file", "size": 12345, "mime": "image/jpeg" },
    { "path": "Report", "kind": "folder", "node_id": "…" },
    { "path": "notes.txt", "kind": "file", "node_id": "…" }
  ]
}
```

- `path` is relative and `/`-separated, and every part of it follows the node name rules: no
  `..`, `.`, empty parts, `\` or control characters, and at most 255 characters. Paths must be
  unique, and every folder named in a path must also be an item of the transfer. The recipient's
  client should still check paths before writing to disk.
- An item with `node_id` takes that file or folder from the sender's tree. A folder brings
  everything live in it, under `path`. Files are hard-linked into
  `<storage>/transfers/<id>/<item>`, or copied when linking fails. Blobs are only replaced by a
  rename, so the link keeps the content as it was when the transfer was made.
- Every other file is uploaded afterwards. Each file is limited by `HOMESPACE_MAX_UPLOAD_BYTES`,
  and the whole transfer by `HOMESPACE_MAX_TRANSFER_BYTES` when that is set (413). There are at
  most 10 000 items.
- The response is `201 {id, state, items: [{id, path, upload_needed}]}`, with folders expanded.
  It is 403 when the device is not visible to the sender, and 400 for a bad manifest.

Each item with `upload_needed` is uploaded through the chunked upload protocol
([chunked-upload.md](chunked-upload.md)), by the sender only:
`POST /uploads {transfer_item_id, size}`, where `size` must equal the item's size. Resume, retry
and the idle expiry work as for any upload. The chunk that completes an item answers
`{offset}` with no `node`, and the item's bytes move into the transfer. When the last item is
in, the transfer turns `ready`.

## Receiving

Any signed-in session of the recipient user may act on the transfer. The target device is the
one notified, and the one `GET /transfers/incoming?this_device=true` filters on.

| Request | Result |
|---|---|
| `GET /transfers/incoming?this_device=` | Ready and declined transfers sent to the caller (or to the calling device), newest first. |
| `GET /transfers/{id}` | The summary plus `items: [{id, path, kind, size, sha256?, mime?, modified_at?}]`. |
| `GET /transfers/{id}/items/{item}/content` | One file. A single `Range: bytes=…` gives 206, so a download can resume. Also accepts `?ticket=` from `POST /tickets {scope: "transfer_item", transfer_id, item_id}`. |
| `GET /transfers/{id}/archive` | Every file and folder as one zip, made while it is sent: no compression, no `Content-Length` and no `Range`. Also accepts `?ticket=` from `POST /tickets {scope: "transfer_archive", transfer_id}`. A failure partway ends the response with an error instead of a cut-off zip. |
| `POST /transfers/{id}/received` | Confirms the download: the bytes are removed and the state becomes `downloaded`. Call it after checking every file's `sha256`. |
| `POST /transfers/{id}/save` `{parent_id?}` | Moves the files into the caller's tree under `parent_id` (the root by default), keeping the folders. A folder name taken at the top becomes `Name (1)`, and a taken file name becomes `name (1).ext`. Answers the nodes created at the top. |
| `POST /transfers/{id}/decline` | State `declined`. The bytes are removed and the row stays in the incoming list until it expires. |

The sender lists their transfers with `GET /transfers/outgoing` and cancels one with
`DELETE /transfers/{id}`.

Summaries in the lists and events carry `item_count` (files and folders), `file_count`, and
`name`: the path of the one file or folder at the top of the transfer, when there is only one.
`mime`: the type of that file, when the transfer is a single file, so a client can tell a list
of links from a file without fetching the items.

Lookups by anyone other than the sender or the recipient answer 404. A sender that calls a
recipient's action, or a recipient that cancels, gets 403. Content, save and received answer 409
while the transfer is still uploading, and 410 once it is finished or expired.

## Links

A link goes as a single file of type `text/uri-list` (RFC 2483: one link per line, CRLF, `#`
lines are comments), named `{device}_{date and time}.uris`. The server stores it like any other
file; old clients see a file to download.

- **Sending.** Android sends shared text as a list of links when every non-blank line is an
  absolute link (`scheme:…`, no spaces). Text with anything else in it, such as words around a
  link, goes as `text/plain` so nothing is lost.
- **Open.** A received transfer offers Open (on the row, and in the toast or notification) when it
  is one file of a type the client can open (today `text/uri-list`) and at most 50 MB
  (`client_core::uri_list::OPEN_MAX_BYTES`). The client reads it into memory, never more than
  that, and takes the first link that may be opened. The cap is for later viewers too (text,
  images); they should render with their own smaller limits rather than raise it.
- **Blocked schemes.** Any scheme may open except those that run script, read local or
  app-private files or start programs: `javascript`, `vbscript`, `data`, `blob`, `file`,
  `filesystem`, `about`, `jar`, `view-source`, `content`, `intent`, `android-app`, `chrome`,
  `chrome-extension`, `moz-extension`, `resource`, `search`, `search-ms`, `shell` and anything
  starting with `ms-`. App links such as `spotify:` work.
- **Confirmation.** Nothing opens without the user's yes. The dialog shows the host in ASCII, so a
  look-alike name reads `xn--…`, the whole link, and how many more the list holds; only the first
  opens. The page itself is never fetched for a preview.
- **Afterwards.** Opening confirms the transfer (`POST /transfers/{id}/received`), as a download
  does. Cancel leaves it waiting.

Each client hands the link to the system: the web client opens a web page in a new tab
(`noopener`) and other schemes through the browser's protocol handlers, the desktop client uses
the default handler (`open` crate), and Android an `ACTION_VIEW` intent.

## Events

On the websocket:

- `transfer_received {transfer}` goes to the recipient user when a transfer turns ready.
  `transfer.recipient_device` names the device it is for, and other clients of the user may only
  refresh what they show.
- `transfer_changed {id, state}` goes to the sender when the transfer turns ready, and to both
  sides when it is downloaded, saved, declined, cancelled or `expired`.

A client that was offline catches up with `GET /transfers/incoming` after connecting and after
each `resync`. Transfers are not node changes, so they do not appear in `/changes`.

## Settings

| Variable | Default | |
|---|---|---|
| `HOMESPACE_TRANSFER_TTL_HOURS` | 24 | How long a transfer is kept, from when it was made. |
| `HOMESPACE_MAX_TRANSFER_BYTES` | unset | The most bytes one transfer may hold. |
