//! Content-addressed blob storage for the kallip-files service.
//!
//! Content is addressed by its SHA-256 digest: the id is the algorithm
//! prefix plus the full lowercase hex digest, never truncated, so safety
//! reduces to hash safety with no collision-handling code, and identical
//! uploads deduplicate by construction.
//!
//! [`BlobStore`] is the object-safe seam the HTTP layer will hold as
//! `Arc<dyn BlobStore>`: `put` streams a reader through hashing and to
//! disk in one pass, `get_range` serves byte windows, and a cloud
//! object-store backend can arrive later behind the same trait without
//! touching routes. The metadata layer (`metadata`, `migration`) tracks
//! uploads with reference counts and `gc` reclaims unreferenced blobs;
//! the service binary arrives with the HTTP API.

pub mod backend;
pub mod blob;
pub mod gc;
pub mod metadata;
pub mod migration;

#[cfg(test)]
mod test_helpers;

pub use backend::LocalBackend;
pub use blob::{BlobId, BlobInfo, BlobStore, Error};
