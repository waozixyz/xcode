//! Amazon Bedrock authentication is not part of this build.
//!
//! The AWS SDK credential chain and SigV4 signing were removed. The types
//! remain so provider selection keeps its shape: no AWS profile is ever
//! discovered, and loading or signing reports that Bedrock is unavailable.

use std::sync::Arc;

use bytes::Bytes;
use http::HeaderMap;
use http::Method;
use thiserror::Error;

/// AWS auth configuration used to resolve credentials and sign requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsAuthConfig {
    pub profile: Option<String>,
    pub region: Option<String>,
    pub service: String,
}

/// Static AWS access keys supplied by a caller.
#[derive(Clone, PartialEq, Eq)]
pub struct AwsAccessKeys {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: Option<String>,
}

impl std::fmt::Debug for AwsAccessKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AwsAccessKeys")
            .field("access_key_id", &"<redacted>")
            .field("secret_access_key", &"<redacted>")
            .field(
                "session_token",
                &self.session_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Supplies AWS access keys on demand.
pub trait AwsCredentialsProvider: std::fmt::Debug + Send + Sync {
    fn credentials(
        &self,
    ) -> impl std::future::Future<Output = std::io::Result<AwsAccessKeys>> + Send;
}

/// Generic HTTP request shape consumed by signing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsRequestToSign {
    pub method: Method,
    pub url: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// Signed request parts returned to the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsSignedRequest {
    pub url: String,
    pub headers: HeaderMap,
}

/// The only outcome of credential loading or signing in this build.
#[derive(Debug, Error)]
pub enum AwsAuthError {
    #[error("Amazon Bedrock authentication is not included in this build")]
    Unsupported,
}

impl AwsAuthError {
    pub fn credentials_provider_error(&self) -> Option<&std::io::Error> {
        None
    }

    pub fn is_retryable(&self) -> bool {
        false
    }
}

/// An AWS auth context. None can be loaded in this build.
#[derive(Debug, Clone)]
pub struct AwsAuthContext {
    region: String,
    service: String,
}

impl AwsAuthContext {
    pub async fn load(
        _config: AwsAuthConfig,
        _factory: codex_http_client::HttpClientFactory,
    ) -> Result<Self, AwsAuthError> {
        Err(AwsAuthError::Unsupported)
    }

    pub async fn load_with_access_keys(
        _config: AwsAuthConfig,
        _access_keys: AwsAccessKeys,
        _factory: codex_http_client::HttpClientFactory,
    ) -> Result<Self, AwsAuthError> {
        Err(AwsAuthError::Unsupported)
    }

    pub async fn load_with_credentials_provider(
        _config: AwsAuthConfig,
        _provider: Arc<impl AwsCredentialsProvider + 'static>,
        _factory: codex_http_client::HttpClientFactory,
    ) -> Result<Self, AwsAuthError> {
        Err(AwsAuthError::Unsupported)
    }

    pub async fn load_profile(
        _config: AwsAuthConfig,
        _factory: codex_http_client::HttpClientFactory,
    ) -> Result<Self, AwsAuthError> {
        Err(AwsAuthError::Unsupported)
    }

    pub fn region(&self) -> &str {
        &self.region
    }

    pub fn service(&self) -> &str {
        &self.service
    }

    pub async fn sign(&self, _request: AwsRequestToSign) -> Result<AwsSignedRequest, AwsAuthError> {
        Err(AwsAuthError::Unsupported)
    }
}

/// A named AWS profile and the region configured directly on that profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwsProfile {
    pub name: String,
    pub region: Option<String>,
}

/// No AWS profiles are discovered in this build.
pub async fn discover_aws_profiles() -> Result<Vec<AwsProfile>, AwsAuthError> {
    Ok(Vec::new())
}

pub async fn validate_aws_profile(
    _profile: &str,
    _region: &str,
    _factory: codex_http_client::HttpClientFactory,
) -> Result<(), AwsAuthError> {
    Err(AwsAuthError::Unsupported)
}
