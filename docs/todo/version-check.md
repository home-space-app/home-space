# Version check

## Problem

Clients and the server are released together (`scripts/release.sh` bumps every version), but
they are not updated together. A phone keeps an old app for weeks; a server is updated when its
administrator gets to it. Today nobody can tell:

- The client does not know the server's version. `ServerInfo.version` exists, but only the admin
  status page shows it.
- The server does not know a client's version. `DeviceInfo` is `{id, name, platform}`.
- `GET /capabilities` reports limits and one flag (`stream_tickets`), not a list of features, and
  clients say nothing about what they need. A new client on an old server finds out a feature is
  missing only when a request fails (as the device remove endpoint from 0.12.3 would on 0.12.2).

## Goal

Every native client (desktop, Android, iOS) shows how it fits the server it is signed in to, in
one of three states, each with a hint:

| State | Colour | When | Hint (example) |
|---|---|---|---|
| Same | Green | Client version == server version | "Up to date. App and server are both 0.13.0." |
| Warning | Yellow | Versions differ, same major, and the server has every feature the client requires | Server newer: "Server is 0.14.0, this app is 0.13.0. Everything works; update the app to get the newest features." Server older: "Server is 0.13.0, this app is 0.13.2. Everything this app needs is supported. Ask the administrator to update the server." |
| Error | Red | Major versions differ, **or** the client requires a feature the server does not have | Missing: "The server (0.12.3) is too old for this app. These won't work until it is updated: Remove devices." Major: "Server 2.0.0 and this app 1.4.0 are different major versions. Many features may fail. Update the app." |

The web client is served by the server, so it always has the server's version: it shows the
version (no state, or always green) and never warns.

## Rules

Computed in one place, `protocol::compat::check(server, client) -> Compatibility`, so every client
(desktop and web in Rust, Android and iOS through `mobile-core`'s UniFFI bindings) agrees.

1. `client.version == server.version` (full string, pre-release suffix included) → **Same**.
2. Different major (`semver` major) → **Error { reason: MajorVersion }**.
3. `client.requires ⊄ server.features` → **Error { reason: MissingFeatures(list) }**.
4. Otherwise → **Warning { newer: Server | Client }**.

Order matters: rule 2 wins over rule 3, so a major mismatch is reported as such even when the
feature lists happen to line up.

Unknown cases:

- **Server too old to report a version** (≤ 0.12.x, no `version` in `/capabilities`): treat its
  feature list as empty. With no requirements (where we start) this is **Warning** "Server
  version unknown, it is older than 0.13.0"; once the client requires anything it is **Error**.
- **Capabilities could not be read at all** (network, proxy): no badge, not red. We only show
  what we know.
- **Version that is not semver** (a dev build, `git describe`): rule 1 still applies; rules 2-4
  compare what parses, and a value that does not parse is treated as a different minor (Warning
  at worst, never Error by itself).

### Pre-1.0 (open question)

We are at 0.12.3. Strict semver treats a 0.x minor bump as breaking, but the rule as asked uses
the major, so 0.12 → 0.13 can never be red by version alone; only missing features make it red.
**Proposal: keep the literal major.** We keep the server backwards compatible anyway, and the
feature list catches real gaps. Revisit at 1.0.

## Features (capabilities)

A **feature** is a short stable name for something the server offers that an older server does
not: `device_remove`, `stream_tickets`, ... Names never change meaning and are never reused.

- **Registry.** `shared/protocol/src/features.rs`: one `pub const` per feature, each with a doc
  comment naming the version that introduced it, plus `ALL: &[&str]`. Strings, not an enum, so a
  client reads a newer server's list without failing on names it does not know.
- **Server.** `/capabilities` gains `version: String` and `features: Vec<String>` (from
  `protocol::features::ALL`). A server test asserts what it reports equals `ALL`, so a feature
  cannot be listed without being reported.
- **Clients.** Each client declares what it **requires** of the server:
  `client_core::REQUIRES` (desktop), `mobile_core::REQUIRES` (Android and iOS share it; a
  platform-only need goes in a second list in the app). A test per crate asserts every entry is in
  `protocol::features::ALL`.
- **Start from zero.** `ALL` and every `REQUIRES` start empty at the release that ships this
  (0.13.0). Earlier features (`device_remove`, `stream_tickets`) are part of the baseline: a server
  that reports a version has them. `stream_tickets: bool` stays for servers from before this.

### When to add one (goes into `CLAUDE.md` and the PR template)

| Change | Server `ALL` | Client `REQUIRES` |
|---|---|---|
| New endpoint, field, query parameter or event the client will rely on | Add the feature | — |
| Client starts using it and **cannot work without it** | — | Add it |
| Client uses it but falls back on an older server (like stream tickets do) | — | Do **not** add it; the fallback handles it |
| Server breaks something old clients use | Bump the **major** version | — |
| Purely internal server or client change | — | — |

The example from the request: the remove endpoint adds `device_remove` to `ALL`; the Android
release that offers "Remove device" adds it to its `REQUIRES`. That app signed in to a server
without it is **red**, with "Remove devices" in the hint. An old app on a new server is
**yellow**.

## Client reports its version

- `DeviceInfo` gains `version: Option<String>` and `requires: Vec<String>` (`#[serde(default)]`,
  so older clients still sign in). Sent where it already is: `POST /auth/login`,
  `POST /auth/oidc/exchange`, and the websocket `Hello`.
- Migration `0026_device_versions.sql`: `devices.client_version text NULL`,
  `devices.client_requires text[] NOT NULL DEFAULT '{}'`, updated on sign-in and `Hello`.
- `GET /devices` returns `version` and a server-computed `compatibility` per device. The device
  list then shows which devices need an update, which the user cannot see from one device.

## UI

Shared labels in `client_core::labels` (state title, hint, feature display names) so every client
words it the same. Each feature constant has a display name ("Remove devices").

- **Desktop (iced).** A version chip in the sidebar footer, above the sync status card, so the
  user stays at the bottom left: coloured dot + "App 0.13.0 · Server 0.13.0". Click shows a
  popover above it: both versions, the state, the hint, and for red the
  missing features. Red also shows a one-line banner under the header once per session,
  dismissible.
- **Web.** The user menu shows "Home Space 0.13.0". No state.
- **Android (Compose).** Account page: an "App version" row with the coloured dot; tapping it
  opens a sheet with the hint. Red shows a banner at the top of Browse until dismissed for this
  server version.
- **iOS (SwiftUI).** Same as Android in `ProfileView`; banner in `MainTabs`.
- **Device list (all clients).** A **Version** column (desktop and web; a line under the name
  on mobile) with the last version the device reported, at sign-in or when it connects, and a
  small dot; "Unknown" for devices that have not connected since 0.13.0. Empty for web
  browsers: they are served by the server, so they run its version.

Designs: Penpot file "Home Space - Version check".

## New version available

A separate check from the one above. The compatibility check compares a client with **its
server**; this one compares the installed app with **the latest release**, so people who
installed by hand (the Android APK, iOS through Xcode, the desktop tarball) learn there is an
update.

### Source

On app start, one request:

```
GET https://api.github.com/repos/home-space-app/home-space/releases/latest
Accept: application/vnd.github+json
```

It returns the newest release that is not a draft or pre-release; we read `tag_name`
(`v0.14.0`, the `v` stripped) and `html_url` (the release page). It is the API form of
`https://github.com/home-space-app/home-space/releases/latest`.

- **Failures are silent.** GitHub allows 60 unauthenticated requests an hour per IP, so a 403 or
  429 is expected now and then. Any error (network, timeout of 5 s, rate limit, a body that does
  not parse, a tag that is not semver) shows nothing, logs at debug level, and is not retried
  until the next start.
- **One call per start.** No background polling. The app does not send anything but the request
  itself (no version, no account).
- **The URL is configurable** (`HOMESPACE_RELEASES_URL`, build-time on mobile) so tests and e2e
  point it at a local stub instead of GitHub.
- Shared code in `client_core::update`: `latest_release() -> Option<Release { version, url }>`
  and `should_offer(installed, latest, skipped) -> bool`, used by the desktop and web in Rust
  and by Android and iOS through `mobile-core`'s UniFFI bindings.

### When to show it

Show the toast when `latest > installed` (semver) and the user has not skipped `latest` or a
newer one: `skipped.map_or(true, |s| latest > s)`. So skipping 0.14.0 hides it until 0.14.1 is
out. A dev build (`0.14.0-dev`) compares as semver, so it sees 0.14.0 as newer.

### The toast

Title **New version available**, text "Home Space 0.14.0 is out. You have 0.13.0." Three ways
out:

| Action | Effect |
|---|---|
| **×** (close) | Hides it for this run. Shown again on the next start. Nothing stored. |
| **Skip** | Stores `update.skipped_version = 0.14.0` in local device storage, then hides it. Not shown again until a newer release. |
| **Open web** (native) / **How to update** (web) | Opens the release page, or the update guide, in the browser. The toast stays until closed or skipped, so it comes back on the next start until the app is updated. |

Where the skipped version lives (per device, not synced, not on the server): desktop
`data_dir()/update.json` next to `session.json`; Android `SharedPreferences("update")`; iOS
the App Group `UserDefaults` (`SessionStore.defaults`); web `localStorage`.

Per client:

- **Desktop.** Bottom-right, in the corner the transfer toast uses, stacked above it. "Open web"
  opens `html_url`.
- **Android.** A card above the bottom navigation (a Snackbar has only one action, we need two
  and a close). "Open web" opens `html_url`, where the APK is attached.
- **iOS.** The same card above the tab bar. There is no iOS download on the release page: the
  text says "Update from the source with Xcode", and "Open web" opens the release notes.
- **Web.** The web client is the server, so the check compares the **server's** version, and the
  update is done by whoever runs the server. Shown to **administrators only** (others cannot act
  on it). The action is **How to update**, which opens
  `https://home-space.app/docs/#update` (the `docker compose pull` and `up -d` steps), not
  GitHub. The request goes from the browser to `api.github.com` (CORS is allowed).

The toast is lower priority than the red compatibility banner: if both apply, both show, the
banner at the top and the toast in its corner.

Site: the update section's pin example still says `homespaceapp/home-space:0.2`; fix it when
linking to it.

## Steps

1. **Protocol.** `features.rs` (empty `ALL`), `compat.rs` with `check` and tests for each rule
   and unknown case; `Capabilities.version/features`; `DeviceInfo.version/requires`.
2. **Server.** Report version and features; store client version; device list fields; tests.
3. **API client.** Keep `Capabilities` (already read on sign-in); expose
   `Client::compatibility()`.
4. **Desktop.** Chip, popover, banner; device list column.
5. **Web.** Version in the user menu; device list column.
6. **mobile-core.** Export `compatibility()` and labels through UniFFI.
7. **Android, iOS.** Account row, sheet, banner; device list.
8. **Process.** `docs/versioning.md` (rules and the table above), a `CLAUDE.md` rule, a PR
   template checkbox "Added server features / client requirements?". README `/capabilities` row.
9. **e2e.** A spec in `docs/specs/version-check.feature`: same, warning (old client), error
   (client requiring a feature a test server leaves out).
10. **New version toast.** `client_core::update` with tests (newer, same, older, skipped,
    skipped-then-newer, rate-limited, bad tag); then the toast in each client and the skipped
    version in local storage. e2e against a stub releases URL: shown, close shows again after
    reload, skip does not, a newer stub release shows again.

## Open questions

- Pre-1.0: literal major (proposed) or minor-as-major?
- Should `/capabilities` stay behind sign-in? Proposed yes: an anonymous version is a small
  information leak, and the check is only useful once signed in.
- Should a red state block anything, or only warn? Proposed only warn: features with fallbacks
  keep working, and the rest fail with `ApiError::Unsupported` as stream tickets do today.
- ~~New version toast on the web: administrators only or everyone?~~ Decided: administrators
  only.
- ~~A setting to turn the GitHub check off?~~ Decided: yes, "Check for new versions", on by
  default, stored per device next to the skipped version.
