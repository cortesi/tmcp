//! Shared URL and header helpers used across the OAuth client and server code.

use reqwest::header::HeaderValue;
use url::{Host, Url};

use crate::error::Error;

/// Returns whether `url` targets this machine.
pub fn is_loopback_url(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| match url.host() {
        Some(Host::Domain(domain)) => domain == "localhost",
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    })
}

/// Validate that a URL uses HTTPS, permitting plain HTTP only for loopback hosts.
pub fn require_https_or_loopback(url: &str, context: &str) -> Result<(), Error> {
    let parsed = Url::parse(url)
        .map_err(|e| Error::InvalidConfiguration(format!("Invalid {context}: {e}")))?;
    match parsed.scheme() {
        "https" => Ok(()),
        "http" if is_loopback_url(url) => Ok(()),
        "http" => Err(Error::InvalidConfiguration(format!(
            "Invalid {context}: plain HTTP is only allowed for loopback hosts"
        ))),
        scheme => Err(Error::InvalidConfiguration(format!(
            "Invalid {context}: unsupported scheme '{scheme}'"
        ))),
    }
}

/// Builds an authorization header without exposing token material through logs.
pub fn bearer_header(token: &str) -> Result<HeaderValue, Error> {
    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| Error::Transport("Invalid authorization token".into()))?;
    value.set_sensitive(true);
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_urls_are_detected() {
        assert!(is_loopback_url("http://127.0.0.1:8080/callback"));
        assert!(is_loopback_url("http://localhost/callback"));
        assert!(is_loopback_url("http://[::1]:9000/callback"));
        assert!(!is_loopback_url("http://example.com/callback"));
        assert!(!is_loopback_url("not a url"));
    }

    #[test]
    fn https_is_required_for_remote_hosts() {
        assert!(require_https_or_loopback("https://example.com/auth", "auth URL").is_ok());
        assert!(require_https_or_loopback("http://127.0.0.1:9090/auth", "auth URL").is_ok());
        assert!(require_https_or_loopback("http://localhost:9090/auth", "auth URL").is_ok());

        let err = require_https_or_loopback("http://example.com/auth", "auth URL").unwrap_err();
        assert!(err.to_string().contains("plain HTTP"));

        let err = require_https_or_loopback("ftp://example.com/auth", "auth URL").unwrap_err();
        assert!(err.to_string().contains("unsupported scheme"));
    }

    #[test]
    fn bearer_header_debug_redacts_token() {
        let header = bearer_header("secret-token").unwrap();
        let debug = format!("{header:?}");

        assert!(!debug.contains("secret-token"));
    }
}
