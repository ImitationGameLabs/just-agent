//! Error vocabulary for the blob layer.

use super::hash::BlobId;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The requested blob does not exist. Missing and invalid ids stay
    /// distinct errors so the HTTP layer can map them to 404 and 400.
    #[error("blob not found: {0}")]
    NotFound(BlobId),

    /// A blob id failed validation: wrong algorithm prefix, wrong
    /// length, or non-canonical hex.
    #[error("invalid blob id: {0}")]
    InvalidId(String),

    /// A range request started at or past the end of the blob. A `len`
    /// running past the end is clamped instead (open-ended ranges must
    /// succeed), so this fires only when there is nothing to serve.
    #[error("range out of bounds: offset {offset} beyond size {size}")]
    RangeOutOfBounds { offset: u64, len: u64, size: u64 },

    /// The OS entropy source failed; no temp file name could be minted.
    #[error("entropy source failure: {0}")]
    Rng(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}
