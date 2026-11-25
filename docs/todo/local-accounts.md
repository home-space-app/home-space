# Local accounts: two-factor sign-in and passkeys

## Problem

HomeSpace has two ways to sign in today:

- **Username and password**, checked by the server (argon2id, throttled since PR #44).
  Administrators create these accounts from the admin pages or `homespace-ctl user`.
- **Single sign-on** through an OpenID Connect provider (Auth0, Keycloak, ...). The provider
  decides how the user proves who they are, so 2FA and passkeys work when the provider offers
  them. That is how passkeys work with HomeSpace today, through Auth0.

A server that does not run an identity provider only has passwords. The goal is to make local
accounts as strong as an identity provider account:

1. **Two-factor sign-in** with an authenticator app (TOTP, RFC 6238), and one-time recovery codes.
2. **Passkeys and security keys** (WebAuthn): as a second factor after the password, and as a
   passwordless sign-in on their own.
3. Both work **alongside single sign-on**. A tenant can offer passwords, a provider, or both, as
   it does now.

All clients have to keep working: web, desktop, Android and iOS, and older clients against a newer
server.

Not in scope: self-service password reset by e-mail (the server sends no e-mail yet),
self-registration, linking a password account to a provider account, SMS codes, push approvals.
See [Open questions](#open-questions).

### Where this leaves single sign-on

With a provider, 2FA and passkeys are the provider's job and HomeSpace does not see them. The
feature comparison should mark them **partly supported** in that case: they work, but only when the
provider offers them, and HomeSpace can neither require them nor show them. This plan adds an
optional check of the provider's `amr` claim, so a tenant can at least require that the provider
did use a second factor ([Single sign-on users](#single-sign-on-users)).

## What exists today

- `auth::AuthService` (one per tenant) is built from `PasswordHasher`, `LoginThrottle`,
  `Sessions`, `Tickets` and `OidcLogin`. `login_from` checks the password and starts a session in
  one call. `POST /auth/login` returns a `TokenPair` (and for a browser, the refresh token in a
  cookie).
- Single sign-on is a PKCE flow the server runs on behalf of the client: `GET /auth/oidc/start`
  sends the browser to the provider, `/auth/oidc/callback` comes back, stores a one-time code in
  `oidc_codes` and redirects to the client, which trades the code at `POST /auth/oidc/exchange`.
  Desktop listens on a loopback port (RFC 8252), Android uses a Custom Tab, iOS uses
  `ASWebAuthenticationSession`.
- A session is one row in `sessions`; access tokens and tickets are only valid while it exists.
  The session records nothing about *how* the user signed in.
- `users.password_hash` is `NULL` for provider users; `Me.external` tells clients.
- `login_attempts` counts wrong passwords per (tenant, username, source) and per address.
- The tenant's public URL (primary domain) is already the WebAuthn RP id of protected folders
  (`docs/todo/protected-folders.md`), which also plans a native `Authenticator` trait for CTAP2.
- Secrets are per tenant in env vars (`{ENVVAR}_{TENANT_ID}`), not in the database.

## Proposal in short

| Topic | Recommendation |
|---|---|
| Second factors | TOTP app, passkey or security key, recovery codes. Any one is enough. |
| Passkey as first factor | Yes: a passkey with user verification signs in without the password. |
| Sign-in protocol | `/auth/login` stays as it is for apps. For a user with 2FA it answers `403 second-factor-required` with a readable detail. The web client opts in to the two-step flow (`two_step: true`) and finishes at `/auth/login/verify`. |
| Sign-in on native apps | TOTP and WebAuthn only on the web. Apps open the server's web sign-in page in the system browser and get a session through PKCE, like single sign-on ("browser sign-in"). A server that reports `browser_login` gets only a "Sign in" button that opens the page; the native password form and the apps' use of `/auth/login` stay until 1.0.0 for older servers and servers without the web client. |
| WebAuthn library | `webauthn-rs` on the server; `navigator.credentials` through `web-sys` in the web client. |
| RP id | The tenant's primary domain host, as for protected folders. |
| Secret storage | TOTP secrets encrypted with a new per-tenant key `HOMESPACE_SECRETS_KEY`. Recovery codes hashed. |
| Sensitive changes | Need a recent sign-in (10 minutes) or re-confirming with password, code or passkey. |
| Enforcement | Off by default. A tenant setting can require 2FA for administrators or everyone. |
| Lost factors | Recovery codes; an administrator or `homespace-ctl` can reset a user's 2FA. |
| Provider users | No local factors. Optional `HOMESPACE_OIDC_REQUIRE_MFA` checks the `amr` claim. |

## Factors

### Authenticator app (TOTP)

- RFC 6238, SHA-1, 6 digits, 30-second steps: the only parameters every app (Google
  Authenticator, Aegis, 1Password, Microsoft Authenticator) handles. A 160-bit random secret.
- Enrolment: the server makes a secret and returns it with the `otpauth://` URI; the client shows
  a QR code (`qrcode` crate in Rust clients, rendered as SVG on the web) and the secret in text.
  It is only saved after the user enters one valid code, so a mistyped scan never locks anyone out.
- Accept the current step and one on each side (±30 s clock drift).
- No replays: store the last accepted step per user and refuse any step at or before it.
- One TOTP per user. Replacing it means removing it and adding a new one, each with a recent
  sign-in (decided 2026-11-25; `POST /me/totp` answers `409 totp-exists` while one is on).
- SHA-1 only, on purpose: several apps ignore the `algorithm` of the QR code and compute SHA-1
  anyway, and HMAC-SHA1 is not weakened by SHA-1's collisions. The window of one step either side
  stays (a code works up to 30 s after it rolls over); asked on 2026-11-25 whether a 2-second
  grace would do instead, it is kept as is unless decided otherwise.
- Done as a 40-line HMAC on `hmac` + `sha1` (`auth::totp`), tested against RFC 6238's vectors.

### Passkeys and security keys (WebAuthn)

Two kinds, one table:

| | Passkey | Security key as second factor |
|---|---|---|
| Credential | Discoverable (`residentKey: required`) | Non-discoverable (`residentKey: discouraged`) |
| User verification | `required` | `preferred` |
| Sign-in | No username, no password ("Sign in with a passkey") | After the password |
| Examples | iCloud Keychain, Google Password Manager, Windows Hello, 1Password, YubiKey with PIN | YubiKey without PIN |

A passkey with user verification proves possession and a PIN or biometric, so it counts as two
factors and the password is not asked. A non-discoverable security key without verification only
replaces the TOTP code.

- Server: `webauthn-rs` (from Kanidm, the most used Rust implementation, does attestation
  `none`, counters, backup flags). Its ceremony state is kept in the database between the two
  requests (feature `danger-allow-state-serialisation`; the state is not secret, only bound to
  the challenge), in a table that expires after 5 minutes, so it works with several server
  processes.
- Attestation: `none`. HomeSpace does not need to know the brand of the key.
- RP id: the host of the tenant's primary domain; RP name: the tenant name. The RP id is stored
  with each credential so a domain move can be detected and explained ("this passkey was made
  for files.old.example").
- Each credential has a name the user can change ("YubiKey 5C", "Pixel"), and shows when it was
  added and last used.
- Signature counters are checked when the authenticator sends them (synced passkeys send 0).
- A user may have up to 20 credentials.

**Same key as protected folders?** A YubiKey can hold a sign-in passkey and a protected-folder
unlocker, but they stay separate credentials and separate tables. The vault unlocker is key
material (`prf_salt`) with its own rules, and removing a sign-in passkey must never remove the
ability to decrypt. What is shared: the RP id helper, the `webauthn-rs` setup, the web client's
`navigator.credentials` wrapper and, later, the native `Authenticator` trait.

### Recovery codes

- 10 codes of 10 characters from an unambiguous alphabet (`xxxxx-xxxxx`), shown once when the
  first second factor is added, and again only by generating a new set (which voids the old one).
- Stored as SHA-256 (they are random and long, argon2 adds nothing), each usable once.
- Using one signs the user in and shows a warning with how many are left.

## Sign-in protocol

### Password, then second factor

```
POST /api/v1/auth/login            { username, password, device, cookie, two_step? }
  no second factor  → 200 TokenPair                      (unchanged)
  has one           → 403 problem "second-factor-required"
                        detail: "This account uses two-factor sign-in. Sign in in the browser,
                                 or update the app."
  has one, two_step → the same problem, plus
                        login_token: "…"                 (5 minutes, single use)
                        methods: ["totp", "webauthn", "recovery"]
                        webauthn: { publicKey: … }       (request options, only if the user has keys)

POST /api/v1/auth/login/verify     { login_token, totp | recovery_code | webauthn, device, cookie }
  → 200 TokenPair, or 401 unauthorized, or 429 too-many-attempts
```

- The current endpoint keeps working for apps until 1.0.0. Only a client that sends
  `two_step: true` (the web client) gets a `login_token`; every other caller just gets the
  error, so no app has to learn a second step.
- **403, not 401.** The API client turns every 401 into `ApiError::Unauthorized` and drops the
  body, and the sign-in screens show "Invalid username or password" for it. Any other problem
  arrives as `ApiError::Api { message: detail }`, which the released web client and the Android
  app already show as is (`session.rs` `login`, `signInMessage` → `Failed`). So with 403, even
  today's apps tell a 2FA user why they cannot sign in, instead of claiming the password is wrong.
  The problem is only sent after the right password, so it tells nothing to someone guessing.
- Clients should still show server errors better now, before 2FA ships (step 0 below), so the
  next problems need no such care.
- The `login_token` is a random value whose hash sits in a new `pending_logins` table
  (tenant, user, device, browser flag, expiry, attempts). It is not a JWT, so it can be used once
  and revoked.
- The password is checked **before** the second factor is asked, and the answer for a wrong
  password stays `401 unauthorized`. Asking for the factor only after a right password tells an
  attacker the password was right; that is accepted everywhere (GitHub, Google), because the factor
  is what protects the account then. The password step is throttled as today.
- Wrong codes are throttled with the existing `LoginThrottle`: a counter per (tenant, username,
  `2fa`), 5 free attempts, then the doubling wait. Five wrong codes also void the `login_token`,
  so the password has to be entered again.
- The session is created only at `/verify`, with what was used (below).

### Passkey sign-in (passwordless)

```
POST /api/v1/auth/passkey/start    {}                    → { challenge_id, publicKey: … }
POST /api/v1/auth/passkey/finish   { challenge_id, credential, device, cookie } → 200 TokenPair
```

The browser offers the passkeys it has for the RP id (or conditional UI: they appear in the
username field's autofill). The server finds the user from the credential's user handle (a random
per-user id, never the username or the UUID).

### Browser sign-in for native apps

Desktop and mobile apps no longer have a sign-in form of their own. They ask for the server
address (native, as now), then open the server's web sign-in page in the system browser, and
the user signs in there with whatever the tenant offers: password, TOTP, recovery code, passkey,
or the identity provider. The app gets its session through PKCE, as it does for single sign-on
today. TOTP, WebAuthn, forced enrolment, "update the app" problems and future factors are then
built and tested once, in the web client.

Why the browser rather than native screens:

- **Passkeys can't be done natively anyway.** WebAuthn needs the RP id's origin. The web client
  has it. Native apps do not:

  - iOS allows passkeys of a domain only when the app lists it under Associated Domains at build
    time, which cannot work for self-hosted servers on any domain.
  - Android Credential Manager needs `/.well-known/assetlinks.json` on the domain naming the
    app's signing key. The server could serve it, so this would be possible.
  - Desktop: Windows has `webauthn.dll`, macOS needs the same associated-domain entitlement as
    iOS, Linux has no platform passkeys at all, only USB keys over CTAP2.
- **One sign-in screen instead of four** (web, iced, Compose, SwiftUI), each with code entry,
  recovery codes, error states and forced enrolment.
- **Password managers** fill the web form better than native fields, especially on desktop.
- **The password never reaches the app.** Only the server's own page sees it (RFC 8252's advice).
- **One flow for every tenant**: with a provider, the web page forwards to it, so apps no longer
  pick between "password" and "single sign-on" buttons.

What it costs:

- The server must serve the web client for 2FA and passkeys. `HOMESPACE_WEB_ROOT` is optional
  today (the Docker image ships it). Until 1.0.0 the native form still works without it for users
  without 2FA; after that it is required (open question).
- A switch to the browser and back on each new sign-in. Sessions last 30 days through refresh, so
  that is rare. iOS shows its "wants to use <domain> to sign in" prompt.
- When the server reports `browser_login`, new apps show only a "Sign in" button that opens the
  page, with no username and password fields (decided 2026-11-24): one way to sign in, which
  covers passwords, single sign-on and every second factor. Until 1.0.0 the native form stays for
  servers that don't report it (older servers, no web client). 1.0.0 removes the
  native form and the apps' use of `/auth/login`; that is a breaking change, so it belongs to the
  major version (`docs/versioning.md`).

The flow: the system browser (loopback on desktop, Custom Tab, `ASWebAuthenticationSession`)
opens the server's own sign-in page. When the user has signed in there, the server hands a
one-time code to the app's redirect, and the app trades it with PKCE.

```
GET  /api/v1/auth/browser/start?redirect_uri&code_challenge&device_name
                                     → 302 /authorize?flow=<id>     (10 minutes, single use)
     (the web client signs in first if needed, password or single sign-on, comes back, and
      shows "The Home Space desktop app wants to sign in to your account alice.", the device
      name the app gave marked as its own claim, a warning that Continue gives full access,
      and Continue / Cancel)
GET  /api/v1/auth/browser/flows/{id}                (web session) → { app, device_name }
POST /api/v1/auth/browser/approve  { flow }         (web session) → { redirect: …?code=… }
POST /api/v1/auth/browser/deny     { flow }         (web session) → { redirect: …?error=access_denied }
POST /api/v1/auth/browser/exchange { code, code_verifier, device } → TokenPair (always in the body)
```

- Only app redirects start a flow: the loopback rule and the mobile address of single sign-on
  (`oidc::native_app`); the web client signs in on its own. The same one-time codes
  (`oidc_codes` is renamed to `login_codes` and gains `tenant_id`) and the same PKCE check
  (`auth::login_codes::LoginCodes`, used by both). Flows are kept in `browser_logins`.
- An unknown, expired or answered flow is `410 login-expired`; a server without the web client
  answers `start` with `503 browser-login-unavailable` and `Providers.browser_login: false`.
- A wrong code at `exchange` counts against the address like a wrong password.
- Anyone may call `start`, and each call stores a flow for 10 minutes, so (decided 2026-11-24):
  - every start, here and at `oidc/start`, counts against the address (`Counter::sign_in_starts`,
    over all tenants): 30 free, then `429 too-many-attempts` with waits doubling up to 15 minutes;
  - a tenant has at most 500 flows waiting, then `503 server-busy` (retry after 60 s);
  - an app address is taken only as `url` writes it back and at most 128 bytes, so a short
    loopback address cannot hide in a long string (leading zeros, `..`, tabs) that would be
    stored; the column has the same check (`0028_sign_in_limits.sql`).
- The code only ever goes to the approving user's own device (their loopback, their mobile app),
  so a link someone sends cannot hand them a session by itself. What is left needs the victim's
  help or a malicious app already on their device, hence the page's wording: the device name is
  the app's own claim, Continue gives full access, and "If someone sent you this link, press
  Cancel".
- The approve step is always an explicit click, even when the browser is already signed in, so a
  link someone sends cannot sign their app in to the victim's account.
- When the tenant has a provider and no passwords, `browser/start` simply forwards to
  `oidc/start`, so apps can always use one flow (done).
- Old apps can still use their single sign-on button against servers that announce browser
  sign-in (see [Features](#features-and-compatibility)).
- App extensions (share sheet, file provider) are unaffected: they get their sessions from the
  app's session through `/auth/device-sessions`, never by signing in.
- Native WebAuthn is not planned. Protected folders still talk to security keys natively (CTAP2),
  because they need the PRF output in the app; that is unlocking, not signing in.

### What a session remembers

New columns on `sessions`:

- `auth_methods TEXT[]`: what proved the user at sign-in, with the `amr` names of RFC 8176:
  `pwd`, `otp`, `hwk` (security key), `swk` / `user` (passkey with verification), plus `oidc` and,
  when the provider says so, its own `mfa`, and `recovery` for a recovery code, which RFC 8176
  has no name for. Re-confirming moves `authenticated_at` and leaves the methods as they were at
  sign-in.
- `authenticated_at TIMESTAMPTZ`: the last time the user proved who they are (sign-in or
  re-confirming). Refreshing tokens does not change it.

Extension sessions (`start_for_extension`) copy both from their parent, and an app let in from
the browser copies them from the web session that pressed Continue (through `login_codes`,
which carry them too), so a click is no new proof. Existing sessions get `{'pwd'}` or
`{'oidc'}` and their `created_at` (done: `0029_session_auth.sql`).

### Re-confirming for sensitive changes ("sudo mode")

These need `authenticated_at` within the last 10 minutes, or the request carries a fresh proof
(current password, a TOTP code or a WebAuthn assertion):

- change password; add or remove TOTP, passkeys or security keys; new recovery codes;
- later: protected folder unlockers, deleting one's own account.

Signing out a device (`DELETE /devices/{id}/sessions`, `DELETE /devices/{id}`) does **not**
need it (decided 2026-11-25). The desktop and Android apps already call it and cannot confirm,
so asking would break them before 1.0.0. A stolen session that signs out the owner's devices
only makes the owner sign in again, and it cannot change the password without the current one.

The answer otherwise is `403 reauthentication-required`, with `methods` like the sign-in step, so
the client can ask and retry. `POST /me/reauthenticate` takes the proof and moves
`authenticated_at` forward. `change_password` keeps its own current-password check, which counts
as the proof and moves it too.

Done (step 1): `AuthService::require_recent_sign_in` answers the problem, with
`methods: ["password"]` for a user with a password and none for a provider user, who signs in
again; `POST /me/reauthenticate { password }` (feature `reauthentication`), throttled with the
same per-session counter as changing the password. No endpoint calls the check yet: the first
are the TOTP and recovery code ones of step 2, and the api-client learns to tell the problem
apart (today it is an `Api { code: Forbidden }`) with the web dialog that answers it.

## Single sign-on users

- Provider users (`password_hash IS NULL`, an identity row) do **not** get local TOTP or passkeys.
  The provider is the place for them, and two places to manage factors would confuse people.
- `HOMESPACE_OIDC_REQUIRE_MFA=true` (per tenant): the ID token must carry an `amr` with `mfa`,
  `otp`, `hwk` or `swk`, else the sign-in is refused with `sso-mfa-required`. Auth0 sends
  `amr: ["mfa"]` when MFA ran; Keycloak only with a configured mapper, so the default stays off.
  The admin docs say how to set up each.
- The session records `oidc` plus the provider's `amr`, so the admin pages can show "2FA: by
  provider" or "2FA: not reported".
- A user who has a password **and** an identity (only possible through the CLI today) can use local
  factors for the password route; the provider route is unchanged.

## Policy and administration

Per tenant, in the `tenants` table (no secrets, so it fits the database):

| Setting | Values | Default |
|---|---|---|
| `mfa_policy` | `optional`, `admins`, `everyone` | `optional` |
| `passkey_login` | on / off | on (when the public URL is HTTPS or `localhost`) |

- With `admins` or `everyone`, a user without a second factor who signs in with a password gets a
  session limited to enrolling one (`403 second-factor-setup-required` everywhere else), so they
  are guided through it instead of being locked out. A passkey sign-in already satisfies it.
- Admin pages: per user, whether 2FA is on and with which methods, and a **Reset 2FA** action
  (needs the admin's own re-confirmation, removes TOTP, keys and recovery codes, ends the user's
  sessions). The same from `homespace-ctl user reset-2fa <username>` for a locked-out admin.
- Disabling `HOMESPACE_PASSWORD_LOGIN` keeps passkey sign-in unless `passkey_login` is also off;
  passkeys are local accounts too. (Open question.)
- WebAuthn needs a secure context: a server reached by IP address or plain HTTP on the LAN cannot
  use passkeys (only TOTP). `GET /auth/providers` says so, and the web client hides the button.

## Data

New migration (`0030_second_factors.sql`; `0027_browser_login.sql` already renamed
`oidc_codes` to `login_codes` and added `browser_logins`, `0028_sign_in_limits.sql` limits them,
and `0029_session_auth.sql` added the session columns below):

```sql
CREATE TABLE user_totp (
    tenant_id    INTEGER NOT NULL,
    user_id      UUID PRIMARY KEY,
    secret       BYTEA NOT NULL,          -- AES-256-GCM: nonce || ciphertext, key = HOMESPACE_SECRETS_KEY
    last_step    BIGINT NOT NULL DEFAULT 0,
    confirmed_at TIMESTAMPTZ,            -- NULL while enrolment waits for the first code
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant_id, user_id) REFERENCES users (tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE user_webauthn (
    id            UUID PRIMARY KEY,
    tenant_id     INTEGER NOT NULL,
    user_id       UUID NOT NULL,
    credential_id BYTEA NOT NULL,
    credential    JSONB NOT NULL,          -- webauthn-rs Passkey / SecurityKey
    discoverable  BOOLEAN NOT NULL,
    rp_id         TEXT NOT NULL,
    name          TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 64),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at  TIMESTAMPTZ,
    UNIQUE (tenant_id, credential_id),
    FOREIGN KEY (tenant_id, user_id) REFERENCES users (tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE user_recovery_codes (
    tenant_id  INTEGER NOT NULL,
    user_id    UUID NOT NULL,
    code_hash  BYTEA NOT NULL,
    used_at    TIMESTAMPTZ,
    PRIMARY KEY (user_id, code_hash),
    FOREIGN KEY (tenant_id, user_id) REFERENCES users (tenant_id, id) ON DELETE CASCADE
);

-- Random handle a passkey carries instead of the user's id or name.
ALTER TABLE users ADD COLUMN webauthn_handle BYTEA UNIQUE;

-- A sign-in that passed the password and waits for the second factor, and the state of a
-- WebAuthn ceremony between its two requests.
CREATE TABLE pending_logins (...);   -- token_hash, tenant_id, user_id NULL, kind, state JSONB, attempts, expires_at

-- Done in 0029_session_auth.sql: sessions.auth_methods TEXT[], sessions.authenticated_at,
-- both also on login_codes.
-- Done in 0030_second_factors.sql: user_totp, user_recovery_codes, and pending_logins
-- (token_hash, tenant_id, user_id, attempts, expires_at). The WebAuthn tables, webauthn_handle
-- and the ceremony state of pending_logins come with step 4, the tenants columns with step 5.

ALTER TABLE tenants ADD COLUMN mfa_policy TEXT NOT NULL DEFAULT 'optional'
    CHECK (mfa_policy IN ('optional', 'admins', 'everyone'));
ALTER TABLE tenants ADD COLUMN passkey_login BOOLEAN NOT NULL DEFAULT true;
```

The background job that already clears `oidc_pending` also clears expired `pending_logins`.

### The TOTP secret key

The server must read the TOTP secret to check a code, so it cannot be hashed. Stored in clear, a
database dump (backups) would hand out every second factor. So:

- New secret `HOMESPACE_SECRETS_KEY` (or `_FILE`), per tenant like the JWT secret
  (`HOMESPACE_SECRETS_KEY_2`, tenant 1 falls back to the unsuffixed one), checked with
  `auth::secrets::check_secret`. AES-256-GCM, key from HKDF-SHA256 over it, the user id as
  associated data so a secret cannot be moved to another row.
- `HOMESPACE_SECRETS_KEY_PREVIOUS` for rotation, like the JWT secret. A `homespace-ctl secrets
  rotate` re-encrypts the rows.
- Without the key, TOTP enrolment is refused with a clear message; passkeys and recovery codes
  still work (they store nothing secret). `homespace-ctl install` generates it.
- Done: `auth::SecretBox`, read per tenant like the JWT secret, refused when it equals the JWT
  secret. A secret opened with `_PREVIOUS` is sealed again with the current key when its code is
  accepted, so `homespace-ctl secrets rotate` is only needed to finish a rotation at once; it is
  not written yet. Administrators see in the server settings whether the key is set.
- Not derived from `HOMESPACE_JWT_SECRET`: rotating that one would then destroy every TOTP.

## API summary

| Method and path | Purpose |
|---|---|
| `POST /auth/login` | Unchanged request; may answer `second-factor-required` |
| `POST /auth/login/verify` | Second step with TOTP or recovery code (done), WebAuthn assertion (step 4) |
| `POST /auth/passkey/start`, `/finish` | Passwordless sign-in |
| `GET /auth/browser/start`, `GET /auth/browser/flows/{id}`, `POST /auth/browser/approve`, `/deny`, `/exchange` | Browser sign-in for native apps (done: server and web) |
| `GET /auth/providers` | Gains `passkeys: bool`, `browser_login: bool` |
| `GET /me` | Gains `second_factors: { totp, recovery_codes_left }` (done); `webauthn: n` with step 4 |
| `GET /me/security` | Factors, credentials with names and dates, the policy (with step 4) |
| `POST /me/totp`, `POST /me/totp/confirm`, `DELETE /me/totp` | Enrol, confirm, remove TOTP (done) |
| `POST /me/webauthn/start`, `/finish`, `PATCH`/`DELETE /me/webauthn/{id}` | Add, rename, remove a key |
| `POST /me/recovery-codes` | New set (voids the old) (done) |
| `POST /me/reauthenticate` | Fresh proof for sensitive changes |
| `DELETE /admin/users/{id}/second-factors` | Reset 2FA (done) |

New problem types for `docs/api-errors.md`: `second-factor-required` (403),
`reauthentication-required` (403), `second-factor-setup-required` (403), `sso-mfa-required` (403),
`invalid-code` (422, enrolment), `secrets-key-missing` (503 on TOTP enrolment).

## Features and compatibility

Per `docs/versioning.md`, each piece a client may rely on gets a feature in
`shared/protocol/src/features.rs` with a display name. No client puts them in `REQUIRES`: every
client falls back to plain password sign-in on older servers.

| Feature | Display name |
|---|---|
| `two_factor` | Two-factor sign-in |
| `passkeys` | Passkeys |
| `browser_login` | Sign-in in the browser |
| `reauthentication` | Confirming sensitive changes |

- Old client, new server, user without 2FA: nothing changes.
- Old client, user with 2FA: the password sign-in fails with `403 second-factor-required` and its
  readable detail (see [Password, then second factor](#password-then-second-factor)). On
  desktop and mobile, the existing single sign-on button can be pointed at `browser/start` by the
  server when no provider is configured (`Providers.oidc` names "Sign in in the browser"), so even
  an old app can sign in through the web page. That is worth doing because it costs one branch.
- New client, old server: the feature list lacks `two_factor`; the security settings are hidden.
- New client, new server: browser sign-in only, for every user.
- No breaking change before 1.0.0. `oidc_codes` became `login_codes` in `0027_browser_login.sql`,
  with no change to `/auth/oidc/*`. 1.0.0 drops the native form and the apps' password path of `/auth/login` (the web
  page keeps using it with `two_step`).

## Clients

- **api-client / client-core**: keep the problem of a 401 sign-in answer instead of collapsing it
  into `Unauthorized`; the browser sign-in generalised from `oidc_start_url` / `oidc_exchange`;
  types for the security settings. The password `login` stays until 1.0.0.
- **Web (Leptos)**: second-step screen; "Sign in with a passkey" and conditional UI on the
  username field; Security page (TOTP with QR, keys, recovery codes, sign out everywhere);
  re-confirm dialog; the browser sign-in approve page. `navigator.credentials` through `web-sys`
  with JSON (de)serialisation of the options (`webauthn-rs-proto` types are shared).
- **Desktop (iced)**: with `browser_login`, one "Sign in" button runs `sso.rs` against
  `browser/start`; otherwise the native form, until 1.0.0. No 2FA code in the app. Security
  settings open the web page.
- **Android / iOS**: the same, with the existing Custom Tab / `ASWebAuthenticationSession` code.
  Security settings open the web page (a native screen can come later if wanted).

## Security notes

- Every factor check uses constant-time comparison; TOTP and recovery code checks look the same
  whether the user has the factor or not, like `verify_or_pretend`.
- Sign-in, verify and passkey endpoints keep the address-wide throttle; passkey `finish` with an
  unknown credential counts as a wrong attempt from the address.
- Adding a factor, removing one and using a recovery code are logged (`tracing::info!`) and, once
  notifications exist for it, sent to the user's other devices.
- Removing the last second factor while the policy requires one is refused.
- Resetting 2FA by an admin ends all the user's sessions; a user removing their own factor keeps
  the current session.
- The browser approve page states the device name and that it was opened from an app, and has
  `frame-ancestors 'none'`.

## Interaction with other plans

- **Multi-tenancy**: everything is per tenant: tables carry `tenant_id` with the composite user
  key, the RP id is the tenant's primary domain, `HOMESPACE_SECRETS_KEY_{TENANT}`. Moving a
  tenant's primary domain breaks its passkeys (as it already does for protected folders);
  `homespace-ctl tenant domain` warns and counts them. WebAuthn
  [related origins](https://w3c.github.io/webauthn/#sctn-related-origins) (`/.well-known/webauthn`)
  could keep aliases working; Chrome and Safari support it, Firefox not yet.
- **Protected folders**: shares the RP id rule and WebAuthn plumbing, not credentials.
- **Security hardening (#44)**: reuses `LoginThrottle` and the session-ended broadcast.
- **Server manager**: `homespace-ctl install` generates `HOMESPACE_SECRETS_KEY`; `user reset-2fa`.
- **Admin panel**: 2FA column and reset action.

## Testing

- `auth` unit tests: TOTP window and replay, recovery codes single use, policy states, login token
  expiry and attempt limit, session methods.
- API tests against Postgres: both login steps, old-client 401 shape, passkey sign-in with
  `webauthn-rs`'s software authenticator (`webauthn-authenticator-rs`), browser sign-in code flow,
  re-confirmation, admin reset, tenant isolation of credentials.
- Web e2e (Playwright): Chrome DevTools virtual authenticator for passkey enrolment and sign-in;
  TOTP from a fixed clock.

## Steps

0. Now, before any 2FA: every client shows the server's problem `detail` on sign-in and
   elsewhere, instead of a fixed message per error kind; the API client keeps the problem of
   401 answers on sign-in. Ships in the next release so it is out before 2FA. Done: the server
   says sign-in refusals as sentences (`AuthError::WrongLogin` "Invalid username or password.",
   `LoginCodeInvalid` for a used or expired code, sentence-case `TooManyAttempts`, `Busy`,
   `AccountDisabled`, …); the api-client's `login`, `oidc_exchange` and
   `browser_login_exchange` turn a 401 with a detail into `Api { code: Unauthorized, message }`,
   and keep `Unauthorized` for older servers' bare "invalid credentials"; desktop, web,
   Android and iOS show that message, with their fixed text only for older servers.
1. Sessions remember `auth_methods` and `authenticated_at`; re-confirmation (`/me/reauthenticate`
   with the password). Done: see [What a session remembers](#what-a-session-remembers) and
   [Re-confirming](#re-confirming-for-sensitive-changes-sudo-mode). Changing the password
   counts as confirming; device sign-out stays without it, for the released apps.
2. TOTP and recovery codes: tables, secret key, two-step login, web screens, admin reset, CLI.
   Done (feature `two_factor`): `0030_second_factors.sql`; `HOMESPACE_SECRETS_KEY`;
   `/auth/login` with `two_step` and `/auth/login/verify`, wrong codes throttled per username
   (`2fa`) and five ending the sign-in; `/me/totp`, `/me/totp/confirm`, `/me/recovery-codes`,
   all behind `require_recent_sign_in`; `/me/reauthenticate` takes `{ code }` too, and
   `methods` names `totp` for a user with an app; `Me.second_factors`, `Account.two_factor`;
   `DELETE /admin/users/{id}/second-factors` and `homespace-ctl user reset-2fa`. Sessions record
   `["pwd", "otp"]` or `["pwd", "recovery"]`. The api-client has `login_two_step`,
   `login_verify` and `ApiError::ReauthenticationRequired`; the web client asks for the code
   at sign-in (also on `/authorize`), has a "Two-factor sign-in" card in Settings with the QR
   code and the recovery codes, a re-confirm dialog for every sensitive change, a reminder when
   three or fewer recovery codes are left, and the administrator's reset in the user's panel.
   Removing the app also removes the recovery codes, since there is no other factor yet.
3. Browser sign-in for native apps: server flow and web approve page (`/authorize`) are done,
   with the `browser_login` feature and `Providers.browser_login`; the api-client has
   `browser_login_start_url` / `browser_login_exchange`. Desktop, Android and iOS show only a
   "Sign in" button that opens the page when the server reports `browser_login`, and the native
   form otherwise (`mobile-core`: `begin_browser_sign_in` / `finish_browser_sign_in`; Cancel on the page comes
   back as `SignInCancelled`). Done too: where there is no provider, `/auth/providers` announces
   a stand-in `oidc: { name: "your browser", browser: true }`, and `/auth/oidc/start` and
   `/auth/oidc/exchange` then run the browser sign-in, so older apps' "Sign in with your
   browser" button works; new clients skip it (`Providers::identity_provider`). A tenant with
   only its provider sends `browser/start` straight to the provider.
4. WebAuthn: security keys as second factor, then passkey sign-in, in the web client.
5. Policy (`mfa_policy`), forced enrolment, `HOMESPACE_OIDC_REQUIRE_MFA`, and an administrator's
   security settings screen in the web client: who must use a second factor, how long sessions
   last, and how often the second factor is asked again.
6. 1.0.0: remove the native password form and the apps' password path of `/auth/login`.
7. Later: e-mail notifications when e-mail exists.

Each step is a release on its own and adds its feature name.

## Open questions

1. **Passkeys without a password.** Decided 2026-11-25: a passkey or security key signs in on its
   own, without the password, as the large providers allow.
2. **Servers without the web client after 1.0.0.** Apps sign in only in the browser from 1.0.0
   (decided 2026-11-24; native form and `/auth/login` for apps kept until then). For a server run
   without `HOMESPACE_WEB_ROOT`, should the server then ship a minimal built-in sign-in page, or
   simply require the web client (recommended: require it, the Docker image and
   `homespace-ctl install` have it)?
3. **TOTP secret key.** Decided 2026-11-25: TOTP secrets are encrypted with
   `HOMESPACE_SECRETS_KEY`, never stored in clear.
4. **Default policy.** Decided 2026-11-25: optional for everyone by default; an administrator can
   require it later (step 5).
5. **Password login off.** Decided 2026-11-25, following 1: passkeys count as a sign-in of their
   own, so with `HOMESPACE_PASSWORD_LOGIN=false` passkey sign-in stays available for local
   accounts; "off" turns off passwords only.
6. **Provider users.** Leave 2FA to the provider (recommended), or also allow local TOTP/passkeys
   for them as an extra layer?
7. **Account recovery.** Decided 2026-11-25: recovery codes plus an admin reset are enough; e-mail
   is not needed now.
8. **Remember this device.** Decided 2026-11-25: no separate "remember this device". A sign-in
   with the password and the second factor keeps its session as sessions are kept today, so the
   factor is asked again only on a new sign-in. How long sessions last and how often the second
   factor is asked again become settings an administrator can tune (step 5).
9. **Scope of "local accounts".** Should this plan also cover invitations (an admin sends a link,
   the user picks their password and adds a passkey) instead of the admin choosing the password?
