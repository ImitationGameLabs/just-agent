//! Model profile registry: provider/model selection with named profile sets.
//!
//! Provides the data model, a TOML/env config loader (returning [`ProfileConfig`]), and re-exports
//! the upstream [`ChatClient`]. The tagma builds backends and assembles them into a
//! [`ProfileRegistry`]; the runtime holds pre-built backends and does selection only.
//!
//! [`ChatClient`]: just_llm_client::ChatClient

pub mod config;
pub mod model;
pub mod registry;

pub use config::{ProfileConfig, config_path, is_valid_set_name, load, normalize_default, save};
pub use just_llm_client::ChatClient;
pub use model::{Profile, ProfileSet, Provider};
pub use registry::{BackendSource, DanglingSet, NO_PROFILE_HINT, ProfileRegistry};
