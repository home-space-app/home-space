//! Configured secrets: how strong a signing secret must be, and reading a secret that is given
//! either as a value or as a file (a Docker/Podman secret).

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

const MIN_SECRET_LEN: usize = 32;
/// Fewer different characters than this in a secret means it was typed, not generated.
const MIN_SECRET_DISTINCT_CHARS: usize = 10;
/// What the example configuration used to ship as the secret, and so known to everyone.
const PLACEHOLDER_SECRET_MARK: &str = "change-me";
const GENERATE_SECRET: &str = "`openssl rand -base64 48`";

/// Why a configured secret was not accepted.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("{name} must be at least {MIN_SECRET_LEN} characters")]
    TooShort { name: String },
    #[error("{name} is still a placeholder; generate one with {GENERATE_SECRET}")]
    Placeholder { name: String },
    #[error("{name} is too easy to guess; generate one with {GENERATE_SECRET}")]
    TooSimple { name: String },
    #[error("set only one of {value_var} and {file_var}")]
    Both { value_var: String, file_var: String },
    #[error("reading {}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Refuses a signing secret that is short, a placeholder left in place, or made of a few
/// repeated characters: whoever can guess it can sign tokens. `name` names it in the error.
///
/// # Errors
///
/// [`SecretError::TooShort`], [`SecretError::Placeholder`] or [`SecretError::TooSimple`].
pub fn check_secret(name: &str, secret: &str) -> Result<(), SecretError> {
    let name = name.to_owned();
    let distinct = secret.chars().collect::<HashSet<_>>().len();
    if secret.len() < MIN_SECRET_LEN {
        Err(SecretError::TooShort { name })
    } else if secret
        .to_ascii_lowercase()
        .contains(PLACEHOLDER_SECRET_MARK)
    {
        Err(SecretError::Placeholder { name })
    } else if distinct < MIN_SECRET_DISTINCT_CHARS {
        Err(SecretError::TooSimple { name })
    } else {
        Ok(())
    }
}

/// A secret given as the variable `value_var` or as a file named by `file_var`, not both.
/// Empty values count as unset, as Compose passes unset variables empty. A file's trailing
/// line break is dropped.
///
/// # Errors
///
/// [`SecretError::Both`] when both are set, [`SecretError::Read`] when the file can't be read.
pub fn value_or_file(
    value_var: &str,
    value: Option<&str>,
    file_var: &str,
    file: Option<&Path>,
) -> Result<Option<String>, SecretError> {
    let value = value.filter(|value| !value.is_empty());
    let file = file.filter(|path| !path.as_os_str().is_empty());
    match (value, file) {
        (Some(_), Some(_)) => Err(SecretError::Both {
            value_var: value_var.to_owned(),
            file_var: file_var.to_owned(),
        }),
        (Some(value), None) => Ok(Some(value.to_owned())),
        (None, Some(path)) => std::fs::read_to_string(path)
            .map(|text| Some(text.trim_end_matches(['\r', '\n']).to_owned()))
            .map_err(|source| SecretError::Read {
                path: path.to_owned(),
                source,
            }),
        (None, None) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn a_generated_secret_is_accepted() {
        check_secret("secret", "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4BbNiC6tM5QzK").expect("accepted");
    }

    #[test]
    fn short_placeholder_and_repetitive_secrets_are_refused() {
        for secret in [
            "kq3ZcV0m8Tn1yWfLr7uGxP2aHs9Eo",
            "change-me-to-a-long-random-string",
            "Change-Me-kq3ZcV0m8Tn1yWfLr7uGxP2aHs9EoJd4",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "abcabcabcabcabcabcabcabcabcabcabcabcabc",
        ] {
            assert!(check_secret("secret", secret).is_err(), "{secret}");
        }
    }

    #[test]
    fn a_value_or_a_file_is_read_and_empty_counts_as_unset() {
        let mut file = tempfile::NamedTempFile::new().expect("temp file");
        writeln!(file, "from-file").expect("write");
        assert_eq!(
            value_or_file("V", Some("value"), "V_FILE", None).expect("value"),
            Some("value".to_owned())
        );
        assert_eq!(
            value_or_file("V", Some(""), "V_FILE", Some(file.path())).expect("file"),
            Some("from-file".to_owned())
        );
        assert_eq!(
            value_or_file("V", None, "V_FILE", Some(Path::new(""))).expect("unset"),
            None
        );
    }

    #[test]
    fn both_a_value_and_a_file_are_refused() {
        let err = value_or_file("V", Some("a"), "V_FILE", Some(Path::new("/x"))).unwrap_err();
        assert!(matches!(err, SecretError::Both { .. }), "{err}");
    }
}
