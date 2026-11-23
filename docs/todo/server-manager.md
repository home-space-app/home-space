# Server manager (`homespace-ctl`)

**Status (2026-11-23):** phase 1 started: the `ctl` crate with `homespace-ctl
user add|list|admin|passwd [--tenant]` and `search reindex` exists; `homespace` only serves.
Phase 2 started: `homespace-ctl install` (interactive; see the README "Installing on a server")
writes one image-based `compose.yaml` + `.env` (+ `secrets/`, + an nginx example), starts the
server and creates the first administrator.

**Decided (2026-11-23, later):** `install` replaces the planned `setup`; the parts below that
differ from what it does were superseded by these:
- `homespace-ctl` is in the server image again, and Postgres is never published on the host.
  Without `DATABASE_URL`, the host `homespace-ctl` runs `user`/`search` (and later `tenant`)
  as `<engine> compose exec app homespace-ctl ...`, found through
  `~/.config/home-space/installation.toml` of the installing user (or `HOMESPACE_CTL_CONFIG`). This settles open question 11 and reverses D1's "not in the image".
  With `DATABASE_URL` set it still works directly, as before.
- Database password: `HOMESPACE_DB_PASSWORD[_FILE]`, which the server and `homespace-ctl` apply
  to a password-less `DATABASE_URL` (open question 3). `HOMESPACE_OIDC_CLIENT_SECRET_FILE` exists.
- The install directory defaults to `/opt/homespace`. With host directories a dedicated system
  user (`homespace`, created through `sudo`) owns the data and runs the containers (`user:`),
  except under rootless Podman, where the invoking user does (`userns_mode: keep-id`).
- The image is pinned to the version of the `homespace-ctl` that installs it (they are released
  together); moving to another version comes with `update` (D7).
- `install` only creates directories (the installation and the data directories must not exist),
  and `homespace-ctl` reads no `.env` from the current directory unless `--env-file` names one.
- With nginx as the proxy, `install` writes an example site rendered from `docs/nginx` into
  `<dir>/nginx/` and leaves installing it to the user (open question 9).

**Decided (2026-11-23):**
- ~~Postgres is never published on the host~~: reopened by shipping `homespace-ctl` outside the
  image, see D1 and open question 11. (Superseded above: it is not published.)
- Secrets: both compose `secrets:` files and a plain env file are offered; compose secrets are
  the default (D5).
- Rootless by default: the tool and the containers run as the invoking user, no root (D3, D5).
- Existing installs made with one of the documented methods are adopted and managed; anything
  else is left alone (D11).
- No terminal UI: `homespace-ctl` is plain subcommands, interactive only where it prompts (the
  setup questions, passwords). A TUI can come later on top of the same commands.
- The `homespace` binary takes no subcommand and only serves; the administration commands moved
  to `homespace-ctl` with no deprecation period (D8).

## Problem

Running Home Space on a server today means reading the "Running on a server" section of the
README, writing a `.env` by hand (generating the JWT secret and database password with
`openssl`/`head -c`), creating directories with the right owner, and typing long
`docker compose -f compose.yaml -f compose.prod.yaml ...` commands. Users are added with
`compose run --rm app user add`, which needs the source checkout because `compose.yaml` builds
the image instead of pulling it. There is no way to see how the server is doing without
`docker stats` and `du`, and no way to know a new release is out other than watching GitHub.

The `homespace` binary also mixes two jobs: serving (`serve`) and administration (`migrate`,
`user add|list|admin|passwd`, `search reindex`). Multi-tenancy (`multi-tenancy.md`) is built in
the server (tenant ids, per-tenant `_n` settings, `HOMESPACE_TENANT_MODE=host`), but its
`tenant ...` administration commands are not.

## Goal

One tool, `homespace-ctl`, written in Rust, with plain subcommands for people and scripts
alike, that:

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
 │ homespace-ctl (CLI)          │  compose CLI   │ app:  homespace                       │
 │  - setup                     │ ─────────────▶ │                                       │
 │  - status (stats, logs)      │  pull/up/ps/   │ db:   postgres:18                     │
 │  - update                    │  stats/logs    └───────────────────────────────────────┘
 │  - user / tenant / search ───┼── DATABASE_URL → 127.0.0.1:5433 (db) ───────────▲
 └──────────────────────────────┘
        │ reads/writes
        ▼
 ~/.local/share/homespace/  (install directory, owned by the invoking user)
   compose.yaml        generated from the template embedded in homespace-ctl
   homespace.env       settings (and secrets in env-file mode), mode 0600
   secrets/            one file per secret, mode 0600 (default secrets mode)
   ctl.toml            engine, compose command, install choices
   db/  files/         bind-mounted data (when chosen)
```

The tool never talks to the Docker or Podman API directly in the first version: it shells out
to the compose CLI the user already has, so it works the same with Docker, rootful Podman and
rootless Podman, and every action can be reproduced by hand.

## Design choices

### D1. Shipped on its own, runs on the host

**Decided (2026-11-23):** `homespace-ctl` is not in the server image. It is released as its own
download for Linux x86_64 (static musl), Windows x86_64 and macOS Apple Silicon (D10), and runs
where the operator is: on the server, or on a laptop that can reach it.

- Compose commands (setup, status, update) only need the compose CLI.
- Database commands (`user`, `tenant`) connect with `DATABASE_URL`, today through
  the Postgres port compose publishes on `127.0.0.1` (`HOMESPACE_DB_PORT`, 5433).
- Storage commands (`search reindex`) also need the blob directory (`HOMESPACE_STORAGE_ROOT`),
  so they only work on the server, with a bind-mounted data directory the user can read.

`homespace-ctl` never migrates (decided): the server applies migrations when it starts, which
every install and update goes through, so a `homespace-ctl` newer than the server cannot move
the database ahead of it. The database commands therefore need a server that has started once. A ctl older or newer than the
schema may still fail on a query; checking the server version first (from `/capabilities` or
`_sqlx_migrations`) and refusing on a mismatch is still to do.

This replaces the earlier plan of shipping the binary in the image and forwarding database
commands into the container (`compose exec app homespace-ctl ...`). It is simpler to release and
keeps the image to the server, at the cost of needing database access from the host, which
conflicts with "Postgres is never published on the host" (open question 11).

Alternatives considered:
- *In the image, forwarded into the container*: the earlier plan; always the server's own
  version, no published database port, but the image carries an admin tool.
- *Go through the admin HTTP API*: needs an admin token, so it can't create the first admin,
  and the API has no tenant or reindex endpoints. Still a candidate for the commands that
  don't need the database directly (storage stats, reindex) once endpoints exist.

### D2. Crate layout

- `server/crates/config`: `Config`, `TenantSettings`, `checks` and the tenant secret loading,
  moved out of `app` so `ctl` validates settings exactly as the server does (secret strength,
  https-or-loopback URLs, OIDC completeness) before writing them.
- `server/crates/admin`: the current `app/src/commands/{user,search}.rs`, plus `tenant` and
  `stats`, as library functions returning data (not printing), so text and `--json` output
  share them. Not split out yet: the commands live in `ctl/src/commands`, and the keyless
  account changes they need are `auth::offline` (no JWT secret, so they work for any tenant).
- `server/crates/ctl`: the `homespace-ctl` binary: clap CLI, compose driver,
  setup, update.
- `server/crates/app`: the `homespace` server, which takes no subcommand (done).

The server workspace keeps one version number, so `homespace-ctl --version` and the image tag
always match for a release.

### D3. Finding the container engine and compose

Probed in this order, first that works wins, and the user can override it in `setup`:

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
- `db` published on `127.0.0.1` only, for the database commands (D1); `psql` and `pg_dump` run
  through `compose exec db`. Whether to keep that is open question 11.
- Bind mounts or named volumes, as chosen in setup (bind mounts are the default for a server so
  `du`, backups and the uid ownership of `compose.prod.yaml` keep working).
- A `healthcheck` on `app` that calls `/api/v1/health` (needs a tiny probe, since the
  chainguard image has no curl and `homespace-ctl` is no longer in it: to be decided).

The user may edit the generated file. The tool keeps a hash of what it wrote and, when the
file was changed, shows a diff and asks before overwriting it on update.

### D5. Configuration: a settings schema and an env file

The setup questions and `homespace-ctl config` are generated from one table in `ctl` (name, description,
kind, default, required, per-tenant or global), kept in sync with `Config` by a test that
compares it with the clap definitions, so a new server setting can't be forgotten.

Kinds: secret (generated, masked, never shown again unless asked), URL, bool, integer with unit
(bytes shown as `10 GiB`), path, choice. Everything is validated with the `config` crate before
it is written.

First-time setup asks, in this order, with defaults so Enter-Enter-Enter gives a working server:

1. Install directory (`~/.local/share/homespace` by default; any directory the user owns).
2. Public URL (`https://files.example.com`), port to publish (8080), bind address
   (`127.0.0.1` behind a proxy, `0.0.0.0` otherwise), and whether a reverse proxy sets
   `X-Forwarded-For` (`HOMESPACE_TRUST_FORWARDED_FOR`).
3. Where the database and files live (bind directories under the install directory by default,
   or named volumes). **Rootless by default**: the containers run as the invoking user (rootless
   Podman, or rootless Docker when that is what is installed), the directories are created with
   mode 700 and owned through `podman unshare chown` so the uid inside the container maps to
   them, and the template has no `user:` lines. Rootful Docker still works but `setup` warns
   that the daemon runs as root and asks before continuing; root installs with a dedicated
   system user are not offered in the first version. A publish port below 1024 needs
   `net.ipv4.ip_unprivileged_port_start` or a reverse proxy; `setup` says so.
4. Secrets: the storage mode (compose secrets, the default, or env file), then
   `HOMESPACE_DB_PASSWORD` (URL-safe characters only, since it may go into `DATABASE_URL`) and
   `HOMESPACE_JWT_SECRET` are generated.
5. Sign-in: password login on/off, optional single sign-on (issuer, client id/secret, button
   name, required group); the issuer is checked by fetching its discovery document.
6. Limits: max upload size, transfer TTL, user deletion delay (pre-filled, can be skipped).
7. Summary, then write files, `compose pull`, `compose up -d`, wait for healthy, then create
   the first administrator (username + password typed twice, passed to the container on stdin
   via `--password-stdin`), and print the URL.

**Secrets** have two modes, chosen in setup and switchable later with `homespace-ctl config`:

- **Compose secrets (default).** Each secret is a file in `secrets/` (mode 0600), declared under
  top-level `secrets:` with `file:` and mounted into the containers at `/run/secrets/<name>`. The
  env file only holds `..._FILE` paths. Values never appear in `docker inspect`, `compose
  config` or the process environment.
- **Env file.** Secrets are plain variables in `homespace.env` (mode 0600), as the README does
  today. Simpler to edit by hand and to adopt existing installs with (D11).

What compose secrets need from the server, since today only `HOMESPACE_JWT_SECRET_FILE` exists:

- `_FILE` variants for every secret: `HOMESPACE_JWT_SECRET_PREVIOUS_FILE`,
  `HOMESPACE_OIDC_CLIENT_SECRET_FILE`, and their per-tenant forms (`..._2_FILE`), read by the
  same `TenantSecrets` seam the multi-tenancy work added.
- The database password out of `DATABASE_URL`: a `HOMESPACE_DB_PASSWORD_FILE` (or
  `DATABASE_URL_FILE`) that the server combines with a password-less URL. Postgres reads
  `POSTGRES_PASSWORD_FILE` already.

Compose implementations handle file secrets a little differently (Docker Compose bind-mounts
them; podman-compose's support of `uid`/`gid`/`mode` has varied between releases, to be checked
against the versions D3 accepts). With rootless containers the mounted file is owned by the invoking user, which is
the uid the container runs as, so no `uid:` mapping is needed. The engine probe (D3) records
whether secrets work and falls back to env-file mode with a warning if not.

**Multi-tenancy** (implemented in the server). `homespace-ctl config` shows tenant 1 with the
unsuffixed names (as the server reads them) and `--tenant <n>` the `_n` names. `tenant add`
(D1) creates the row and domain, then generates that tenant's JWT
secret, writes `HOMESPACE_JWT_SECRET_<n>` (or `secrets/jwt_secret_<n>` and
`HOMESPACE_JWT_SECRET_<n>_FILE`, adding the secret to the compose file) and the other settings, switches
`HOMESPACE_TENANT_MODE` to `host` when a second tenant appears (after asking), and offers the
restart the server needs (the multi-tenancy plan: tenant changes need a restart). The
`ensure_distinct_secrets` check runs before writing.

**Changing a setting later** writes the file and offers `compose up -d` (which recreates only
`app`). Settings that are dangerous to change are marked and need confirmation: the public URL
or a domain (WebAuthn RP id of protected folders, OIDC callbacks, saved client URLs), the
database password (Postgres keeps the old one, the tool runs `ALTER USER` through
`compose exec db psql` first), and the JWT secret (offered as a rotation: the old value moves to
`HOMESPACE_JWT_SECRET_PREVIOUS`).

### D6. Status

`homespace-ctl status` prints a snapshot (`--watch` refreshes it every 2 seconds, `--json` for
monitoring scripts).

- **Containers**: from `compose ps --format json`: state, health, uptime, restart count, image
  tag. From `docker stats --no-stream --format '{{json .}}'` /
  `podman stats --no-stream --format json` (field names differ, normalised into one struct):
  CPU %, memory used / limit, network and block I/O.
- **Storage**: `homespace-ctl stats storage --json` (D1; through the admin API when run away from
  the server) reports bytes of
  live files, versions and the recycle bin (the same numbers as the admin panel's Server
  status), unfinished uploads, the top users by usage, free space of the `/data` filesystem,
  and `pg_database_size`. With `--watch` refreshed every minute, since it queries the database.
- **Version**: running version, latest release, and an "update available" badge (D7).
- **Logs**: `homespace-ctl logs [-f] [--level warn]` wraps `compose logs --tail 200 app`.
- `start`, `stop` and `restart` wrap the compose commands.

Rootless Podman without cgroup v2 delegation can't report CPU; `status` says so instead of
showing zeros.

### D7. Update check and update

**Check.** The latest release comes from the GitHub releases API
(`/repos/home-space-app/home-space/releases/latest`, which skips pre-releases), cached for 6
hours in the install directory. The running version comes from a new `version` field in
`GET /api/v1/capabilities` (fallback: the image's `org.opencontainers.image.version` label via
`docker image inspect`). `update` shows the release notes. The check runs only when the tool runs;
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
| `serve` | `homespace`, no subcommand (still runs migrations on start) |
| `migrate` | removed: starting `homespace` migrates |
| `user add <name> [--admin] [--password-stdin]` | `homespace-ctl user add ...` |
| `user list` | `homespace-ctl user list [--json]` |
| `user admin <name> [--revoke]` | `homespace-ctl user admin ...` |
| `user passwd <name> [--password-stdin]` | `homespace-ctl user passwd ...` |
| `search reindex [--all]` | `homespace-ctl search reindex ...` |
| (planned in `multi-tenancy.md`, not built) `tenant ...` | `homespace-ctl tenant ...` |

Same argument names, so scripts only change the binary name. The `user` commands take
`--tenant <id|domain>` (done), optional while there is one tenant and required once there are
several. Every command gets `--json` output for scripts.

Compatibility (decided): no deprecation period. `homespace` rejects the old subcommands,
including `serve`, so a compose file with `command: serve` must drop it. The Dockerfile keeps
`ENTRYPOINT ["homespace"]` without a `CMD`; `homespace-ctl` is not in the image (D1). Things to update: the comment at the top of
`compose.yaml`, the README sections "Compose" and "Running on a server", `e2e/fixtures/server.ts`
(calls `homespace user add --admin --password-stdin`), and `docs/todo/admin-panel.md` /
`multi-tenancy.md` references.

### D9. CLI stack

- `clap` (already used) for subcommands; with no subcommand it prints help.
- Prompts (setup questions, confirmations) only when standard input is a terminal; otherwise
  every answer must come from flags or `--answers`, so scripts never hang.
- `tokio` for running compose commands and polling stats.
- `serde_json` for compose/stats output, `toml` for `ctl.toml`, `reqwest` (rustls) for the
  release check and the OIDC discovery probe.
- Compose commands are run through one `Compose` type that builds the argument list
  (`-p homespace -f <dir>/compose.yaml --env-file <dir>/homespace.env`), so it is unit-testable
  with recorded outputs of Docker, Podman and podman-compose.

### D10. Distribution

The release workflow (`ctl` job, done) builds `homespace-ctl` for `x86_64-unknown-linux-musl`,
`x86_64-pc-windows-msvc` and `aarch64-apple-darwin` and attaches
`homespace-ctl-<version>-{linux-x86_64.tar.gz,windows-x86_64.zip,macos-arm64.tar.gz}`, covered by
the release's `SHA256SUMS`. Not in the server image. Not signed or notarized, like the desktop
app. Later, if asked for: Linux aarch64. Optional later: an install script (`curl ... | sh` that only downloads and checks
the binary), `.deb`/`.rpm` like the desktop client.

### D11. Existing installs

`homespace-ctl adopt [<dir>]` (also offered by `setup` when it finds one) takes over an install
only when it matches one of the documented methods; anything else is reported as "not managed"
and left untouched, and the tool's other commands refuse to touch it.

Recognised:
- the README "Running on a server" setup: a checkout with `compose.yaml` + `compose.prod.yaml`
  and a `.env` next to them;
- the README "Compose" setup: `compose.yaml` alone, named volumes `pgdata` and `files`;
- an install made by `homespace-ctl` itself.

Detection: the compose project's containers (`com.docker.compose.project` labels, `compose ls`)
and their config files, compared with the known files of released versions by hash; local
edits are shown as a diff and the user decides whether to keep or drop them.

Adopting: stop the stack, write the image-based template (D4) keeping the same volumes or bind
directories and the same project name, so no data moves; carry the `.env` over in env-file mode
(switching to compose secrets is offered afterwards); remove the Postgres port mapping; pin
`HOMESPACE_VERSION` to the version that was running; start and wait for healthy. The original
files are kept as `*.pre-ctl` for going back. Installs that run as root with a `homespace`
system user (the README's production setup) are adopted as they are, rootful, since moving the
data to rootless ownership is a separate step the tool offers but doesn't force.

## Phases

1. **Split**: `ctl` binary with the migrated commands (D8) running locally with
   `DATABASE_URL`, `homespace` serving only, released on its own (done). Still to do: `config`
   and `admin` crates, `--json` output. Version in `/capabilities` (done).
2. **Compose driver and setup**: engine detection (D3), `deploy/compose.yaml` template (D4),
   settings schema and env file (D5), non-interactive
   `homespace-ctl setup --answers answers.toml`, then interactive `setup`.
3. **Status**: stats, storage, logs (D6), `status --json`.
4. **Updates**: check, update with dump, rollback, self-update (D7); release workflow (D10).
5. **Tenants**: the `tenant ...` commands and per-tenant settings (D5); the server side exists.

Testing: unit tests for parsing compose/stats output (recorded fixtures for Docker 27+,
Podman 5, podman-compose 1.x), the settings schema test (D5), and an integration test in CI
that runs `setup --answers` against Docker, checks the server comes up, then `update --to` the
same version.

## Open questions

1. **Name**: `homespace-ctl`, `hsctl` or `homespace admin` (a subcommand of one binary)?
2. **Adopted root installs (D11)**: adopt the README's rootful production setup as it is (the
   plan), or require moving it to rootless first?
3. **Database password**: `HOMESPACE_DB_PASSWORD_FILE` combined with the URL, or a whole
   `DATABASE_URL_FILE`?
4. **Update channel**: only stable releases, or an opt-in for pre-releases?
5. **Automatic updates**: out of scope, or a systemd timer that runs `update --yes` (with the
   dump) at night?
6. **Rollback**: is restoring the pre-update dump acceptable when migrations ran, given writes
   made after the update are lost?
7. **Remote management**: should the tool manage a server over SSH from a laptop
   (`--host ssh://server`, using `DOCKER_HOST`/`CONTAINER_HOST`), or always run on the server?
8. ~~**Deprecation**~~: decided, dropped at once (D8).
9. **Reverse proxy**: should setup also write a Caddy or nginx config, or only print a sample?
10. **systemd**: for rootless Podman, generate a Quadlet / `systemctl --user` unit so the server
   starts at boot (compose `restart: unless-stopped` doesn't survive a reboot there without
   `podman-restart.service`)?
11. **Database access from the host** (D1): keep Postgres published on `127.0.0.1` for
   `homespace-ctl`, or reach it another way (an SSH tunnel, `compose exec db` with a
   `psql`-speaking proxy, or admin API endpoints with a bootstrap token for the first admin)?
