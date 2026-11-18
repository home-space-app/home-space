use std::{fmt, sync::Arc};

use tokio::{
    sync::mpsc,
    task::{self, JoinHandle},
};

use db::{ContentState, Db, DbError, Node, NodeId, NodeKind, UserId};
use files::BlobStore;
use protocol::SearchHit;

use crate::{
    provider::{ContentProvider, ExtractError, tag},
    terms::{index_terms, tsquery},
};

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

/// Errors from indexing or searching.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Db(#[from] DbError),
    #[error("reading content failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Extract(#[from] ExtractError),
    #[error("content extraction task failed: {0}")]
    Task(#[from] task::JoinError),
}

/// What a [`SearchIndex::catch_up`] pass repaired.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CatchUp {
    pub names_indexed: usize,
    pub content_queued: usize,
}

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
mod tests {
    use std::{io::Read, time::Duration};

    use bytes::Bytes;
    use futures_util::stream;
    use sqlx::PgPool;

    use db::StoreOutcome;
    use files::FileService;

    use super::*;

    struct Plain;

    impl ContentProvider for Plain {
        fn id(&self) -> &'static str {
            "plain"
        }

        fn version(&self) -> u32 {
            1
        }

        fn accepts(&self, mime: &str, _name: &str) -> bool {
            mime == "text/plain"
        }

        fn extract(&self, reader: &mut dyn Read, limit: usize) -> Result<String, ExtractError> {
            let mut text = String::new();
            reader
                .take(u64::try_from(limit).unwrap_or(u64::MAX))
                .read_to_string(&mut text)?;
            Ok(text)
        }
    }

    const POLL_ATTEMPTS: u32 = 100;
    const POLL_INTERVAL: Duration = Duration::from_millis(50);

    struct Fixture {
        index: SearchIndex,
        _worker: Worker,
        files: FileService,
        db: Db,
        alice: UserId,
        bob: UserId,
        _dir: tempfile::TempDir,
    }

    async fn fixture(pool: PgPool, providers: Vec<Arc<dyn ContentProvider>>) -> Fixture {
        let db = Db::from_pool(pool);
        let dir = tempfile::tempdir().expect("tempdir");
        let store = BlobStore::open(dir.path()).await.expect("store");
        let alice = db
            .create_user(db::TenantId::DEFAULT, "alice", "x")
            .await
            .expect("alice")
            .id;
        let bob = db
            .create_user(db::TenantId::DEFAULT, "bob", "x")
            .await
            .expect("bob")
            .id;
        let (index, worker) = SearchIndex::start(db.clone(), store.clone(), providers);
        // Lost & Found folders are created without an index entry like any other node.
        index.catch_up().await.expect("initial catch up");
        Fixture {
            index,
            _worker: worker,
            files: FileService::new(db.clone(), store, 1024 * 1024),
            db,
            alice,
            bob,
            _dir: dir,
        }
    }

    fn body(text: &'static str) -> impl futures_util::Stream<Item = std::io::Result<Bytes>> {
        stream::iter([Ok(Bytes::from_static(text.as_bytes()))])
    }

    async fn upload(fx: &Fixture, parent: Option<NodeId>, name: &str, text: &'static str) -> Node {
        let node = fx
            .files
            .upload(fx.alice, parent, name, None, None, body(text))
            .await
            .expect("upload")
            .into_node();
        fx.index.index_name(&node).await.expect("index name");
        node
    }

    async fn names(index: &SearchIndex, owner: UserId, query: &str) -> Vec<String> {
        index
            .search(owner, query, None)
            .await
            .expect("search")
            .into_iter()
            .map(|hit| hit.node.name)
            .collect()
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn finds_a_file_by_a_prefix_of_a_word_in_its_name(pool: PgPool) {
        let fx = fixture(pool, Vec::new()).await;
        upload(&fx, None, "IMG_2024-holidayPhoto.jpg", "x").await;
        assert_eq!(
            names(&fx.index, fx.alice, "holi").await,
            ["IMG_2024-holidayPhoto.jpg"]
        );
        assert_eq!(
            names(&fx.index, fx.alice, "photo 2024").await,
            ["IMG_2024-holidayPhoto.jpg"]
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn every_word_must_match(pool: PgPool) {
        let fx = fixture(pool, Vec::new()).await;
        upload(&fx, None, "holiday.jpg", "x").await;
        assert!(names(&fx.index, fx.alice, "holiday work").await.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn blank_query_returns_nothing(pool: PgPool) {
        let fx = fixture(pool, Vec::new()).await;
        upload(&fx, None, "holiday.jpg", "x").await;
        assert!(names(&fx.index, fx.alice, "  - ").await.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn hits_report_their_location(pool: PgPool) {
        let fx = fixture(pool, Vec::new()).await;
        let folder = fx
            .files
            .create_folder(fx.alice, None, "trips")
            .await
            .expect("folder");
        upload(&fx, Some(folder.id), "paris.txt", "x").await;
        let hits = fx
            .index
            .search(fx.alice, "paris", None)
            .await
            .expect("search");
        assert_eq!(hits[0].location, ["trips"]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rename_reindexes_the_name(pool: PgPool) {
        let fx = fixture(pool, Vec::new()).await;
        let node = upload(&fx, None, "old.txt", "x").await;
        let renamed = fx
            .db
            .update_node(
                fx.alice,
                node.id,
                db::NodeUpdate {
                    name: Some("fresh.txt".to_owned()),
                    ..db::NodeUpdate::default()
                },
            )
            .await
            .expect("rename")
            .node;
        fx.index.index_name(&renamed).await.expect("reindex");
        assert!(names(&fx.index, fx.alice, "old").await.is_empty());
        assert_eq!(names(&fx.index, fx.alice, "fresh").await, ["fresh.txt"]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn users_only_find_their_own_nodes(pool: PgPool) {
        let fx = fixture(pool, Vec::new()).await;
        upload(&fx, None, "secret.txt", "x").await;
        assert!(names(&fx.index, fx.bob, "secret").await.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn catch_up_indexes_nodes_created_without_the_index(pool: PgPool) {
        let fx = fixture(pool, Vec::new()).await;
        fx.files
            .upload(fx.alice, None, "legacy.txt", None, None, body("x"))
            .await
            .expect("upload");
        assert!(names(&fx.index, fx.alice, "legacy").await.is_empty());
        let report = fx.index.catch_up().await.expect("catch up");
        assert_eq!(report.names_indexed, 1);
        assert_eq!(names(&fx.index, fx.alice, "legacy").await, ["legacy.txt"]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn catch_up_does_nothing_for_an_up_to_date_index(pool: PgPool) {
        let fx = fixture(pool, Vec::new()).await;
        upload(&fx, None, "a.txt", "x").await;
        assert_eq!(
            fx.index.catch_up().await.expect("catch up"),
            CatchUp::default()
        );
    }

    async fn eventually_finds(fx: &Fixture, query: &str, expected: &[&str]) {
        for _ in 0..POLL_ATTEMPTS {
            if names(&fx.index, fx.alice, query).await == expected {
                return;
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        assert_eq!(names(&fx.index, fx.alice, query).await, expected);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn content_is_searchable_once_a_provider_extracted_it(pool: PgPool) {
        let fx = fixture(pool, vec![Arc::new(Plain)]).await;
        let node = upload(&fx, None, "notes.txt", "remember the zebra crossing").await;
        assert!(names(&fx.index, fx.alice, "zebra").await.is_empty());
        fx.index.enqueue_content(fx.alice, node.id);
        eventually_finds(&fx, "zebra", &["notes.txt"]).await;
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn files_without_a_matching_provider_keep_name_only_entries(pool: PgPool) {
        let fx = fixture(pool, vec![Arc::new(Plain)]).await;
        let node = fx
            .files
            .upload(
                fx.alice,
                None,
                "photo.jpg",
                Some("image/jpeg"),
                None,
                body("zebra"),
            )
            .await
            .expect("upload")
            .into_node();
        fx.index.index_name(&node).await.expect("index name");
        fx.index.enqueue_content(fx.alice, node.id);
        fx.index.catch_up().await.expect("catch up");
        tokio::time::sleep(POLL_INTERVAL * 5).await;
        assert!(names(&fx.index, fx.alice, "zebra").await.is_empty());
        assert_eq!(names(&fx.index, fx.alice, "photo").await, ["photo.jpg"]);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn catch_up_re_extracts_content_that_changed(pool: PgPool) {
        let fx = fixture(pool, vec![Arc::new(Plain)]).await;
        let node = upload(&fx, None, "notes.txt", "alpha").await;
        fx.index.enqueue_content(fx.alice, node.id);
        eventually_finds(&fx, "alpha", &["notes.txt"]).await;
        fx.files
            .replace_content(fx.alice, node.id, None, None, body("bravo"))
            .await
            .expect("replace");
        let report = fx.index.catch_up().await.expect("catch up");
        assert_eq!(report.content_queued, 1);
        eventually_finds(&fx, "bravo", &["notes.txt"]).await;
        assert!(names(&fx.index, fx.alice, "alpha").await.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn a_new_version_replaces_the_indexed_content(pool: PgPool) {
        let fx = fixture(pool, vec![Arc::new(Plain)]).await;
        let node = upload(&fx, None, "notes.txt", "alpha").await;
        fx.index.enqueue_content(fx.alice, node.id);
        eventually_finds(&fx, "alpha", &["notes.txt"]).await;
        let outcome = fx
            .files
            .upload(fx.alice, None, "notes.txt", None, None, body("bravo"))
            .await
            .expect("upload");
        assert!(matches!(outcome, StoreOutcome::Versioned { .. }));
        fx.index.enqueue_content(fx.alice, node.id);
        eventually_finds(&fx, "bravo", &["notes.txt"]).await;
        assert!(names(&fx.index, fx.alice, "alpha").await.is_empty());
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn catch_up_leaves_extracted_content_alone(pool: PgPool) {
        let fx = fixture(pool, vec![Arc::new(Plain)]).await;
        let node = upload(&fx, None, "notes.txt", "alpha").await;
        fx.index.enqueue_content(fx.alice, node.id);
        eventually_finds(&fx, "alpha", &["notes.txt"]).await;
        assert_eq!(
            fx.index.catch_up().await.expect("catch up"),
            CatchUp::default()
        );
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn rebuild_restores_names_and_content(pool: PgPool) {
        let fx = fixture(pool, vec![Arc::new(Plain)]).await;
        upload(&fx, None, "notes.txt", "alpha").await;
        let report = fx.index.rebuild().await.expect("rebuild");
        // The file and the two Lost & Found folders.
        assert_eq!(report.names_indexed, 3);
        eventually_finds(&fx, "alpha", &["notes.txt"]).await;
    }
}
