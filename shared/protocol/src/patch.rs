//! [`Patch`]: one field of a `PATCH` body, which can be left alone, removed or set.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A change to one field. On the wire a missing field is [`Keep`](Self::Keep), `null` is
/// [`Clear`](Self::Clear) and a value is [`Set`](Self::Set).
///
/// A field of this type needs `#[serde(default, skip_serializing_if = "Patch::is_keep")]`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Patch<T> {
    /// Leave the field as it is.
    #[default]
    Keep,
    /// Remove the value: no password, no expiry, no quota, or for a parent, the root.
    Clear,
    Set(T),
}

impl<T> Patch<T> {
    pub fn is_keep(&self) -> bool {
        matches!(self, Self::Keep)
    }

    /// The value being set, if this sets one.
    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Set(value) => Some(value),
            Self::Keep | Self::Clear => None,
        }
    }

    /// A change to `value`, where `None` removes the value.
    pub fn to(value: Option<T>) -> Self {
        value.map_or(Self::Clear, Self::Set)
    }

    pub fn as_ref(&self) -> Patch<&T> {
        match self {
            Self::Keep => Patch::Keep,
            Self::Clear => Patch::Clear,
            Self::Set(value) => Patch::Set(value),
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Patch<U> {
        match self {
            Self::Keep => Patch::Keep,
            Self::Clear => Patch::Clear,
            Self::Set(value) => Patch::Set(f(value)),
        }
    }

    /// The value after the change, given the `current` one.
    pub fn apply(self, current: Option<T>) -> Option<T> {
        match self {
            Self::Keep => current,
            Self::Clear => None,
            Self::Set(value) => Some(value),
        }
    }
}

impl<T: Serialize> Serialize for Patch<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            // Left out by `skip_serializing_if`; written as `null` if it is not.
            Self::Keep | Self::Clear => serializer.serialize_none(),
            Self::Set(value) => value.serialize(serializer),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Patch<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Option::<T>::deserialize(deserializer).map(Self::to)
    }
}

/// A [`Patch`] of a time, written in RFC 3339, for `#[serde(with = "patch::rfc3339")]`.
pub mod rfc3339 {
    use serde::{Deserializer, Serializer};
    use time::OffsetDateTime;

    use super::Patch;

    pub fn serialize<S: Serializer>(
        value: &Patch<OffsetDateTime>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let time = match value {
            Patch::Set(time) => Some(*time),
            Patch::Keep | Patch::Clear => None,
        };
        time::serde::rfc3339::option::serialize(&time, serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Patch<OffsetDateTime>, D::Error> {
        time::serde::rfc3339::option::deserialize(deserializer).map(Patch::to)
    }
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};

    use super::Patch;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Change {
        #[serde(default, skip_serializing_if = "Patch::is_keep")]
        quota: Patch<u64>,
    }

    #[test]
    fn missing_null_and_a_value_are_keep_clear_and_set() {
        let read = |json| serde_json::from_str::<Change>(json).expect("json").quota;
        assert_eq!(read("{}"), Patch::Keep);
        assert_eq!(read(r#"{"quota":null}"#), Patch::Clear);
        assert_eq!(read(r#"{"quota":5}"#), Patch::Set(5));
    }

    #[test]
    fn a_patch_is_written_the_way_it_is_read() {
        for quota in [Patch::Keep, Patch::Clear, Patch::Set(5)] {
            let json = serde_json::to_string(&Change { quota }).expect("json");
            let back: Change = serde_json::from_str(&json).expect("back");
            assert_eq!(back.quota, quota, "{json}");
        }
    }
}
