//! Workspace-binary resolution, mirroring the tagma sandbox harness lookup:
//! `KALLIP_BIN_DIR` (container/dev states it) → `CARGO_BIN_EXE_<name>`
//! (cargo test injects it for same-package bins) → PATH (bare name).

use std::path::PathBuf;

pub fn resolve(name: &str) -> PathBuf {
    if let Ok(dir) = std::env::var("KALLIP_BIN_DIR")
        && let p = std::path::Path::new(&dir).join(name)
        && p.is_file()
    {
        return p;
    }
    let var = format!("CARGO_BIN_EXE_{name}");
    if let Ok(p) = std::env::var(&var) {
        return PathBuf::from(p);
    }
    PathBuf::from(name)
}
