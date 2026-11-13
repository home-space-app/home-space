# home-space

## Designs

The UI designs for every client live in Penpot, which is the single source of truth: the file
**Home Space Client**, with one page per platform (Desktop, Web, Android, iOS). Colours, radii,
spacing and type sizes are Penpot design tokens (token sets `core`, `mode/*`, `android/*` and
`ios/*`); switch the **Mode** theme between Light and Dark to see every page in the other
appearance. The designs are not kept in this repository.

The product site has its own Penpot file, **Web Site**: [open](https://design.penpot.app/?screen=workspace&team-id=d3aee43a-8d21-8092-8008-e8ca87f0cb91&project-id=124ca123-b2be-80ce-8008-e8cc36166a1c&file-id=d3aee43a-8d21-8092-8008-e94f65479b3a&page-id=d3aee43a-8d21-8092-8008-e94f65479b3b).

Pages:

- [Desktop](https://design.penpot.app/?screen=workspace&team-id=d3aee43a-8d21-8092-8008-e8ca87f0cb91&project-id=124ca123-b2be-80ce-8008-e8cc36166a1c&file-id=124ca123-b2be-80ce-8008-e8e7873acf90&page-id=124ca123-b2be-80ce-8008-e8e7873acf91)
- [Web](https://design.penpot.app/?screen=workspace&team-id=d3aee43a-8d21-8092-8008-e8ca87f0cb91&project-id=124ca123-b2be-80ce-8008-e8cc36166a1c&file-id=124ca123-b2be-80ce-8008-e8e7873acf90&page-id=a1e49826-f19a-8041-8008-e92a55d71661)
- [Android](https://design.penpot.app/?screen=workspace&team-id=d3aee43a-8d21-8092-8008-e8ca87f0cb91&project-id=124ca123-b2be-80ce-8008-e8cc36166a1c&file-id=124ca123-b2be-80ce-8008-e8e7873acf90&page-id=a1e49826-f19a-8041-8008-e92a486bc5c1)
- [iOS](https://design.penpot.app/?screen=workspace&team-id=d3aee43a-8d21-8092-8008-e8ca87f0cb91&project-id=124ca123-b2be-80ce-8008-e8cc36166a1c&file-id=124ca123-b2be-80ce-8008-e8e7873acf90&page-id=a1e49826-f19a-8041-8008-e92a50a1d5a8)

## Server

Rust workspace in `server/`: `db` (sqlx/Postgres), `auth` (argon2 + JWT access/refresh tokens, optional OpenID Connect sign-in),
`files` (blobs on disk + ownership-scoped tree), `search` (full-text index), `notifications`
(per-user websocket events), `api` (axum REST + `/ws`), `app` (the `homespace` binary).

```sh
cd server
cp .env.example .env            # set HOMESPACE_JWT_SECRET: openssl rand -base64 48
./scripts/db.sh up              # Postgres in podman (down | destroy | psql | logs)
cargo run -p app -- user add me --admin # prompts for a password (8-1024 characters); there is no public signup
cargo run -p app -- serve       # http://127.0.0.1:8080/api/v1
```

Administrators get the Administration page in the web client (server status, disk and storage
per user, and a Users tab to add, edit, disable and delete accounts), but no access to other
users' files. Grant or revoke the role with `user admin <username> [--revoke]`; `user list` marks
administrators, disabled users and users marked for deletion.

| Variable | Default | |
|---|---|---|
| `HOMESPACE_BIND` | `127.0.0.1:8080` | Address the server listens on (`0.0.0.0:8080` in the container). |
| `HOMESPACE_TRUST_FORWARDED_FOR` | `false` | Take the client's address from `X-Forwarded-For`. Set it to `true` behind a reverse proxy that sets the header (the [nginx example](docs/nginx/example.domain.com.conf) does): wrong passwords are counted per client address, and without it everyone behind the proxy counts as one client. Leave it off when clients reach the server directly, since they could forge the header. |
| `HOMESPACE_STORAGE` | `local` | Where file content is kept. Only `local` exists today; object storage is planned ([docs/todo/object-storage.md](docs/todo/object-storage.md)). |
| `HOMESPACE_STORAGE_ROOT` | `./data` | Directory for the local backend (`/data` in the container). |
| `HOMESPACE_PORT` | unset | Host port the container is published on. Compose maps it to the container's 8080; the server only shows it on the Administration page. |

Tests need the database running and `DATABASE_URL` set (`.env` is picked up):
`cargo test --workspace`.

### Search

Names are indexed in Postgres (`search_entries`: a weighted `tsvector` with a GIN index) when a
node is created or renamed; deleting a node removes its entry. Files in the recycle bin are not
indexed (nor is the bin itself) and are indexed again when restored. On startup the server indexes
anything missing, and `cargo run -p app -- search reindex [--all]` does the same on demand
(`--all` drops the index first).

File content is indexed through `ContentProvider`s (`server/crates/search/src/provider.rs`). None
are registered yet, so only names are searched. To add one, implement the trait (`id`, `version`,
`accepts`, `extract`) and pass it to `SearchIndex::start` in `server/crates/app/src/main.rs`.
Extraction runs in a background worker after upload or replace, and the startup/`reindex` pass
re-extracts files whose content or provider version changed. Content matches rank below name
matches.

### Names and versions

Names are unique per folder and kind (case-sensitive), so a file and a folder may share a name.
Creating a folder or renaming onto a taken name is a 409. Everything else resolves collisions:

- Uploading (or replacing) a file whose name exists: identical content (size + SHA-256) is ignored
  (200); different content archives the old content as a version and bumps `version`.
- Moving a file onto a same-named file keeps the destination node. Identical content drops the source;
  different content archives the destination's content and the moved content becomes current. The
  source's older versions are appended to the history, and the dates become the earliest
  `created_at` and the latest `updated_at` of both.
- Moving a folder onto a same-named folder merges it recursively into the destination, applying
  the file rule to each conflict; the source folder is removed.

Deleting moves files to the recycle bin instead of removing them. Deleting a file bins it;
deleting a folder bins every file below it (flat, each remembering its `original_path`) and
removes the folders, so folders are never in the bin. Binned files keep their id, content and
versions, and may share names: uniqueness only applies to live nodes, so a deleted file never
blocks its name. They disappear from listings, search and every node endpoint (404) until they
are restored or purged.

Every user has two special folders, created with the user. The recycle bin is a hidden root
folder (`.recycle-bin-<id>`) that no node endpoint lists or accepts (404). `Lost & Found` is a
root folder that restored files land in, directly under their own names. It can be renamed
or moved but not deleted, nor can a folder containing it (409).

Restoring (`POST /trash/restore`) and moving several nodes (`POST /nodes/move`) take an
`on_conflict` mode for files whose name is taken at the destination, or that share a name within
the selection (folders merge and never clash):

- `abort` (default): change nothing and return the `conflicts` (the path relative to what is
  moved, or the original path when restoring; `existing` is null for a clash within the selection).
- `override`: apply the move rule above; of several same-named binned files, the most recently
  deleted ends up current and the older ones become versions.
- `skip`: never modify the destination, and return the clashing files in `skipped`. A restore
  leaves them in the bin (all of them, for same-named files in the selection); a move puts them
  in the bin with their current path, so the source folders they leave behind are removed.

Previous versions live in `file_versions`; their blobs are under `<storage>/versions/`.
Folders have the MIME type `inode/directory`.

Nodes and versions carry `edited_on`, the id of the registered device that wrote the content
(or created the folder), for "Edited on Laptop". A trigger sets it from the transaction's device
when a node is created or its content changes; renames and moves keep it. Clients name the
device through `GET /devices`.

### Uploads

After signing in, clients read `HOMESPACE_UPLOAD_CHUNK_BYTES` from `GET /capabilities` as
`upload_chunk_bytes`. Files up to that size go up in one request; larger ones in chunks of that
size (`/uploads`), so no request exceeds a proxy's body limit, and an interrupted upload continues
from the bytes the server already has. A client that cannot read the capabilities uses 512 KiB,
which stays below nginx's default limit of 1 MiB. Unfinished uploads are kept under `<storage>/uploads/`. See
[`docs/chunked-upload.md`](docs/chunked-upload.md).

| Variable | Default | |
|---|---|---|
| `HOMESPACE_MAX_UPLOAD_BYTES` | 10737418240 (10 GiB) | Largest file accepted, however it is uploaded. |
| `HOMESPACE_UPLOAD_CHUNK_BYTES` | 33554432 (32 MiB) | Largest request of any upload, single or chunked. Keep it below the proxy's limit (Cloudflare: 100 MB). It does not limit the file size. |
| `HOMESPACE_UPLOAD_EXPIRY_HOURS` | 24 | Unfinished uploads idle this long are deleted. |
| `HOMESPACE_USER_DELETION_DELAY_MINUTES` | 10 | How long after an administrator deletes a user their account and files are removed. The user is signed out at once and the deletion can be cancelled until then. |
| `HOMESPACE_TRANSFER_TTL_HOURS` | 24 | Files sent to a device are deleted this long after they were sent, received or not ([`docs/send-to-device.md`](docs/send-to-device.md)). |
| `HOMESPACE_MAX_TRANSFER_BYTES` | unset | The most bytes one transfer to a device may hold. |

A reverse proxy needs a body limit above `HOMESPACE_UPLOAD_CHUNK_BYTES` on `/api/v1/files` (for
single-request uploads and replacements) and on `/api/v1/uploads`. Larger single requests (from a
script, say) are then refused by the proxy and have to go through `/uploads`. With nginx, a regex
location for `/api/v1/files/{id}/content` (such as one that turns off buffering for downloads)
takes over replacements too, so give it the same limit. See [`docs/nginx/example.domain.com.conf`](docs/nginx/example.domain.com.conf).

Sync clients catch up through a journal of changes (`GET /changes?since=<cursor>`). Every day the
server compacts it to the latest entry of each file and folder, which is all a client reads, so a
client can catch up however long it was away. It does not grow with every edit. Each entry names
the device (`device_id`) that made the latest change, when it came from a client that registered
one.

| Variable | Default | |
|---|---|---|
| `HOMESPACE_CHANGES_RETENTION_DAYS` | unset | Also drop journal entries older than this. A client offline for longer then reads its folders again in full. |

### API (`/api/v1`)

| Method | Path | Purpose |
|---|---|---|
| POST | `/auth/login`, `/auth/refresh`, `/auth/logout` | JWT access + rotating refresh tokens; login (and `/auth/oidc/exchange`) take an optional `device: {id, name, platform}`; after 3 wrong passwords for a username from one client address (an IPv6 /64 counts as one), login answers that address `429 too_many_requests` with `Retry-After` for 30 s, then 1, 2, 4 min and so on (up to a day) until the right password; other addresses are not held up, so guessing cannot lock the owner out, and a device the user has signed in from before has its own count, so it still gets in when its address is blocked; an address is also counted over all usernames and waits after 20 wrong passwords within an hour (30 s doubling up to 15 min), again except for known devices; passwords are checked one per processor at a time, and a sign-in that waits over 5 s for its turn gets `429` too |
| GET | `/auth/providers` | how users can sign in: `{password, oidc: {name}?}` |
| GET / GET / POST | `/auth/oidc/start`, `/auth/oidc/callback`, `/auth/oidc/exchange` | single sign-on, see [Single sign-on](#single-sign-on) |
| GET | `/me` | current user: `{id, username, lost_and_found_id, is_admin, external}` (`external`: signs in only through the identity provider) |
| POST | `/me/password` | `{current_password, new_password}` → 204; ends the user's other sessions, this one stays signed in. A wrong current password is 400 and counts towards the sign-in throttle (429). Not for single sign-on accounts (400) |
| GET | `/storage` | the user's bytes `{usage: {files, versions, trash}, disk: {total, available}?}`; versions count binned files too |
| GET | `/admin/status` | administrators only (403 `forbidden` otherwise): version, start time, the storage root's disk, every user's `usage`, the read-only configuration and the database version |
| GET | `/admin/users` | administrators only: every account with its role, sign-in method, `disabled`, `quota_bytes`, `delete_at`, `usage`, device count and `last_seen_at` |
| POST | `/admin/users` | `{username, password, is_admin?, quota_bytes?}`: creates a password account (201; 409 `conflict` if the name is taken) |
| PATCH | `/admin/users/{id}` | `{is_admin?, disabled?, quota_bytes?}` (`null` removes the quota; it is recorded, not enforced). Disabling signs the user out and refuses sign-in (403 `forbidden`). Administrators cannot demote or disable themselves |
| PUT | `/admin/users/{id}/password` | `{password}`: a new password for an account that has one (400 for single sign-on accounts); signs the user out everywhere |
| GET | `/admin/users/{id}/devices` | the account's devices, most recently seen first |
| DELETE | `/admin/users/{id}` | marks the account for deletion `HOMESPACE_USER_DELETION_DELAY_MINUTES` from now and signs it out; a background job then deletes it with all its files |
| DELETE | `/admin/users/{id}/deletion` | cancels a pending deletion |
| GET | `/capabilities` | what the server supports: `{upload_chunk_bytes}`, the largest request body of an upload |
| GET | `/nodes?parent_id=` | list folder (root when omitted) |
| GET / PATCH / DELETE | `/nodes/{id}` | metadata / rename or move / delete (to the recycle bin; 409 for Lost & Found) |
| POST | `/nodes/move` | `{ids, parent_id, on_conflict?}` → `{moved, skipped, conflicts, target}`; `on_conflict` is `abort` (default: report clashes, change nothing), `override` (merge) or `skip` (destination untouched, clashing files go to the recycle bin) |
| POST | `/folders` | `{parent_id?, name}` |
| POST | `/files?name=&parent_id=` | raw-body streaming upload (201 new, 200 unchanged or new version) |
| GET / PUT | `/files/{id}/content` | download / replace content |
| POST | `/uploads` | `{name, parent_id?, size, modified_at?, mime?, node_id?, keep_both?, version_of?, auto_upload?, transfer_item_id?}` → 201 `{id, offset, size, chunk_size}`; starts a resumable upload (`node_id` replaces that file's content; `transfer_item_id` uploads a file of a transfer; for the other options see [`docs/chunked-upload.md`](docs/chunked-upload.md)) |
| PATCH | `/uploads/{id}` | raw-body chunk with `Upload-Offset: N` → `{offset, node?}`; 409 with the right offset on a mismatch, `node` once complete |
| GET / HEAD / DELETE | `/uploads/{id}` | current offset (also in `Upload-Offset`) / cancel |
| GET | `/uploads?this_device=` | unfinished uploads `{id, parent_id?, node_id?, name, size, device_id?, transfer_item_id?, updated_at}`, or only the calling device's |
| GET | `/devices` | the user's devices: `{id, name, platform, created_at, last_seen_at, signed_in, current, uploads, visibility, visible_to?}` |
| PATCH / DELETE | `/devices/{id}` | rename `{name}` / sign out: end its sessions and cancel its unfinished uploads (it stays listed) |
| PUT | `/devices/{id}/visibility` | `{visibility, users?}`: who can send files to the device: `owner` (default), `hidden`, `everyone` or `users` (the owner plus `users`) |
| GET | `/devices/targets` | the devices the caller can send to, grouped by user, own first: `[{user: {id, username}, own, devices: [{id, name, platform, signed_in, online, current}]}]` |
| GET | `/users` | the other users `[{id, username}]`, for choosing who sees a device |
| POST | `/transfers` | `{recipient_id, recipient_device, items: [{path, kind, size?, node_id?, mime?, modified_at?}], message?}` → 201 `{id, state, items: [{id, path, upload_needed}]}`; send files to a device ([`docs/send-to-device.md`](docs/send-to-device.md)) |
| GET | `/transfers/incoming?this_device=`, `/transfers/outgoing` | transfers waiting for the caller (ready or declined) / transfers the caller sent |
| GET / DELETE | `/transfers/{id}` | summary with items (sender or recipient) / cancel (sender) |
| GET | `/transfers/{id}/items/{item}/content` | download one file of a ready transfer (recipient); supports `Range` |
| GET | `/transfers/{id}/archive` | download a ready transfer as one zip (recipient) |
| POST | `/transfers/{id}/received`, `/transfers/{id}/decline`, `/transfers/{id}/save` | recipient: confirm the download / decline / move into the tree under `{parent_id?}`; each removes the transfer's bytes |
| GET | `/nodes/{id}/properties` | predefined properties of a node: `{source_device?}`, the name of the device that last auto-uploaded the file |
| GET | `/nodes/{id}/versions` | previous versions of a file, newest first |
| DELETE | `/nodes/{id}/versions/{version}` | delete a previous version |
| GET | `/files/{id}/versions/{version}/content` | download a previous version |
| GET | `/nodes/{id}/ancestors` | path from the root down to the node's parent |
| GET | `/trash` | binned files, newest first: `{node, original_path, deleted_at}` |
| POST | `/trash/restore` | `{ids, on_conflict?}` → `{moved, skipped, conflicts, target}`; restores into Lost & Found |
| DELETE | `/trash` | empty the recycle bin (permanent) |
| DELETE | `/trash/{id}` | permanently delete one binned file and its versions (404 unless binned) |
| GET | `/search?q=&limit=` | full-text search; every word matches as a prefix, hits carry their `location` |
| POST | `/tickets` | `{scope: "events"}`, `{scope: "download", node_id}`, `{scope: "download_version", node_id, version}` or `{scope: "transfer_item", transfer_id, item_id}` → ticket valid for 30 s and for one request |
| GET | `/ws` | live `node_created` / `node_updated` / `node_deleted` / `trash_changed` / `devices_changed` / `transfer_received` / `transfer_changed` / `resync` events; a user may have 32 sockets open (`429` beyond that) and a client message over 4 KiB closes the socket |
| GET | `/health` | liveness + DB ping |

All node and trash endpoints return 404 for ids owned by someone else. Authenticate with `Authorization: Bearer <access_token>`.
Browsers cannot send that header on a websocket or a plain download link, so `/ws?ticket=` and
`/files/{id}/content?ticket=` accept a short-lived, single-scope, single-use ticket instead.

The apps send their refresh token in the body of `/auth/refresh` and `/auth/logout` and get the next
one back the same way. A browser sets `cookie: true` when it signs in (`/auth/login`,
`/auth/oidc/exchange`): its refresh token then goes into an `HttpOnly`, `SameSite=Strict` cookie
limited to `/api/v1/auth` and is left out of the response, so no script of the page can read it.
Requests that use the cookie, and the websocket, are refused with 403 unless their `Origin` is the
server itself: the `Host` they were sent to, or `HOMESPACE_PUBLIC_URL`. A reverse proxy must
therefore pass `Host` on unchanged or that variable must be set. All tabs of a browser share the
cookie; when two refresh at the same moment, the second gets an access token for the same session
within 10 s instead of ending it.

Every response carries `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`,
`Referrer-Policy: no-referrer` and a `Content-Security-Policy`: nothing at all for API responses
(which are also `Cache-Control: no-store`), and for the web client only its own files, with the
scripts written into its `index.html` allowed by their hashes.

## Shared protocol

`shared/protocol` holds the wire types (DTOs, events, error codes, route paths) used by the
server and every client. It has no server dependencies and compiles to WebAssembly.

## Clients

Rust workspace in `clients/`: `api-client` (typed HTTP client with serialized token refresh,
shared by every client), `web` (Leptos CSR app), `desktop` (iced) and `mobile-core` (the same
client for the mobile apps). A CLI is planned.

The desktop and mobile apps refuse to sign in to an `http://` server outside the home network,
where the password would cross the internet unencrypted. Plain `http://` stays allowed for this
machine, private and link-local addresses (including the 100.64.0.0/10 range of VPNs such as
Tailscale) and names only a local network resolves (`nas`, `nas.local`, `files.lan`, `*.ts.net`).

The mobile clients live in `clients/mobile`: an Android app (Jetpack Compose) and an iOS app
(SwiftUI), both calling the Rust client through `clients/mobile-core` (UniFFI bindings over
`api-client` and `client-core`). See
[`clients/mobile/README.md`](clients/mobile/README.md).

Needs `rustup target add wasm32-unknown-unknown` and `cargo install trunk --locked`.

```sh
# development: server on :8080, web client with live reload on :8081 (proxies /api)
cd clients/web && trunk serve

# production: one origin serves both
cd clients/web && trunk build --release
cd ../../server && HOMESPACE_WEB_ROOT=../clients/web/dist cargo run -p app -- serve
```

The desktop client (`clients/desktop`, iced) runs with `cargo run -p desktop`. It keeps the
session in `~/.local/share/home-space/session.json`, so rebuilt dev binaries sign in again without
a keychain prompt; build with `--features keyring` to keep it in the system keyring instead
(macOS Keychain, Secret Service).

Release builds come from `clients/desktop/scripts/build-release.sh <macos|linux|windows>`, which
builds with `--features keyring` for the current CPU and writes to `clients/target/dist/<os>/`:

- `macos`: `Home Space.app` (ad-hoc signed; set `CODESIGN_IDENTITY` to use a real identity) and a
  zip of it. Only an app bundle opens without a Terminal window and shows the Dock icon.
  `--universal` builds for Apple Silicon and Intel together (needs both Rust targets).
- `linux`: a tar.gz with the binary, `home-space.desktop` and the icons.
- `windows` (run from Git Bash): `HomeSpace.exe`, plus a zip when a zip tool is available. Release
  builds have no console window.
The icons in `clients/desktop/packaging` are rendered from `home-space.svg`.

The Linux client is also packaged as a Flatpak (`app.home-space.home-space-client`, on the
`org.freedesktop.Platform` runtime). With `flatpak` and `flatpak-builder` installed,
`clients/desktop/scripts/build-flatpak.sh` builds `clients/target/dist/flatpak/HomeSpace-<version>-linux-<arch>.flatpak`
(`--install` also installs it for the current user). The build is offline, so the script first lists
the crates from `Cargo.lock` in `packaging/flatpak/cargo-sources.json` with
`scripts/generate-flatpak-sources.sh` (git and python3). The sandbox can reach the network, the home
folder (sync folders are changed in place) and the Secret Service keyring; sync folders outside home
need an override (`flatpak override --user --filesystem=/mnt/data app.home-space.home-space-client`).
CI builds the bundle on pull requests that touch the desktop client, and tagged releases attach it.

## Single sign-on

Besides passwords, users can sign in through any OpenID Connect provider: Auth0, Keycloak,
Authentik, Zitadel and so on. home-space is a confidential client of the provider (authorization
code flow with PKCE); once the provider has vouched for the user, home-space issues its own
tokens, so the rest of the API works as before. Setting `HOMESPACE_OIDC_ISSUER` turns it on:

| Variable | |
|---|---|
| `HOMESPACE_PUBLIC_URL` | Address users reach the server at, e.g. `https://files.example.com`. Required. (Without single sign-on it is optional, and names the origin the web client is served from when a proxy does not pass `Host` on.) |
| `HOMESPACE_OIDC_ISSUER` | Issuer URL of the provider. Required. |
| `HOMESPACE_OIDC_CLIENT_ID`, `HOMESPACE_OIDC_CLIENT_SECRET` | The client registered at the provider. |
| `HOMESPACE_OIDC_NAME` | Label on the sign-in button (default `Single sign-on`). |
| `HOMESPACE_OIDC_REQUIRED_GROUP` | Only members of this group may sign in. Optional. |
| `HOMESPACE_OIDC_GROUPS_CLAIM` | ID token claim listing the groups (default `groups`). |
| `HOMESPACE_OIDC_SCOPES` | Default `openid profile email`. |
| `HOMESPACE_PASSWORD_LOGIN` | `false` allows only the provider. Default `true`. |

The issuer and public URL must use https (plain http is accepted for `localhost` only). Register
`<HOMESPACE_PUBLIC_URL>/api/v1/auth/oidc/callback` as the redirect URI at the provider.

- **Keycloak:** create a confidential client in your realm with the standard flow on; the issuer
  is `https://<host>/realms/<realm>`. For groups, add a *Group Membership* mapper (full path off)
  to the client's dedicated scope.
- **Auth0:** create a *Regular Web Application*; the issuer is `https://<tenant>.<region>.auth0.com/`
  (with the trailing slash). Auth0 only puts custom claims in tokens under a namespace, so for
  groups add an Action that sets e.g. `https://files.example.com/groups` on the ID token and
  point `HOMESPACE_OIDC_GROUPS_CLAIM` at it.

A user is created on first sign-in, named after `preferred_username`, then the e-mail's local
part, then the subject (a number is added if the name is taken). Accounts are matched on the
provider's stable subject id, never on e-mail. They have no password, so they cannot use password
login, and the CLI can still give them one (`user passwd`).

Desktop clients open the system browser and receive the result on a loopback port; the web client
is redirected to `/login/callback`. In both, the server hands back a 60 second, single-use code
that only the client holding the PKCE verifier can exchange for tokens.

To try it locally with Keycloak (user `demo` / password `demo`, in group `family`):

```sh
podman-compose --profile oidc up -d keycloak     # http://127.0.0.1:8180, admin / admin
cd server && HOMESPACE_PUBLIC_URL=http://127.0.0.1:8080 \
  HOMESPACE_OIDC_ISSUER=http://127.0.0.1:8180/realms/homespace \
  HOMESPACE_OIDC_CLIENT_ID=homespace HOMESPACE_OIDC_CLIENT_SECRET=homespace-dev-secret \
  HOMESPACE_OIDC_NAME=Keycloak HOMESPACE_OIDC_REQUIRED_GROUP=family cargo run -p app -- serve
```

The realm (`docs/keycloak/homespace-realm.json`) allows the redirect URIs of the server on 8080 and
of `trunk serve` on 8081; set `HOMESPACE_PUBLIC_URL` to the address the browser uses. Keycloak runs
on the host network name `127.0.0.1` here, so run the server on the host rather than in Compose.

## Compose

`compose.yaml` runs Postgres and the app (API plus the web client, built by the `Dockerfile`) and
can run the whole test suite. Works with `podman compose` / `podman-compose`.

```sh
export HOMESPACE_JWT_SECRET=$(head -c 32 /dev/urandom | base64)   # 32+ characters
podman-compose up -d --build                  # http://127.0.0.1:8080
podman-compose run --rm app user add me      # prompts for a password (add -it if needed)
podman-compose --profile test run --rm test   # server + client tests against the compose Postgres
podman-compose down                           # add -v to also delete the database and files
```

The server does not start with a signing secret that is shorter than 32 characters, still a
placeholder (`change-me...`) or made of only a few different characters.
The signing secret can also come from a file (`HOMESPACE_JWT_SECRET_FILE`, e.g. a Docker/Podman
secret). To rotate it, set the new value and move the old one to `HOMESPACE_JWT_SECRET_PREVIOUS`;
sessions keep working, and the old value can be removed once access tokens (15 minutes by default)
signed with it have expired.

Access tokens and tickets are signed with the same secret and each names its kind in a `typ`
claim, so neither is accepted in place of the other.

Access tokens are only accepted while the session they were issued for exists in the database. A
session starts after a verified password or provider login and ends on logout, on refresh token
reuse, on a password change or when it expires. So the signing secret alone cannot forge access:
a token it signs for a session that does not exist is rejected, and logging out takes effect at
once rather than when the access token expires.

### Running on a server

`compose.prod.yaml` keeps the database and the blobs in directories you choose and runs both
containers as one host user, so only that user (and root) can read them:

```sh
sudo useradd --system --no-create-home --shell /usr/sbin/nologin homespace
sudo install -d -o homespace -g homespace -m 700 /srv/homespace/db /srv/homespace/files

cat > .env <<EOF
HOMESPACE_DB_DIR=/srv/homespace/db
HOMESPACE_DATA_DIR=/srv/homespace/files
HOMESPACE_UID=$(id -u homespace)
HOMESPACE_GID=$(id -g homespace)
HOMESPACE_DB_USER=homespace
HOMESPACE_DB_PASSWORD=$(head -c 24 /dev/urandom | base64 | tr -d '/+=')
HOMESPACE_JWT_SECRET=$(head -c 32 /dev/urandom | base64)
EOF
chmod 600 .env

docker compose -f compose.yaml -f compose.prod.yaml up -d --build
docker compose -f compose.yaml -f compose.prod.yaml run --rm app user add me
```

The directories must exist and be owned by that user before the first start (mode `700` keeps
everyone else out). With rootless Podman the uid is remapped, so own the directories through
`podman unshare chown` instead, or drop the `user:` lines and rely on the rootless user itself.
The database user and password come from `HOMESPACE_DB_USER` and `HOMESPACE_DB_PASSWORD` (both
default to `homespace`); set your own before exposing anything. They go into `DATABASE_URL`, so keep
the password free of URL-special characters (`@ / : ?`). Postgres only applies them when it first
creates the data directory; for an existing database change the role with `ALTER USER` instead.

Postgres is published on `127.0.0.1:5433` (`HOMESPACE_DB_PORT`) and the app on `127.0.0.1:8080`
(`HOMESPACE_PORT`), so it does not clash with the `scripts/db.sh` database on 5432.
