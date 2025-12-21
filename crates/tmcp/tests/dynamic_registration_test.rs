//! Dynamic registration smoke tests.

#[cfg(test)]
mod tests {
    use tmcp::auth::{
        ClientMetadata, ClientRegistrationResponse, DynamicRegistrationClient, OAuth2Config,
    };

    #[test]
    fn test_imports() {
        // Test that we can import and use the types
        let metadata = ClientMetadata::new("Test", "http://localhost:8080/callback");
        assert_eq!(metadata.client_name, Some("Test".to_string()));

        let _client = DynamicRegistrationClient::new();

        // Test OAuth2Config::from_registration
        let registration = ClientRegistrationResponse {
            client_id: "test_id".to_string(),
            client_secret: Some("test_secret".to_string()),
            client_id_issued_at: None,
            client_secret_expires_at: None,
            metadata,
        };

        let config = OAuth2Config::from_registration(
            registration,
            "https://auth.example.com".to_string(),
            "https://token.example.com".to_string(),
            "https://resource.example.com".to_string(),
        );

        assert_eq!(config.client_id, "test_id");
        assert_eq!(config.client_secret, Some("test_secret".to_string()));
    }
}
