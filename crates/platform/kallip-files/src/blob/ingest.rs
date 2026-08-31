//! Single-pass ingest: hash and write in one stream, then commit by
//! rename.

use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use super::Error;
use super::hash::BlobId;

/// Bytes per ingest read. One buffer bounds peak memory no matter how
/// large the upload is; 64 KiB keeps syscall count low without
/// pressuring the allocator.
const CHUNK: usize = 64 * 1024;

/// Stage `content` into a fresh temp file under `<root>/tmp/` -- same
/// volume as the blobs, so the final rename is atomic -- hashing on the
/// way, then commit: if the addressed blob already exists (same digest
/// means same bytes) the temp file is removed and the incumbent stays;
/// otherwise the temp file is renamed into `blobs/`. Nothing partial is
/// ever visible under `blobs/`, and any failure removes the temp file
/// best-effort.
///
/// Concurrent uploads of the same content each stage their own temp
/// file: the first rename wins, the rest see the target and clean up
/// after themselves.
pub(crate) async fn ingest(
    root: &Path,
    content: &mut (dyn AsyncRead + Unpin + Send),
) -> Result<BlobId, Error> {
    fs::create_dir_all(tmp_dir(root)).await?;

    let tmp_path = fresh_tmp_path(root)?;
    let mut hasher = Sha256::new();
    let mut tmp = fs::File::create(&tmp_path).await?;
    let mut buf = vec![0u8; CHUNK];
    let staged = async {
        loop {
            let n = content.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            tmp.write_all(&buf[..n]).await?;
        }
        tmp.flush().await?;
        tmp.sync_all().await?;
        Ok::<(), Error>(())
    }
    .await;
    if let Err(err) = staged {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(err);
    }

    let digest = finalize_sha256(&mut hasher);
    let id = BlobId::from_digest(digest);

    let target = blob_path(root, &id);
    // The bucket directory is created before the existence check so a
    // first-of-its-bucket rename cannot fail on a missing parent (that
    // error would surface as a bare ENOENT, indistinguishable from the
    // blob-missing case for callers).
    fs::create_dir_all(target.parent().expect("blob path always has a parent")).await?;
    match fs::metadata(&target).await {
        // Same digest is same bytes: the stored copy is authoritative.
        Ok(_) => {
            let _ = fs::remove_file(&tmp_path).await;
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            if let Err(rename_err) = fs::rename(&tmp_path, &target).await {
                let _ = fs::remove_file(&tmp_path).await;
                return Err(rename_err.into());
            }
        }
        Err(err) => {
            let _ = fs::remove_file(&tmp_path).await;
            return Err(err.into());
        }
    }
    Ok(id)
}

/// `<root>/blobs/<first two hex chars>/<id>`.
pub(crate) fn blob_path(root: &Path, id: &BlobId) -> PathBuf {
    root.join("blobs").join(id.bucket()).join(id.as_str())
}

fn tmp_dir(root: &Path) -> PathBuf {
    root.join("tmp")
}

/// A fresh random temp file name: pure randomness with no content
/// semantics, so concurrent ingests never stage into the same file.
/// 128 bits makes a stray collision negligible; no retry loop to test.
fn fresh_tmp_path(root: &Path) -> Result<PathBuf, Error> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| Error::Rng(e.to_string()))?;
    Ok(tmp_dir(root).join(hex::encode(bytes)))
}

fn finalize_sha256(hasher: &mut Sha256) -> [u8; 32] {
    let digest = hasher.finalize_reset();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}
