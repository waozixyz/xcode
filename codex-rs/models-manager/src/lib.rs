pub mod cache;
mod catalog;
pub mod collaboration_mode_presets;
pub(crate) mod config;
pub mod manager;
pub mod model_info;
pub mod model_presets;
mod originator;
pub mod test_support;

pub use catalog::CatalogModel;
pub use catalog::ModelOrigin;
pub use codex_protocol::auth::AuthMode;
pub use config::ModelsManagerConfig;
pub use originator::ModelCatalogOriginator;

/// Load the bundled model catalog shipped with `codex-models-manager`.
pub fn bundled_models_response()
-> std::result::Result<codex_protocol::openai_models::ModelsResponse, serde_json::Error> {
    serde_json::from_str(include_str!("../models.json"))
}

pub use codex_protocol::client_version::TRACKED_UPSTREAM_CLIENT_VERSION;

/// The whole client version reported for catalog requests and cache entries.
///
/// See [`codex_protocol::client_version`] for why it is not the `0.0.0`
/// development placeholder.
pub fn client_version_to_whole() -> String {
    codex_protocol::client_version::reported_client_version().to_string()
}

#[cfg(test)]
#[path = "client_version_tests.rs"]
mod client_version_tests;
