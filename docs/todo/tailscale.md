# Native Tailscale: reach the server from anywhere without exposing it or setting up Tailscale on it

## Problem

Today a user who wants to reach Home Space away from home has two choices:

- **Expose it to the internet:** port forwarding or a public reverse proxy, a domain and a TLS
  certificate. Many people running a home server don't want their files on the public internet.
- **Put the server on a VPN:** install and sign in to Tailscale (or WireGuard) on the host or in
  a sidecar container, then point the clients at the tailnet address. That is extra setup, and in
  Docker or Podman you also have to deal with `/dev/net/tun`, capabilities and the host network.

We want a third choice. The server joins a tailnet **by itself**, and so can the apps:

1. The admin opens the web client on the LAN and clicks **Connect to Tailscale** in the admin panel.
2. The server shows a Tailscale sign-in link (or QR code). The admin approves it in their Tailscale
   account, and the server is now a tailnet node such as `homespace.<tailnet>.ts.net`, with no
   port open to the internet.
3. On the phone, the user picks **Connect through Tailscale** at sign-in, signs in to the same
   tailnet, and the app reaches the server through the tunnel, from anywhere.

There's no Tailscale daemon, no TUN device and no extra container on the server.

## Building blocks

- **tsnet** (Go) runs Tailscale inside a process, as a library. It uses a userspace network
  stack, so it needs no TUN device, root or `NET_ADMIN`. A process using it becomes its own
  tailnet node with its own name, and it can:
  - `Listen` / `ListenTLS` on the tailnet. `ListenTLS` serves a valid `*.ts.net` certificate
    through Let's Encrypt, as long as HTTPS is turned on for the tailnet.
  - `Dial` tailnet addresses.
  - `Loopback()`: start a local SOCKS5 proxy (with credentials) into the tailnet.
  - Report an interactive sign-in URL (`AuthURL` from `LocalClient().Status`), or sign in with an
    auth key (`AuthKey`).
  - Use another control server (`ControlURL`), so **Headscale** works too.
  - `WhoIs(remoteAddr)`: identify the tailnet user and node behind a connection.
- **libtailscale** ([github.com/tailscale/libtailscale](https://github.com/tailscale/libtailscale))
  is a C library around tsnet. It has `tailscale_listen` / `tailscale_dial` / `tailscale_loopback`
  and passes connections as file descriptors, which is what lets a Rust process use it.
- **A pure Rust implementation:** check whether there is a maintained one we could use by now
  (Tailscale has published experimental Rust work). It would let us avoid the Go toolchain and
  cgo. Until one is stable, plan on libtailscale.

## Proposal

### Phase 0: document the sidecar (no code)

Add a `compose.tailscale.yaml` example: the `tailscale/tailscale` image with `TS_AUTHKEY`,
`TS_SERVE_CONFIG` (`https:443 → http://homespace:8080`) and `TS_USERSPACE=true`, with Home Space
publishing no ports. It works today and gives users something while the native version is built.
It also gives us a baseline to compare against.

### Phase 1: the server joins the tailnet (`crates/tailnet`)

- **New crate `server/crates/tailnet`**, a safe Rust wrapper around libtailscale (built
  statically from a pinned Go module through `build.rs`, behind a `tailscale` cargo feature that
  is on in the release image).
  - `Tailnet::start(state_dir, hostname, control_url, auth_key?)`
  - `listener(port, tls) -> tokio::net::TcpListener`: turns the fd from `tailscale_listen` into
    a std then a tokio listener.
  - `status() -> {state: NeedsLogin | Starting | Running | Stopped, auth_url?, dns_name?, ips}`
  - `logout()`, `whois(peer_addr)`
- **Serving:** `main.rs::serve` keeps the normal `HOMESPACE_BIND` listener (LAN and localhost)
  and adds a second `axum::serve` on the tailnet listener with the same router. On the tailnet it
  serves HTTPS on 443 with the `ts.net` certificate when the tailnet allows it, and plain HTTP on
  80 otherwise. The admin can also turn the normal listener off (bind to `127.0.0.1` only) once the
  tailnet works.
- **Config** (env, plus the admin panel below):

  | Variable | Default | Meaning |
  |---|---|---|
  | `HOMESPACE_TAILSCALE` | `off` | `off`, or `on` to join the tailnet at start-up |
  | `HOMESPACE_TAILSCALE_HOSTNAME` | `homespace` | node name in the tailnet |
  | `HOMESPACE_TAILSCALE_STATE_DIR` | `<data>/tailscale` | node key and state; must persist (volume) |
  | `HOMESPACE_TAILSCALE_AUTH_KEY` | none | optional, for headless installs; otherwise the interactive sign-in |
  | `HOMESPACE_TAILSCALE_CONTROL_URL` | Tailscale's | for Headscale |

  Keep the state dir **outside** the storage root, with mode `0700`. It holds the node's private
  key.
- **Settings stored in the DB** (`server_settings` table, or the admin panel's existing settings)
  so the admin can turn it on from the UI without editing env. Env wins when it's set.
- **`HOMESPACE_PUBLIC_URL` and OIDC:** the tailnet name is a second origin. The CORS / origin
  checks, the cookie `Secure` flag and the OIDC redirect URI have to accept it. Simplest rule:
  `public_url` can be a list, and the tailnet URL is added to it automatically while connected.
- **Docker / Chainguard image:** the build stage needs Go (or a prebuilt `libtailscale.a` for each
  arch). The runtime image doesn't change, because tsnet is userspace. Measure what it adds to the
  image size (roughly +20–30 MB).

### Phase 2: admin panel ("sign in to Tailscale from the web client")

- **Endpoints (admin only):**
  - `GET /api/v1/admin/tailscale`: status, `auth_url`, `dns_name`, IPs, and the tailnet URL
    clients should use.
  - `POST /api/v1/admin/tailscale/connect`: start tsnet (optionally with an auth key or a control
    URL) and return the status. When the node needs to sign in, the status carries `auth_url`.
  - `POST /api/v1/admin/tailscale/disconnect`: `logout()` and stop.
  - Push status changes over the existing WebSocket (`tailscale_status`), so the page moves from
    "Waiting for approval…" to "Connected as homespace.tail1234.ts.net" without polling.
- **Web UI** (`clients/web/src/pages/admin.rs`, new Tailscale section):
  - **Connect** button → "Open this link to approve the server in your Tailscale account" (link
    and QR code).
  - Once connected: node name, the URL clients should use, a **Disconnect** button, and a warning
    when HTTPS certificates are off in the tailnet ("turn on HTTPS in the Tailscale admin console
    for a trusted certificate").
  - A **pairing QR code** for the apps: `homespace://connect?url=https://homespace.<tailnet>.ts.net&tailnet=1`.
- The web client itself can't join a tailnet. A browser can't run tsnet: Tailscale's WASM build
  isn't a supported product and would need a DERP-only WebSocket path. So away from home, the
  web client is reached from a computer that runs the normal Tailscale app. Say this plainly in
  the UI and the docs.

### Phase 3: the mobile apps get a built-in tunnel

The goal is for the phone to reach the server **without the user installing the Tailscale app**
or giving the app VPN permission.

- **`mobile-core`** gets an optional `tailnet` module that links libtailscale for Android
  (`aarch64`/`x86_64-linux-android`) and iOS. The Go side is cross-compiled the same way the
  Tailscale mobile apps do it.
  - `TailnetSession::start(state_dir, hostname = "homespace-<device name>")` → status, `auth_url`.
  - After sign-in, `tailscale_loopback` gives a local SOCKS5 address and credentials.
    `api_client::Client` gets an optional proxy (`reqwest::Proxy::all("socks5h://…")` with basic
    auth), used for HTTP and for the WebSocket (`tokio-tungstenite` through the SOCKS stream, or
    `tailscale_dial` directly).
  - `socks5h` lets the `*.ts.net` name resolve inside the tailnet (MagicDNS), not on the phone.
- **UniFFI API:** `start_tailnet`, `tailnet_status`, `stop_tailnet` and
  `Client::new_with_tailnet(base_url, session)`.
- **Android (`androidApp`) and iOS (`iosApp`):**
  - Server screen: a **Connect through Tailscale** switch, or scan the pairing QR code from the
    admin panel.
  - Sign-in opens the `auth_url` in a Custom Tab / `ASWebAuthenticationSession`. The user signs in
    to Tailscale (Google/Microsoft/GitHub/…) and approves the device. Then the normal Home Space
    sign-in follows (password or OIDC).
  - Keep the tsnet state in app-private storage (Android `filesDir`, iOS Application Support,
    excluded from backup) so the node survives restarts and doesn't sign in again.
  - Start the tunnel lazily when the app or a background job needs the server, and stop it when
    idle. Watch battery and the iOS background time limits: auto-upload jobs, WorkManager and
    `BGTaskScheduler` need the tunnel up for the length of the job.
- **If the official Tailscale app is already on the phone**, the server's tailnet URL works
  without the built-in tunnel. Detect it: try a direct connection first, and only start tsnet
  when the name doesn't resolve. That way two nodes don't fight over the same device.
- **iOS extensions:** the File Provider and Share extensions run in their own processes with
  tight memory limits (File Provider about 20–50 MB). tsnet in each of them would mean extra nodes
  and memory pressure. Options: (a) share one node state through the app group and run tsnet in
  the extension too (two processes using one node key at the same time is **not** allowed, so it
  needs a lock or separate keys), (b) give each extension its own ephemeral node, or (c) say that
  extensions need the official Tailscale app. Prototype before deciding, starting with (c) for the
  first release.

### Phase 4: desktop

- Same `tailnet` module in `client-core` / `desktop` behind a feature flag. Same flow: a switch in
  the server settings → open `auth_url` in the browser → SOCKS loopback for `api_client`.
- The Linux FUSE, macOS File Provider and Windows Cloud Files integrations (other todo items) talk
  to the server through the desktop process, so they get the tunnel for free when the main app
  owns it.
- Lower priority than mobile, because desktop users can easily install the official Tailscale app.

### Phase 5 (optional): tailnet identity as sign-in

- On the tailnet listener, `WhoIs(peer)` tells the server which Tailscale user and node is
  connecting. An admin setting **Trust Tailscale identity** could map a tailnet login
  (`alice@github`) to a Home Space user and skip the password, like `tailscale serve` identity
  headers.
- Off by default. Only accept it on the tailnet listener, never from headers on the LAN listener,
  and still issue normal JWT sessions so the rest of the API doesn't change.

## Security notes

- The tailnet listener is only reachable by nodes the tailnet's ACLs allow. Recommend an ACL in
  the docs (`tag:homespace` for the server, only the household's users allowed to it).
- Never enable **Funnel**. That would make it public again, which defeats the point.
- Treat the server's node key like a secret: state dir `0700`, not in backups of the storage root,
  wiped on Disconnect.
- Mobile nodes: consider **ephemeral** nodes for extensions and key expiry for phones, and show
  the user that the phone appears as a device in their Tailscale admin console.
- Rate limits and the "known device" sign-in logic key off the client address. On the tailnet
  that is a stable `100.x` address per node, which is fine, but test it.

## Open questions

- **libtailscale vs a pure Rust implementation:** what is the state of a Rust tsnet now?
  Cgo + Go in the build is the main cost of this item (Docker build, Android NDK, iOS xcframework).
- **One tailnet or two?** Should a user be able to put the phone on *their own* tailnet while the
  server is on the admin's, through node sharing? Probably out of scope at first. The docs can
  point to Tailscale's "Share node" feature.
- **Pairing without a second sign-in:** the server could mint a pre-auth key for the phone (that
  needs a Tailscale API / OAuth client, not just the node sign-in) and put it in the pairing QR
  code, so the phone joins without the user signing in to Tailscale. That's nicer, but it means
  storing a Tailscale API credential on the server. Decide after phase 3.
- **Device limits:** Tailscale's free plan limits devices per user. Every phone and every
  extension node counts.
- **Headscale:** test the whole flow against Headscale (the interactive sign-in URL looks
  different there).

## Rough size

- Phase 0: S. Phase 1 + 2: M–L (mostly build plumbing and the second listener). Phase 3: L (Go
  cross-compiling for mobile, the proxy in `api_client`, background behaviour). Phase 4: M.
  Phase 5: S–M.
