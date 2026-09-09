//! Boot-time provisioning of the archeion's own secrets.
//!
//! Two secrets, two lifecycles, one rule: plaintext never reaches the
//! journal. The admin token comes in two forms — a pinned operator asset
//! (lives wherever the operator put it) and a generated short-lived
//! bootstrap credential (rewritten on every start into runtime state,
//! valid for one run). The platform-internal secret is
//! machine-internal alignment material: generated once into persistent
//! state, then only read, because the whole platform must keep agreeing
//! on one value while services restart around it.

use std::path::Path;

use anyhow::Context;
use kallip_common::authtoken::MintedToken;

use crate::token;

/// Load the platform-internal secret from `path`, generating and persisting
/// it on first boot.
///
/// The load-bearing rule is the match order: an existing file is read as-is
/// and never rewritten, because consumers serve with the value they read at
/// their own boot — a silent rewrite would split the platform into token
/// generations that fail each other's checks. Regeneration is a deliberate
/// act (delete the file, restart the group). Mode 0640: the group bits
/// read as whatever group the process runs as -- the deployment runs
/// the archeion with the gate group as primary (the NixOS module's
/// Group=), so consumers joining that group can read; the world must not.
pub(crate) fn provision_internal_token(path: &Path) -> anyhow::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let secret = raw.trim();
            if secret.is_empty() {
                anyhow::bail!(
                    "{} exists but is empty; refusing to guess (delete the file to re-provision)",
                    path.display()
                );
            }
            Ok(secret.to_owned())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let minted = MintedToken::generate(token::INTERNAL);
            kallip_common::secret_file::write_atomic(path, minted.secret().as_bytes(), 0o640)
                .with_context(|| {
                    format!(
                        "provisioning the platform-internal secret at {}",
                        path.display()
                    )
                })?;
            Ok(minted.secret().to_owned())
        }
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Persist a freshly generated admin token as a 0600 `KEY=value` file.
///
/// The caller picks the location (the NixOS module points at the unit's
/// runtime directory): a generated admin token is a short-lived bootstrap
/// credential: it is
/// rewritten on every start and its plaintext never touches the journal.
/// `KEY=value` format lets an operator source the file straight into
/// `kallip-admin`'s environment.
pub(crate) fn write_generated_admin(path: &Path, secret: &str) -> anyhow::Result<()> {
    kallip_common::secret_file::write_atomic(
        path,
        format!("KALLIP_ARCHEION_ADMIN_TOKEN={secret}\n").as_bytes(),
        0o600,
    )
    .with_context(|| format!("writing the generated admin token to {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn internal_existing_value_is_read_never_rewritten() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("internal-token");
        fs::write(&path, "pre-existing-dev-secret\n").expect("write");
        let got = provision_internal_token(&path).expect("read");
        assert_eq!(got, "pre-existing-dev-secret");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "pre-existing-dev-secret\n"
        );
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "pre-existing file must not be touched");
    }

    #[test]
    fn internal_missing_file_generates_0640_with_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("internal-token");
        let got = provision_internal_token(&path).expect("generate");
        assert!(got.starts_with("sk-internal-"), "{got}");
        let meta = fs::metadata(&path).expect("meta");
        assert_eq!(meta.permissions().mode() & 0o777, 0o640);
        assert_eq!(fs::read_to_string(&path).expect("read"), got);
    }

    #[test]
    fn internal_second_boot_keeps_the_first_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("internal-token");
        let first = provision_internal_token(&path).expect("first");
        let second = provision_internal_token(&path).expect("second");
        assert_eq!(first, second);
    }

    #[test]
    fn internal_empty_file_fails_closed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("internal-token");
        fs::write(&path, "\n").expect("write");
        let err = provision_internal_token(&path).expect_err("must fail");
        assert!(err.to_string().contains("empty"), "{err}");
    }

    #[test]
    fn internal_missing_parent_is_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nope").join("internal-token");
        let err = provision_internal_token(&path).expect_err("must fail");
        assert!(err.to_string().contains("provisioning"), "{err}");
    }

    #[test]
    fn admin_out_file_is_0600_key_value() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("admin-token.env");
        write_generated_admin(&path, "sk-admin-t").expect("write");
        assert_eq!(
            fs::read_to_string(&path).expect("read"),
            "KALLIP_ARCHEION_ADMIN_TOKEN=sk-admin-t\n"
        );
        let mode = fs::metadata(&path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
