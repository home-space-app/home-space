//! Helpers shared by the route modules. Route modules depend on these, never on each other
//! (D2 in docs/todo/server-refactoring.md).

pub(crate) mod archive;
pub(crate) mod body;
pub(crate) mod convert;
pub(crate) mod disk;
pub(crate) mod download;
pub(crate) mod outcome;
