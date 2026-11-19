//! `homespace search`: maintaining the search index.

use anyhow::{Context, Result};

use db::Db;
use files::BlobStore;
use search::SearchIndex;

use crate::{cli::SearchCommand, config::Config};

pub async fn run(command: SearchCommand, db: Db, config: &Config) -> Result<()> {
    match command {
        SearchCommand::Reindex { all } => reindex(db, config, all).await,
    }
}

/// Indexes what is missing or outdated; with `all`, drops the index and indexes everything.
async fn reindex(db: Db, config: &Config, all: bool) -> Result<()> {
    let store = BlobStore::open(&config.storage_root)
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
