//! Wire types for the kallip local daemon UDS protocol.
//!
//! Shared by the daemon, `kallipctl`, and a future web proxy — this
//! crate holds no runtime code, only the protocol shape and its round-trip
//! guarantees.

pub mod wire;
