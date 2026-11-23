# Multi-tenancy

## Problem

One server process and one database serve one group of users today. Hosting several unrelated
groups (families, small teams) means running a server, a database and a storage root for each.
The goal is that one installation serves several **tenants**, each reached on its own domain
(`files.example.com`, `cloud.other.org`), with its own users, its own signing key and its own
identity provider, and with no way for anything of one tenant to be seen from another.

Nothing changes for users: clients talk to their tenant's domain exactly as they talk to the
server today. This is a database and security change. Existing installs keep working without any
configuration change; they become tenant 1.

Not in scope: per-tenant branding, per-tenant server limits (upload size, TTLs), a web UI for
managing tenants, a secrets vault (only the seam for one), moving a user between tenants.

## Proposed approach (from the request)

1. New table `tenants` with one row, `id = 1`, `name = 'files.example.com'`.
2. A `tenant_id` column, default `1`, `ON DELETE CASCADE`.
3. Every query filters by tenant.
4. Access tokens carry the tenant id in a new claim.
5. CLI commands to add a tenant and change its name or domain.
6. Per-tenant secrets (JWT signing secret, OIDC authority and client) stay out of the database
   and are read from `{ENVVAR}_{TENANT_ID}` environment variables, with room for a vault later.

The review below keeps the approach and changes five details. Each is explained where it comes
up; in short:

| Request | Change | Why |
|---|---|---|
| `tenant_id` on every table | On `users` and the few tables that are not reached through a user | Everything else is already keyed by a user's UUID; the leaks are in the queries that cross users, not in the owner-scoped ones (see [Where tenant_id goes](#where-tenant_id-goes)) |
| `DEFAULT 1` | Same intent, spelled out: `NOT NULL DEFAULT 1` backfills existing rows, then the default is dropped | New rows must name their tenant (from the parent user or the request), so a forgotten insert fails on `NOT NULL` |
| `ON DELETE CASCADE` | `ON DELETE RESTRICT`, deletion through a background job | One `DELETE FROM tenants` would wipe every row of a tenant in a single transaction and leave its files on disk |
| `name` is the domain | Separate `tenant_domains` table, one primary per tenant | A domain can't be renamed safely: OIDC callbacks, security keys of protected folders, share links and installed clients are all bound to it |
| Tenant from the JWT claim | Tenant from the request's `Host`, confirmed by the claim | Sign-in, OIDC, public share links and `/capabilities` have no token, so the claim can't be the only source |

## What exists today

### Schema

Every table hangs off `users`, keyed by UUIDs that are unique across the whole database:

- Owned by a user: `nodes`, `file_versions`, `search_entries`, `trash_entries` (via node),
  `user_folders`, `node_changes`, `node_properties` (via node), `uploads`, `devices`,
  `device_viewers`, `sessions`, `refresh_tokens`, `oidc_codes`, `user_identities`, `shares`,
  `share_items`, `share_links`, `share_users`, `transfers`, `transfer_items`.
- Not tied to a user: `login_attempts` (keyed by the submitted username, which may not exist),
  `oidc_pending` (a sign-in before we know who it is), `node_changes_pruned`.

"Username" in this plan always means the sign-in name (`users.username`, text), never the user
id. User ids are UUIDs and stay globally unique, so nothing keyed by a user id needs a tenant.
The name is the one thing a person types to sign in, so it is what has to become unique per
tenant. `login_attempts` has to stay keyed by the name too: it also counts names that belong
to no user, so that throttling does not tell which names exist.

Constraints that are global today and must become per tenant:

| Constraint | Today | With tenants |
|---|---|---|
| `users.username` | `UNIQUE` | `UNIQUE (tenant_id, username)`: two tenants may both have `admin` |
| `user_identities` | `PRIMARY KEY (issuer, subject)` | `(tenant_id, issuer, subject)`: two tenants may use the same Auth0 / Keycloak realm, and the same person may be a user in both |
| `login_attempts` | `PRIMARY KEY (username, source)` | `(tenant_id, username, source)` |

Constraints that can stay global, because the value is random: `refresh_tokens.token_hash`,
`share_links.token`, `oidc_pending.state_hash`, `oidc_codes.code_hash`, every UUID primary key.
Their lookups still have to check the tenant (see [Auth flow](#auth-flow)).

### Auth

- `AuthService` holds one `Keys` (current and previous HS256 secret) and one optional
  `OidcService`, built once in `serve()` from `Config` (`server/crates/app/src/config.rs`).
- Access tokens (`typ=access`), tickets (`typ=ticket`, used by `/ws` and downloads) and share
  tokens (`typ=share`) are signed with the same keys. `AccessClaims` has
  `#[serde(deny_unknown_fields)]`.
- Every access token is checked against a live `sessions` row (`verify_access` →
  `live_session(sid, sub)`), so a signature alone does not grant access.
- Refresh tokens are opaque, looked up by hash (`find_refresh_token`).
- OIDC: one issuer, one callback URL derived from `HOMESPACE_PUBLIC_URL`. `oidc_pending` is
  looked up by state hash, `oidc_codes` by code hash.
- Throttling (`login_from`): per `(username, source)` and per address over all usernames (the
  `ANY_USERNAME` row `''`). One hashing semaphore for the process.
- Websockets: `Hub` and the "sessions ended" broadcast are keyed by `UserId`.
- The browser refresh cookie is `Path`-scoped, `HttpOnly`, `SameSite=Strict`, and the browser
  keeps it per host, so it is already per domain. `is_own_origin` compares `Origin` with `Host`
  or `HOMESPACE_PUBLIC_URL`.

### Storage

`BlobStore` keeps `blobs/<owner_id>/<node_id>`, `versions/<owner_id>/<version_id>`, and
`tmp/`, `uploads/`, `transfers/` keyed by random UUIDs. Paths contain no names, only UUIDs, so
tenants can't collide or reach each other through a path.

## Proposal

### Schema

```sql
CREATE TABLE tenants (
    id          INTEGER PRIMARY KEY GENERATED BY DEFAULT AS IDENTITY,
    name        TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 100),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- A disabled tenant answers 503 on every route and its users can't sign in.
    disabled_at TIMESTAMPTZ,
    -- Like users.delete_at: a background job deletes the tenant's users, then the tenant.
    delete_at   TIMESTAMPTZ
);

-- The hosts a tenant answers on. Exactly one is primary: it makes the public URL (OIDC
-- callback, links in e-mails later). The others are aliases that keep old links and
-- installed clients working after a move.
CREATE TABLE tenant_domains (
    domain     TEXT PRIMARY KEY CHECK (domain = lower(domain)),  -- host, with :port if not 443
    tenant_id  INTEGER NOT NULL REFERENCES tenants (id) ON DELETE CASCADE,
    is_primary BOOLEAN NOT NULL DEFAULT false
);
CREATE UNIQUE INDEX tenant_domains_primary ON tenant_domains (tenant_id) WHERE is_primary;

INSERT INTO tenants (id, name) OVERRIDING SYSTEM VALUE VALUES (1, 'Default');
SELECT setval(pg_get_serial_sequence('tenants', 'id'), 1);

ALTER TABLE users ADD COLUMN tenant_id INTEGER NOT NULL DEFAULT 1
    REFERENCES tenants (id) ON DELETE RESTRICT;
ALTER TABLE users ALTER COLUMN tenant_id DROP DEFAULT;
ALTER TABLE users DROP CONSTRAINT users_username_key;
ALTER TABLE users ADD CONSTRAINT users_tenant_username UNIQUE (tenant_id, username);
-- For composite foreign keys below.
ALTER TABLE users ADD CONSTRAINT users_tenant_id_key UNIQUE (tenant_id, id);

ALTER TABLE user_identities ADD COLUMN tenant_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE user_identities ALTER COLUMN tenant_id DROP DEFAULT;
ALTER TABLE user_identities DROP CONSTRAINT user_identities_pkey;
ALTER TABLE user_identities ADD PRIMARY KEY (tenant_id, issuer, subject);
-- The identity's tenant is always its user's tenant.
ALTER TABLE user_identities ADD FOREIGN KEY (tenant_id, user_id)
    REFERENCES users (tenant_id, id) ON DELETE CASCADE;

ALTER TABLE login_attempts ADD COLUMN tenant_id INTEGER NOT NULL DEFAULT 1;  -- no FK, see below
ALTER TABLE login_attempts ALTER COLUMN tenant_id DROP DEFAULT;
ALTER TABLE login_attempts DROP CONSTRAINT login_attempts_pkey;
ALTER TABLE login_attempts ADD PRIMARY KEY (tenant_id, username, source);

ALTER TABLE oidc_pending ADD COLUMN tenant_id INTEGER NOT NULL DEFAULT 1;
ALTER TABLE oidc_pending ALTER COLUMN tenant_id DROP DEFAULT;
```

Notes:

- **The tenant id is an integer**, as proposed, so that it fits in an environment variable name
  (`HOMESPACE_JWT_SECRET_2`). It is never secret and never the only thing that grants access.
- **`NOT NULL`, backfilled with 1, no lasting default.** `ADD COLUMN ... NOT NULL DEFAULT 1`
  fills every existing row with tenant 1 in the same statement; `ALTER COLUMN ... DROP DEFAULT`
  right after leaves the column `NOT NULL` without a default. From then on every insert names
  its tenant: a new user takes the tenant of the request (or of `--tenant` in the CLI), an
  identity or a pending sign-in takes the tenant of its user or request. An insert that forgets
  it fails with a `NOT NULL` violation, which tests catch, instead of quietly landing in tenant 1.
  Rows below a user (nodes, sessions, ...) have no column of their own and take the tenant from
  their user through `owner_id` / `user_id`.
- **`ON DELETE RESTRICT`, not `CASCADE`.** A cascade from `tenants` would delete every node,
  version, session and search entry of a tenant in one transaction (long locks on the shared
  tables, a huge WAL burst), and the blobs on disk would stay behind. Deleting a tenant reuses
  the existing user-deletion job: mark the tenant, mark its users with `delete_at`, let
  `delete_users` remove each user and their files, then delete the empty tenant.
- **The address-wide throttle row stays shared.** `login_attempts` rows with username `''`
  count wrong passwords per address over all usernames. Keeping them per tenant would let an
  attacker get `FREE_ADDRESS_FAILURES` free guesses per tenant. They are stored with
  `tenant_id = 0`, a value no tenant has, which is why `login_attempts.tenant_id` has no foreign
  key. Rows for a real username are per tenant.
- **`user_folders`, `devices`, `sessions` and the rest need no column.** They are only reached
  through a user id.

#### Where tenant_id goes

The request puts `tenant_id` on every table and a tenant filter on every query. That is about
twenty tables and most of the roughly 200 queries in `server/crates/db`. I recommend against it
as the first step:

- Almost every query is already scoped by `owner_id`, `user_id` or `caller`, which comes from a
  verified token. Once a token can only be used on its own tenant's domain (next section), an
  owner-scoped query can't return another tenant's rows. Adding `AND tenant_id = $n` there adds
  nothing a test could catch.
- The real leaks are the queries that look **across users** or **by a value that isn't a user**:
  listing users, devices visible to "everyone", share recipients, admin endpoints, username
  lookups, share tokens, refresh tokens. That list is short and is spelled out in
  [Queries that cross users](#queries-that-cross-users). Each one gets a `TenantId` argument and
  an isolation test.
- Denormalising onto `nodes` means a backfill of the biggest table, a composite foreign key
  `(tenant_id, owner_id) → users (tenant_id, id)` on each child table so the copy can't drift,
  and an extra column in every insert of the move/merge/trash code. All of it for a value that
  can be joined from `users`.

If defence in depth is wanted later, the right tool is Postgres row-level security with the
tenant set per transaction (the code already does this for the device in
`homespace.device_id`), and that is when denormalising pays off. It is listed as phase 2 in
[Steps](#steps) and as an open question.

### Resolving the tenant of a request

**From the `Host` header, confirmed by the token.** The claim alone can't work: `/auth/login`,
`/auth/oidc/*`, `/auth/providers`, `/capabilities`, `/auth/refresh` (a browser sends only the
cookie) and `/public/shares/*` carry no token, and they must already know the tenant (which IdP,
which users, which share). The `Host` alone is not enough either, because a token of tenant A
replayed against tenant B's host must fail even if an operator reused a signing secret.

A new extractor in `server/crates/api/src/extract.rs`:

```rust
/// The tenant a request was sent to, by its `Host`.
pub struct Tenant(pub Arc<TenantContext>);
```

- `HOMESPACE_TENANT_MODE=single` (the default): every request is tenant 1, whatever its host.
  This is today's behaviour, so installs reached by IP address, `localhost`, a LAN name and a
  Tailscale name at once keep working. Starting in `single` mode with more than one tenant in
  the database is refused.
- `HOMESPACE_TENANT_MODE=host`: the tenant whose `tenant_domains.domain` equals the request's
  host (lower-cased, default port stripped). An unknown host gets `421 Misdirected Request`
  without a body that names other tenants. A disabled tenant gets 503.
- Behind a reverse proxy, `Host` must be passed through (`proxy_set_header Host $host;` in
  nginx; the shipped `docs/nginx` config already does). `X-Forwarded-Host` is only read with
  `HOMESPACE_TRUST_FORWARDED_FOR`, like `X-Forwarded-For`.

A forged `Host` is not an attack in itself: it selects a tenant, it does not grant anything in
it. Credentials, tokens and share links are still checked against that tenant.

`AuthUser` gains `tenant: TenantId` and is built only when the token's `tid` equals the
request's tenant. `AdminUser` becomes a tenant administrator (see [Administration](#administration)).

### Tokens

All three JWT kinds (`access`, `ticket`, `share`) get a `tid: i32` claim, signed with the
tenant's own keys and checked on decode against the request's tenant:

```rust
struct AccessClaims { typ: String, tid: TenantId, sub: UserId, sid: Uuid, iat: i64, exp: i64 }
```

- `AccessClaims` and `ShareClaims` deny unknown fields, so a server **rolled back** to a version
  without tenants refuses new tokens. That is harmless: clients get 401 and refresh, because
  refresh tokens are opaque. No compatibility shim: a token without `tid` is refused, clients
  refresh once (access tokens live 15 minutes by default), and nobody is signed out.
- The `kid` stays a hash of the secret. Two tenants with the same secret would share `kid`s;
  the server refuses to start in that case (see [Per-tenant configuration](#per-tenant-configuration)).
- Session check: `live_session(sid, sub)` gets the tenant and joins `users` on it. With `tid`
  checked and the session tied to a user of that tenant, a token can't move.

### Auth flow

| Step | Change |
|---|---|
| Password sign-in (`login_from`) | `find_user_by_username(tenant, name)`. Throttle rows per `(tenant, username, source)`; the address-wide row stays at tenant `0`. `is_device_of_username` gets the tenant. |
| Hashing semaphore | Stays one per process. A burst of sign-ins on one tenant slows sign-ins on the others; acceptable for small installs, noted as an open question. |
| Refresh (`refresh_with`) | `find_refresh_token` joins `users` and checks the user's tenant equals the request's. A refresh token sent to another tenant's domain is refused **without** revoking its family, otherwise anyone could end other tenants' sessions by replaying a leaked token on another host. |
| Logout | Same check. |
| Device sessions (`start_device_session`) | Parent session must belong to the tenant (comes with the access token check). |
| Tickets (`/ws`, downloads) | `tid` in the ticket; `verify_ticket` uses the request tenant's keys. `used_tickets` is keyed by `jti`, which is unique, so it can stay shared. |
| OIDC start | Uses the tenant's `OidcService`; `oidc_pending` stores `tenant_id`. |
| OIDC callback | `take_oidc_pending` only takes a row of the request's tenant. The IdP calls back on the primary domain's callback URL, so each tenant registers its own callback with its IdP. |
| OIDC provisioning | `find_user_by_identity(tenant, issuer, subject)`, `create_user_with_identity` with the tenant; the username candidate loop is per tenant. |
| OIDC exchange | `oidc_codes` is tied to a user; the exchange checks that user's tenant. |
| `/auth/providers`, `/capabilities` | Answer for the request's tenant: password sign-in on or off and the IdP button name are per tenant. |
| Share links (`/public/shares/{token}`) | `share_by_token(tenant, token)` joins `shares → users` on the tenant. A share link of tenant A opened on tenant B's domain is "not found". Share access tokens carry `tid`. |
| Browser `Origin` check | `is_own_origin` compares with the request's host or the tenant's primary public URL instead of the one `HOMESPACE_PUBLIC_URL`. The cookie needs no change: browsers already keep it per host. |
| Websockets | `Hub` and the "sessions ended" broadcast stay keyed by `UserId` (UUIDs don't collide). The upgrade goes through the same ticket/token check. |

### Per-tenant configuration

Secrets and IdP settings stay out of the database, as requested. A small seam makes the source
replaceable:

```rust
/// Where per-tenant settings come from: environment variables now, a vault later.
#[async_trait]
pub trait TenantSecrets: Send + Sync {
    async fn get(&self, tenant: TenantId, key: &str) -> Result<Option<Secret>>;
}

/// `HOMESPACE_JWT_SECRET_2`, `HOMESPACE_JWT_SECRET_2_FILE`, … Tenant 1 falls back to the
/// unsuffixed name, so existing installs need no change.
pub struct EnvSecrets;
```

Per tenant (`{NAME}_{ID}`, also as `{NAME}_{ID}_FILE` where today's variable has a `_FILE` form):

| Setting | Variable |
|---|---|
| Signing secret, previous secret | `HOMESPACE_JWT_SECRET`, `HOMESPACE_JWT_SECRET_PREVIOUS` |
| OIDC | `HOMESPACE_OIDC_ISSUER`, `_CLIENT_ID`, `_CLIENT_SECRET`, `_SCOPES`, `_REQUIRED_GROUP`, `_GROUPS_CLAIM`, `_NAME` |
| Password sign-in on or off | `HOMESPACE_PASSWORD_LOGIN` |
| Public URL override | `HOMESPACE_PUBLIC_URL` (otherwise `https://<primary domain>`; for `http://localhost` in development) |

Server-wide and unchanged: `DATABASE_URL`, storage, bind, upload/transfer limits, TTLs, change
retention, `HOMESPACE_TRUST_FORWARDED_FOR`.

Rules:

- **Tenant 1 falls back to the unsuffixed name; no other tenant falls back to anything.** A
  tenant 2 without `HOMESPACE_JWT_SECRET_2` must not quietly sign with tenant 1's secret.
- The same `check_secret` rules apply to each tenant's secret, and the server refuses to start
  if two tenants have the same signing secret.
- A tenant whose secret is missing or invalid is **not loaded**: it answers 503 and the
  server logs why, while the other tenants keep working. Refusing to start would let one
  tenant's typo take every tenant down. (Open question.)
- Clap's `env = ...` can't express dynamic names, so per-tenant settings move from `Config` into
  a `TenantConfig` read through `TenantSecrets` at start-up. The unsuffixed flags stay on
  `Config` for tenant 1 and the CLI.

At start-up `serve()` builds a `Tenants` registry: for each enabled tenant one `TenantContext`
holding its domains, public URL and an `AuthService` with its own `Keys` and `OidcService`.
The `AuthService`s share the `Db`, the hashing semaphore, the ticket cache and the "sessions
ended" channel. OIDC discovery runs per tenant, in parallel, with a failure isolating that
tenant only.

**Changes need a restart.** Adding a tenant or a domain from the CLI writes the database, and
secrets live in the environment, which a running process can't re-read. The CLI prints
"restart the server to apply". A reload (`SIGHUP`, or Postgres `LISTEN tenants_changed` for the
domain table) can come later; secrets would need the vault to change without a restart anyway.

### CLI

```
homespace tenant add <domain> [--name <name>]      # prints the new id and the variables to set
homespace tenant list                              # id, name, domains, state, user count
homespace tenant rename <id> <name>
homespace tenant domain add <id> <domain> [--primary]
homespace tenant domain primary <id> <domain>
homespace tenant domain remove <id> <domain>       # refuses the primary
homespace tenant disable|enable <id>
homespace tenant delete <id> [--now]               # marks users and tenant for deletion

homespace-ctl user add|list|admin|passwd ... [--tenant <id|domain>]
```

`--tenant` defaults to 1 while tenant 1 is the only tenant, and is required once there are two,
so a script can't create a user in the wrong tenant.

**Renaming a domain is "add a new primary, keep the old one as an alias"**, never an in-place
update. A domain is baked into more than the display name:

- the OIDC callback URL registered at the IdP (the new one must be registered first);
- the WebAuthn relying party id of protected folders (see below): a changed domain makes every
  enrolled security key useless;
- share links already sent around;
- the server URL saved in every desktop and mobile client.

### Queries that cross users

These are the places where a missing tenant filter would leak. Each gets a `TenantId` argument
and a test with two tenants.

| Where | Query | Leak without a filter |
|---|---|---|
| `db/users.rs` | `find_user_by_username`, `create_user*`, `find_user_by_identity`, `list_users` | Sign-in to another tenant's account; the share picker lists every user of the server |
| `api/routes/users.rs` | `GET /users` | Same |
| `db/devices.rs` | `transfer_targets`, `can_send_to` (`can_see_device!` with `visibility = 'everyone'`) | Send-to-device lists and sends to devices of other tenants |
| `db/devices.rs` | `set_device_visibility` (viewer ids), `device_viewers` | A device made visible to a user of another tenant |
| `db/login_attempts.rs` | `is_device_of_username` | A known device in one tenant skips throttling in another |
| `db/shares.rs` | `create_user_share` (recipient ids), `share_by_token` | Sharing with, or opening shares from, another tenant |
| `api/routes/shares.rs` | `check_recipients` | Same, checked before the insert |
| `db/transfers.rs`, `files/transfers.rs` | `create_transfer` recipient | A transfer to another tenant's user |
| `db/accounts.rs` | `accounts`, `account`, `set_quota`, `set_disabled`, `mark_for_deletion`, `cancel_deletion` | A tenant admin manages another tenant's users |
| `db/storage.rs` | `storage_by_user` | Usernames and usage of every tenant in the admin status |
| `db/refresh_tokens.rs`, `db/sessions.rs`, `db/oidc.rs` | see [Auth flow](#auth-flow) | Session use or revocation across tenants |

Background jobs (`prune`, `expire_uploads`, `expire_transfers`, `delete_users`, search catch-up)
work by id across all tenants and need no filter. `compact_changes`, `prune_changes` likewise.

Recipient checks should compare tenants in SQL (`JOIN users r ON r.id = ANY($2) AND r.tenant_id = $3`
and comparing the counts) rather than loading users in Rust, so they hold under concurrency.

### Administration

- `users.is_admin` becomes **tenant administrator**: everything under `/admin` is scoped to the
  tenant of the request.
- `/admin/status` today shows the storage root path, disk space, the database version and the
  configuration summary. Those describe the whole server and are not a tenant admin's business.
  For tenants other than 1, hide them (or show only the tenant's usage). For tenant 1 keep them,
  so a single-tenant install looks the same as today.
- Managing tenants is CLI-only, by whoever runs the server. There is no "super admin" in the
  web UI. (Open question.)

### File storage

Paths are made of UUIDs only, so tenants can't reach each other's files through them, and
**no file has to move**. For new tenants, a prefix still helps the operator:

- `BlobStore` gets a per-tenant root: tenant 1 keeps `<root>/blobs/...` as it is, tenant *n*
  uses `<root>/tenants/<n>/blobs/...` (likewise `versions`). `tmp`, `uploads` and `transfers`
  stay shared: they are keyed by random ids and short-lived.
- That makes "how much does tenant 3 use", backing up or exporting one tenant, and deleting a
  tenant a directory operation.
- A transfer between two users is always within one tenant, so `link_into_transfer` never
  crosses tenant roots.

`FileService` takes the owner's tenant along with the owner id (it is on `AuthUser`), so the
store picks the root without a lookup.

## Interaction with other plans

- **Protected folders (PR #107).** The WebAuthn PRF relying party id is the `public_url` host,
  and security keys are bound to it for good. With tenants it must be **stored per tenant at
  first enrolment** (a column on `tenants`, or with the vault records) and never re-derived from
  the current primary domain. Otherwise moving a tenant to a new domain locks its users out of
  their encrypted folders. The RP id must also be a registrable suffix of every alias the web
  client may be used on, or the web client only works on the primary domain. The native
  clients' rule "same RP id everywhere" becomes "the tenant's RP id".
- **Storage quotas (merged).** Per-user quotas are unchanged. A per-tenant quota (the sum of
  its users) is a natural addition, and the per-tenant storage root makes it cheap to report.
  The free-space check (`disk_space`) is shared by all tenants on the same disk: one tenant
  filling the disk stops uploads for all of them. Worth a per-tenant quota before hosting
  strangers.
- **Object storage.** Its plan already takes an optional key prefix (`HOMESPACE_STORAGE_URL`
  `s3://bucket/prefix`). Per tenant: tenant 1 uses the configured prefix, tenant *n* uses
  `<prefix>/tenants/<n>/`. A bucket per tenant is possible later through `TenantSecrets`
  (`HOMESPACE_STORAGE_URL_3`), which would also let a tenant keep its data in its own account.
  The `blob_journal` table hangs off `owner_id` and needs no column.
- **Recycle bin.** Per user (`user_folders`), unaffected. `.recycle-bin-<id>` stays unique.
- **Send to device.** Device visibility `everyone` must mean "everyone in my tenant"; covered in
  the query table. The `Hub` presence check is keyed by user and device ids and needs nothing.
- **Sharing.** User shares and their recipient picker are tenant-bounded; public links resolve
  only on their own tenant's domains.
- **Native Tailscale.** The server joins one tailnet as one node (`homespace.<tailnet>.ts.net`),
  and its plan adds that name as a second origin of the one public URL. With tenants, the
  tailnet name is just another domain: in `host` mode it has to be a `tenant_domains` alias of
  exactly one tenant, so the tailnet only reaches that tenant. Several tenants over Tailscale
  would need one tailnet node per tenant; out of scope here.
- **Login throttling (PR #44).** Covered above: per tenant for usernames, shared for addresses.

## Testing

- An integration test helper that sets up two tenants with two domains and a user of the same
  name in each, used by one test per row of [Queries that cross users](#queries-that-cross-users)
  and per step of [Auth flow](#auth-flow): token of A on B's host, refresh token of A on B's
  host (refused, family not revoked), share link of A on B's host, OIDC state of A on B's
  callback, `GET /users`, device targets, transfers, admin endpoints.
- Unit tests for host resolution (case, port, IDN in punycode, unknown host, `single` mode).
- Config tests: tenant 1 falls back to unsuffixed variables, tenant 2 doesn't; same secret in
  two tenants refused; a tenant with a missing secret isn't loaded and answers 503.
- Migration test on a database with users, identities and login attempts: everything is in
  tenant 1, sign-in, refresh and OIDC still work in `single` mode without new variables.
- The existing test suite runs unchanged in `single` mode.

## Steps

Phase 1, each a reviewable PR:

1. **Schema and types.** Migration above, `TenantId` in `db`, `tenant_id` on `User`, per-tenant
   uniques. Every call passes `TenantId(1)`. No behaviour change.
2. **Tenant registry and configuration.** `tenants`/`tenant_domains` reads, `TenantSecrets` +
   `EnvSecrets`, `TenantContext` with its own `AuthService`, `HOMESPACE_TENANT_MODE`.
3. **Request tenant and tokens.** `Tenant` extractor, `tid` in all three token kinds, the auth
   flow changes, `is_own_origin` per tenant.
4. **Cross-user queries.** The table above, with the two-tenant tests.
5. **Administration and CLI.** Tenant-scoped `/admin`, `homespace tenant ...`, `--tenant` on
   `homespace-ctl user`.
6. **Storage roots** for new tenants, tenant deletion through the user-deletion job.
7. **Docs.** `docs/multi-tenancy.md` for operators: DNS, proxy `Host`, variables per tenant, IdP
   callback per tenant, restarting after changes.

Phase 2, optional:

8. Row-level security as a second line of defence: `tenant_id` denormalised onto the owned
   tables with composite foreign keys, `SET LOCAL homespace.tenant_id` per transaction, and
   policies. This is where the request's "tenant_id everywhere" fits best.
9. A vault implementation of `TenantSecrets` (HashiCorp Vault / OpenBao, Azure Key Vault), and
   reloading tenants without a restart.

## Open questions

1. **Where `tenant_id` goes**: only on `users` and the tables not reached through a user (the
   recommendation), or on every table from the start as proposed?
2. **A tenant with a missing or bad secret**: skip it and serve the others (recommended), or
   refuse to start?
3. **`/admin/status` for tenant administrators**: hide server-wide facts (disk, database,
   storage path, configuration) from tenants other than 1, or from every tenant once there are
   two?
4. **Server operator in the web UI**: is the CLI enough to manage tenants, or should tenant 1's
   admins become server admins who can see and manage other tenants?
5. **Address throttling across tenants**: shared (recommended, so an attacker gets no free
   guesses per tenant) or per tenant (so one tenant's users behind a NAT don't slow another's)?
6. **Per-tenant limits**: should upload size, transfer size, TTLs and a total quota become
   per-tenant settings now, or stay server-wide?
7. **Tenant storage root**: per-tenant directories for new tenants only (recommended, no data
   moves), or also move tenant 1 under `tenants/1/` for a uniform layout?
8. **Reload**: is a restart after `homespace tenant add` acceptable for now?
9. **Domain aliases**: should an alias redirect browsers to the primary domain (one cookie, one
   origin), or serve them as they are?
