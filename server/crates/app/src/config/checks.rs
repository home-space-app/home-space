//! Checks on configuration values.

/// The value of an optional setting, treating an empty one (as Compose passes) as unset.
pub(super) fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|value| !value.is_empty())
}
