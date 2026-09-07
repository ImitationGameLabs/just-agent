//! Content-addressed spill files under temp_dir()/kallipai/spill/.
//!
//! One safe-write implementation serves every spill family: the message-entry
//! spill (message/, this module's first consumer, called from the agent
//! runtime) and the shell capture spill (bash-exec/, migrated from the
//! capture layer's kallip/ uuid names to hash names). Shared here
//! so the safety semantics — TOCTOU-safe creation, private-permission chain,
//! pre-occupation verification — stay single-sourced instead of drifting per
//! face.
//!
//! Naming: sha256 of the content, truncated to the layout's hex width; the
//! first two hex chars shard the directory (message/ab/...) so no single
//! directory accumulates unbounded entries. The name carries only the
//! truncated hash, but verification always compares full hashes: a truncated
//! name colliding with different content is an error, never a silent reuse.
//!
//! Cleanup: none in-process. Spill files live under the system temp
//! directory, so the distro's tmpfiles/system tmp cleanup owns their
//! lifecycle; duplicating that here would only diverge from the system's
//! policy.

use nix::fcntl::{OFlag, open, openat};
use nix::sys::stat::Mode;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Parameters that distinguish one spill family from another.
pub struct SpillLayout {
    /// Subdirectory under the spill root (`message`; `bash-exec` after the
    /// capture migration).
    pub subdir: &'static str,
    /// Filename prefix before the hash segment (empty for `message`).
    pub prefix: &'static str,
    /// Hex chars of the sha256 kept in the name: the first 2 shard the
    /// directory, the rest name the file. Verification compares full hashes
    /// regardless of this truncation, so it only shortens names.
    pub hash_hex_chars: usize,
}

/// The message-entry spill family: kallipai/spill/message/{2 hex}/{14 hex}.txt.
pub const MESSAGE_SPILL: SpillLayout = SpillLayout {
    subdir: "message",
    prefix: "",
    hash_hex_chars: 16,
};

/// Root for all spill families — one constant so a root relocation is a
/// one-line change. Cleanup belongs to the system /tmp mechanisms.
pub fn spill_root() -> PathBuf {
    std::env::temp_dir().join("kallipai").join("spill")
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// Spill content under root/layout: create-or-verify-and-reuse.
///
/// Ownership contract: the private-permission chain covers the given root,
/// its parent, the source subdir, and the shard — all get mode 0700. Callers
/// therefore pass a root they own the parent of (spill_root() and tests
/// alike), never a shared directory.
///
/// Creation is TOCTOU-safe at the leaf the same way the capture spill is:
/// the directory chain is built level by level at 0700 (spill content is
/// private message text, so the whole path is owner-only), the leaf dir is
/// opened O_DIRECTORY | O_NOFOLLOW (refusing a symlink swapped in at the
/// leaf and pinning the real dir inode), and the file itself is created
/// openat-relative to that dirfd with O_CREAT | O_EXCL | O_NOFOLLOW at
/// mode 0600. Once the dirfd is held, swapping the path for a symlink
/// cannot redirect the write.
///
/// Pre-occupation (a file already exists at the content's path — its hash
/// is predictable by design): read the existing file back, compare full
/// sha256 hashes, and on a match reuse the file as-is; on a mismatch
/// return an error. A mismatched file is never silently reused.
pub fn spill_content(root: &Path, layout: &SpillLayout, content: &str) -> std::io::Result<PathBuf> {
    let full_hash = hex_sha256(content.as_bytes());
    let (shard, stem) = full_hash.split_at(2);
    let stem = &stem[..layout.hash_hex_chars - 2];
    let filename = format!("{}{stem}.txt", layout.prefix);
    let shard_dir = root.join(layout.subdir).join(shard);

    // Build the chain level by level at 0700; on an existing level the mode
    // is re-asserted so a stale wider mode cannot quietly persist. A symlink
    // at any leaf is then refused by the O_NOFOLLOW open below, so this only
    // ever creates or tightens real dirs.
    let mut chain = vec![root.to_path_buf(), root.join(layout.subdir)];
    chain.push(shard_dir.clone());
    if let Some(grand) = root.parent() {
        chain.insert(0, grand.to_path_buf());
    }
    for dir in &chain {
        let _ = std::fs::create_dir(dir);
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let dirfd = open(
        &shard_dir,
        OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let created = openat(
        dirfd.as_fd(),
        filename.as_str(),
        OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW,
        Mode::from_bits_truncate(0o600),
    );
    match created {
        Ok(file) => {
            File::from(file).write_all(content.as_bytes())?;
            Ok(shard_dir.join(filename))
        }
        Err(nix::errno::Errno::EEXIST) => {
            reuse_verified(&dirfd, &filename, &full_hash)?;
            Ok(shard_dir.join(filename))
        }
        Err(e) => Err(std::io::Error::from(e)),
    }
}

/// The pre-occupied path holds a file already: verify it really is this
/// content (never silently reuse), then leave it untouched — identical
/// content needs no rewrite.
fn reuse_verified(
    dirfd: &std::os::fd::OwnedFd,
    filename: &str,
    full_hash: &str,
) -> std::io::Result<()> {
    let file = openat(
        dirfd.as_fd(),
        filename,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW,
        Mode::empty(),
    )
    .map_err(std::io::Error::from)?;
    let mut existing = Vec::new();
    File::from(file).read_to_end(&mut existing)?;
    if hex_sha256(&existing) != full_hash {
        return Err(std::io::Error::other(format!(
            "spill collision at {filename}: existing content does not match incoming content"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn layout() -> SpillLayout {
        SpillLayout {
            subdir: "message",
            prefix: "",
            hash_hex_chars: 16,
        }
    }

    #[test]
    fn spills_content_with_sharded_hash_name() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("kallipai").join("spill");
        let path = spill_content(&root, &layout(), "hello").unwrap();
        let shard = path.parent().unwrap();
        assert_eq!(shard.file_name().unwrap().to_str().unwrap().len(), 2);
        let stem = path.file_stem().unwrap().to_str().unwrap();
        assert_eq!(stem.len(), 14, "2 + 14 = 16 hex chars of the sha256");
        assert_eq!(std::fs::read(&path).unwrap(), b"hello");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn replay_reuses_the_same_file() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("kallipai").join("spill");
        let first = spill_content(&root, &layout(), "same content").unwrap();
        let second = spill_content(&root, &layout(), "same content").unwrap();
        assert_eq!(first, second);
        let entries = std::fs::read_dir(first.parent().unwrap()).unwrap().count();
        assert_eq!(entries, 1, "reuse must not create a second file");
    }

    #[test]
    fn collision_with_different_content_is_an_error() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("kallipai").join("spill");
        // Occupy the path "other" would take with different bytes.
        let occupied = spill_content(&root, &layout(), "other").unwrap();
        std::fs::write(&occupied, "tampered").unwrap();
        assert!(spill_content(&root, &layout(), "other").is_err());
        // The mismatched file is left as-is, never silently replaced.
        assert_eq!(std::fs::read(&occupied).unwrap(), b"tampered");
    }

    /// Spill content is private message text: the whole directory chain is
    /// owner-only (0700), not just the file. An existing level has its mode
    /// re-asserted, so the chain cannot silently widen.
    #[test]
    fn directory_chain_is_private() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("kallipai").join("spill");
        spill_content(&root, &layout(), "private").unwrap();
        let shard = std::fs::read_dir(root.join("message"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let dirs = [
            base.path().join("kallipai"),
            root.clone(),
            root.join("message"),
            shard,
        ];
        for dir in dirs {
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "chain dir {:?} must be 0700", dir);
        }
    }
}
