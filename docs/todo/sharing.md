# Sharing files and folders

**Status (2026-11-16):** done in every client, read-only. It differs from the proposal below:

- A share holds several items (`share_items`), and how it is reached lives in its own tables:
  `share_links` for anyone with the link, `share_users` for named users (migrations `0021`,
  `0022`). The link token is kept as it is, not hashed, so the owner can copy the link again.
- Links may have a password. Wrong tries are limited like sign-ins.
- Recipients browse and download under "Shared with me" (`/shared`), download a folder as a zip
  (`/shared/{id}/archive`, `/public/shares/{token}/archive`), and save copies into their own
  files (`/shared/{id}/copy`). The share dialog picks users from `GET /users`.

Still open: groups (they need groups from `admin-panel.md`), write access, `share_changed` and
`shared_with_me_changed` events, and deleting shares that expired long ago.


## Problem

Every node belongs to one user, and all node endpoints return 404 for ids owned by someone else
(`owner_id` scoping in `server/crates/db/src/nodes.rs`). There is no way to hand a file or a folder
to another household member, or to someone outside the server.

## Proposal

### Share links

A share points at one node (file or folder). For a folder, everything below it is shared, and the
share follows the folder's current contents.

```sql
CREATE TABLE shares (
    id          UUID PRIMARY KEY,
    node_id     UUID NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    owner_id    UUID NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    token_hash  BYTEA NOT NULL UNIQUE,      -- only the hash, like refresh tokens
    audience    TEXT NOT NULL CHECK (audience IN ('anyone', 'users')),
    permission  TEXT NOT NULL DEFAULT 'read' CHECK (permission IN ('read')),
    expires_at  TIMESTAMPTZ,                -- NULL means no expiry
    revoked_at  TIMESTAMPTZ,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE share_recipients (
    share_id UUID NOT NULL REFERENCES shares (id) ON DELETE CASCADE,
    user_id  UUID REFERENCES users (id) ON DELETE CASCADE,
    group_id UUID,                          -- see admin-panel.md (user groups)
    CHECK ((user_id IS NULL) <> (group_id IS NULL))
);
```

- **Anyone with the link** (`audience = 'anyone'`): no sign-in needed. The URL is
  `<HOMESPACE_PUBLIC_URL>/s/<token>`, where the token is 32 random bytes, base64url.
- **Specific users** (`audience = 'users'`): the same link, but the visitor has to sign in, and
  the server checks that they are a recipient (directly or through a group). The share also
  shows up under "Shared with me" for the recipients, so they do not need the link at all.
- **Expiry**: `expires_at` is optional. The presets are 1 day, 7 days, 30 days and never, plus a
  custom date. An expired or revoked share returns 410 Gone. A background task deletes rows that
  expired more than 30 days ago.

### API

| Method | Path | Purpose |
|---|---|---|
| POST | `/shares` | `{node_id, audience, recipients?: {users, groups}, expires_at?}` → `{id, url}`; the token is returned only here |
| GET | `/shares?node_id=` | the caller's shares (of one node) |
| PATCH / DELETE | `/shares/{id}` | change recipients or expiry / revoke |
| GET | `/shared-with-me` | shares where the caller is a recipient |
| GET | `/public/shares/{token}` | share metadata + root node (no auth for `anyone`) |
| GET | `/public/shares/{token}/nodes?parent_id=` | list a folder inside the share |
| GET | `/public/shares/{token}/files/{id}/content` | download (supports `Range`) |
| GET | `/public/shares/{token}/archive` | folder as a streamed zip (optional, later) |

- Every `/public/shares` lookup hashes the token, loads the share, checks `revoked_at` and
  `expires_at`, and, for a node below the root, checks that the node is a descendant of
  `shares.node_id` (a recursive CTE on `parent_id`, or walk the ancestors as
  `/nodes/{id}/ancestors` does). Getting this check wrong exposes the owner's whole tree, so it
  needs its own tests.
- Rate-limit the token lookups per IP so tokens cannot be guessed by brute force.
- Moving a shared node elsewhere keeps the share. Binning it stops the share from resolving until
  the node is restored. Deleting it for good removes the share (`ON DELETE CASCADE`).
- New events: `share_changed` for the owner, `shared_with_me_changed` for recipients.

### Clients

- Web: a "Share" action on files and folders that opens a dialog with the audience, recipients
  (user and group picker), expiry and a copy-link button, and lists the node's existing shares
  with a revoke button. A "Shared with me" entry in the sidebar. A public `/s/<token>` page that
  needs no sign-in for `anyone` links: a file preview with download, or a read-only folder
  browser.
- Desktop: copy a share link from the context menu. "Shared with me" can come later.

## Open questions

- Write access (recipients uploading into a shared folder)? Leave it out for now; the
  `permission` column makes room for `'write'` later. With write access, it matters whose quota
  the uploaded bytes count against.
- Optional password on `anyone` links?
- Should shares made by OIDC users be limited (e.g. only `users` links) by a config flag?
- Download counter / access log per share?
