use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    errors::{Error as JwtError, ErrorKind},
    jwk::{Jwk, JwkSet},
};
use reqwest::Client;
use serde_json::Value;
use tokio::sync::Mutex;

use super::{
    AuthError, AuthInfo, DEFAULT_JWKS_CACHE_TTL, TokenValidator, extra_claims, parse_scope_set,
};

/// Validates JWT bearer tokens using a local JWK set or a remote JWKS endpoint.
pub struct JwtValidator {
    /// Expected issuer.
    issuer: String,
    /// Accepted audiences.
    audiences: Vec<String>,
    /// JWK source.
    source: JwkSource,
    /// Cached JWKS lifetime.
    cache_ttl: Duration,
    /// Minimum interval between forced refreshes caused by unknown key ids.
    unknown_kid_refresh_cooldown: Duration,
    /// HTTP client for JWKS fetches.
    client: Client,
    /// Cached key set.
    cache: Mutex<Option<CachedJwkSet>>,
    /// Serializes refreshes so concurrent misses do not stampede.
    refresh_lock: Mutex<()>,
}

/// Source for verification keys.
enum JwkSource {
    /// Fetch keys from a remote JWKS endpoint.
    Remote(String),
    /// Use a fixed in-memory key set.
    Static(JwkSet),
}

/// Cached key set state.
struct CachedJwkSet {
    /// Time the set was fetched.
    fetched_at: Instant,
    /// Cached keys.
    jwk_set: JwkSet,
    /// Last time an unknown `kid` triggered a forced refresh.
    last_unknown_kid_refresh: Option<Instant>,
}

/// Default cooldown between forced unknown-`kid` refreshes.
const DEFAULT_UNKNOWN_KID_REFRESH_COOLDOWN: Duration = Duration::from_secs(30);

impl JwtValidator {
    /// Create a validator that fetches and caches keys from a JWKS endpoint.
    pub fn new(
        issuer: impl Into<String>,
        audiences: impl IntoIterator<Item = impl Into<String>>,
        jwks_url: impl Into<String>,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            audiences: audiences.into_iter().map(Into::into).collect(),
            source: JwkSource::Remote(jwks_url.into()),
            cache_ttl: DEFAULT_JWKS_CACHE_TTL,
            unknown_kid_refresh_cooldown: DEFAULT_UNKNOWN_KID_REFRESH_COOLDOWN,
            client: Client::new(),
            cache: Mutex::new(None),
            refresh_lock: Mutex::new(()),
        }
    }

    /// Create a validator backed by a pre-loaded JWK set.
    pub fn from_jwk_set(
        issuer: impl Into<String>,
        audiences: impl IntoIterator<Item = impl Into<String>>,
        jwk_set: JwkSet,
    ) -> Self {
        Self {
            issuer: issuer.into(),
            audiences: audiences.into_iter().map(Into::into).collect(),
            source: JwkSource::Static(jwk_set),
            cache_ttl: DEFAULT_JWKS_CACHE_TTL,
            unknown_kid_refresh_cooldown: DEFAULT_UNKNOWN_KID_REFRESH_COOLDOWN,
            client: Client::new(),
            cache: Mutex::new(None),
            refresh_lock: Mutex::new(()),
        }
    }

    /// Override the JWKS cache TTL.
    pub fn with_cache_ttl(mut self, cache_ttl: Duration) -> Self {
        self.cache_ttl = cache_ttl;
        self
    }

    /// Override the minimum interval between unknown-`kid` forced JWKS refreshes.
    pub fn with_unknown_kid_refresh_cooldown(
        mut self,
        unknown_kid_refresh_cooldown: Duration,
    ) -> Self {
        self.unknown_kid_refresh_cooldown = unknown_kid_refresh_cooldown;
        self
    }

    /// Load the current JWK set, refreshing the cache when required.
    async fn load_jwk_set(&self, force_refresh: bool) -> Result<JwkSet, AuthError> {
        match &self.source {
            JwkSource::Static(jwk_set) => Ok(jwk_set.clone()),
            JwkSource::Remote(jwks_url) => {
                if !force_refresh && let Some(jwk_set) = self.cached_jwk_set().await {
                    return Ok(jwk_set);
                }

                let _refresh_guard = self.refresh_lock.lock().await;
                if !force_refresh && let Some(jwk_set) = self.cached_jwk_set().await {
                    return Ok(jwk_set);
                }

                self.store_remote_jwk_set(jwks_url, None).await
            }
        }
    }

    /// Return the cached JWK set if it is still fresh.
    async fn cached_jwk_set(&self) -> Option<JwkSet> {
        let cache = self.cache.lock().await;
        cache.as_ref().and_then(|cached| {
            if cached.fetched_at.elapsed() < self.cache_ttl {
                Some(cached.jwk_set.clone())
            } else {
                None
            }
        })
    }

    /// Fetch a JWK set from the configured remote endpoint.
    async fn fetch_jwk_set(&self, jwks_url: &str) -> Result<JwkSet, AuthError> {
        let response =
            self.client.get(jwks_url).send().await.map_err(|error| {
                AuthError::Unavailable(format!("Failed to fetch JWKS: {error}"))
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(AuthError::Unavailable(format!(
                "JWKS endpoint returned {status}",
            )));
        }

        response
            .json::<JwkSet>()
            .await
            .map_err(|error| AuthError::Unavailable(format!("Invalid JWKS response: {error}")))
    }

    /// Fetch and store a remote JWK set.
    async fn store_remote_jwk_set(
        &self,
        jwks_url: &str,
        last_unknown_kid_refresh: Option<Instant>,
    ) -> Result<JwkSet, AuthError> {
        let jwk_set = self.fetch_jwk_set(jwks_url).await?;
        *self.cache.lock().await = Some(CachedJwkSet {
            fetched_at: Instant::now(),
            jwk_set: jwk_set.clone(),
            last_unknown_kid_refresh,
        });
        Ok(jwk_set)
    }

    /// Select the verification key for a JWT header.
    async fn select_jwk(&self, token_kid: Option<&str>) -> Result<Jwk, AuthError> {
        let jwk_set = self.load_jwk_set(false).await?;
        match token_kid {
            Some(kid) => {
                if let Some(jwk) = jwk_set.find(kid) {
                    return Ok(jwk.clone());
                }

                let jwk_set = self.refresh_unknown_kid(kid).await?;
                jwk_set.find(kid).cloned().ok_or(AuthError::Invalid)
            }
            None if jwk_set.keys.len() == 1 => Ok(jwk_set.keys[0].clone()),
            None => Err(AuthError::Invalid),
        }
    }

    /// Refresh the remote JWK set for an unknown `kid`, subject to a cooldown.
    async fn refresh_unknown_kid(&self, kid: &str) -> Result<JwkSet, AuthError> {
        match &self.source {
            JwkSource::Static(jwk_set) => Ok(jwk_set.clone()),
            JwkSource::Remote(jwks_url) => {
                let _refresh_guard = self.refresh_lock.lock().await;

                let cache = self.cache.lock().await;
                if let Some(cached) = cache.as_ref() {
                    if cached.jwk_set.find(kid).is_some() {
                        return Ok(cached.jwk_set.clone());
                    }
                    if cached
                        .last_unknown_kid_refresh
                        .is_some_and(|last| last.elapsed() < self.unknown_kid_refresh_cooldown)
                    {
                        return Ok(cached.jwk_set.clone());
                    }
                }
                drop(cache);

                self.store_remote_jwk_set(jwks_url, Some(Instant::now()))
                    .await
            }
        }
    }

    /// Build validation rules for a JWT algorithm.
    fn validation(&self, algorithm: Algorithm) -> Validation {
        let mut validation = Validation::new(algorithm);
        validation.leeway = 0;
        validation.set_audience(&self.audiences);
        validation.set_issuer(&[&self.issuer]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation
    }

    /// Convert decoded claims into the public auth context.
    fn auth_info_from_claims(claims: Value) -> Result<AuthInfo, AuthError> {
        let Value::Object(claims) = claims else {
            return Err(AuthError::Invalid);
        };
        let claims: HashMap<String, Value> = claims.into_iter().collect();

        let subject = claims
            .get("sub")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or(AuthError::Invalid)?;
        let audiences = audiences_from_claims(&claims)?;
        let scopes = scopes_from_claims(&claims);
        let extra = extra_claims(claims);

        Ok(AuthInfo {
            subject,
            scopes,
            audiences,
            extra,
        })
    }
}

#[async_trait::async_trait]
impl TokenValidator for JwtValidator {
    async fn validate(&self, token: &str) -> Result<AuthInfo, AuthError> {
        let header = decode_header(token).map_err(|error| map_jwt_error(&error))?;
        let jwk = self.select_jwk(header.kid.as_deref()).await?;
        let decoding_key = DecodingKey::from_jwk(&jwk)
            .map_err(|error| AuthError::Unavailable(format!("Invalid JWK: {error}")))?;
        let validation = self.validation(header.alg);
        let claims = decode::<Value>(token, &decoding_key, &validation)
            .map_err(|error| map_jwt_error(&error))?
            .claims;
        Self::auth_info_from_claims(claims)
    }
}

/// Parse audiences from a decoded claim map.
fn audiences_from_claims(claims: &HashMap<String, Value>) -> Result<Vec<String>, AuthError> {
    let Some(value) = claims.get("aud") else {
        return Err(AuthError::Invalid);
    };
    match value {
        Value::String(audience) => Ok(vec![audience.clone()]),
        Value::Array(values) => values
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(ToOwned::to_owned)
                    .ok_or(AuthError::Invalid)
            })
            .collect(),
        _ => Err(AuthError::Invalid),
    }
}

/// Parse granted scopes from standard scope claims.
fn scopes_from_claims(claims: &HashMap<String, Value>) -> HashSet<String> {
    let mut scopes = HashSet::new();
    if let Some(Value::String(scope)) = claims.get("scope") {
        scopes.extend(parse_scope_set(scope));
    }
    if let Some(scp) = claims.get("scp") {
        match scp {
            Value::String(scope) => {
                scopes.extend(parse_scope_set(scope));
            }
            Value::Array(values) => {
                scopes.extend(
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned),
                );
            }
            _ => {}
        }
    }
    scopes
}

/// Map a `jsonwebtoken` error into the public auth error type.
fn map_jwt_error(error: &JwtError) -> AuthError {
    match error.kind() {
        ErrorKind::ExpiredSignature => AuthError::Expired,
        ErrorKind::Base64(_)
        | ErrorKind::InvalidAlgorithm
        | ErrorKind::InvalidAudience
        | ErrorKind::InvalidClaimFormat(_)
        | ErrorKind::InvalidIssuer
        | ErrorKind::InvalidKeyFormat
        | ErrorKind::InvalidSignature
        | ErrorKind::InvalidSubject
        | ErrorKind::InvalidToken
        | ErrorKind::Json(_)
        | ErrorKind::MissingAlgorithm
        | ErrorKind::MissingRequiredClaim(_)
        | ErrorKind::Utf8(_)
        | ErrorKind::ImmatureSignature
        | ErrorKind::InvalidAlgorithmName => AuthError::Invalid,
        ErrorKind::InvalidEcdsaKey
        | ErrorKind::InvalidEddsaKey
        | ErrorKind::InvalidRsaKey(_)
        | ErrorKind::Provider(_)
        | ErrorKind::RsaFailedSigning
        | ErrorKind::Signing(_) => AuthError::Unavailable(error.to_string()),
        _ => AuthError::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use axum::{Json, Router, extract::State, routing::get};
    use jsonwebtoken::{
        Algorithm, EncodingKey, Header, encode,
        jwk::{Jwk, KeyAlgorithm},
    };
    use rsa::{
        RsaPrivateKey,
        pkcs8::{EncodePrivateKey, LineEnding},
        rand_core::OsRng,
    };
    use tokio::net::TcpListener;

    use super::*;

    #[derive(Clone)]
    struct JwksState {
        requests: Arc<AtomicUsize>,
        first: JwkSet,
        second: JwkSet,
    }

    async fn jwks_handler(State(state): State<JwksState>) -> Json<JwkSet> {
        let call = state.requests.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Json(state.first)
        } else {
            Json(state.second)
        }
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn signing_key(kid: &str) -> (EncodingKey, JwkSet) {
        let private_key = RsaPrivateKey::new(&mut OsRng, 2048).unwrap();
        let pem = private_key
            .to_pkcs8_pem(LineEnding::LF)
            .unwrap()
            .to_string();
        let encoding_key = EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap();
        let mut jwk = Jwk::from_encoding_key(&encoding_key, Algorithm::RS256).unwrap();
        jwk.common.key_id = Some(kid.to_string());
        jwk.common.key_algorithm = Some(KeyAlgorithm::RS256);
        (encoding_key, JwkSet { keys: vec![jwk] })
    }

    fn token(key: &EncodingKey, kid: &str, exp: u64, aud: &Value, scope: &str) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        encode(
            &header,
            &serde_json::json!({
                "sub": "user-123",
                "iss": "https://issuer.example.com",
                "aud": aud.clone(),
                "exp": exp,
                "scope": scope,
                "role": "admin",
            }),
            key,
        )
        .unwrap()
    }

    #[tokio::test]
    async fn validates_static_jwk_set_tokens() {
        let (encoding_key, jwk_set) = signing_key("kid-1");
        let validator = JwtValidator::from_jwk_set("https://issuer.example.com", ["tmcp"], jwk_set);

        let auth = validator
            .validate(&token(
                &encoding_key,
                "kid-1",
                now() + 300,
                &Value::String("tmcp".to_string()),
                "tools:call resources:read",
            ))
            .await
            .unwrap();

        assert_eq!(auth.subject, "user-123");
        assert_eq!(auth.audiences, vec!["tmcp".to_string()]);
        assert!(auth.has_all_scopes(&["tools:call", "resources:read"]));
        assert_eq!(auth.extra["role"], "admin");
    }

    #[tokio::test]
    async fn rejects_expired_tokens() {
        let (encoding_key, jwk_set) = signing_key("kid-1");
        let validator = JwtValidator::from_jwk_set("https://issuer.example.com", ["tmcp"], jwk_set)
            .with_cache_ttl(Duration::from_secs(1));

        let error = validator
            .validate(&token(
                &encoding_key,
                "kid-1",
                now() - 1,
                &Value::Array(vec![Value::String("tmcp".to_string())]),
                "tools:call",
            ))
            .await
            .unwrap_err();

        assert!(matches!(error, AuthError::Expired));
    }

    #[tokio::test]
    async fn refetches_jwks_when_kid_is_unknown() {
        let (first_key, first_set) = signing_key("kid-1");
        let (second_key, second_set) = signing_key("kid-2");
        let requests = Arc::new(AtomicUsize::new(0));

        let router = Router::new()
            .route("/jwks", get(jwks_handler))
            .with_state(JwksState {
                requests: requests.clone(),
                first: first_set,
                second: second_set.clone(),
            });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let validator = JwtValidator::new(
            "https://issuer.example.com",
            ["tmcp"],
            format!("http://{addr}/jwks"),
        )
        .with_cache_ttl(Duration::from_secs(300));

        let auth = validator
            .validate(&token(
                &second_key,
                "kid-2",
                now() + 300,
                &Value::String("tmcp".to_string()),
                "tools:call",
            ))
            .await
            .unwrap();

        assert_eq!(auth.subject, "user-123");
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        let _ = first_key;
    }

    #[tokio::test]
    async fn unknown_kid_refresh_is_rate_limited() {
        let (_first_key, first_set) = signing_key("kid-1");
        let (_second_key, second_set) = signing_key("kid-2");
        let requests = Arc::new(AtomicUsize::new(0));

        let router = Router::new()
            .route("/jwks", get(jwks_handler))
            .with_state(JwksState {
                requests: requests.clone(),
                first: first_set,
                second: second_set,
            });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });

        let validator = JwtValidator::new(
            "https://issuer.example.com",
            ["tmcp"],
            format!("http://{addr}/jwks"),
        )
        .with_unknown_kid_refresh_cooldown(Duration::from_secs(60));

        let first_error = validator
            .validate(&token(
                &signing_key("kid-3").0,
                "kid-3",
                now() + 300,
                &Value::String("tmcp".to_string()),
                "tools:call",
            ))
            .await
            .unwrap_err();
        assert!(matches!(first_error, AuthError::Invalid));
        assert_eq!(requests.load(Ordering::SeqCst), 2);

        let second_error = validator
            .validate(&token(
                &signing_key("kid-4").0,
                "kid-4",
                now() + 300,
                &Value::String("tmcp".to_string()),
                "tools:call",
            ))
            .await
            .unwrap_err();
        assert!(matches!(second_error, AuthError::Invalid));
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }
}
