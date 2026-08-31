//! Blob id encoding: `sha256-` plus the full 64 lowercase hex digest
//! characters.

use std::fmt;

use super::Error;

/// A content address: the algorithm prefix plus the full digest.
///
/// The id is never truncated. A shorter prefix would shrink the space an
/// adversary must collide in, while the full hex keeps safety identical
/// to hash safety with no collision-handling code.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BlobId(String);

impl BlobId {
    /// The algorithm prefix every blob id starts with.
    pub const PREFIX: &'static str = "sha256-";

    /// The id of a SHA-256 digest: the canonical encoding path.
    pub fn from_digest(digest: [u8; 32]) -> Self {
        Self(format!("{}{}", Self::PREFIX, hex::encode(digest)))
    }

    /// Validates and wraps an id string: [`Self::PREFIX`] plus exactly 64
    /// lowercase hex characters. Upper case is rejected too, so the
    /// on-disk layout stays canonical.
    pub fn parse(id: &str) -> Result<Self, Error> {
        let Some(hex_part) = id.strip_prefix(Self::PREFIX) else {
            return Err(Error::InvalidId(id.to_string()));
        };
        let well_formed = hex_part.len() == 64
            && hex_part
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !well_formed {
            return Err(Error::InvalidId(id.to_string()));
        }
        Ok(Self(id.to_string()))
    }

    /// The storage bucket: the first two hex characters of the digest,
    /// spreading blobs over 256 directories so no single one accumulates
    /// every entry.
    pub fn bucket(&self) -> &str {
        &self.0[Self::PREFIX.len()..Self::PREFIX.len() + 2]
    }

    /// The canonical string form.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
