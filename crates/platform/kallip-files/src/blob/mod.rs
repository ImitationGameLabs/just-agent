//! The blob layer: id encoding, the storage seam, and the ingest engine.

pub mod error;
pub mod hash;
pub mod ingest;
pub mod store;

pub use self::error::Error;
pub use self::hash::BlobId;
pub use self::store::{BlobInfo, BlobStore};
