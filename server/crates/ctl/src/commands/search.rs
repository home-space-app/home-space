//! `homespace-ctl search`: maintaining the search index.

use std::path::Path;

use anyhow::{Context, Result};

use db::Db;
use files::BlobStore;
use search::SearchIndex;

use crate::cli::SearchCommand;

pub async fn run(command: SearchCommand, db: Db) -> Result<()> {
    match command {
        SearchCommand::Reindex { all, storage_root } => reindex(db, &storage_root, all).await,
    }
}

/// Indexes what is missing or outdated; with `all`, drops the index and indexes everything.
async fn reindex(db: Db, storage_root: &Path, all: bool) -> Result<()> {
    let store = BlobStore::open(storage_root)
        .await
        .context("opening storage root")?;
    let (search, worker) = SearchIndex::start(db, store, Vec::new());
    let report = if all {
        search.rebuild().await
    } else {
        search.catch_up().await
    }
    .context("reindexing")?;
    drop(search);
    worker.await.context("waiting for content indexing")?;
    println!(
        "indexed {} names, extracted content of {} files",
        report.names_indexed, report.content_queued
    );
    Ok(())
}
