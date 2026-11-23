# Server manager (`homespace-ctl`)

**Status (2026-11-23):** plan only, nothing implemented.

## Problem

Running Home Space on a server today means reading the "Running on a server" section of the
README, writing a `.env` by hand (generating the JWT secret and database password with
`openssl`/`head -c`), creating directories with the right owner, and typing long
`docker compose -f compose.yaml -f compose.prod.yaml ...` commands. Users are added with
`compose run --rm app user add`, which needs the source checkout because `compose.yaml` builds
the image instead of pulling it. There is no way to see how the server is doing without
`docker stats` and `du`, and no way to know a new release is out other than watching GitHub.

The `homespace` binary also mixes two jobs: serving (`serve`) and administration (`migrate`,
`user add|list|admin|passwd`, `search reindex`). The multi-tenancy plan adds a third group
(`tenant ...`, see `multi-tenancy.md`), which is not built yet.

## Goal

One tool, `homespace-ctl`, written in Rust, with a terminal UI (TUI) for people and plain
subcommands for scripts, that:

1. **Sets up a server for the first time**: finds Docker or Podman and a compose
   implementation, asks for the settings, writes the environment and compose files, starts the
   server and creates the first administrator.
2. **Shows how the running server is doing**: CPU and memory of each container (from
   `docker stats` / `podman stats`), health, uptime, file storage size, database size and free
   disk space.
3. **Takes over the administration commands** of the `homespace` binary, which keeps only
   `serve`.
4. **Shows when an update is available and installs it** with compose (`pull` + `up -d`).

Not in scope for the first version: Kubernetes or bare-metal (non-container) installs, Windows
and macOS hosts (Linux only; Docker Desktop may work but is not tested), reverse proxy and TLS
setup (the tool prints a sample nginx/Caddy block and links `docs/nginx`), backups beyond a
database dump before an update, Postgres major-version upgrades.

## Overview

```
 host                                            containers (compose project "homespace")
 ┌──────────────────────────────┐                ┌───────────────────────────────────────┐
 │ homespace-ctl (TUI / CLI)    │  compose CLI   │ app:  homespace serve                 │
 │  - setup wizard              │ ─────────────▶ │       homespace-ctl (admin commands)  │
 │  - dashboard (stats, logs)   │  pull/up/ps/   │ db:   postgres:18                     │
 │  - update                    │  stats/logs    └───────────────────────────────────────┘
 │  - user / tenant / search ───┼── compose exec app homespace-ctl user add ... ──▲
 └──────────────────────────────┘
        │ reads/writes
        ▼
 /srv/homespace/  (install directory)
   compose.yaml        generated from the template embedded in homespace-ctl
   homespace.env       settings and secrets, mode 0600
   ctl.toml            engine, compose command, install choices
   db/  files/         bind-mounted data (when chosen)
```

The tool never talks to the Docker or Podman API directly in the first version: it shells out
to the compose CLI the user already has, so it works the same with Docker, rootful Podman and
rootless Podman, and every action can be reproduced by hand.

## Design choices

### D1. One binary, two places it runs

`homespace-ctl` is one statically linked binary (musl, x86_64 and aarch64). It runs:

- **on the host**: TUI, setup, dashboard, update. These only need the compose CLI.
- **inside the app container**: the administration commands that need the database and the
  storage root (`user`, `tenant`, `search`, `stats storage`). The image ships it next to
  `homespace` at `/usr/local/bin/homespace-ctl`.

When a database command is run on the host, the tool forwards it into the container with the
same arguments: `compose exec app homespace-ctl user add me` when the app is running,
`compose run --rm --no-deps app ...` (with the entrypoint overridden) when it is stopped but
the database is up. The container's copy decides it is "inside" because `DATABASE_URL` is set
and `HOMESPACE_CTL_IN_CONTAINER=1` is in the image's environment.

Why: the admin code then always runs with the exact server version that owns the schema, needs
no published database port, no database password on the host, and no access to the blob
directory (which with rootless Podman or `user:` is owned by another uid). It also works
unchanged for object storage later.

Alternatives considered:
- *Connect to Postgres from the host* over the published `127.0.0.1:5433`: version skew between
  the host tool and the schema, needs the password, can't reach the blobs for `search reindex`.
- *Go through the admin HTTP API*: needs an admin token, so it can't create the first admin,
  and the API has no tenant or reindex endpoints.
- *Two binaries* (`homespace-admin` in the image, `homespace-ctl` on the host): cleaner
  dependency split but two things to name, version and document; the TUI dependencies
  (ratatui, crossterm) add well under a megabyte, so one binary is the simpler choice.

### D2. Crate layout

- `server/crates/config`: `Config`, `TenantSettings`, `checks` and the tenant secret loading,
  moved out of `app` so `ctl` validates settings exactly as the server does (secret strength,
  https-or-loopback URLs, OIDC completeness) before writing them.
- `server/crates/admin`: the current `app/src/commands/{user,search}.rs`, plus `tenant` and
  `stats`, as library functions returning data (not printing), so the TUI and the CLI share them.
- `server/crates/ctl`: the `homespace-ctl` binary: clap CLI, ratatui TUI, compose driver,
  setup, update.
- `server/crates/app`: `homespace serve` only.

The server workspace keeps one version number, so `homespace-ctl --version` and the image tag
always match for a release.

### D3. Finding the container engine and compose

Probed in this order, first that works wins, and the user can override it in the wizard:

1. `docker compose version` (Compose v2 plugin)
2. `podman compose version` (Podman's wrapper; it calls docker-compose or podman-compose)
3. `docker-compose version` (standalone v2, or v1, which is refused: too old for `depends_on`
   conditions and `--format json`)
4. `podman-compose version` (needs 1.1 or newer for `--format json` on `ps`)

For the engine it checks `docker info` / `podman info` (daemon running, user in the `docker`
group, rootless or not, cgroup v2 for stats). When nothing is found it shows the install
command for the detected distribution (`/etc/os-release`) and stops; it never installs
packages itself.

The result is saved in `ctl.toml` (`engine = "podman"`, `compose = ["podman", "compose"]`,
`rootless = true`) so later runs don't probe again; `homespace-ctl doctor` re-probes.

### D4. Compose file: an image-based template, owned by the tool

`compose.yaml` in the repository builds from source and is for development. The tool writes
its own `compose.yaml` into the install directory from a template embedded in the binary
(`deploy/compose.yaml` in the repo, also usable by hand). It differs from the development one:

- `image: docker.io/homespaceapp/home-space:${HOMESPACE_VERSION}` instead of `build:`; the
  version is pinned (never `latest`) so an update is an explicit step and can be rolled back.
- `env_file: homespace.env` on `app` instead of listing every variable. This matters for
  multi-tenancy: tenant *n*'s variables are `HOMESPACE_JWT_SECRET_2`, `HOMESPACE_OIDC_ISSUER_2`
  and so on, and a fixed `environment:` list can't pass names that aren't known in advance.
- Postgres not published on the host by default.
- Bind mounts or named volumes, as chosen in setup (bind mounts are the default for a server so
  `du`, backups and the uid ownership of `compose.prod.yaml` keep working).
- A `healthcheck` on `app` that calls `/api/v1/health` (needs a tiny probe, since the
  chainguard image has no curl: `homespace-ctl health` inside the container does it).

The user may edit the generated file. The tool keeps a hash of what it wrote and, when the
file was changed, shows a diff and asks before overwriting it on update.

### D5. Configuration: a settings schema and an env file

The wizard and the settings screen are generated from one table in `ctl` (name, description,
kind, default, required, per-tenant or global), kept in sync with `Config` by a test that
compares it with the clap definitions, so a new server setting can't be forgotten.

Kinds: secret (generated, masked, never shown again unless asked), URL, bool, integer with unit
(bytes shown as `10 GiB`), path, choice. Everything is validated with the `config` crate before
it is written.

First-time setup asks, in this order, with defaults so Enter-Enter-Enter gives a working server:

1. Install directory (`/srv/homespace` as root, `~/.local/share/homespace` otherwise).
2. Public URL (`https://files.example.com`), port to publish (8080), bind address
   (`127.0.0.1` behind a proxy, `0.0.0.0` otherwise), and whether a reverse proxy sets
   `X-Forwarded-For` (`HOMESPACE_TRUST_FORWARDED_FOR`).
3. Where the database and files live (bind directories or named volumes), and which host user
   owns them. As root it offers to create a system user `homespace` and the directories with
   mode 700; rootless Podman uses `podman unshare chown` as the README describes.
4. Secrets: `HOMESPACE_DB_PASSWORD` (URL-safe characters only, since it goes into
   `DATABASE_URL`) and `HOMESPACE_JWT_SECRET` are generated.
5. Sign-in: password login on/off, optional single sign-on (issuer, client id/secret, button
   name, required group); the issuer is checked by fetching its discovery document.
6. Limits: max upload size, transfer TTL, user deletion delay (pre-filled, can be skipped).
7. Summary, then write files, `compose pull`, `compose up -d`, wait for healthy, then create
   the first administrator (username + password typed twice, passed to the container on stdin
   via `--password-stdin`), and print the URL.

**Secrets** go into `homespace.env` with mode 0600, owned by the user running the tool. The
server already reads `HOMESPACE_JWT_SECRET_FILE`, so a later option is compose `secrets:` files
instead (see open questions).

**Multi-tenancy.** The settings screen shows tenant 1 with the unsuffixed names (as the server
reads them) and a tab per further tenant with the `_n` names. `tenant add` (forwarded into the
container, D1) creates the row and domain; the TUI then opens that tenant's tab with a freshly
generated JWT secret, writes `HOMESPACE_JWT_SECRET_<n>` etc. into `homespace.env`, switches
`HOMESPACE_TENANT_MODE` to `host` when a second tenant appears (after asking), and offers the
restart the server needs (the multi-tenancy plan: tenant changes need a restart). The
`ensure_distinct_secrets` check runs before writing.

**Changing a setting later** writes the file and offers `compose up -d` (which recreates only
`app`). Settings that are dangerous to change are marked and need confirmation: the public URL
or a domain (WebAuthn RP id of protected folders, OIDC callbacks, saved client URLs), the
database password (Postgres keeps the old one, the tool runs `ALTER USER` through
`compose exec db psql` first), and the JWT secret (offered as a rotation: the old value moves to
`HOMESPACE_JWT_SECRET_PREVIOUS`).

### D6. Dashboard

The TUI's home screen when the server is installed. Refreshes every 2 seconds.

- **Containers**: from `compose ps --format json`: state, health, uptime, restart count, image
  tag. From `docker stats --no-stream --format '{{json .}}'` /
  `podman stats --no-stream --format json` (field names differ, normalised into one struct):
  CPU %, memory used / limit, network and block I/O. A sparkline of the last few minutes of CPU
  and memory, kept in memory only.
- **Storage**: `homespace-ctl stats storage --json` inside the container (D1) reports bytes of
  live files, versions and the recycle bin (the same numbers as the admin panel's Server
  status), unfinished uploads, the top users by usage, free space of the `/data` filesystem,
  and `pg_database_size`. Refreshed every minute, since it queries the database.
- **Version**: running version, latest release, and an "update available" badge (D7).
- **Logs**: a pane with `compose logs -f --tail 200 app`, filterable by level.
- Keys for start, stop, restart, update, settings, users, tenants, reindex.

Rootless Podman without cgroup v2 delegation can't report CPU; the dashboard says so instead of
showing zeros. `homespace-ctl status --json` prints the same data for monitoring scripts.

### D7. Update check and update

**Check.** The latest release comes from the GitHub releases API
(`/repos/home-space-app/home-space/releases/latest`, which skips pre-releases), cached for 6
hours in the install directory. The running version comes from a new `version` field in
`GET /api/v1/capabilities` (fallback: the image's `org.opencontainers.image.version` label via
`docker image inspect`). The TUI shows the release notes. The check runs only when the tool runs;
there is no background service. It can be turned off (`update_check = false` in `ctl.toml`) and
sends nothing but the HTTP request (generic User-Agent, no install id).

**Update** (`homespace-ctl update [--to <version>]`):

1. Refuse if the target is older than the running version (downgrades go through `rollback`).
2. Show the release notes and warn about notes marked breaking.
3. Dump the database: `compose exec db pg_dump -Fc` into `backups/homespace-<version>-<date>.dump`
   (on by default, can be skipped). Migrations are forward-only, so this is the way back.
4. Write `HOMESPACE_VERSION=<new>` into `homespace.env`, keeping the previous value in
   `ctl.toml`.
5. If the new release's compose template differs from the installed file, show the diff (D4).
6. `compose pull app`, then `compose up -d`; the server applies migrations on start.
7. Wait for the app to be healthy (timeout 2 minutes). On failure show the last log lines and
   offer `rollback`: restore the previous version and, if migrations ran, restore the dump.

The database image is pinned in the template (`postgres:18`) and never changed by `update`; a
Postgres major upgrade is a separate, documented procedure.

**Updating the tool itself** is needed before a server update whose template changed. Plan:
`homespace-ctl self-update` downloads the matching release binary, checks it against the
`SHA256SUMS` file of the release, and replaces itself; when installed from a package (.deb/.rpm)
it tells the user to use the package manager instead.

### D8. Migrating the existing commands

| Today (`homespace ...`) | After |
|---|---|
| `serve` | `homespace serve` (unchanged; still runs migrations on start) |
| `migrate` | `homespace-ctl db migrate` (runs inside the container) |
| `user add <name> [--admin] [--password-stdin]` | `homespace-ctl user add ...` |
| `user list` | `homespace-ctl user list [--json]` |
| `user admin <name> [--revoke]` | `homespace-ctl user admin ...` |
| `user passwd <name> [--password-stdin]` | `homespace-ctl user passwd ...` |
| `search reindex [--all]` | `homespace-ctl search reindex ...` |
| (planned) `tenant ...` from `multi-tenancy.md` | `homespace-ctl tenant ...` |

Same argument names, so scripts only change the binary name. Every command gets `--json`
output for the TUI and for scripts, and `--tenant` once tenants exist.

Compatibility: for one release `homespace user ...` etc. still work but print a deprecation
warning and call the `admin` crate; the release after removes them. The Dockerfile keeps
`ENTRYPOINT ["homespace"]` / `CMD ["serve"]`. Things to update: the comment at the top of
`compose.yaml`, the README sections "Compose" and "Running on a server", `e2e/fixtures/server.ts`
(calls `homespace user add --admin --password-stdin`), and `docs/todo/admin-panel.md` /
`multi-tenancy.md` references.

### D9. TUI and CLI stack

- `clap` (already used) for subcommands; running `homespace-ctl` with no subcommand in a
  terminal opens the TUI, without a terminal it prints help.
- `ratatui` + `crossterm` for the TUI; `tokio` for running compose commands and polling stats
  without blocking the UI.
- `serde_json` for compose/stats output, `toml` for `ctl.toml`, `reqwest` (rustls) for the
  release check and the OIDC discovery probe.
- Screens: Setup wizard, Dashboard, Settings (per tenant), Users, Tenants, Updates, Logs.
  Mouse optional, everything reachable by keyboard, works over SSH in 80x24.
- Compose commands are run through one `Compose` type that builds the argument list
  (`-p homespace -f <dir>/compose.yaml --env-file <dir>/homespace.env`), so it is unit-testable
  with recorded outputs of Docker, Podman and podman-compose.

### D10. Distribution

The release workflow builds `homespace-ctl` for `x86_64-unknown-linux-musl` and
`aarch64-unknown-linux-musl`, attaches them with a `SHA256SUMS` file, and copies it into the
server image. Optional later: an install script (`curl ... | sh` that only downloads and checks
the binary), `.deb`/`.rpm` like the desktop client.

## Phases

1. **Split**: `config` and `admin` crates, `ctl` binary with the migrated commands (D8) running
   locally with `DATABASE_URL`, deprecation shims in `homespace`, binary in the image. Version
   in `/capabilities`.
2. **Compose driver and setup**: engine detection (D3), `deploy/compose.yaml` template (D4),
   settings schema and env file (D5), forwarding into the container (D1), non-interactive
   `homespace-ctl setup --answers answers.toml`, then the TUI wizard.
3. **Dashboard**: stats, storage, logs (D6), `status --json`.
4. **Updates**: check, update with dump, rollback, self-update (D7); release workflow (D10).
5. **Tenants** in the tool once the `tenant` commands exist (D5, multi-tenancy).

Testing: unit tests for parsing compose/stats output (recorded fixtures for Docker 27+,
Podman 5, podman-compose 1.x), the settings schema test (D5), and an integration test in CI
that runs `setup --answers` against Docker, checks the server comes up, then `update --to` the
same version.

## Open questions

1. **Name**: `homespace-ctl`, `hsctl` or `homespace admin` (a subcommand of one binary)?
2. **One binary in two places (D1)** vs. the TUI on the host connecting to Postgres directly.
   The plan picks forwarding into the container; is a published database port acceptable
   instead?
3. **Secrets**: plain `homespace.env` (0600) for the first version, or compose `secrets:` files
   with the `_FILE` variants from the start? Podman-compose's secret support is limited.
4. **Install directory and owner**: `/srv/homespace` run as root with a `homespace` system user,
   or rootless by default (the tool's user owns everything)?
5. **Existing installs** set up from the README (repo checkout, `compose.yaml` +
   `compose.prod.yaml`, `build:`): should `homespace-ctl adopt` import their `.env` and switch
   them to the image-based template, or only new installs?
6. **Update channel**: only stable releases, or an opt-in for pre-releases?
7. **Automatic updates**: out of scope, or a systemd timer that runs `update --yes` (with the
   dump) at night?
8. **Rollback**: is restoring the pre-update dump acceptable when migrations ran, given writes
   made after the update are lost?
9. **Remote management**: should the TUI manage a server over SSH from a laptop
   (`--host ssh://server`, using `DOCKER_HOST`/`CONTAINER_HOST`), or always run on the server?
10. **Deprecation**: keep `homespace user ...` working for one release (the plan), or drop it at
    once since there are few installs?
11. **Reverse proxy**: should setup also write a Caddy or nginx config, or only print a sample?
12. **systemd**: for rootless Podman, generate a Quadlet / `systemctl --user` unit so the server
    starts at boot (compose `restart: unless-stopped` doesn't survive a reboot there without
    `podman-restart.service`)?
