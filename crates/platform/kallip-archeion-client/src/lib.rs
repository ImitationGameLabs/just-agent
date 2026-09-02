//! HTTP client for the kallip-archeion relay. See [`ArcheionClient`] for the surface.

mod client;

pub use client::{ArcheionClient, ArcheionClientBuilder};
// Re-export the shared admin DTOs so callers depend on this crate alone for the
// archeion HTTP surface. `DeviceKey` is the one e2e type surfaced (for `enroll`);
// the rest of the private-key API stays in `kallip-e2ee`.
pub use kallip_archeion_common::admin::{
    CreateEnrollmentCodeRequest, CreateEnrollmentCodeResponse, Page, PageQuery, PasskeySummary,
    UpdateUserRequest, UserSummary,
};
pub use kallip_archeion_common::ids::TagmaId;
pub use kallip_common::protocol::ApiError;
pub use kallip_e2ee::DeviceKey;
