# Rotating the signing secret and the secrets key

Done (2026-11-26). How it works and why, for whoever changes it next.

## Why

A value in the old `HOMESPACE_JWT_SECRET_PREVIOUS` or `HOMESPACE_SECRETS_KEY_PREVIOUS` was as
good as the current one: a leaked secret kept working while it sat there. And the secrets key's
`_PREVIOUS` could never be removed safely: authenticator app secrets were sealed again only when
their code was accepted, and password-folder keys never were.

## Decisions

- **No `_PREVIOUS` values.** `token::Keys` and `SecretBox` hold one key each. The server refuses
  to start while either `_PREVIOUS` variable is set (`TenantSettings::validate`).
- **Both keys are always rotated together.** After a breach nobody can tell which one leaked.
- **Everyone is signed out.** Rotation deletes every session of the tenant. The new signing
  secret also voids access tokens, tickets and share tokens.
- **`homespace-ctl secrets rotate` is the only supported way.** Changing `HOMESPACE_SECRETS_KEY`
  by hand makes authenticator apps and password folders unreadable.
- **Key guard.** `secrets_key_checks` holds, per tenant, a value sealed with the secrets key
  (`SecretBox::seal_check`). At start (`app::tenants::check_secrets_key`) a key that does not
  open it is refused (host mode: the tenant is left out and logged), as is a missing key while a
  check exists. Without a check yet, one is recorded only if the key opens every sealed row.

## The command

`auth::secrets_key::rotate`, in one transaction (`db::SecretsRotation`, which locks the tables it
rewrites): open every TOTP secret and password-folder key with the current key (or, once, the
legacy `HOMESPACE_SECRETS_KEY_PREVIOUS` still in the environment), seal them with the new key,
replace the check, delete the tenant's sessions. A check that already opens with the new key
means an earlier run committed: nothing is done.

- **Automatic** (`commands/secrets/installed.rs`) for an installation from `homespace-ctl
  install` (no `DATABASE_URL`): new values go to `secrets/*.new` or `.env.new`, `compose stop
  app`, `compose run --rm --no-deps -T --entrypoint homespace-ctl app secrets reseal` with the
  new key on standard input (the container has the current one), then the `.new` files replace
  the old ones and `compose up -d --no-deps --force-recreate app` (a bind-mounted file is fixed
  at container creation). A failed run keeps the `.new` files; running again reuses them.
  Without a secrets key in the installation only the signing secret is rotated and everyone is
  signed out.
- **Guided** (`commands/secrets/wizard.rs`) with `--database-url` or `--env-file`, for secrets
  kept anywhere (a secret manager that passes them in at start): the administrator confirms the
  server is stopped, gives the current secrets key (unless the environment has it), stores the
  new values shown, types the new key back, and starts the server once it is done.
