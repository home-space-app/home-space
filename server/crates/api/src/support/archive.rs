//! Sending files and folders as one zip, made while it is sent.

use std::{
    fs::File,
    io::{self, BufWriter, Write},
};

use axum::{
    body::{Body, Bytes},
    http::{
        HeaderValue,
        header::{CONTENT_DISPOSITION, CONTENT_TYPE},
    },
    response::Response,
};
use time::PrimitiveDateTime;
use tokio::sync::mpsc;
use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

use files::ArchiveEntry;
use protocol::{NodeKind, encoding::percent_encode};

/// Names the zip of a share with several items, before the owner's name.
const SHARED_BY: &str = "Shared by";
const ZIP_MIME: &str = "application/zip";
const ZIP_EXTENSION: &str = ".zip";
/// How much of an archive is gathered before it is sent on.
const ARCHIVE_CHUNK_BYTES: usize = 64 * 1024;
/// How many chunks of an archive may wait for a slow client.
const ARCHIVE_QUEUE: usize = 4;

/// `entries` as a zip attachment called `name.zip`. The zip is written without compression
/// while it is sent, so it has no length up front and cannot resume; a failure on the way
/// ends the response with an error, so the client does not keep a cut-off zip.
pub(crate) fn zip_response(name: &str, entries: Vec<ArchiveEntry>) -> Response {
    let (sender, receiver) = mpsc::channel::<io::Result<Bytes>>(ARCHIVE_QUEUE);
    let archive = name.to_owned();
    tokio::task::spawn_blocking(move || {
        let out = BufWriter::with_capacity(ARCHIVE_CHUNK_BYTES, ChunkWriter(sender.clone()));
        if let Err(err) = write_archive(out, &entries) {
            tracing::warn!(archive, error = %err, "archive failed");
            let _ = sender.blocking_send(Err(io::Error::other(err)));
        }
    });
    let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
        receiver.recv().await.map(|chunk| (chunk, receiver))
    });
    let mut response = Response::new(Body::from_stream(stream));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(ZIP_MIME));
    let disposition = format!(
        "attachment; filename*=UTF-8''{}",
        percent_encode(&format!("{name}{ZIP_EXTENSION}"))
    );
    if let Ok(value) = HeaderValue::from_str(&disposition) {
        headers.insert(CONTENT_DISPOSITION, value);
    }
    response
}

/// Hands what is written to the response, a chunk at a time.
struct ChunkWriter(mpsc::Sender<io::Result<Bytes>>);

impl Write for ChunkWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .blocking_send(Ok(Bytes::copy_from_slice(buf)))
            .map_err(|_| io::Error::from(io::ErrorKind::BrokenPipe))?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Writes `entries` into a zip on `out`: folders as entries of their own, files with their
/// bytes at their path.
fn write_archive(out: impl Write, entries: &[ArchiveEntry]) -> zip::result::ZipResult<()> {
    let mut zip = ZipWriter::new_stream(out);
    for entry in entries {
        let mut options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .large_file(entry.size >= i64::from(u32::MAX));
        if let Some(modified) = entry
            .modified_at
            .and_then(|at| DateTime::try_from(PrimitiveDateTime::new(at.date(), at.time())).ok())
        {
            options = options.last_modified_time(modified);
        }
        match entry.kind {
            NodeKind::Folder => zip.add_directory(entry.path.as_str(), options)?,
            NodeKind::File => {
                zip.start_file(entry.path.as_str(), options)?;
                if let Some(source) = &entry.source {
                    io::copy(&mut File::open(source)?, &mut zip)?;
                }
            }
        }
    }
    zip.finish()?.into_inner().flush()?;
    Ok(())
}

/// The name of a share's zip: its only top-level entry, or who shared it.
pub(crate) fn archive_name(owner_name: &str, entries: &[ArchiveEntry]) -> String {
    let mut top = entries.iter().filter(|entry| !entry.path.contains('/'));
    match (top.next(), top.next()) {
        (Some(only), None) => only.path.clone(),
        _ => format!("{SHARED_BY} {owner_name}"),
    }
}
