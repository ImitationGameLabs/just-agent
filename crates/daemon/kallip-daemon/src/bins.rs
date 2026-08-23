//! Workspace-binary resolution, mirroring the tagma sandbox harness lookup:
//! `KALLIP_BIN_DIR` (container/dev states it) → `CARGO_BIN_EXE_<name>`
//! (cargo test injects it for same-package bins) → one level out of
//! `deps/` (the sibling workspace binaries under `cargo test`) → PATH.

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
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(std::path::Path::to_path_buf))
    {
        // Under `cargo test` the test binary lives in `deps/` while the
        // sibling workspace binaries sit one level up; installed or under
        // `cargo run`, they sit in the same directory as this binary.
        let sibling_dir = if exe_dir.ends_with("deps") {
            exe_dir.parent().map(std::path::Path::to_path_buf)
        } else {
            Some(exe_dir.clone())
        };
        if let Some(dir) = sibling_dir
            && let in_dir = dir.join(name)
            && in_dir.is_file()
        {
            return in_dir;
        }
    }
    PathBuf::from(name)
}
