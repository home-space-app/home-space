use std::{collections::HashSet, time::Duration};

use bytes::Bytes;
use futures_util::{Stream, stream};
use sha2::{Digest, Sha256};
use tokio::fs;
use uuid::Uuid;

use db::{NodeId, UserId};

use super::*;
use crate::{BodyError, FilesError};

fn body(parts: &[&'static str]) -> impl Stream<Item = Result<Bytes, BodyError>> + use<> {
    stream::iter(
        parts
            .iter()
            .map(|part| Ok(Bytes::from_static(part.as_bytes())))
            .collect::<Vec<_>>(),
    )
}

#[tokio::test]
async fn disk_space_is_reported_for_the_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(dir.path()).await.expect("open");
    let space = store.disk_space().await.expect("statvfs");
    if cfg!(unix) {
        let space = space.expect("space");
        assert!(space.total > 0);
        assert!(space.available <= space.total);
    }
}

#[tokio::test]
async fn staged_blob_reports_size_and_hash() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(dir.path()).await.expect("open");
    let staged = store.stage(body(&["hel", "lo"]), 100).await.expect("stage");
    assert_eq!(staged.size, 5);
    assert_eq!(staged.sha256, Sha256::digest(b"hello").to_vec());
}

#[tokio::test]
async fn oversized_upload_is_rejected_and_cleaned_up() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(dir.path()).await.expect("open");
    let result = store.stage(body(&["hello"]), 4).await;
    assert!(matches!(result, Err(FilesError::TooLarge { limit: 4 })));
    let mut leftovers = fs::read_dir(dir.path().join(TMP_DIR))
        .await
        .expect("read_dir");
    assert!(leftovers.next_entry().await.expect("entry").is_none());
}

#[tokio::test]
async fn committed_blob_can_be_read_and_removed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(dir.path()).await.expect("open");
    let (owner, node) = (UserId::new(), NodeId::new());
    let staged = store.stage(body(&["data"]), 100).await.expect("stage");
    store.commit(&staged, owner, node).await.expect("commit");
    assert!(store.open_blob(owner, node).await.is_ok());
    store.remove(owner, node).await.expect("remove");
    assert!(store.open_blob(owner, node).await.is_err());
}

#[tokio::test]
async fn appended_chunks_hash_like_a_single_stage() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(dir.path()).await.expect("open");
    let id = Uuid::now_v7();
    store.create_partial(id).await.expect("create");
    let mut hasher = Sha256::new();
    let len = store
        .append(id, 0, 5, body(&["he", "l"]), |piece| hasher.update(piece))
        .await
        .expect("first");
    assert_eq!(len, 3);
    let len = store
        .append(id, 3, 5, body(&["lo"]), |piece| hasher.update(piece))
        .await
        .expect("second");
    assert_eq!(len, 5);
    let digest = Sha256::digest(b"hello").to_vec();
    assert_eq!(hasher.finalize().to_vec(), digest);
    let rehashed = store.hash_partial(id).await.expect("hash");
    assert_eq!(rehashed.finalize().to_vec(), digest);
}

#[tokio::test]
async fn append_at_the_wrong_offset_reports_the_current_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(dir.path()).await.expect("open");
    let id = Uuid::now_v7();
    store.create_partial(id).await.expect("create");
    store
        .append(id, 0, 10, body(&["abc"]), |_| ())
        .await
        .expect("append");
    let result = store.append(id, 1, 10, body(&["x"]), |_| ()).await;
    assert!(matches!(
        result,
        Err(FilesError::OffsetMismatch { current: 3 })
    ));
}

#[tokio::test]
async fn append_past_the_declared_size_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(dir.path()).await.expect("open");
    let id = Uuid::now_v7();
    store.create_partial(id).await.expect("create");
    let result = store.append(id, 0, 4, body(&["abc", "de"]), |_| ()).await;
    assert!(matches!(result, Err(FilesError::TooLarge { limit: 4 })));
    assert_eq!(store.partial_len(id).await.expect("len"), 3);
}

#[tokio::test]
async fn sweep_removes_only_stale_orphans() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = BlobStore::open(dir.path()).await.expect("open");
    let (live, orphan) = (Uuid::now_v7(), Uuid::now_v7());
    store.create_partial(live).await.expect("live");
    store.create_partial(orphan).await.expect("orphan");
    let known = HashSet::from([live]);
    assert_eq!(
        store
            .sweep(&known, Duration::from_secs(3600))
            .await
            .expect("fresh"),
        0
    );
    assert_eq!(store.sweep(&known, Duration::ZERO).await.expect("stale"), 1);
    assert_eq!(store.partial_len(live).await.expect("live"), 0);
    assert!(matches!(
        store.partial_len(orphan).await,
        Err(FilesError::NotFound)
    ));
}
