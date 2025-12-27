use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// Client metadata for dynamic registration as per RFC7591
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientMetadata {
    /// Array of redirection URIs for use in redirect-based flows
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_uris: Option<Vec<String>>,

    /// Human-readable name of the client
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,

    /// URL of the home page of the client
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_uri: Option<String>,

    /// URL that references a logo for the client
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<String>,

    /// OAuth 2.0 scope values that the client can use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,

    /// OAuth 2.0 grant types the client can use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_types: Option<Vec<String>>,

    /// OAuth 2.0 response types the client can use
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_types: Option<Vec<String>>,

    /// Authentication method for the token endpoint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_method: Option<String>,

    /// Contacts for the client (usually email addresses)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contacts: Option<Vec<String>>,

    /// Software ID (UUID) for the client software
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software_id: Option<String>,

    /// Version of the client software
    #[serde(skip_serializing_if = "Option::is_none")]
    pub software_version: Option<String>,

    /// URL for the client's terms of service
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tos_uri: Option<String>,

    /// URL for the client's privacy policy
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_uri: Option<String>,

    /// MCP-specific: resource parameter for audience binding
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource: Option<String>,

    /// Additional metadata fields
    #[serde(flatten)]
    pub additional: HashMap<String, serde_json::Value>,
}

impl ClientMetadata {
    /// Create a new ClientMetadata with minimal required fields for MCP
    pub fn new(client_name: impl Into<String>, redirect_uri: impl Into<String>) -> Self {
        Self {
            redirect_uris: Some(vec![redirect_uri.into()]),
            client_name: Some(client_name.into()),
            client_uri: None,
            logo_uri: None,
            scope: None,
            grant_types: Some(vec!["authorization_code".to_string()]),
            response_types: Some(vec!["code".to_string()]),
            token_endpoint_auth_method: Some("client_secret_basic".to_string()),
            contacts: None,
            software_id: None,
            software_version: None,
            tos_uri: None,
            policy_uri: None,
            resource: None,
            additional: HashMap::new(),
        }
    }

    /// Set the resource parameter for MCP
    pub fn with_resource(mut self, resource: impl Into<String>) -> Self {
        self.resource = Some(resource.into());
        self
    }

    /// Set the scopes
    pub fn with_scopes(mut self, scopes: &[String]) -> Self {
        self.scope = Some(scopes.join(" "));
        self
    }

    /// Set the client URI
    pub fn with_client_uri(mut self, uri: impl Into<String>) -> Self {
        self.client_uri = Some(uri.into());
        self
    }

    /// Set the contacts
    pub fn with_contacts(mut self, contacts: Vec<String>) -> Self {
        self.contacts = Some(contacts);
        self
    }

    /// Set the software information
    pub fn with_software_info(
        mut self,
        software_id: impl Into<String>,
        software_version: impl Into<String>,
    ) -> Self {
        self.software_id = Some(software_id.into());
        self.software_version = Some(software_version.into());
        self
    }

    /// Set the token endpoint auth method
    pub fn with_token_endpoint_auth_method(mut self, method: impl Into<String>) -> Self {
        self.token_endpoint_auth_method = Some(method.into());
        self
    }
}

/// Response from dynamic client registration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientRegistrationResponse {
    /// The client identifier
    pub client_id: String,

    /// The client secret (if issued)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,

    /// Time at which the client identifier was issued (seconds since epoch)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id_issued_at: Option<u64>,

    /// Time at which the client secret will expire (0 = no expiration)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret_expires_at: Option<u64>,

    /// All registered metadata about the client
    #[serde(flatten)]
    pub metadata: ClientMetadata,
}

/// Error response from registration endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrationError {
    /// Error code
    pub error: String,

    /// Human-readable error description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_description: Option<String>,
}

/// Dynamic client registration client
pub struct DynamicRegistrationClient {
    /// HTTP client used for registration requests.
    http_client: reqwest::Client,
}

impl Default for DynamicRegistrationClient {
    fn default() -> Self {
        Self {
            http_client: reqwest::Client::new(),
        }
    }
}

impl DynamicRegistrationClient {
    /// Create a new dynamic registration client
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a client with the authorization server
    pub async fn register(
        &self,
        registration_endpoint: &str,
        metadata: ClientMetadata,
        access_token: Option<&str>,
    ) -> Result<ClientRegistrationResponse, Error> {
        let mut request = self
            .http_client
            .post(registration_endpoint)
            .json(&metadata)
            .header("Content-Type", "application/json");

        // Add authorization header if provided (for protected registration endpoints)
        if let Some(token) = access_token {
            request = request.header("Authorization", format!("Bearer {token}"));
        }

        let response = request
            .send()
            .await
            .map_err(|e| Error::Transport(format!("Failed to send registration request: {e}")))?;

        let status = response.status();

        if status.is_success() {
            let registration_response = response
                .json::<ClientRegistrationResponse>()
                .await
                .map_err(|e| {
                    Error::InvalidConfiguration(format!(
                        "Failed to parse registration response: {e}"
                    ))
                })?;

            Ok(registration_response)
        } else {
            // Try to parse error response
            match response.json::<RegistrationError>().await {
                Ok(error) => Err(Error::AuthorizationFailed(format!(
                    "Registration failed: {} - {}",
                    error.error,
                    error.error_description.unwrap_or_default()
                ))),
                Err(_) => Err(Error::AuthorizationFailed(format!(
                    "Registration failed with status: {status}"
                ))),
            }
        }
    }

    /// Discover the registration endpoint from OAuth metadata
    pub async fn discover_registration_endpoint(
        &self,
        issuer_url: &str,
    ) -> Result<Option<String>, Error> {
        // OAuth 2.0 Authorization Server Metadata (RFC8414)
        let metadata_url = if issuer_url.ends_with('/') {
            format!("{issuer_url}.well-known/oauth-authorization-server")
        } else {
            format!("{issuer_url}/.well-known/oauth-authorization-server")
        };

        let response = self
            .http_client
            .get(&metadata_url)
            .send()
            .await
            .map_err(|e| Error::Transport(format!("Failed to fetch OAuth metadata: {e}")))?;

        let status = response.status();
        if !status.is_success() {
            return Err(Error::AuthorizationFailed(format!(
                "Metadata request failed with status: {status}"
            )));
        }

        let metadata: serde_json::Value = response.json().await.map_err(|e| {
            Error::InvalidConfiguration(format!("Failed to parse OAuth metadata: {e}"))
        })?;

        Ok(metadata
            .get("registration_endpoint")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()))
    }
}

use super::oauth_client::OAuth2Config;

impl OAuth2Config {
    /// Create OAuth2Config from dynamic registration response
    pub fn from_registration(
        registration: ClientRegistrationResponse,
        auth_url: String,
        token_url: String,
        resource: String,
    ) -> Self {
        Self {
            client_id: registration.client_id,
            client_secret: registration.client_secret,
            auth_url,
            token_url,
            redirect_url: registration
                .metadata
                .redirect_uris
                .and_then(|uris| uris.first().cloned())
                .unwrap_or_else(|| "http://localhost:8080/callback".to_string()),
            resource,
            scopes: registration
                .metadata
                .scope
                .map(|s| s.split_whitespace().map(String::from).collect())
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_client_metadata_creation() {
        let metadata = ClientMetadata::new("Test Client", "http://localhost:8080/callback")
            .with_resource("https://example.com/api")
            .with_scopes(&["read".to_string(), "write".to_string()])
            .with_contacts(vec!["admin@example.com".to_string()]);

        assert_eq!(metadata.client_name, Some("Test Client".to_string()));
        assert_eq!(
            metadata.redirect_uris,
            Some(vec!["http://localhost:8080/callback".to_string()])
        );
        assert_eq!(
            metadata.resource,
            Some("https://example.com/api".to_string())
        );
        assert_eq!(metadata.scope, Some("read write".to_string()));
        assert_eq!(
            metadata.grant_types,
            Some(vec!["authorization_code".to_string()])
        );
    }

    #[test]
    fn test_client_metadata_serialization() {
        let metadata = ClientMetadata::new("Test Client", "http://localhost:8080/callback");

        let json = serde_json::to_string(&metadata).unwrap();
        assert!(json.contains("\"client_name\":\"Test Client\""));
        assert!(json.contains("\"redirect_uris\":[\"http://localhost:8080/callback\"]"));
        assert!(json.contains("\"grant_types\":[\"authorization_code\"]"));
    }
}
