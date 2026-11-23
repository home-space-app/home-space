# Versions, server features and client requirements

Every client shows how it fits the server it is signed in to, and every app tells its user when
a newer release is out. Both rest on a few rules that each change has to keep.

## Versions

- `scripts/release.sh` gives the server, every client and `shared/protocol` the same version.
  The web client is built into the server, so it always has the server's version.
- A **breaking** server change (one an older client cannot work with) bumps the **major**
  version. They are rare: the server stays backwards compatible.
- Before 1.0 the major is `0`, so only missing features make a client red.

## Compatibility: green, yellow, red

`protocol::compat::check(server_version, server_features, client_version, client_requires)`:

| State | When |
|---|---|
| Same (green) | The versions are equal |
| Error (red) | The major versions differ, or the client requires a feature the server does not list |
| Warning (yellow) | Anything else: different versions, and the server has what the client needs |

A server from before 0.13.0 reports no version and no features. Clients that cannot read
`/capabilities` show nothing.

## Features and requirements

- **Server features**: `shared/protocol/src/features.rs`, `ALL`, reported by
  `GET /capabilities`. A test checks the server reports exactly `ALL`.
- **Client requirements**: `REQUIRES` in `clients/desktop/src/device.rs` (desktop) and
  `clients/mobile-core/src/version.rs` (Android and iOS). The web client requires nothing.
  Clients send them with their version in `DeviceInfo` on sign-in and in the websocket hello.

| Change | Server `ALL` | Client `REQUIRES` |
|---|---|---|
| New endpoint, field, query parameter or event a client will rely on | Add a feature, with a display name and the version in its doc comment | — |
| A client starts using it and **cannot work without it** | — | Add it |
| A client uses it but falls back on an older server (as stream tickets do) | — | Do **not** add it |
| The server breaks something older clients use | Bump the major version | — |
| Internal change | — | — |

A feature name never changes meaning and is never reused.

## New version available

Each app asks `https://api.github.com/repos/home-space-app/home-space/releases/latest` once
when it starts (the web client: only for administrators). Any failure, such as GitHub's rate
limit, shows nothing. When the latest release is newer than the app and than the version the
user skipped, a toast offers it: close (again next start), Skip (stored on the device), and
Open web (the release page; on the web, the update guide at
`https://home-space.app/docs/#update`). "Check for new versions" turns it off per device.
`HOMESPACE_RELEASES_URL` points the desktop app at a stub for tests.
