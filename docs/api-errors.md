# API errors

Every error response of the server is an RFC 9457 problem, sent as
`application/problem+json`:

```json
{
  "type": "/api/v1/problems/quota-exceeded",
  "title": "Not enough storage left",
  "status": 507,
  "detail": "not enough storage: 2 of your 10 bytes are left",
  "instance": "/api/v1/files",
  "code": "quota_exceeded",
  "quota": 10,
  "available": 2,
  "error": "quota_exceeded",
  "message": "not enough storage: 2 of your 10 bytes are left"
}
```

- `type` names the exact problem. It is a path under `/api/v1/problems/`, relative to the
  server. Clients that need to tell failures apart match on it (`protocol::ProblemType`), never
  on the text of `detail`.
- `title` is the same for every occurrence of the type. `detail` says what went wrong this
  time. For `internal` and `identity-provider-failed` the detail is the title, and the cause is
  in the server log.
- `instance` is the path of the request that failed.
- `code` is a coarse category (`protocol::ErrorCode`), stable since the first release.
- `error` and `message` repeat `code` and `detail` for clients from before RFC 9457. They will
  be removed in a later release.

Extension members, present only where they apply:

| Member | Type | Sent with |
|---|---|---|
| `retry_after` | seconds | `too-many-attempts`, `server-busy`, `too-many-connections`, also as the `Retry-After` header |
| `limit` | bytes or items | `too-large`, `too-many-items` |
| `quota`, `available` | bytes | `quota-exceeded` |
| `current_offset` | bytes | `offset-mismatch` |
| `errors` | `[{ "pointer", "detail" }]` | `validation-failed`: every field at fault, as a JSON Pointer into the body or query |

## Status codes

The rule for input: **400** when the request cannot be read (malformed JSON, a wrong field
type, a bad id in the path, a missing header), **415** for a body in the wrong format, and
**422** when it can be read but a value is not acceptable.

| Type | Status | Code | When |
|---|---|---|---|
| `bad-request` | 400 | `bad_request` | The body, query, path or a header cannot be read. |
| `body-interrupted` | 400 | `bad_request` | The request body stopped arriving. What arrived of an upload is kept. |
| `unauthorized` | 401 | `unauthorized` | No session, an invalid, expired or revoked token, or a wrong username or password. |
| `forbidden` | 403 | `forbidden` | Signed in but not allowed, such as a non-administrator on `/admin`. |
| `account-disabled` | 403 | `forbidden` | The account is disabled or marked for deletion. |
| `sso-access-denied` | 403 | `forbidden` | The identity provider signed the user in, but they are not in the allowed group. |
| `not-found` | 404 | `not_found` | No such item, or it belongs to someone else. Also unknown API paths. |
| `sso-disabled` | 404 | `not_found` | Single sign-on is not set up. |
| `method-not-allowed` | 405 | `bad_request` | The path exists, but not for this method. |
| `name-taken` | 409 | `conflict` | An item with this name already exists in the folder. |
| `username-taken` | 409 | `conflict` | The username is taken. |
| `lost-and-found` | 409 | `conflict` | Lost & Found, or a folder holding it, cannot be deleted. |
| `offset-mismatch` | 409 | `conflict` | A chunk does not start at `current_offset`, where the upload ends. |
| `transfer-not-ready` | 409 | `conflict` | The transfer's files are still being uploaded. |
| `marked-for-deletion` | 409 | `conflict` | An account marked for deletion cannot be enabled before its deletion is cancelled. |
| `gone` | 410 | `gone` | The share has expired. |
| `changes-pruned` | 410 | `gone` | The changes asked for are no longer kept; list everything again. |
| `transfer-gone` | 410 | `gone` | The transfer was declined, cancelled, saved or has expired. |
| `login-expired` | 410 | `gone` | The browser sign-in of an app was already used, cancelled or has expired; start again from the app. |
| `too-large` | 413 | `payload_too_large` | The body or upload is larger than `limit` bytes. |
| `too-many-items` | 413 | `payload_too_large` | More than `limit` items at once, such as in a zip. |
| `unsupported-media-type` | 415 | `bad_request` | The body is not in the format the endpoint takes. |
| `range-not-satisfiable` | 416 | `bad_request` | The `Range` lies outside the file. `Content-Range` gives its size. |
| `unknown-host` | 421 | `not_found` | No site answers on the host the request was sent to. |
| `validation-failed` | 422 | `bad_request` | Some values are not acceptable; `errors` lists each one. |
| `wrong-password` | 422 | `bad_request` | The current password given to change it is wrong. |
| `no-password` | 422 | `bad_request` | The account signs in through single sign-on and has no password. |
| `move-into-itself` | 422 | `bad_request` | A folder cannot be moved into itself or into one of its folders. |
| `not-a-file` | 422 | `bad_request` | Only a file can do this, and the item is a folder. |
| `invalid-name` | 422 | `bad_request` | The name is empty, too long, `.` or `..`, or holds `/` or `\`. |
| `invalid-transfer` | 422 | `bad_request` | The list of files of a transfer is not acceptable. |
| `too-many-attempts` | 429 | `too_many_requests` | Too many wrong passwords; wait `retry_after` seconds. |
| `server-busy` | 429 | `too_many_requests` | Too many passwords are waiting to be checked; wait `retry_after` seconds. |
| `too-many-connections` | 429 | `too_many_requests` | The account has as many live connections as it may; wait `retry_after` seconds. |
| `internal` | 500 | `internal` | Something went wrong on the server. |
| `identity-provider-failed` | 502 | `internal` | The identity provider could not be reached or answered with something invalid. |
| `tenant-unavailable` | 503 | `internal` | The site of this host is disabled, being deleted, or not set up. |
| `browser-login-unavailable` | 503 | `internal` | Apps cannot sign in in the browser: the server does not serve the web client (`HOMESPACE_WEB_ROOT`). |
| `quota-exceeded` | 507 | `quota_exceeded` | Storing this would pass the account's `quota`; `available` bytes are left. |

A test in `shared/protocol` checks that this table lists every problem type with its status.
