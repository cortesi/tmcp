use std::collections::HashMap;

use http::{
    HeaderMap,
    header::{CONTENT_TYPE, WWW_AUTHENTICATE},
};
use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use url::Url;

use super::dynamic_registration::ClientMetadata;
use crate::error::Error;

/// Well-known path segment for protected resource metadata discovery.
const WELL_KNOWN_PROTECTED_RESOURCE: &str = "oauth-protected-resource";
/// Well-known path segment for OAuth authorization server metadata discovery.
const WELL_KNOWN_AUTHORIZATION_SERVER: &str = "oauth-authorization-server";
/// Well-known path segment for OpenID Connect discovery metadata.
const WELL_KNOWN_OPENID_CONFIGURATION: &str = "openid-configuration";

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Protected resource metadata as defined by RFC 9728.
pub struct ProtectedResourceMetadata {
    /// Resource identifier for the protected resource.
    pub resource: String,
    /// Authorization server issuer URLs that can issue tokens for this resource.
    pub authorization_servers: Vec<String>,
    /// Optional list of supported scopes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes_supported: Option<Vec<String>>,
    /// Optional list of supported bearer token methods.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_methods_supported: Option<Vec<String>>,
    /// Optional URL to resource documentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_documentation: Option<String>,
    /// Additional metadata fields.
    #[serde(flatten)]
    pub additional: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
/// Authorization server metadata from RFC 8414 or OpenID Connect discovery.
pub struct AuthorizationServerMetadata {
    /// Issuer identifier for the authorization server.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issuer: Option<String>,
    /// Authorization endpoint URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization_endpoint: Option<String>,
    /// Token endpoint URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_endpoint: Option<String>,
    /// Dynamic client registration endpoint URL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,
    /// Supported scopes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes_supported: Option<Vec<String>>,
    /// Supported response types.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_types_supported: Option<Vec<String>>,
    /// Supported grant types.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_types_supported: Option<Vec<String>>,
    /// Supported token endpoint authentication methods.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,
    /// Whether Client ID metadata documents are supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_id_metadata_document_supported: Option<bool>,
    /// Additional metadata fields.
    #[serde(flatten)]
    pub additional: HashMap<String, Value>,
}

#[derive(Debug, Clone)]
/// Discovery result for protected resource metadata.
pub struct ProtectedResourceDiscovery {
    /// Parsed protected resource metadata document.
    pub metadata: ProtectedResourceMetadata,
    /// URL used to retrieve the metadata document.
    pub metadata_url: Url,
    /// Scopes indicated in a WWW-Authenticate challenge, if present.
    pub challenge_scopes: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
/// Discovery result for authorization servers associated with a resource.
pub struct AuthorizationServerDiscovery {
    /// Metadata for the protected resource.
    pub resource_metadata: ProtectedResourceMetadata,
    /// URL used to retrieve the protected resource metadata.
    pub resource_metadata_url: Url,
    /// Scopes indicated in a WWW-Authenticate challenge, if present.
    pub challenge_scopes: Option<Vec<String>>,
    /// Metadata documents for each discovered authorization server.
    pub authorization_servers: Vec<AuthorizationServerMetadata>,
}

#[derive(Debug, Clone)]
/// Client metadata document resolved from an HTTPS client id.
pub struct ClientIdMetadataDocument {
    /// Client metadata resolved from the client id URL.
    pub metadata: ClientMetadata,
    /// URL used to retrieve the metadata document.
    pub metadata_url: Url,
}

#[derive(Debug, Clone)]
/// Parsed resource metadata challenge information.
struct ResourceMetadataChallenge {
    /// Metadata URL provided by the challenge.
    metadata_url: Url,
    /// Optional scope list provided by the challenge.
    scopes: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
/// Parsed WWW-Authenticate challenge data.
struct AuthChallenge {
    /// Authentication scheme name.
    scheme: String,
    /// Parsed key/value parameters for the scheme.
    params: HashMap<String, String>,
}

/// OAuth authorization discovery helper.
pub struct AuthorizationDiscoveryClient {
    /// HTTP client used to fetch metadata documents.
    http_client: Client,
}

impl AuthorizationDiscoveryClient {
    /// Create a new discovery client with a default HTTP client.
    pub fn new() -> Self {
        Self {
            http_client: Client::new(),
        }
    }

    /// Create a new discovery client using the provided HTTP client.
    pub fn with_http_client(http_client: Client) -> Self {
        Self { http_client }
    }

    /// Discover protected resource metadata for the provided resource URL.
    pub async fn discover_protected_resource_metadata(
        &self,
        resource: &str,
        www_authenticate: Option<&HeaderMap>,
    ) -> Result<ProtectedResourceDiscovery, Error> {
        let resource_url = parse_http_url(resource, "resource URL")?;
        let challenge = if let Some(headers) = www_authenticate {
            parse_resource_metadata_challenge(headers, &resource_url)?
        } else {
            None
        };

        let mut errors = Vec::new();
        if let Some(challenge) = challenge {
            match fetch_protected_resource_metadata(
                &self.http_client,
                &challenge.metadata_url,
                &resource_url,
            )
            .await
            {
                Ok(metadata) => {
                    return Ok(ProtectedResourceDiscovery {
                        metadata,
                        metadata_url: challenge.metadata_url,
                        challenge_scopes: challenge.scopes,
                    });
                }
                Err(err) => errors.push(err),
            }
        }

        let fallback_urls = protected_resource_metadata_urls(&resource_url);
        for metadata_url in fallback_urls {
            match fetch_protected_resource_metadata(&self.http_client, &metadata_url, &resource_url)
                .await
            {
                Ok(metadata) => {
                    return Ok(ProtectedResourceDiscovery {
                        metadata,
                        metadata_url,
                        challenge_scopes: None,
                    });
                }
                Err(err) => errors.push(err),
            }
        }

        Err(Error::AuthorizationFailed(format!(
            "Protected resource metadata discovery failed: {}",
            format_errors(&errors)
        )))
    }

    /// Discover authorization server metadata for all servers advertised by a resource.
    pub async fn discover_authorization_servers(
        &self,
        resource: &str,
        www_authenticate: Option<&HeaderMap>,
    ) -> Result<AuthorizationServerDiscovery, Error> {
        let discovery = self
            .discover_protected_resource_metadata(resource, www_authenticate)
            .await?;

        if discovery.metadata.authorization_servers.is_empty() {
            return Err(Error::AuthorizationFailed(
                "Protected resource metadata missing authorization_servers".to_string(),
            ));
        }

        let resource_url = parse_http_url(resource, "resource URL")?;
        let mut authorization_servers = Vec::new();
        for issuer in &discovery.metadata.authorization_servers {
            let issuer_url = resolve_relative_url(&resource_url, issuer)?;
            let metadata = self
                .discover_authorization_server_metadata(issuer_url.as_str())
                .await?;
            authorization_servers.push(metadata);
        }

        Ok(AuthorizationServerDiscovery {
            resource_metadata: discovery.metadata,
            resource_metadata_url: discovery.metadata_url,
            challenge_scopes: discovery.challenge_scopes,
            authorization_servers,
        })
    }

    /// Discover authorization server metadata for the provided issuer URL.
    pub async fn discover_authorization_server_metadata(
        &self,
        issuer: &str,
    ) -> Result<AuthorizationServerMetadata, Error> {
        let issuer_url = parse_http_url(issuer, "authorization server issuer")?;
        let candidate_urls = authorization_server_metadata_urls(&issuer_url);

        let mut errors = Vec::new();
        for metadata_url in candidate_urls {
            match fetch_authorization_server_metadata(&self.http_client, &metadata_url, &issuer_url)
                .await
            {
                Ok(metadata) => return Ok(metadata),
                Err(err) => errors.push(err),
            }
        }

        Err(Error::AuthorizationFailed(format!(
            "Authorization server metadata discovery failed: {}",
            format_errors(&errors)
        )))
    }

    /// Fetch the client metadata document for an HTTPS client id.
    pub async fn fetch_client_id_metadata_document(
        &self,
        client_id: &str,
    ) -> Result<ClientIdMetadataDocument, Error> {
        let client_id_url = parse_https_url(client_id, "client id metadata document")?;
        let metadata = fetch_json::<ClientMetadata>(&self.http_client, &client_id_url).await?;
        validate_client_id_metadata_document(&client_id_url, &metadata)?;
        Ok(ClientIdMetadataDocument {
            metadata,
            metadata_url: client_id_url,
        })
    }
}

impl Default for AuthorizationDiscoveryClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse an HTTP or HTTPS URL with a clear validation error.
fn parse_http_url(value: &str, context: &str) -> Result<Url, Error> {
    let url = Url::parse(value)
        .map_err(|e| Error::InvalidConfiguration(format!("Invalid {context}: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::InvalidConfiguration(format!(
            "Invalid {context}: unsupported scheme",
        )));
    }
    Ok(url)
}

/// Parse an HTTPS URL with a clear validation error.
fn parse_https_url(value: &str, context: &str) -> Result<Url, Error> {
    let url = Url::parse(value)
        .map_err(|e| Error::InvalidConfiguration(format!("Invalid {context}: {e}")))?;
    if url.scheme() != "https" {
        return Err(Error::InvalidConfiguration(format!(
            "Invalid {context}: HTTPS required",
        )));
    }
    Ok(url)
}

/// Resolve a URL that may be relative to a base.
fn resolve_relative_url(base: &Url, value: &str) -> Result<Url, Error> {
    if let Ok(url) = Url::parse(value) {
        return Ok(url);
    }
    base.join(value)
        .map_err(|e| Error::InvalidConfiguration(format!("Invalid authorization server URL: {e}")))
}

/// Fetch protected resource metadata and validate its contents.
async fn fetch_protected_resource_metadata(
    client: &Client,
    metadata_url: &Url,
    resource_url: &Url,
) -> Result<ProtectedResourceMetadata, Error> {
    let metadata = fetch_json::<ProtectedResourceMetadata>(client, metadata_url).await?;
    validate_protected_resource_metadata(resource_url, &metadata)?;
    Ok(metadata)
}

/// Fetch authorization server metadata and validate its contents.
async fn fetch_authorization_server_metadata(
    client: &Client,
    metadata_url: &Url,
    issuer_url: &Url,
) -> Result<AuthorizationServerMetadata, Error> {
    let metadata = fetch_json::<AuthorizationServerMetadata>(client, metadata_url).await?;
    validate_authorization_server_metadata(issuer_url, &metadata)?;
    Ok(metadata)
}

/// Fetch JSON from a URL and deserialize it as the requested type.
async fn fetch_json<T: DeserializeOwned>(client: &Client, url: &Url) -> Result<T, Error> {
    let response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|e| Error::Transport(format!("Failed to fetch metadata: {e}")))?;

    let status = response.status();
    if !status.is_success() {
        return Err(Error::AuthorizationFailed(format!(
            "Metadata request failed with status: {status}",
        )));
    }

    ensure_json_content_type(response.headers())?;

    response
        .json::<T>()
        .await
        .map_err(|e| Error::InvalidConfiguration(format!("Invalid metadata JSON: {e}")))
}

/// Ensure the response includes an application/json Content-Type.
fn ensure_json_content_type(headers: &HeaderMap) -> Result<(), Error> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            Error::AuthorizationFailed("Metadata response missing Content-Type".to_string())
        })?;

    if !content_type
        .to_ascii_lowercase()
        .starts_with("application/json")
    {
        return Err(Error::AuthorizationFailed(
            "Metadata response must be application/json".to_string(),
        ));
    }

    Ok(())
}

/// Validate required protected resource metadata fields.
fn validate_protected_resource_metadata(
    resource_url: &Url,
    metadata: &ProtectedResourceMetadata,
) -> Result<(), Error> {
    if metadata.authorization_servers.is_empty() {
        return Err(Error::AuthorizationFailed(
            "Protected resource metadata missing authorization_servers".to_string(),
        ));
    }

    let metadata_resource = Url::parse(&metadata.resource).map_err(|e| {
        Error::AuthorizationFailed(format!(
            "Protected resource metadata has invalid resource URL: {e}",
        ))
    })?;

    if !urls_match(resource_url, &metadata_resource) {
        return Err(Error::AuthorizationFailed(
            "Protected resource metadata resource mismatch".to_string(),
        ));
    }

    Ok(())
}

/// Validate required authorization server metadata fields.
fn validate_authorization_server_metadata(
    issuer_url: &Url,
    metadata: &AuthorizationServerMetadata,
) -> Result<(), Error> {
    let issuer = metadata.issuer.as_ref().ok_or_else(|| {
        Error::AuthorizationFailed("Authorization server metadata missing issuer".to_string())
    })?;

    let issuer_value = Url::parse(issuer).map_err(|e| {
        Error::AuthorizationFailed(format!(
            "Authorization server metadata has invalid issuer: {e}",
        ))
    })?;

    if !urls_match(issuer_url, &issuer_value) {
        return Err(Error::AuthorizationFailed(
            "Authorization server metadata issuer mismatch".to_string(),
        ));
    }

    Ok(())
}

/// Validate required fields within a client id metadata document.
fn validate_client_id_metadata_document(
    metadata_url: &Url,
    metadata: &ClientMetadata,
) -> Result<(), Error> {
    let redirect_uris = metadata.redirect_uris.as_ref().ok_or_else(|| {
        Error::AuthorizationFailed("Client metadata document missing redirect_uris".to_string())
    })?;

    if redirect_uris.is_empty() {
        return Err(Error::AuthorizationFailed(
            "Client metadata document redirect_uris is empty".to_string(),
        ));
    }

    for redirect_uri in redirect_uris {
        parse_http_url(redirect_uri, "redirect URI").map_err(|e| {
            Error::AuthorizationFailed(format!(
                "Client metadata document has invalid redirect URI: {e}",
            ))
        })?;
    }

    if metadata_url.scheme() != "https" {
        return Err(Error::AuthorizationFailed(
            "Client metadata document must be served over HTTPS".to_string(),
        ));
    }

    Ok(())
}

/// Compare two URLs ignoring trailing slashes.
fn urls_match(left: &Url, right: &Url) -> bool {
    left.as_str().trim_end_matches('/') == right.as_str().trim_end_matches('/')
}

/// Parse a resource metadata challenge from WWW-Authenticate headers.
fn parse_resource_metadata_challenge(
    headers: &HeaderMap,
    resource_url: &Url,
) -> Result<Option<ResourceMetadataChallenge>, Error> {
    for header_value in headers.get_all(WWW_AUTHENTICATE) {
        let Ok(value) = header_value.to_str() else {
            continue;
        };
        for challenge in parse_www_authenticate(value) {
            if !challenge.scheme.eq_ignore_ascii_case("bearer") {
                continue;
            }
            if let Some(url_value) = challenge.params.get("resource_metadata") {
                let metadata_url = resolve_relative_url(resource_url, url_value)?;
                let scopes = challenge
                    .params
                    .get("scope")
                    .map(|scope| parse_scopes(scope));
                return Ok(Some(ResourceMetadataChallenge {
                    metadata_url,
                    scopes,
                }));
            }
        }
    }

    Ok(None)
}

/// Split a scope string into individual scope values.
fn parse_scopes(scopes: &str) -> Vec<String> {
    scopes
        .split_whitespace()
        .filter(|scope| !scope.is_empty())
        .map(String::from)
        .collect()
}

/// Build candidate protected resource metadata URLs from a resource URL.
fn protected_resource_metadata_urls(resource_url: &Url) -> Vec<Url> {
    let mut urls = Vec::new();
    let path_suffix = resource_path_suffix(resource_url);
    let mut path_url = resource_url.clone();
    let mut path = String::from("/.well-known/");
    path.push_str(WELL_KNOWN_PROTECTED_RESOURCE);
    if let Some(path_suffix) = path_suffix.as_deref() {
        path.push('/');
        path.push_str(path_suffix);
    }
    path_url.set_path(&path);
    path_url.set_query(resource_url.query());
    path_url.set_fragment(None);
    urls.push(path_url);

    let mut root_url = resource_url.clone();
    let mut root_path = String::from("/.well-known/");
    root_path.push_str(WELL_KNOWN_PROTECTED_RESOURCE);
    root_url.set_path(&root_path);
    root_url.set_query(None);
    root_url.set_fragment(None);
    if root_url.as_str() != urls[0].as_str() {
        urls.push(root_url);
    }

    urls
}

/// Build candidate authorization server metadata URLs from an issuer URL.
fn authorization_server_metadata_urls(issuer_url: &Url) -> Vec<Url> {
    let mut urls = Vec::new();
    let path_suffix = resource_path_suffix(issuer_url);

    let oauth_insert = well_known_insert_url(
        issuer_url,
        WELL_KNOWN_AUTHORIZATION_SERVER,
        path_suffix.as_deref(),
    );
    urls.push(oauth_insert);

    let openid_insert = well_known_insert_url(
        issuer_url,
        WELL_KNOWN_OPENID_CONFIGURATION,
        path_suffix.as_deref(),
    );
    urls.push(openid_insert);

    if path_suffix.is_some() {
        let openid_append = well_known_append_url(issuer_url, WELL_KNOWN_OPENID_CONFIGURATION);
        urls.push(openid_append);
    }

    urls
}

/// Return the path portion of a URL without leading or trailing slashes.
fn resource_path_suffix(url: &Url) -> Option<String> {
    let path = url.path().trim_matches('/');
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Insert a well-known suffix before the path, optionally adding a path suffix.
fn well_known_insert_url(base: &Url, suffix: &str, path_suffix: Option<&str>) -> Url {
    let mut url = base.clone();
    let mut path = String::from("/.well-known/");
    path.push_str(suffix);
    if let Some(path_suffix) = path_suffix {
        path.push('/');
        path.push_str(path_suffix);
    }
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    url
}

/// Append a well-known suffix to an existing path.
fn well_known_append_url(base: &Url, suffix: &str) -> Url {
    let mut url = base.clone();
    let mut path = base.path().trim_end_matches('/').to_string();
    if path.is_empty() {
        path = String::new();
    }
    if !path.starts_with('/') {
        path.insert(0, '/');
    }
    if !path.ends_with('/') {
        path.push('/');
    }
    path.push_str(".well-known/");
    path.push_str(suffix);
    url.set_path(&path);
    url.set_query(None);
    url.set_fragment(None);
    url
}

/// Parse a WWW-Authenticate header value into challenges.
fn parse_www_authenticate(value: &str) -> Vec<AuthChallenge> {
    let segments = split_authenticate_segments(value);
    let mut challenges = Vec::new();
    let mut current: Option<AuthChallenge> = None;

    for segment in segments {
        let segment = segment.trim();
        if segment.is_empty() {
            continue;
        }

        if let Some((scheme, rest)) = split_scheme_segment(segment) {
            if let Some(challenge) = current.take() {
                challenges.push(challenge);
            }
            let mut params = HashMap::new();
            if let Some((key, value)) = parse_param(rest) {
                params.insert(key, value);
            }
            current = Some(AuthChallenge {
                scheme: scheme.to_string(),
                params,
            });
            continue;
        }

        if let Some(challenge) = current.as_mut()
            && let Some((key, value)) = parse_param(segment)
        {
            challenge.params.insert(key, value);
        }
    }

    if let Some(challenge) = current {
        challenges.push(challenge);
    }

    challenges
}

/// Split a WWW-Authenticate value into comma-delimited segments.
fn split_authenticate_segments(value: &str) -> Vec<&str> {
    let mut segments = Vec::new();
    let mut in_quotes = false;
    let mut escape = false;
    let mut start = 0;

    for (idx, ch) in value.char_indices() {
        if escape {
            escape = false;
            continue;
        }

        match ch {
            '\\' if in_quotes => {
                escape = true;
            }
            '"' => {
                in_quotes = !in_quotes;
            }
            ',' if !in_quotes => {
                segments.push(&value[start..idx]);
                start = idx + 1;
            }
            _ => {}
        }
    }

    segments.push(&value[start..]);
    segments
}

/// Split the scheme token from the first parameter segment.
fn split_scheme_segment(segment: &str) -> Option<(&str, &str)> {
    let trimmed = segment.trim_start();
    let mut end = None;
    for (idx, ch) in trimmed.char_indices() {
        if ch.is_whitespace() {
            end = Some(idx);
            break;
        }
        if ch == '=' {
            return None;
        }
    }

    let end = end?;
    let scheme = trimmed[..end].trim();
    let rest = trimmed[end..].trim_start();
    if rest.starts_with('=') || scheme.is_empty() {
        return None;
    }

    Some((scheme, rest))
}

/// Parse a key/value parameter from an auth challenge segment.
fn parse_param(segment: &str) -> Option<(String, String)> {
    let trimmed = segment.trim();
    let mut parts = trimmed.splitn(2, '=');
    let key = parts.next()?.trim();
    let value = parts.next()?.trim();

    if key.is_empty() || value.is_empty() {
        return None;
    }

    let value = parse_param_value(value);
    Some((key.to_ascii_lowercase(), value))
}

/// Parse a parameter value, handling quoted strings and escapes.
fn parse_param_value(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(without_quote) = trimmed.strip_prefix('"') {
        let mut result = String::new();
        let mut escape = false;
        for ch in without_quote.chars() {
            if escape {
                result.push(ch);
                escape = false;
                continue;
            }
            match ch {
                '\\' => escape = true,
                '"' => break,
                _ => result.push(ch),
            }
        }
        result
    } else {
        trimmed
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    }
}

/// Join error messages for display.
fn format_errors(errors: &[Error]) -> String {
    if errors.is_empty() {
        return "unknown error".to_string();
    }
    errors
        .iter()
        .map(|err| err.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use axum::{Json, Router, routing::get};
    use tokio::net::TcpListener;

    use super::*;

    fn metadata_response(resource: &str, issuer: &str) -> ProtectedResourceMetadata {
        ProtectedResourceMetadata {
            resource: resource.to_string(),
            authorization_servers: vec![issuer.to_string()],
            scopes_supported: None,
            bearer_methods_supported: None,
            resource_documentation: None,
            additional: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_discover_protected_resource_metadata_www_authenticate() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let resource_url = format!("http://{addr}/mcp");
        let metadata_url = format!("http://{addr}/.well-known/oauth-protected-resource/mcp");
        let issuer_url = format!("http://{addr}/auth");
        let resource_url_clone = resource_url.clone();
        let issuer_url_clone = issuer_url.clone();

        let router = Router::new().route(
            "/.well-known/oauth-protected-resource/mcp",
            get(|| async move { Json(metadata_response(&resource_url_clone, &issuer_url_clone)) }),
        );

        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let mut headers = HeaderMap::new();
        headers.insert(
            WWW_AUTHENTICATE,
            format!("Bearer resource_metadata=\"{metadata_url}\", scope=\"files:read\"")
                .parse()
                .unwrap(),
        );

        let client = AuthorizationDiscoveryClient::new();
        let discovery = client
            .discover_protected_resource_metadata(&resource_url, Some(&headers))
            .await
            .unwrap();

        assert_eq!(discovery.metadata.resource, resource_url);
        assert_eq!(discovery.metadata_url.as_str(), metadata_url);
        assert_eq!(
            discovery.challenge_scopes,
            Some(vec!["files:read".to_string()])
        );
    }

    #[tokio::test]
    async fn test_discover_protected_resource_metadata_fallback() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let resource_url = format!("http://{addr}/mcp");
        let issuer_url = format!("http://{addr}/auth");
        let root_metadata = format!("http://{addr}/.well-known/oauth-protected-resource");
        let resource_url_clone = resource_url.clone();
        let issuer_url_clone = issuer_url.clone();

        let router = Router::new().route(
            "/.well-known/oauth-protected-resource",
            get(|| async move { Json(metadata_response(&resource_url_clone, &issuer_url_clone)) }),
        );

        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let client = AuthorizationDiscoveryClient::new();
        let discovery = client
            .discover_protected_resource_metadata(&resource_url, None)
            .await
            .unwrap();

        assert_eq!(discovery.metadata_url.as_str(), root_metadata);
    }

    #[test]
    fn test_authorization_server_metadata_urls_with_path() {
        let issuer = Url::parse("https://auth.example.com/tenant1").unwrap();
        let urls = authorization_server_metadata_urls(&issuer);

        assert_eq!(
            urls.iter().map(Url::as_str).collect::<Vec<_>>(),
            vec![
                "https://auth.example.com/.well-known/oauth-authorization-server/tenant1",
                "https://auth.example.com/.well-known/openid-configuration/tenant1",
                "https://auth.example.com/tenant1/.well-known/openid-configuration",
            ]
        );
    }

    #[test]
    fn test_protected_resource_metadata_urls_with_path() {
        let resource = Url::parse("https://example.com/public/mcp").unwrap();
        let urls = protected_resource_metadata_urls(&resource);

        assert_eq!(
            urls.iter().map(Url::as_str).collect::<Vec<_>>(),
            vec![
                "https://example.com/.well-known/oauth-protected-resource/public/mcp",
                "https://example.com/.well-known/oauth-protected-resource",
            ]
        );
    }

    #[test]
    fn test_validate_client_metadata_document_requires_redirects() {
        let metadata = ClientMetadata::new("Client", "http://localhost/callback");
        let url = Url::parse("https://client.example.com/metadata").unwrap();
        assert!(validate_client_id_metadata_document(&url, &metadata).is_ok());
    }

    #[test]
    fn test_validate_client_metadata_document_missing_redirects() {
        let mut metadata = ClientMetadata::new("Client", "http://localhost/callback");
        metadata.redirect_uris = None;
        let url = Url::parse("https://client.example.com/metadata").unwrap();
        let err = validate_client_id_metadata_document(&url, &metadata).unwrap_err();
        assert!(
            err.to_string()
                .contains("Client metadata document missing redirect_uris")
        );
    }

    #[test]
    fn test_parse_https_url_rejects_http() {
        let err = parse_https_url("http://example.com/metadata", "client").unwrap_err();
        assert!(err.to_string().contains("HTTPS required"));
    }
}
