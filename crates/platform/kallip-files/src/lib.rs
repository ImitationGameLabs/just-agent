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
//! touching routes. This crate ships the storage layer only: metadata,
//! reference counting, and garbage collection live in the metadata
//! layer; the service binary arrives with the HTTP API.

pub mod backend;
pub mod blob;

pub use backend::LocalBackend;
pub use blob::{BlobId, BlobInfo, BlobStore, Error};
