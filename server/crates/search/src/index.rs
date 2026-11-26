use std::{fmt, sync::Arc};

use tokio::{
    sync::mpsc,
    task::{self, JoinHandle},
};

use db::{ContentState, Db, Node, NodeId, NodeKind, UserId};
use files::BlobStore;
use protocol::SearchHit;

use crate::{
    SearchError,
    provider::{ContentProvider, tag},
    terms::{index_terms, tsquery},
};

mod catch_up;
pub use catch_up::CatchUp;

const JOB_QUEUE: usize = 1024;
const CATCH_UP_BATCH: i64 = 200;
const MAX_CONTENT_BYTES: usize = 1024 * 1024;
const MAX_CONTENT_TERMS: usize = 50_000;
const MAX_NAME_TERMS: usize = 256;
const DEFAULT_RESULTS: u32 = 50;
const MAX_RESULTS: u32 = 200;

/// Handle to the background task that extracts content; it ends once every
/// [`SearchIndex`] clone is dropped and the queued jobs are done.
pub type Worker = JoinHandle<()>;

#[derive(Clone, Copy, Debug)]
struct Job {
    owner: UserId,
    node: NodeId,
}

struct Inner {
    db: Db,
    store: BlobStore,
    providers: Vec<Arc<dyn ContentProvider>>,
}

impl Inner {
    fn provider_for(&self, node: &Node) -> Option<&Arc<dyn ContentProvider>> {
        let mime = node.mime.as_str();
        (node.kind == NodeKind::File)
            .then(|| {
                self.providers
                    .iter()
                    .find(|provider| provider.accepts(mime, &node.name))
            })
            .flatten()
    }

    async fn index_content(&self, job: Job) -> Result<(), SearchError> {
        // What a protected folder holds is never indexed: its content is stored encrypted.
        if self.db.in_protected_folder(job.owner, job.node).await? {
            return Ok(());
        }
        let node = self.db.get_node(job.owner, job.node).await?;
        match self.provider_for(&node) {
            None => {
                self.db
                    .set_content_entry(job.owner, job.node, None, None, None)
                    .await?;
            }
            Some(provider) => {
                let mut reader = self
                    .store
                    .open_blob(job.owner, job.node)
                    .await?
                    .into_std()
                    .await;
                let extractor = Arc::clone(provider);
                let text =
                    task::spawn_blocking(move || extractor.extract(&mut reader, MAX_CONTENT_BYTES))
                        .await??;
                self.db
                    .set_content_entry(
                        job.owner,
                        job.node,
                        Some(&index_terms(&text, MAX_CONTENT_TERMS)),
                        Some(&tag(provider.as_ref())),
                        node.sha256.as_deref(),
                    )
                    .await?;
            }
        }
        Ok(())
    }

    async fn clear_content(&self, job: Job) {
        if let Err(err) = self
            .db
            .set_content_entry(job.owner, job.node, None, None, None)
            .await
        {
            tracing::warn!(node_id = %job.node, error = %err, "clearing stale content failed");
        }
    }

    fn is_stale(&self, state: &ContentState) -> bool {
        match self.provider_for(&state.node) {
            None => state.content_provider.is_some(),
            Some(provider) => {
                state.content_provider.as_deref() != Some(tag(provider.as_ref()).as_str())
                    || state.content_sha256 != state.node.sha256
            }
        }
    }
}

/// Full-text index over the names (and, through providers, the content) of a user's nodes.
#[derive(Clone)]
pub struct SearchIndex {
    inner: Arc<Inner>,
    jobs: mpsc::Sender<Job>,
}

impl fmt::Debug for SearchIndex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SearchIndex")
            .field("providers", &self.inner.providers.len())
            .finish_non_exhaustive()
    }
}

impl SearchIndex {
    /// Creates the index and spawns its content worker on the current tokio runtime.
    ///
    /// With no `providers` only names are indexed.
    pub fn start(
        db: Db,
        store: BlobStore,
        providers: Vec<Arc<dyn ContentProvider>>,
    ) -> (Self, Worker) {
        let inner = Arc::new(Inner {
            db,
            store,
            providers,
        });
        let (jobs, mut queue) = mpsc::channel::<Job>(JOB_QUEUE);
        let worker_inner = Arc::clone(&inner);
        let worker = tokio::spawn(async move {
            while let Some(job) = queue.recv().await {
                if let Err(err) = worker_inner.index_content(job).await {
                    tracing::warn!(node_id = %job.node, error = %err, "content indexing failed");
                    worker_inner.clear_content(job).await;
                }
            }
        });
        (Self { inner, jobs }, worker)
    }

    /// Indexes the name of a node; call after it is created or renamed.
    pub async fn index_name(&self, node: &Node) -> Result<(), SearchError> {
        self.inner
            .db
            .upsert_name_entry(
                node.owner_id,
                node.id,
                &index_terms(&node.name, MAX_NAME_TERMS),
            )
            .await?;
        Ok(())
    }

    /// Schedules content extraction for a file; call after its content changed.
    ///
    /// A full queue drops the job, which [`catch_up`](Self::catch_up) repairs later.
    pub fn enqueue_content(&self, owner: UserId, node: NodeId) {
        if self.jobs.try_send(Job { owner, node }).is_err() {
            tracing::warn!(node_id = %node, "content indexing queue unavailable, job dropped");
        }
    }

    /// Searches the owner's nodes; every word of `input` must match as a prefix.
    pub async fn search(
        &self,
        owner: UserId,
        input: &str,
        limit: Option<u32>,
    ) -> Result<Vec<SearchHit>, SearchError> {
        let Some(query) = tsquery(input) else {
            return Ok(Vec::new());
        };
        let limit = limit.unwrap_or(DEFAULT_RESULTS).clamp(1, MAX_RESULTS);
        let ranked = self
            .inner
            .db
            .search_nodes(owner, &query, i64::from(limit))
            .await?;
        let ids = ranked.iter().map(|hit| hit.node.id).collect::<Vec<_>>();
        let mut locations = self.inner.db.list_locations(owner, &ids).await?;
        Ok(ranked
            .into_iter()
            .map(|hit| SearchHit {
                location: locations.remove(&hit.node.id).unwrap_or_default(),
                node: hit.node.into(),
            })
            .collect())
    }

    /// Repairs the index: indexes nodes without an entry and queues files whose extracted
    /// content is missing, outdated or from a different provider.
    pub async fn catch_up(&self) -> Result<CatchUp, SearchError> {
        let mut report = CatchUp::default();
        loop {
            let missing = self.inner.db.nodes_missing_entries(CATCH_UP_BATCH).await?;
            if missing.is_empty() {
                break;
            }
            let mut failed = false;
            for node in &missing {
                match self.index_name(node).await {
                    Ok(()) => report.names_indexed += 1,
                    Err(err) => {
                        failed = true;
                        tracing::warn!(node_id = %node.id, error = %err, "name indexing failed");
                    }
                }
            }
            if failed {
                break;
            }
        }
        let mut after = None;
        loop {
            let states = self
                .inner
                .db
                .list_content_states(after, CATCH_UP_BATCH)
                .await?;
            let Some(last) = states.last() else {
                break;
            };
            after = Some(last.node.id);
            for state in states.iter().filter(|state| self.inner.is_stale(state)) {
                let job = Job {
                    owner: state.node.owner_id,
                    node: state.node.id,
                };
                if self.jobs.send(job).await.is_err() {
                    return Ok(report);
                }
                report.content_queued += 1;
            }
        }
        Ok(report)
    }

    /// Drops every index entry and rebuilds it with [`catch_up`](Self::catch_up).
    pub async fn rebuild(&self) -> Result<CatchUp, SearchError> {
        self.inner.db.clear_search_entries().await?;
        self.catch_up().await
    }
}

#[cfg(test)]
mod tests;
