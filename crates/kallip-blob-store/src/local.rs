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

use crate::ingest;
use crate::store::{BlobInfo, BlobStore};
use crate::{BlobId, Error};

/// Content-addressed storage on the local filesystem.
pub struct LocalBackend {
    root: PathBuf,
}

impl LocalBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The backend behind the object-safe seam, ready for service state
    /// (services hold an `Arc<dyn BlobStore>`).
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn put_get_and_dedup_round_trip() {
        let root = tempfile::tempdir().unwrap();
        let backend = LocalBackend::new(root.path());

        let bytes = b"hello content-addressed world".repeat(100);
        let id = backend.put(&mut bytes.as_slice()).await.unwrap();
        assert_eq!(backend.get(&id).await.unwrap(), bytes);

        // Same bytes again: idempotent, one stored copy.
        let id2 = backend.put(&mut bytes.as_slice()).await.unwrap();
        assert_eq!(id, id2);

        // Range windows: past-end rejected, over-long clamped.
        let window = backend.get_range(&id, 6, 7).await.unwrap();
        assert_eq!(&window, &bytes[6..13]);
        assert!(matches!(
            backend.get_range(&id, bytes.len() as u64, 1).await,
            Err(Error::RangeOutOfBounds { .. })
        ));
        let tail = backend
            .get_range(&id, (bytes.len() - 2) as u64, 99)
            .await
            .unwrap();
        assert_eq!(&tail, &bytes[bytes.len() - 2..]);

        // The committed layout matches the exposed path helper.
        assert!(ingest::blob_path(root.path(), &id).is_file());

        // Delete is idempotent.
        backend.delete(&id).await.unwrap();
        backend.delete(&id).await.unwrap();
        assert_eq!(backend.stat(&id).await.unwrap(), None);
    }
}
