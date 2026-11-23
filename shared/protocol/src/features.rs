//! Server features: short stable names for what a server offers that an older one does not.
//!
//! The server reports [`ALL`] in `GET /capabilities`; each client declares the ones it cannot
//! work without (`REQUIRES`), and [`crate::compat::check`] compares the two. A name never
//! changes meaning and is never reused. See `docs/versioning.md` for when to add one.
//!
//! The list starts empty at 0.13.0: everything older is the baseline every server that
//! reports a version has.

/// Every feature this server provides, each added with the release that introduced it.
pub const ALL: &[&str] = &[];

/// What the user reads for each feature, in hints such as "These won't work: …".
const DISPLAY_NAMES: &[(&str, &str)] = &[];

/// What the user reads for `feature`; the name itself for one this build does not know.
pub fn display_name(feature: &str) -> &str {
    DISPLAY_NAMES
        .iter()
        .find(|(name, _)| *name == feature)
        .map_or(feature, |(_, display)| display)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_feature_has_a_display_name() {
        for feature in ALL {
            assert!(
                DISPLAY_NAMES.iter().any(|(name, _)| name == feature),
                "{feature} has no display name"
            );
        }
    }

    #[test]
    fn feature_names_are_unique() {
        let mut names = ALL.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ALL.len());
    }

    #[test]
    fn an_unknown_feature_shows_its_name() {
        assert_eq!(display_name("from_the_future"), "from_the_future");
    }
}
