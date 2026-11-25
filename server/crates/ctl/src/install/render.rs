//! The files an installation is made of, rendered from the plan. Nothing here touches the
//! system, so all of it is tested.

use std::fmt::Write as _;

use super::plan::{InstallPlan, Owner, SecretStore, Storage, defaults};

/// The nginx example shipped in the docs, rendered for the chosen server name and port.
const NGINX_TEMPLATE: &str = include_str!("../../../../../docs/nginx/example.domain.com.conf");
const NGINX_EXAMPLE_NAME: &str = "example.domain.com";
const NGINX_EXAMPLE_UPSTREAM: &str = "server 127.0.0.1:8080;";

/// Where secret files go, relative to the installation directory.
pub const SECRETS_DIR: &str = "secrets";
/// Where compose mounts a secret inside a container.
const SECRETS_MOUNT: &str = "/run/secrets";

/// A secret kept in its own file, mounted as a compose secret named after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretName {
    DbPassword,
    JwtSecret,
    SecretsKey,
    OidcClientSecret,
}

impl SecretName {
    pub fn file_name(self) -> &'static str {
        match self {
            Self::DbPassword => "db_password",
            Self::JwtSecret => "jwt_secret",
            Self::SecretsKey => "secrets_key",
            Self::OidcClientSecret => "oidc_client_secret",
        }
    }

    /// The variable that holds it in `.env`.
    fn env_var(self) -> &'static str {
        match self {
            Self::DbPassword => "HOMESPACE_DB_PASSWORD",
            Self::JwtSecret => "HOMESPACE_JWT_SECRET",
            Self::SecretsKey => "HOMESPACE_SECRETS_KEY",
            Self::OidcClientSecret => "HOMESPACE_OIDC_CLIENT_SECRET",
        }
    }
}

/// The secrets the plan has, with their values.
pub fn secrets(plan: &InstallPlan) -> Vec<(SecretName, &str)> {
    let mut secrets = vec![
        (SecretName::DbPassword, plan.db_password.value.as_str()),
        (SecretName::JwtSecret, plan.jwt_secret.value.as_str()),
        (SecretName::SecretsKey, plan.secrets_key.value.as_str()),
    ];
    if let Some(secret) = plan
        .auth
        .oidc()
        .and_then(|oidc| oidc.client_secret.as_deref())
    {
        secrets.push((SecretName::OidcClientSecret, secret));
    }
    secrets
}

/// One line of `.env`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EnvVar {
    pub key: &'static str,
    pub value: String,
    pub secret: bool,
}

/// The variables of `.env`. `owner` is who the containers run as, with host directories.
pub fn env_vars(plan: &InstallPlan, owner: Option<&Owner>) -> Vec<EnvVar> {
    let var = |key, value: String| EnvVar {
        key,
        value,
        secret: false,
    };
    let mut vars = vec![
        var("HOMESPACE_PORT", plan.port.to_string()),
        var("HOMESPACE_LISTEN", plan.listen.clone()),
        var("HOMESPACE_PUBLIC_URL", plan.public_url.clone()),
        var(
            "HOMESPACE_TRUST_FORWARDED_FOR",
            plan.proxy.trusts_forwarded_for().to_string(),
        ),
        var(
            "HOMESPACE_PASSWORD_LOGIN",
            plan.auth.password_login().to_string(),
        ),
        var("HOMESPACE_DB_USER", plan.db_user.clone()),
        var(
            "HOMESPACE_MAX_UPLOAD_BYTES",
            defaults::MAX_UPLOAD_BYTES.to_string(),
        ),
        var(
            "HOMESPACE_ACCESS_TTL_SECS",
            defaults::ACCESS_TTL_SECS.to_string(),
        ),
        var(
            "HOMESPACE_REFRESH_TTL_SECS",
            defaults::REFRESH_TTL_SECS.to_string(),
        ),
    ];
    if let Storage::HostDirs { db, files, .. } = &plan.storage {
        vars.push(var("HOMESPACE_DB_DIR", db.display().to_string()));
        vars.push(var("HOMESPACE_DATA_DIR", files.display().to_string()));
    }
    if let Some(owner) = owner {
        vars.push(var("HOMESPACE_UID", owner.uid.to_string()));
        vars.push(var("HOMESPACE_GID", owner.gid.to_string()));
    }
    if let Some(oidc) = plan.auth.oidc() {
        vars.push(var("HOMESPACE_OIDC_ISSUER", oidc.issuer.clone()));
        vars.push(var("HOMESPACE_OIDC_CLIENT_ID", oidc.client_id.clone()));
        vars.push(var("HOMESPACE_OIDC_NAME", oidc.name.clone()));
    }
    if plan.secrets == SecretStore::EnvFile {
        vars.extend(secrets(plan).into_iter().map(|(name, value)| EnvVar {
            key: name.env_var(),
            value: value.to_owned(),
            secret: true,
        }));
    }
    vars
}

/// `.env` as written, or with the secrets masked for showing it.
pub fn env_file(vars: &[EnvVar], mask_secrets: bool) -> String {
    let mut out = String::from(
        "# Home Space settings, written by `homespace-ctl install`. compose.yaml reads them;\n\
         # after a change run `compose up -d` in this directory.\n",
    );
    for var in vars {
        let value = if var.secret && mask_secrets {
            "********"
        } else {
            &var.value
        };
        // Single quotes: compose takes the value literally, `$` included.
        let _ = writeln!(out, "{}='{value}'", var.key);
    }
    out
}

/// Whether `value` can be written to `.env` as it is: single-quoted, so no `'` and no line
/// break.
pub fn env_safe(value: &str) -> bool {
    !value.contains(['\'', '\n', '\r'])
}

/// `compose.yaml` for the plan. All values come from `.env`; only the shape differs.
pub fn compose(plan: &InstallPlan) -> String {
    let files_mode = plan.secrets == SecretStore::Files;
    let host_dirs = matches!(plan.storage, Storage::HostDirs { .. });
    let secret_names: Vec<SecretName> = secrets(plan).into_iter().map(|(name, _)| name).collect();
    let mut out = String::from(
        "# Home Space, written by `homespace-ctl install`. The settings are in .env next to this\n\
         # file; manage users with `homespace-ctl user ...`.\n",
    );
    let _ = write!(out, "name: {}\n\nservices:\n", plan.project);
    db_service(&mut out, plan, files_mode, host_dirs);
    app_service(&mut out, plan, files_mode, host_dirs, &secret_names);
    if !host_dirs {
        out.push_str("\nvolumes:\n  pgdata:\n  files:\n");
    }
    out
}

fn db_service(out: &mut String, plan: &InstallPlan, files_mode: bool, host_dirs: bool) {
    out.push_str("  db:\n");
    let _ = writeln!(out, "    image: {}", defaults::DB_IMAGE);
    out.push_str("    restart: unless-stopped\n");
    run_as(out, plan, host_dirs);
    out.push_str(
        "    environment:\n      POSTGRES_USER: ${HOMESPACE_DB_USER}\n      POSTGRES_DB: homespace\n",
    );
    if files_mode {
        let _ = writeln!(
            out,
            "      POSTGRES_PASSWORD_FILE: {SECRETS_MOUNT}/{}",
            SecretName::DbPassword.file_name()
        );
    } else {
        out.push_str("      POSTGRES_PASSWORD: ${HOMESPACE_DB_PASSWORD}\n");
    }
    out.push_str("    volumes:\n");
    if host_dirs {
        out.push_str("      - ${HOMESPACE_DB_DIR}:/var/lib/postgresql:Z\n");
        out.push_str(
            "      # Postgres looks its user up by uid; the host user has no entry in the image.\n      \
             - /etc/passwd:/etc/passwd:ro\n",
        );
    } else {
        out.push_str("      - pgdata:/var/lib/postgresql\n");
    }
    if files_mode {
        secret_mount(out, SecretName::DbPassword);
    }
    out.push_str(
        "    healthcheck:\n      \
         test: [\"CMD-SHELL\", \"pg_isready -U $${POSTGRES_USER} -d homespace\"]\n      \
         interval: 2s\n      timeout: 3s\n      retries: 30\n\n",
    );
}

fn app_service(
    out: &mut String,
    plan: &InstallPlan,
    files_mode: bool,
    host_dirs: bool,
    secret_names: &[SecretName],
) {
    out.push_str("  app:\n");
    let _ = writeln!(out, "    image: {}", defaults::APP_IMAGE);
    out.push_str(
        "    restart: unless-stopped\n    depends_on:\n      db:\n        condition: service_healthy\n",
    );
    run_as(out, plan, host_dirs);
    out.push_str("    environment:\n");
    out.push_str("      DATABASE_URL: postgres://${HOMESPACE_DB_USER}@db:5432/homespace\n");
    for name in secret_names {
        if files_mode {
            let _ = writeln!(
                out,
                "      {}_FILE: {SECRETS_MOUNT}/{}",
                name.env_var(),
                name.file_name()
            );
        } else {
            let _ = writeln!(out, "      {0}: ${{{0}}}", name.env_var());
        }
    }
    let mut plain = vec![
        "HOMESPACE_PUBLIC_URL",
        "HOMESPACE_PASSWORD_LOGIN",
        "HOMESPACE_TRUST_FORWARDED_FOR",
        "HOMESPACE_MAX_UPLOAD_BYTES",
        "HOMESPACE_ACCESS_TTL_SECS",
        "HOMESPACE_REFRESH_TTL_SECS",
        // Shown on the administration page; the server listens on 8080 inside the container.
        "HOMESPACE_PORT",
    ];
    if plan.auth.oidc().is_some() {
        plain.extend([
            "HOMESPACE_OIDC_ISSUER",
            "HOMESPACE_OIDC_CLIENT_ID",
            "HOMESPACE_OIDC_NAME",
        ]);
    }
    for key in plain {
        let _ = writeln!(out, "      {key}: ${{{key}}}");
    }
    out.push_str("      RUST_LOG: info\n");
    out.push_str("    ports:\n      - \"${HOMESPACE_LISTEN}:${HOMESPACE_PORT}:8080\"\n");
    out.push_str("    volumes:\n");
    if host_dirs {
        out.push_str("      - ${HOMESPACE_DATA_DIR}:/data:Z\n");
    } else {
        out.push_str("      - files:/data\n");
    }
    if files_mode {
        for &name in secret_names {
            secret_mount(out, name);
        }
    }
}

/// Mounts a secret file read-only where the `_FILE` variables point. A plain bind mount rather
/// than a compose `secrets:` entry: compose implementations don't relabel secret files for
/// SELinux (podman-compose doesn't), so a container could not read them. `z` labels the file
/// for every container that mounts it, and is ignored where SELinux is off.
fn secret_mount(out: &mut String, name: SecretName) {
    let _ = writeln!(
        out,
        "      - ./{SECRETS_DIR}/{0}:{SECRETS_MOUNT}/{0}:ro,z",
        name.file_name()
    );
}

/// The `user:` of a service that runs as the host user owning the data directories.
fn run_as(out: &mut String, plan: &InstallPlan, host_dirs: bool) {
    if host_dirs {
        out.push_str("    user: \"${HOMESPACE_UID}:${HOMESPACE_GID}\"\n");
        if plan.keep_id() {
            out.push_str("    userns_mode: keep-id\n");
        }
    }
}

/// The nginx site for `server_name`, proxying to the app on `port`.
pub fn nginx(server_name: &str, port: u16) -> String {
    // The first line of the config proper; the comments above it are replaced.
    let body_start = NGINX_TEMPLATE.find("\nupstream ").map_or(0, |at| at + 1);
    let body = NGINX_TEMPLATE[body_start..]
        .replace(NGINX_EXAMPLE_UPSTREAM, &format!("server 127.0.0.1:{port};"))
        .replace(
            "/path/to/",
            &format!("/etc/letsencrypt/live/{server_name}/"),
        )
        .replace(NGINX_EXAMPLE_NAME, server_name);
    format!(
        "# nginx site for Home Space at https://{server_name}, written by `homespace-ctl install`.\n\
         #\n\
         #   sudo cp {server_name}.conf /etc/nginx/conf.d/{server_name}.conf\n\
         #   # check the certificate paths below, then:\n\
         #   sudo nginx -t && sudo systemctl reload nginx\n\
         #\n\
         # If nginx.conf already defines a \"map $http_upgrade $connection_upgrade\", delete the one below.\n\n\
         {body}"
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::install::plan::{Auth, Oidc, Proxy, Runtime, Secret, ServiceUser};
    use crate::installation::Engine;

    fn plan(secrets: SecretStore, storage: Storage, auth: Auth) -> InstallPlan {
        InstallPlan {
            runtime: Runtime {
                engine: Engine::Docker,
                rootless: false,
            },
            dir: PathBuf::from("/opt/homespace"),
            project: "homespace".to_owned(),
            secrets,
            storage,
            db_user: "homespace".to_owned(),
            db_password: Secret {
                value: "dbpass".to_owned(),
                generated: true,
            },
            jwt_secret: Secret {
                value: "jwtsecret".to_owned(),
                generated: true,
            },
            secrets_key: Secret {
                value: "secretskey".to_owned(),
                generated: true,
            },
            proxy: Proxy::Nginx {
                server_name: "files.example.com".to_owned(),
            },
            public_url: "https://files.example.com".to_owned(),
            auth,
            port: 18080,
            listen: "127.0.0.1".to_owned(),
        }
    }

    fn host_dirs() -> Storage {
        Storage::HostDirs {
            db: PathBuf::from("/srv/homespace/db"),
            files: PathBuf::from("/srv/homespace/files"),
            user: ServiceUser::Create {
                name: "homespace".to_owned(),
            },
        }
    }

    fn oidc() -> Oidc {
        Oidc {
            issuer: "https://idp.example.com".to_owned(),
            client_id: "homespace".to_owned(),
            client_secret: Some("oidcsecret".to_owned()),
            name: "Company login".to_owned(),
        }
    }

    #[test]
    fn volumes_with_env_secrets() {
        let plan = plan(SecretStore::EnvFile, Storage::Volumes, Auth::Passwords);
        let compose = compose(&plan);
        assert!(compose.contains("\nname: homespace\n"));
        assert!(compose.contains("image: docker.io/library/postgres:18"));
        assert!(compose.contains(concat!(
            "image: docker.io/homespaceapp/home-space:",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(compose.contains("POSTGRES_PASSWORD: ${HOMESPACE_DB_PASSWORD}"));
        assert!(compose.contains("HOMESPACE_JWT_SECRET: ${HOMESPACE_JWT_SECRET}"));
        assert!(compose.contains("- pgdata:/var/lib/postgresql"));
        assert!(compose.contains("\nvolumes:\n  pgdata:\n  files:\n"));
        assert!(!compose.contains("user:"), "volumes keep the images' users");
        assert!(!compose.contains("secrets"));
        assert!(!compose.contains("OIDC"));
        // The DB is reachable only on the compose network.
        assert_eq!(compose.matches("ports:").count(), 1, "{compose}");
    }

    #[test]
    fn host_dirs_with_secret_files_and_oidc() {
        let plan = plan(
            SecretStore::Files,
            host_dirs(),
            Auth::OidcAndPasswords(oidc()),
        );
        let compose = compose(&plan);
        assert_eq!(
            compose
                .matches("user: \"${HOMESPACE_UID}:${HOMESPACE_GID}\"")
                .count(),
            2
        );
        assert!(compose.contains("- ${HOMESPACE_DB_DIR}:/var/lib/postgresql:Z"));
        assert!(compose.contains("- /etc/passwd:/etc/passwd:ro"));
        assert!(compose.contains("- ${HOMESPACE_DATA_DIR}:/data:Z"));
        assert!(compose.contains("POSTGRES_PASSWORD_FILE: /run/secrets/db_password"));
        assert!(compose.contains("HOMESPACE_DB_PASSWORD_FILE: /run/secrets/db_password"));
        assert!(compose.contains("HOMESPACE_JWT_SECRET_FILE: /run/secrets/jwt_secret"));
        assert!(compose.contains("HOMESPACE_SECRETS_KEY_FILE: /run/secrets/secrets_key"));
        assert!(
            compose.contains("HOMESPACE_OIDC_CLIENT_SECRET_FILE: /run/secrets/oidc_client_secret")
        );
        assert!(compose.contains(
            "      - ./secrets/oidc_client_secret:/run/secrets/oidc_client_secret:ro,z\n"
        ));
        assert_eq!(
            compose
                .matches("- ./secrets/db_password:/run/secrets/db_password:ro,z")
                .count(),
            2,
            "both containers read the database password"
        );
        assert_eq!(
            compose.matches("jwt_secret:ro,z").count(),
            1,
            "only the app"
        );
        assert!(compose.contains("HOMESPACE_OIDC_ISSUER: ${HOMESPACE_OIDC_ISSUER}"));
        assert!(!compose.contains("userns_mode"));
        assert!(!compose.contains("\nvolumes:\n"));
        assert!(!compose.contains("${HOMESPACE_DB_PASSWORD}"), "{compose}");
    }

    #[test]
    fn rootless_podman_with_host_dirs_keeps_the_users_id() {
        let mut plan = plan(SecretStore::EnvFile, host_dirs(), Auth::Passwords);
        plan.runtime = Runtime {
            engine: Engine::Podman,
            rootless: true,
        };
        assert_eq!(compose(&plan).matches("userns_mode: keep-id").count(), 2);
    }

    #[test]
    fn env_file_has_secrets_only_in_env_mode() {
        let owner = Owner {
            name: "homespace".to_owned(),
            uid: 980,
            gid: 975,
        };
        let env_mode = plan(SecretStore::EnvFile, host_dirs(), Auth::Oidc(oidc()));
        let text = env_file(&env_vars(&env_mode, Some(&owner)), false);
        assert!(text.contains("HOMESPACE_DB_PASSWORD='dbpass'\n"));
        assert!(text.contains("HOMESPACE_JWT_SECRET='jwtsecret'\n"));
        assert!(text.contains("HOMESPACE_SECRETS_KEY='secretskey'\n"));
        assert!(text.contains("HOMESPACE_OIDC_CLIENT_SECRET='oidcsecret'\n"));
        assert!(text.contains("HOMESPACE_PASSWORD_LOGIN='false'\n"));
        assert!(text.contains("HOMESPACE_TRUST_FORWARDED_FOR='true'\n"));
        assert!(text.contains("HOMESPACE_UID='980'\nHOMESPACE_GID='975'\n"));
        assert!(text.contains("HOMESPACE_MAX_UPLOAD_BYTES='10737418240'\n"));
        assert!(text.contains("HOMESPACE_ACCESS_TTL_SECS='900'\n"));
        assert!(text.contains("HOMESPACE_REFRESH_TTL_SECS='2592000'\n"));
        let masked = env_file(&env_vars(&env_mode, Some(&owner)), true);
        assert!(!masked.contains("dbpass") && !masked.contains("jwtsecret"));

        let files_mode = plan(SecretStore::Files, Storage::Volumes, Auth::Passwords);
        let text = env_file(&env_vars(&files_mode, None), false);
        assert!(
            !text.contains("dbpass") && !text.contains("jwtsecret"),
            "{text}"
        );
        assert!(!text.contains("HOMESPACE_UID"));
    }

    #[test]
    fn env_values_with_quotes_or_line_breaks_are_unsafe() {
        assert!(env_safe("p@ss:w/rd$x"));
        assert!(!env_safe("it's"));
        assert!(!env_safe("two\nlines"));
    }

    #[test]
    fn nginx_is_rendered_for_the_server_name_and_port() {
        let conf = nginx("files.example.com", 9123);
        assert!(conf.starts_with("# nginx site for Home Space at https://files.example.com"));
        assert!(conf.contains("server 127.0.0.1:9123;"));
        assert!(conf.contains("server_name  files.example.com;"));
        assert!(conf.contains("/etc/letsencrypt/live/files.example.com/fullchain.pem"));
        assert!(!conf.contains("example.domain.com"));
        assert!(!conf.contains("127.0.0.1:8080"), "{conf}");
        assert_eq!(conf.matches("delete the one below").count(), 1, "{conf}");
        assert!(conf.contains("below.\n\nupstream homespace {"), "{conf}");
    }
}
