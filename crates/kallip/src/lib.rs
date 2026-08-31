//! Library face of the `kallip` CLI: the pieces that outlive the binary's
//! `main` -- the files-service client -- live here so the integration tests
//! (and future faces) can drive them without shelling out to the binary.

pub mod file;
