# Administration panel

## Problem

Users are managed only through the CLI (`homespace user add | list | passwd` in
`server/crates/app/src/main.rs`), which needs shell access to the server. There is no notion of an
administrator, no quotas and no groups.

## Proposal

### Admin role

- `users.is_admin BOOLEAN NOT NULL DEFAULT false`, plus `users.disabled_at TIMESTAMPTZ`.
- `homespace user add --admin` and `homespace user admin <username> [--revoke]` so the first admin
  can be created from the CLI.
- Optionally `HOMESPACE_OIDC_ADMIN_GROUP`: members of this provider group are made admins on
  sign-in (re-checked at every sign-in).
- `is_admin` goes into the access token claims and into `GET /me`. An `AdminUser` extractor in
  `server/crates/api/src/extract.rs` returns 403 otherwise.
- Admins do **not** get access to other users' files through these endpoints.

### Users

| Method | Path | Purpose |
|---|---|---|
| GET | `/admin/users` | list with usage, quota, groups, sign-in method, last seen |
| POST | `/admin/users` | create a local user `{username, password?}` (password or a one-time reset link) |
| PATCH | `/admin/users/{id}` | rename, set admin, disable / enable, quota |
| POST | `/admin/users/{id}/password` | reset password: set one or issue a reset link; revokes all refresh tokens and sessions |
| DELETE | `/admin/users/{id}` | delete the user and their files (confirm by typing the username) |

- Renaming and resetting passwords apply to local (password) users only. OIDC users are managed
  at the provider, so the panel shows them read-only apart from quota, admin, disable and groups.
- Disabling a user revokes their refresh tokens, closes their websocket sessions and rejects
  logins.
- An admin cannot remove their own admin flag or disable themselves if they are the last admin.

### Quotas

- `users.quota_bytes BIGINT` (NULL = unlimited) and `HOMESPACE_DEFAULT_QUOTA_BYTES` for new
  users.
- Usage = current files + versions + recycle bin, the same figure as `GET /storage` in
  `storage-limits.md`. Build both on one per-user usage counter.
- Enforce the quota where the free-space check runs: `POST /uploads` (claimed `size`), streaming
  single-request uploads, and new versions. Return a dedicated `QuotaExceeded` error code (413).
  Done: enforced with 507 `quota_exceeded`, also when saving a received transfer; usage is
  summed per request, not kept in a counter yet.
- Clients show "used / quota" instead of "used / disk free" when a quota is set.

### Groups

```sql
CREATE TABLE groups (
    id         UUID PRIMARY KEY,
    name       TEXT NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE group_members (
    group_id UUID NOT NULL REFERENCES groups (id) ON DELETE CASCADE,
    user_id  UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    PRIMARY KEY (group_id, user_id)
);
```

- `GET/POST /admin/groups`, `PATCH/DELETE /admin/groups/{id}`, `PUT/DELETE
  /admin/groups/{id}/members/{user_id}`.
- A non-admin `GET /directory?q=` returns users and groups by name, for the share dialog's
  recipient picker (`sharing.md`). It returns ids and names only.
- Optionally sync membership from the OIDC groups claim (`HOMESPACE_OIDC_GROUPS_CLAIM`) into
  groups marked `source = 'oidc'`, which then cannot be edited by hand.

### Web UI

- An "Administration" entry in the user menu, shown only to admins, with Users and Groups tabs.
- Users: a table with usage bars, filters and row actions (edit, reset password, disable, delete).
- Groups: a list with member editing.

### Desktop sync ignore list

The desktop client skips some names when it syncs (`IGNORED_NAMES`, `IGNORED_PREFIXES` and
`IGNORED_SUFFIXES` in `clients/sync-core/src/scan.rs`, applied by `is_ignored`). The list is
hard-coded, so changing it means a new client build.

- Make it a server-side setting that admins edit in the panel: exact names, prefixes and
  suffixes (later maybe glob patterns), with the current built-in list as the default.
- `GET /sync/ignore` (any signed-in user) returns the list; `PUT /admin/sync/ignore` replaces it.
  Clients fetch it at sign-in and on a sync pass, and fall back to the built-in list when the
  server is unreachable or too old to have the endpoint.
- Decide what happens to files that are already synced and then match a new rule: leave them
  as they are on both sides, and only stop treating new matches as changes.
- Optionally let a user add their own patterns on top, in the desktop app's settings.

## Order

The admin role and groups come before or together with `sharing.md`, because shares to specific
users and groups need the directory endpoint. Quotas depend on the usage counter from
`storage-limits.md`.

## Open questions

- Password reset by e-mail? There is no mailer today, so the admin copies a one-time link
  instead.
- Should admins be able to see storage per user broken down by folder, or only totals?
- Audit log of admin actions?
