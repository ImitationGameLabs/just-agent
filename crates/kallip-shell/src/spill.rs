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
use nix::errno::Errno;

use nix::fcntl::{AT_FDCWD, AtFlags, OFlag, open, openat};
use nix::sys::stat::Mode;
use nix::unistd::linkat;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// Parameters that distinguish one spill family from another.
#[derive(Clone, Copy)]
pub struct SpillLayout {
    /// Subdirectory under the spill root (`message` or `bash-exec`).
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
/// The bash-exec capture spill family: kallipai/spill/bash-exec/{2 hex}/{14 hex}.txt.
pub const BASH_EXEC_SPILL: SpillLayout = SpillLayout {
    subdir: "bash-exec",
    prefix: "",
    hash_hex_chars: 16,
};

/// Root for all spill families — one constant so a root relocation is a
/// one-line change. Cleanup belongs to the system /tmp mechanisms.
pub fn spill_root() -> PathBuf {
    std::env::temp_dir().join("kallipai").join("spill")
}

fn digest_to_hex(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

fn hex_sha256(bytes: &[u8]) -> String {
    digest_to_hex(Sha256::digest(bytes))
}

/// The 0700 chain for one spill family: root's parent, root, the family
/// dir, and (when a shard is named) the shard dir. Built level by level;
/// on an existing level the mode is re-asserted so a stale wider mode
/// cannot quietly persist. A symlink at any leaf is then refused by the
/// caller's O_NOFOLLOW open, so this only ever creates or tightens real
/// dirs.
fn ensure_private_chain(
    root: &Path,
    subdir: &str,
    shard: Option<&str>,
) -> std::io::Result<PathBuf> {
    let family = root.join(subdir);
    let mut chain = vec![root.to_path_buf(), family.clone()];
    if let Some(shard) = shard {
        chain.push(family.join(shard));
    }
    if let Some(grand) = root.parent() {
        chain.insert(0, grand.to_path_buf());
    }
    for dir in &chain {
        let _ = std::fs::create_dir(dir);
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(match shard {
        Some(shard) => family.join(shard),
        None => family,
    })
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
    let shard_dir = ensure_private_chain(root, layout.subdir, Some(shard))?;

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

/// A streaming spill: an append-facing writer for a stream whose content —
/// and therefore whose content-addressed name — is unknown until it
/// completes. While the stream runs, bytes go to
/// `<root>/<subdir>/.tmp-{stream key}`, a name that exists from the first
/// overflow so an early banner can reference a live path (a timed-out
/// foreground exec converts to a background task whose banner keeps
/// naming a file that is still growing). `finalize` completes the
/// incremental sha256 (no re-read) and links the temp file to its
/// content-addressed name under the same pre-occupation contract as
/// spill_content: an existing file is read back and full hashes compared
/// — a match reuses the existing file without re-linking, a mismatch is
/// an error, never a silent reuse. The temp name survives the link on
/// purpose: banners that pointed at the in-flight path stay valid, and
/// the leftover is reclaimed by the system /tmp cleanup like every other
/// spill file.
/// A twin orphaned by a crash between create and
/// finalize or discard is the same residue: no code path re-reads a
/// temp name, so a stale or poisoned twin stays inert until the
/// system cleanup reclaims it.
pub struct StreamingSpill {
    file: File,
    tmp_path: PathBuf,
    hasher: Sha256,
    layout: SpillLayout,
}

impl StreamingSpill {
    /// Create the temp file under `<root>/<layout.subdir>/`, building the
    /// 0700 directory chain the same way spill_content does.
    pub fn create(root: &Path, layout: &SpillLayout, stream_key: &str) -> std::io::Result<Self> {
        // Refuse a symlink swapped in at the caller-given root itself: the
        // family mkdir below would otherwise follow it and land the
        // in-flight file under the symlink's target. (O_NOFOLLOW only
        // guards a final component, and root is an interior one here.)
        // Root may legitimately not exist yet (lazy chain creation), so
        // the branch below admits only plain ENOENT: any other error —
        // including every symlink form the open can surface — fails
        // closed, and the mkdir chain then builds a real directory.
        match open(
            root,
            OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        ) {
            Ok(_) => {}
            Err(Errno::ENOENT) => {}
            Err(e) => return Err(std::io::Error::from(e)),
        }
        let family = ensure_private_chain(root, layout.subdir, None)?;
        let name = format!(".tmp-{stream_key}");
        let dirfd = open(
            &family,
            OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        let file = openat(
            dirfd.as_fd(),
            name.as_str(),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW,
            Mode::from_bits_truncate(0o600),
        )
        .map_err(std::io::Error::from)?;
        Ok(Self {
            file: File::from(file),
            tmp_path: family.join(name),
            hasher: Sha256::new(),
            layout: *layout,
        })
    }

    /// Append one chunk to the spill and fold it into the running hash.
    pub fn append(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.file.write_all(bytes)?;
        self.hasher.update(bytes);
        Ok(())
    }

    /// Path of the in-flight temp file (valid from creation until the
    /// process exits; the finalize link does not remove it).
    pub fn tmp_path(&self) -> &Path {
        &self.tmp_path
    }

    /// Complete the stream: finish the hash, then link the temp file to
    /// its content-addressed name. See the type doc for the pre-occupation
    /// contract and why the temp name is left in place.
    pub fn finalize(self) -> std::io::Result<PathBuf> {
        let StreamingSpill {
            file,
            tmp_path,
            hasher,
            layout,
        } = self;
        drop(file);
        let full_hash = digest_to_hex(hasher.finalize());
        let (shard, stem) = full_hash.split_at(2);
        let stem = &stem[..layout.hash_hex_chars - 2];
        let filename = format!("{}{stem}.txt", layout.prefix);
        // Rebuild the chain through the shared helper so every level is
        // re-asserted 0700, then link and verify through a dirfd — the
        // same channel spill_content uses, so both families share one
        // hardening story instead of two.
        let family = tmp_path.parent().expect("tmp path always has a parent");
        let root = family.parent().expect("family always has a parent");
        let shard_dir = ensure_private_chain(root, layout.subdir, Some(shard))?;
        let dirfd = open(
            &shard_dir,
            OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map_err(std::io::Error::from)?;
        match linkat(
            AT_FDCWD,
            tmp_path.as_path(),
            dirfd.as_fd(),
            filename.as_str(),
            AtFlags::empty(),
        ) {
            Ok(()) => {}
            Err(Errno::EEXIST) => {
                // The content-addressed file already exists: verify it
                // really is this content, and reuse it without re-linking.
                // Equivalence goes through the hash, not a byte compare:
                // the streaming design never retains the incoming stream,
                // so the running digest is all we have to compare against.
                reuse_verified(&dirfd, &filename, &full_hash)?;
            }
            Err(e) => return Err(std::io::Error::from(e)),
        }
        Ok(shard_dir.join(filename))
    }
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

    /// The finalized content-addressed file exists and the `.tmp-` twin is
    /// still in place: banners that named the in-flight path stay valid
    /// after completion. The twin is intentional residue, reclaimed by the
    /// system /tmp cleanup like every other spill file.
    #[test]
    fn finalize_leaves_the_tmp_twin_in_place() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("kallipai").join("spill");
        let mut spill = StreamingSpill::create(&root, &MESSAGE_SPILL, "nonce-1").unwrap();
        spill.append(b"hello").unwrap();
        let finalized = spill.finalize().unwrap();
        assert_eq!(std::fs::read(&finalized).unwrap(), b"hello");
        let twin = root.join("message").join(".tmp-nonce-1");
        assert!(twin.exists(), "tmp twin survives finalize for banners");
        assert_eq!(std::fs::read(&twin).unwrap(), b"hello");
    }

    /// A poisoned `.tmp-` twin (e.g. left behind by a crashed run) is
    /// inert: content spill only ever touches content-addressed names, a
    /// fresh stream never reopens another stream's temp name, and the
    /// poison is neither read nor rewritten — it waits for the system
    /// /tmp cleanup. No cleanup path exists in-process on purpose.
    #[test]
    fn poisoned_tmp_twin_is_never_read() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("kallipai").join("spill");
        std::fs::create_dir_all(root.join("message")).unwrap();
        let poison = root.join("message").join(".tmp-poisoned");
        std::fs::write(&poison, b"garbage").unwrap();
        let path = spill_content(&root, &layout(), "real").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"real");
        let mut spill = StreamingSpill::create(&root, &MESSAGE_SPILL, "fresh").unwrap();
        spill.append(b"stream").unwrap();
        let finalized = spill.finalize().unwrap();
        assert_eq!(std::fs::read(&finalized).unwrap(), b"stream");
        assert_eq!(std::fs::read(&poison).unwrap(), b"garbage");
    }

    /// A stale twin at a reused stream key fails the create closed: O_EXCL
    /// refuses the name, so the stale bytes are never truncated, read, or
    /// silently reused as if they were this stream's content.
    #[test]
    fn same_stream_key_fails_closed_over_a_stale_twin() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("kallipai").join("spill");
        let mut first = StreamingSpill::create(&root, &MESSAGE_SPILL, "dup").unwrap();
        first.append(b"first").unwrap();
        let second = StreamingSpill::create(&root, &MESSAGE_SPILL, "dup");
        let err = match second {
            Ok(_) => panic!("same stream key must fail closed"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        let finalized = first.finalize().unwrap();
        assert_eq!(std::fs::read(&finalized).unwrap(), b"first");
    }
}
