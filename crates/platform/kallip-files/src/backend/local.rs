//! The local-disk backend: blobs under `<root>/blobs/`, staging under
//! `<root>/tmp/`, both on one volume so commits are atomic renames.
//!
//! Durability: `put` fsyncs the staging file before the rename, so the
//! bytes are on disk when it returns; the directory entry itself is not
//! fsynced, so a power loss right after a commit can still lose the
//! rename. Content addressing makes a client retry a safe,
//! byte-identical heal.

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};

use crate::blob::Error;
use crate::blob::hash::BlobId;
use crate::blob::ingest;
use crate::blob::store::{BlobInfo, BlobStore};

/// Content-addressed storage on the local filesystem.
pub struct LocalBackend {
    root: PathBuf,
}

impl LocalBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The backend behind the object-safe seam, ready for service state
    /// (the HTTP layer holds an `Arc<dyn BlobStore>`).
    pub fn arc(root: impl Into<PathBuf>) -> Arc<dyn BlobStore> {
        Arc::new(Self::new(root))
    }

    fn blob_path(&self, id: &BlobId) -> PathBuf {
        ingest::blob_path(&self.root, id)
    }
}

/// A missing file is the caller-facing `NotFound`/`None` everywhere;
/// anything else stays an IO error.
fn map_missing(err: io::Error, id: &BlobId) -> Error {
    if err.kind() == io::ErrorKind::NotFound {
        Error::NotFound(id.clone())
    } else {
        err.into()
    }
}

#[async_trait]
impl BlobStore for LocalBackend {
    async fn put(&self, content: &mut (dyn AsyncRead + Unpin + Send)) -> Result<BlobId, Error> {
        ingest::ingest(&self.root, content).await
    }

    async fn get(&self, id: &BlobId) -> Result<Vec<u8>, Error> {
        fs::read(self.blob_path(id))
            .await
            .map_err(|err| map_missing(err, id))
    }

    async fn get_range(&self, id: &BlobId, offset: u64, len: u64) -> Result<Vec<u8>, Error> {
        let mut file = fs::File::open(self.blob_path(id))
            .await
            .map_err(|err| map_missing(err, id))?;
        let size = file.metadata().await?.len();
        if offset >= size {
            return Err(Error::RangeOutOfBounds { offset, len, size });
        }
        file.seek(io::SeekFrom::Start(offset)).await?;
        let mut limited = file.take(len);
        let mut out = Vec::new();
        limited.read_to_end(&mut out).await?;
        Ok(out)
    }

    async fn stat(&self, id: &BlobId) -> Result<Option<BlobInfo>, Error> {
        match fs::metadata(self.blob_path(id)).await {
            Ok(meta) => Ok(Some(BlobInfo {
                id: id.clone(),
                size: meta.len(),
            })),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        }
    }

    async fn delete(&self, id: &BlobId) -> Result<(), Error> {
        match fs::remove_file(self.blob_path(id)).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }
}
