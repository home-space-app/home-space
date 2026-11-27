//! `Debug` output that leaves secrets out: passwords, codes, recovery keys.

use std::fmt;

/// Stands for a secret in `Debug` output.
#[derive(Clone, Copy)]
pub struct Redacted;

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// [`Redacted`] for a secret that is there, `None` for one that is not.
pub fn redacted<T>(secret: Option<&T>) -> Option<Redacted> {
    secret.map(|_| Redacted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_shows_only_whether_it_is_there() {
        assert_eq!(format!("{Redacted:?}"), "[redacted]");
        assert_eq!(
            format!("{:?}", redacted(Some(&"hunter2"))),
            "Some([redacted])"
        );
        assert_eq!(format!("{:?}", redacted::<String>(None)), "None");
    }
}
