//! HTTP endpoint classification shared by transports and authorization clients.

use url::{Host, Url};

/// Return whether a URL is an HTTP endpoint on the local loopback interface.
pub fn is_loopback_http_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        if !matches!(url.scheme(), "http" | "https") {
            return false;
        }
        match url.host() {
            Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
            Some(Host::Ipv4(address)) => address.is_loopback(),
            Some(Host::Ipv6(address)) => address.is_loopback(),
            None => false,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_loopback_http_urls() {
        for url in [
            "http://localhost",
            "https://LOCALHOST/path",
            "http://127.0.0.1:8080/callback",
            "https://127.42.7.9/resource",
            "http://[::1]",
            "https://[::1]:9443/path",
        ] {
            assert!(is_loopback_http_url(url), "{url}");
        }

        for url in [
            "http://example.com",
            "https://192.0.2.1/path",
            "ftp://localhost/resource",
            "file://localhost/path",
            "not a url",
        ] {
            assert!(!is_loopback_http_url(url), "{url}");
        }
    }
}
