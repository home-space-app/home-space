/// What a [`SearchIndex::catch_up`](crate::SearchIndex::catch_up) pass repaired.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatchUp {
    pub names_indexed: usize,
    pub content_queued: usize,
}
