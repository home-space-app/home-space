# Server clean-up and refactoring

**Status (2026-11-19):** phases 1–4 done (guardrails, mechanical splits, API contract, services),
phase 5 open. F5 waits for its second release, which drops the old `error`/`message` members.
Scope is `server/crates/*` and the error types in `shared/protocol`. The server is ~30.6k lines of
Rust, about half of it tests.

Two tests already failed on `main` before this work started. Both come from the tenant changes and
are not part of this backlog: `api/tests/tenants.rs` `a_device_shown_to_everyone_is_hidden_from_other_tenants`
(204 instead of 200), and `db` `sessions::tests::upgrade_keeps_live_logins_and_drops_dead_ones` (its
fixture inserts into `users` without `tenant_id`).

Each finding has an ID so a PR can say which ones it closes ("closes S3, L1"). Size is the usual S/M/L.
Line counts are production lines (everything before `#[cfg(test)]`) / whole file.

## Progress

| Done | Notes |
|---|---|
| G1 | `server/clippy.toml` and workspace `[lints]`. Each of the 7 existing offenders has an `#[expect]` naming its finding (K1, K7, K8, S5). |
| G2 | `server/scripts/check-file-length.sh` with `long-files.allow` (auth service, db nodes, files transfers). `.github/workflows/server-checks.yml` runs fmt, clippy `-D warnings` and the length check on pull requests that touch `server/` or `shared/`. |
| L5 and test moves | Test modules over ~250 lines moved to `<module>/tests.rs`: files service, auth service, db nodes, db devices, db shares, auth token, search index, files uploads. |
| T1–T7 | Types split out (`db/{nodes,shares,transfers}/types.rs`, `files/uploads/types.rs`, `search/error.rs`, `search/index/catch_up.rs`, `auth/token/{keys,access,ticket,share}.rs`, `api/extract/*`). `TicketQuery` moved to `protocol`. The outcome types in T1 stay in `db` until S3 moves them to `files`. |
| L10, L11, T8 | `app` is now `main`, `cli`, `server`, `jobs`, `commands/{user,search}` and `config/{tenant,checks}`. The 78-complexity `prune` loop became `prune_once` with one `report` helper. Job logs now skip "0 removed" lines, as the upload and transfer jobs already did. |
| D1 | One `From<DbError> for ApiError`. The two special meanings stay at their single call site (`marked_for_deletion` for `set_disabled`, `invalid_viewer` for `set_device_visibility`). Behaviour is unchanged, except that a `NotFound` from the routes that used `Internal(err.to_string())` is now a 404, not a 500. |
| D2 | `api/src/support/{archive,body,convert,disk,download,publish}.rs`. Route modules no longer import each other. `publish_transfer_expired` reuses the transfer publisher. |
| D4 (partly) | Conversions and clamping live in `support/convert.rs`. The device list still builds `Device` inline, because it needs the hub's online state. |
| K6 | `request_host` and `forwarded_for` share `last_header` and `last_entry`, with new unit tests for the host. |
| F1 | Every error is `application/problem+json` (`protocol::Problem`): `type`, `title`, `status`, `detail`, `instance` (filled in by the `error::with_instance` middleware), `code`, and the extension members `retry_after`, `limit`, `quota`/`available`, `current_offset` and `errors`. Types and statuses are listed in [`docs/api-errors.md`](../api-errors.md). |
| F2 | `ApiJson`, `ApiQuery` and `ApiPath` (`api/src/extract/{json,query,path}.rs`) turn axum's rejections into problems: 400 when the input cannot be read, 415 for a wrong media type, 413 for a body too large. The API router answers unknown paths and wrong methods (405, with `Allow`) as problems. A 416 is a problem with `Content-Range`. |
| F3 | `protocol::Validate` on every JSON body and query, checked by `ApiJson`/`ApiQuery`. Failures are one 422 `validation-failed` with every field at fault in `errors[]` (JSON Pointers). The rules live in `protocol::rules` and the services use the same functions (usernames, passwords, device names). Node names are checked before the database `CHECK`, which stays as the last line of defence; a NUL in a name is now a 422, not a 500. The share limits moved out of the routes. Rules that need stored state (a share for its owner, link vs. user share) stay in the routes as `ApiError::invalid(pointer, detail)` until S1. |
| F4 | 37 problem types in `protocol::ProblemType`, one row each (status, slug, code, title). Status changes: input that parses but is not acceptable is 422 (was 400); enabling an account marked for deletion is 409 `marked-for-deletion` (was 400); `AuthError::Oidc` is 502 `identity-provider-failed` (was 500); `OidcDenied` is 403 (was 500); `too-many-connections` sends `Retry-After: 30`. The `conflict`s are told apart by type (`name-taken`, `lost-and-found`, `offset-mismatch`, `transfer-not-ready`). A protocol test checks `docs/api-errors.md` against the table. |
| F5 (first release) | The server sends `error` and `message` next to `code` and `detail`. `api-client` and the web upload read `Problem` through `code()` and `detail()`, which also read an old server's body. `ErrorCode` is unchanged, so clients branching on it behave as before. |
| D5 | `protocol::AsProblem` (`problem_type`, `extend`) is implemented by `AuthError`, `FilesError` and `SearchError` in their own crates with exhaustive matches, and by `ApiError`, which only delegates. A new variant does not compile until it names a problem type. |
| S1, S4 | New crates `accounts` (`AccountService`, `DeviceService`) and `shares` (`ShareService`). They are built per tenant in `TenantContext::new` from the process-wide `SharedServices`, next to `auth`. Every route makes one service call; the share rules (link vs. users, not for the owner, token length, expiry), the admin rules (not yourself, marked for deletion, no password) and the device rules live in the services, each with its own error type implementing `AsProblem`. `AdminUser` calls `AccountService::require_admin`. `ShareLink` and `ReceivedShare` share `db::SharedAccess` (root, expiry), so the service has one `live` check and one `file`/`archive`/`children` path for both (D3, second half). **Fix found on the way:** the admin account queries were not scoped by tenant, so an administrator of one tenant could list, change and delete the accounts of another. `db::accounts`, `db::account` and `db::storage_by_user` now take the tenant, and `AccountService` checks the account before every change (test `an_administrator_manages_only_the_accounts_of_their_own_tenant`). |
| S2, K5 | New crate `events`: `DomainEvent` (node created/updated/moved/deleted, content changed, trash changed, devices changed, transfer ready/changed) and `EventSink`/`Events`. The file, device and transfer services publish their own events; `api::EventDispatcher` is the one place that turns them into search index updates and hub messages, in the same order the handlers used. Handlers no longer know about search or the hub, `support/publish.rs` and `api::publish_transfer_expired` are gone, and the expiry job no longer needs the hub. Handlers pick the status from the outcome (`support/outcome.rs`). |
| S5, D7 | `FileService` is split over a shared crate-private `ContentStore` (database, blob store, quota, `finish_store`, events) into `NodeService`, `VersionService`, `TrashService`, `UploadService` and `TransferService`, bundled in `FileServices` (`FilesConfig` replaces the `with_*` builders). Changes take an explicit `files::Actor { user, device }` instead of `on_device` cloning; `AuthUser::actor()` builds it. `files/transfers.rs` is split into `transfers/{manifest,save}.rs`. `AppState` holds services only: `db` is gone, the change journal is `NodeService::changes`, the health check is `FileServices::ping`, and the maintenance jobs get the database from `main`. |
| S6, K1 | `AuthService` is a facade over `PasswordHasher`, `LoginThrottle`, `Sessions`, `Tickets` and `OidcLogin` (`auth/src/{hasher,throttle,sessions,tickets,oidc_login}.rs`); 1168 → 462 lines. `LoginThrottle::admit(counters)` returns an `Attempt` settled once with the outcome: a success clears the counts, a wrong password stays counted, anything else is taken back. The three compensating `take_back_login_attempt` calls of `login_from`, `change_password` and `unlock_share` are gone, and the existing tests are unchanged apart from where they reach into the internals. Share passwords and share-access tokens are `auth::SharePasswords`, which `ShareService` holds instead of the whole `AuthService`. |
| D3 | The owner's downloads (file and version) go through `support::download::ranged` like shares and transfers, so they resume with `Range`; `attachment` is gone. |
| K2, T9 | `files::BodyError { Interrupted, TooLarge }` is the error of every request body stream; `support::body::limited_body_stream` enforces the chunk size. `FilesError::BodyInterrupted` replaces the `axum::Error` downcast and `ChunkTooLarge` is gone. |
| K4 | `protocol::Patch<T> { Keep, Clear, Set(T) }`, with its own serde (missing, `null`, value), replaces every `Option<Option<T>>`: `UpdateShare.{password,expires_at}`, `UpdateAccount.quota_bytes`, `UpdateRequest.parent_id`, `db::ShareUpdate` and `db::NodeUpdate`. The wire format is unchanged; the clients build `Patch` too. |
| K7 | The websocket loop is one `select!` that asks a `Connection` what to do for each event (`on_session_notice`, `on_ping`, `on_message`, `forward`) and acts on the returned `Step`. |
| K8 | `load_hosts` is `validate_tenants` → `ensure_distinct_secrets` → `discover_all` → `contexts`, with `validate_tenant` and `tenant_auth` for one tenant. |

New findings from G1:

| ID | Size | Where | Finding |
|---|---|---|---|
| K7 | M | `api/src/routes/ws.rs` `session` | Cognitive complexity 28. The socket loop handles pings, idle, expiry, session end and messages in one `select!`. Split it into one handler per event. |
| K8 | S | `app/src/tenants.rs` `load_hosts` | Cognitive complexity 52. It validates, discovers OIDC and builds contexts in one loop. Split it into `validate_tenants`, `discover_all` and `contexts`. |

New findings from phase 4:

| ID | Size | Where | Finding |
|---|---|---|---|
| K9 | S | `files/src/uploads.rs` `create_upload` (too long), `append_chunk_as`, `files/src/transfers/save.rs` `save_transfer_as` (complexity) | Still flagged after S5. Each mixes checking the target, writing bytes and recording the outcome; split them into named steps. |
| D8 | S | `shares` (`ShareService`) | The share rules that need stored state are in the service now, but the service still receives the request type of the wire (`CreateShare`, `UpdateShare`). Fine while the API is the only caller; give it its own input type when a second caller appears. |

## Ground rules

These are the rules the findings are measured against. New code follows them from now on.

1. **File length.** A production section over **500 lines** is split. **300–500** is on the watch list:
   do not grow it, split it the next time it is touched. A unit-test module over ~300 lines moves to
   its own file (`foo/tests.rs`).
2. **One public type per file**, plus its `impl`s. A small private helper type used in one place
   (a `struct Claim` inside one function, a two-field `Inner`) may stay next to its only user.
   Anything public, reused elsewhere, or with its own methods gets its own file.
3. **Layers.** `api` parses, validates, authorizes the caller, calls **one** service method and
   maps the result to HTTP. Services (`auth`, `files`, new ones below) own the rules and the
   side effects. `db` stores and loads. It has no policy and no domain errors.
4. **Errors.** Every error response is `application/problem+json` (RFC 9457). Each error has one
   problem type and one status code. A new variant cannot become a 500 by accident.
5. **KISS.** No error smuggled through `io::Error` and downcast later. No `Option<Option<T>>`. No
   clever iterator chains where a loop reads better. Add traits only where a second implementation
   is actually planned. Today that is the blob store (`object-storage.md`).

## Guardrails (do first)

| ID | Size | Item |
|---|---|---|
| G1 | S | Add `server/clippy.toml` with `too-many-lines-threshold = 80` and `cognitive-complexity-threshold = 20`. Enable `clippy::too_many_lines` and `clippy::cognitive_complexity` as `warn` in the workspace `[lints]`. Existing offenders get an `#[expect(...)]` naming the finding ID here, so the list cannot grow without anyone noticing. |
| G2 | S | Add a `scripts/` check that fails CI when a file's production section passes 500 lines. It counts lines up to `#[cfg(test)]`. |

## L: Long files

| ID | Prod / total | Fns | File | Action |
|---|---|---|---|---|
| L1 | 1165 / 1968 | 111 | `auth/src/service.rs` | Split by concern, see **S6**. |
| L2 | 1131 / 1589 | 52 | `db/src/nodes.rs` | `nodes/{query,store,move,versions}.rs`. Move policy out, see **S3**. |
| L3 | 600 / 646 | 32 | `files/src/transfers.rs` | Becomes `TransferService`, see **S5**. |
| L4 | 562 / 914 | 38 | `db/src/shares.rs` | `shares/{links,user_shares,read}.rs` and a types file. |
| L5 | 482 / **1669** | 99 | `files/src/service.rs` | 1187 lines of tests. Move them to `service/tests.rs`. Split the service, see **S5**. |
| L6 | 492 / 625 | 46 | `files/src/store.rs` | `store/{blobs,staging,uploads,transfers}.rs` behind the `BlobStore` trait, see **D6**. |
| L7 | 450 / 460 | 23 | `api/src/routes/shares.rs` | The rules move to `ShareService` (**S1**). Owner routes and `public/` routes go in separate files. |
| L8 | 436 / 715 | 35 | `files/src/uploads.rs` | Becomes `UploadService`, see **S5**. |
| L9 | 433 / 635 | 21 | `db/src/transfers.rs` | Types to their own file, queries stay. |
| L10 | 407 / 454 | 17 | `app/src/config.rs` | `config/{server,tenant,storage}.rs`. |
| L11 | 404 / 404 | 12 | `app/src/main.rs` | `cli.rs` (clap types), `jobs.rs` (prune, expire, delete loops), `commands/{user,search}.rs`. `main.rs` only wires. |
| L12 | 398 / 398 | 22 | `api/src/routes/nodes.rs` | Shared helpers leave (**D2**). Versions routes go to `routes/versions.rs`. |

**Watch list (300–500):** `api/routes/transfers.rs` 352, `app/tenants.rs` 313, `auth/token.rs` 309,
`db/devices.rs` 288, `db/trash.rs` 287, `auth/oidc.rs` 276, `search/index.rs` 273.

**Integration tests:** `api/tests/shares.rs` 1001, `transfers.rs` 970, `trash.rs` 750, `uploads.rs` 631,
`oidc.rs` 611. Split them into a folder per feature (`tests/shares/{links,users,public}.rs`), one file per
scenario group.

## T: Files declaring several types

| ID | File | Types | Action |
|---|---|---|---|
| T1 | `db/src/nodes.rs` | 15: `StoreOptions`, `NodeProperties`, `NewNode`, `NewContent`, `BlobOp`, `StoreOutcome`, `MoveOutcome`, `MoveConflict`, `MoveBatch`, `DeletedNodes`, `Effects`, `Placement`, `NodeUpdate`, … | Inputs (`NewNode`, `NewContent`, `NodeUpdate`) go to `nodes/types.rs`. Outcomes (`StoreOutcome`, `MoveOutcome`, `MoveBatch`, `MoveConflict`) are domain results and move to `files` with **S3**. `Claim` inside `plan_conflicts` may stay. |
| T2 | `db/src/shares.rs` | 9: `ShareLink`, `SharedRoot`, `ReceivedShare`, `OwnedShare`, `ShareRecipient`, `NewUserShare`, `NewShareLink`, `ShareUpdate` | `shares/types.rs`. `ShareLink` and `ReceivedShare` both have `is_expired` and `root`, see **D3**. |
| T3 | `auth/src/token.rs` | 8: `Keys`, `Claims`, `AccessClaims`, `TicketClaims`, `TicketGrant`, `ShareClaims`, `ShareGrant` | `token/{keys,access,ticket,share}.rs`, one token kind per file. |
| T4 | `db/src/transfers.rs` | 7: `Transfer`, `TransferItem`, `NewTransfer`, `IncomingScope`, `ExpiredTransfer`, `SubtreeNode` | `transfers/types.rs`. |
| T5 | `search/src/index.rs` | 7, including `SearchError` | `SearchError` goes to `error.rs` like every other crate. `CatchUp` and `Job` go to their own files. |
| T6 | `files/src/uploads.rs` | `UploadTarget`, `UploadStatus`, `ChunkOutcome`, `Hashing` | Public ones go to `uploads/types.rs`. |
| T7 | `api/src/extract.rs` | `Tenant`, `ClientIp`, `AuthUser`, `AdminUser`, `TicketQuery` | `extract/{tenant,client_ip,auth_user,admin_user}.rs`. `TicketQuery` is a wire type and moves to `protocol`. |
| T8 | `app/src/main.rs`, `app/src/config.rs` | `Cli`, `Command`, `SearchCommand`, `UserCommand` / `Config`, `StorageBackend`, `TenantSettings`, `TenantConfig` | Handled by **L10** and **L11**. |
| T9 | `api/src/routes/uploads.rs` | `ChunkTooLarge`, a marker error | Removed by **K2**. |

## S: Layering (API / business / DB)

| ID | Size | Finding | Target |
|---|---|---|---|
| S1 | M | **Routes talk to the database directly.** There are 25 `state.db.*` calls in `routes/{shares,shared,admin,devices,ws}.rs`. `routes/shares.rs` holds business rules: the 1–1000 item limit, the 1–100 recipient limit, "a link has no recipients", "a user share has no password", "expiry in the future", password hashing and token minting. | New `ShareService` (`crates/shares`, or a `files::shares` module), `DeviceService`, and an `AccountService` for admin. Handlers make one service call each. |
| S2 | M | **Side effects are wired in handlers.** Search indexing and `hub.publish` run in `publish_stored`, `publish_changes` and `publish_ready` (`routes/nodes.rs`, `uploads.rs`, `trash.rs`, `transfers.rs`), and in `api::publish_transfer_expired` in `api/src/lib.rs`, which `app` calls from a background job. Every new endpoint that changes nodes has to remember all three. | Services return or emit **domain events** (`NodeCreated`, `NodesMoved`, `ContentChanged`, `TrashChanged`, `TransferReady`, …). One `EventDispatcher` in the app turns each event into search jobs and hub messages. Handlers no longer know about search or the hub. |
| S3 | L | **The DB layer makes business decisions.** `db/src/nodes.rs` plans move conflicts (`plan_conflicts`), decides merge-or-version (`merge_file`, `archive_older`, `same_content`), numbers names (`numbered_name`) and clamps client times (`clamp_to_now`). `DbError` carries domain errors: `Cycle`, `LostAndFound`, `Pruned`. | `files` owns the policy. `db` exposes a transaction handle (`db::Tx`) with primitives: lock, find sibling, relocate, insert version, apply content. Planning becomes a pure function over a snapshot of the subtree, so it can be unit-tested without Postgres. The three domain variants leave `DbError`. **Largest item, do last.** It must stay one transaction. |
| S4 | S | **Authorization lookups in extractors.** `AdminUser` queries `db.find_user` and maps the error to `Internal(err.to_string())`. | `AccountService::require_admin(user)`. The extractor only calls it. |
| S5 | L | **`FileService` is a god object.** It is implemented across five files (`service`, `uploads`, `transfers`, `copy`, `archive`), carries transfer config (`with_transfers`), and is cloned per request by `on_device` to carry the device. | Split into `NodeService`, `UploadService`, `TransferService`, `TrashService` and `VersionService` over a shared `ContentStore` (blob store, quota, `finish_store`). Pass an explicit `Actor { user, device }` instead of `on_device` cloning. |
| S6 | L | **`AuthService` is a god object** with 111 functions. It hashes passwords (and runs the hashing semaphore), throttles logins, manages sessions and refresh, verifies access tokens, issues tickets, handles share passwords and share-access tokens, runs the OIDC flow, registers devices and generates usernames. | `PasswordHasher`, `LoginThrottle` (**K1**), `SessionService`, `TicketService`, `OidcLogin`. Share passwords and share-access tokens move to `ShareService`. `AuthService` stays as a thin facade only if the API still needs one. |

## D: SOLID / DRY / KISS

| ID | Size | Finding | Target |
|---|---|---|---|
| D1 | S | **`DbError` → `ApiError` is written five ways.** There are three `fn db_error` in `routes/{shares,devices,admin}.rs` with conflicting meanings: admin turns `Conflict` into a **400** about deletion, and devices turns `Invalid` into a 400 about devices. There are also 10 inline `map_err(\|err\| ApiError::Internal(err.to_string()))`. `auth.rs` borrows `super::shares::db_error`. | Write `impl From<DbError>` once. Context-specific meaning comes from service errors (**S1**), not from per-route mappers. |
| D2 | S | **Route modules import from each other.** `nodes::percent_encode` is used by `archive` and `content`. `trash::move_response` is used by `nodes`, and `nodes::publish_changes` by `trash`. `storage::{disk_space, usage}` is used by `admin`, and `shares::{db_error, shared_node, archive_name}` by `shared` and `auth`. | Add `api/src/support/{download,body,convert}.rs`. Route modules depend only on `support`, never on each other. |
| D3 | S | **Two download implementations.** `nodes::attachment` serves the owner's own files and versions without `Range`. `content::ranged` serves shares and transfers with `Range`. So an owner's interrupted download cannot resume, but a visitor's can. `ShareLink` and `ReceivedShare` also duplicate `is_expired` and `root`. | Use one `support::download` (ranged) everywhere. Add one `SharedAccess` trait or shared struct for the expiry and root logic. |
| D4 | S | **db → protocol conversion is done three ways:** `From` impls, free functions in routes (`shared_node`, `account`, `owner_views`), and inline struct literals (`devices::list`). Numbers are silently clamped in each place (`unwrap_or(u32::MAX)`, `unwrap_or_default()`, `to_u64`, `quota`). | Put all `From`/`TryFrom` in `api/src/support/convert.rs`. Decide once whether a negative size is a bug (log it) or 0. |
| D5 | M | **`ApiError::status_and_code` breaks open/closed.** It knows every variant of `AuthError`, `FilesError` and `SearchError`, and ends in `Self::Auth(_) \| Self::Files(_) \| … => 500`. A new variant silently becomes a 500. For example, `AuthError::Oidc` (identity provider unreachable) is a 500 today, not a 502. | Each crate's error implements a small `ProblemKind` trait (`status()`, `type_slug()`, `title()`) with an **exhaustive** match. `ApiError` only delegates. Done together with **F1**. |
| D6 | M | **No abstraction where one is planned.** `BlobStore` is a concrete local-filesystem struct, but `object-storage.md` adds S3 and Azure. | Add a `BlobStore` trait with a `LocalBlobStore`. Add no other traits until a second implementation exists (KISS). |
| D7 | S | **`AppState` is one bag** (`db`, `files`, `hub`, `search`, `tenants`) handed to every handler, so any handler can reach anything. | After S1, S2 and S5, `AppState` holds services only, not `db`. That makes the layering in rule 3 enforceable. |

## K: Complex or "trying to be smart" code

| ID | Size | Where | Problem | Target |
|---|---|---|---|---|
| K1 | M | `auth/src/service.rs` `login_from` (~95 lines) | It runs two throttle counters (per user+source and per address) and authentication side by side. It has three compensating `take_back_login_attempt` calls in different branches, guarded by negated `matches!`. Getting one branch wrong either locks users out or lifts the rate limit. | `LoginThrottle::admit(...) -> Attempt`. Then `attempt.succeeded()` or `attempt.wrong_password()`, and dropping or `attempt.not_checked()` takes the attempt back. `login_from` shrinks to about 15 lines. Keep the existing tests unchanged as the safety net. |
| K2 | S | `routes/uploads.rs` `ChunkTooLarge`, `error.rs` `is_interrupted_body` | Errors are smuggled through `io::Error`: a marker type is wrapped in `io::Error::other` and later found with `get_ref().is::<ChunkTooLarge>()`. Interrupted bodies are found by downcasting to `axum::Error` inside `FilesError::Io`. | A `LimitedBody` stream with a typed error. `FilesError::BodyInterrupted` and `FilesError::TooLarge` are produced where the condition happens, not reconstructed. |
| K3 | M | `db/src/nodes.rs` `plan_conflicts` | A breadth-first walk with a `HashMap<(parent, kind, name), Claim>`, a `reported` flag and `Entry` matches with guards. It is dense, async and hits the database inside the loop. | A pure function over a preloaded subtree (**S3**), with table-driven unit tests. |
| K4 | S | `routes/shares.rs` `update` | It uses `Option<Option<String>>` tri-states (`password_hash.as_ref().map(Option::as_deref)`). | `enum Patch<T> { Keep, Clear, Set(T) }` in `protocol`, with its own serde. |
| K5 | S | `routes/nodes.rs` `publish_stored` | It returns `(StatusCode, Json<Node>)` and also indexes and publishes. Callers destructure it (`let (_, Json(node)) = …`) to get the node back. | Goes away with **S2**. The service returns the outcome and the handler picks the status. |
| K6 | S | `extract.rs` `request_host`, `forwarded_for` | Long `Option` chains with `next_back()?.to_str().ok()?.rsplit(',').next()?.trim()`. | Keep the behaviour (it is tested) and rewrite as small named steps. Low priority. |

## F: API validation and RFC 9457 errors

| ID | Size | Finding | Target |
|---|---|---|---|
| F1 | M | **The error body is not RFC 9457.** Every error is `application/json` `{ "error": code, "message": text }` (`protocol::ErrorBody`). | `application/problem+json` with `type`, `title`, `status`, `detail` and `instance` (the request path), plus extension members: `code` (today's `ErrorCode`, so clients keep a stable machine key), `retry_after`, `limit`, `quota`/`available`, `current_offset`, and `errors` (see F3). `type` is a stable URI per problem (e.g. `/api/v1/problems/quota-exceeded`), documented in the README. |
| F2 | S | **Framework rejections skip `ApiError`.** A malformed JSON body, a bad UUID in the path, a missing query field, a wrong `Content-Type` and 405 all come back as axum's `text/plain` 400, 415 or 422, with axum's own wording. The `Range` 416 and the bare `StatusCode` answers have no body. | Add `ApiJson<T>`, `ApiQuery<T>` and `ApiPath<T>` extractors (or `#[from_request(via(...), rejection(ApiError))]`) that map rejections to problems. Add a `method_not_allowed_fallback` on the router. Use one rule: **400** when the input cannot be parsed, **422** when it parses but is invalid, **415** for a wrong media type. |
| F3 | M | **Validation is scattered and partly done by the database.** Node names are checked by a Postgres `CHECK` (`DbError::Invalid` → `FilesError::InvalidName`). Usernames are checked in `auth`, device names in `auth::device_name` (called from a route), share limits in the route, and the `Upload-Offset` header in the route. A client cannot learn every problem with a request at once. | Add a `Validate` trait on the `protocol` request types (hand-written, no macro crate) that returns `Vec<FieldError { pointer, detail }>`. `ApiJson`/`ApiQuery` call it, and the result is a **422** with `errors[]`. Services still enforce their invariants. Database constraints stay as the last line of defence, not the first. |
| F4 | S | **Ambiguous status codes and codes.** `DbError::Conflict` is a 400 in admin (should be 409). `UnknownHost` is a 421 with code `not_found`. `TenantUnavailable` is a 503 with code `internal`. `FilesError::{Conflict, LostAndFound, NotReady, OffsetMismatch}` are all 409 `conflict`, so clients have to parse the message. `Invalid` in devices means "unknown user, **or** the device's owner". `AuthError::Oidc` is a 500 (should be 502). `TooManyConnections` is a 429 with no `Retry-After`. | Each of these gets its own problem `type`. Keep a status-code table (status, problem type, when) in the README, checked by a test that walks every error variant. |
| F5 | S | **Breaking change for clients.** `clients/api-client/src/client.rs` and `clients/web/src/upload.rs` parse `ErrorBody`. | Release N: the server sends the problem fields **and** the old `error`/`message`, which RFC 9457 allows as extension members, and the clients switch to `code`/`detail`. Release N+1: drop the old fields. |

## Order of work

1. ~~**Guardrails:** G1, G2.~~ Done.
2. ~~**Mechanical, no behaviour change:** L5 test move, T1–T8, D1, D2, D4, L10, L11, K6.~~ Done. T9 waits
   for K2. K7 and K8 are new and can go with phase 4.
3. ~~**API contract:** F1, F2, F4, F5, then F3 with D5.~~ Done. Still open: the second F5 release,
   which removes `error` and `message` from `Problem` once the clients in use read `code`/`detail`.
4. ~~**Services:** S1, S4 (shares, devices, admin), S2 (domain events), then S6 with K1 (auth) and S5 (files).
   Then D7, D3 and K2, K4, K5.~~ Done, with K7 and K8. K9 and D8 are new and small.
5. **Domain/DB boundary:** S3 with K3, then D6 as preparation for `object-storage.md`.

Every step keeps the existing integration tests in `server/crates/api/tests` passing unchanged. Only F1, F2 and F4
(response shape) may update test assertions, and then only for the body format.
