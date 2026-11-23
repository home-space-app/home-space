//! Release versions (`0.13.0`, `v0.14.0-rc.1`) as the release script writes them.

use std::cmp::Ordering;

/// A parsed `major.minor.patch[-pre][+build]` version. Build metadata is dropped; a
/// pre-release sorts before its release, and two pre-releases compare as text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Version {
    /// Parses `text`, with or without a leading `v`. `None` when it is not a version.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text
            .split_once('+')
            .map_or(text, |(version, _build)| version);
        let (numbers, pre) = match text.split_once('-') {
            Some((numbers, pre)) if !pre.is_empty() => (numbers, Some(pre.to_owned())),
            Some(_) => return None,
            None => (text, None),
        };
        let mut parts = numbers.split('.').map(|part| part.parse::<u64>().ok());
        let (Some(Some(major)), Some(Some(minor)), Some(Some(patch)), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return None;
        };
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(mine), Some(theirs)) => mine.cmp(theirs),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).expect("a version")
    }

    #[test]
    fn parses_a_release() {
        assert_eq!(
            version("0.13.2"),
            Version {
                major: 0,
                minor: 13,
                patch: 2,
                pre: None
            }
        );
    }

    #[test]
    fn parses_a_tag_with_a_v_and_a_pre_release() {
        assert_eq!(version("v1.0.0-rc.1").pre.as_deref(), Some("rc.1"));
    }

    #[test]
    fn drops_build_metadata() {
        assert_eq!(version("1.2.3+abc"), version("1.2.3"));
    }

    #[test]
    fn rejects_what_is_not_a_version() {
        for text in ["", "1.2", "1.2.3.4", "a.b.c", "1.2.3-", "nightly"] {
            assert_eq!(Version::parse(text), None, "{text}");
        }
    }

    #[test]
    fn orders_by_numbers_not_text() {
        assert!(version("0.10.0") > version("0.9.9"));
    }

    #[test]
    fn a_pre_release_comes_before_its_release() {
        assert!(version("0.14.0-dev") < version("0.14.0"));
        assert!(version("0.14.0-dev") > version("0.13.9"));
    }
}
