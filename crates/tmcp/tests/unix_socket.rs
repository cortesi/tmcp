//! Unix domain socket transport tests.

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use std::{
        env, fs,
        os::unix::fs::PermissionsExt,
        path::PathBuf,
        process,
        sync::atomic::{AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use tmcp::{Client, Result, Server, ServerCtx, ServerHandler, schema::*};

    /// A server that answers the initialize request only.
    struct EchoServer;

    #[async_trait]
    impl ServerHandler for EchoServer {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("unix-echo"))
        }
    }

    /// Distinguish the socket paths of the tests of one process.
    static NEXT_SOCKET: AtomicUsize = AtomicUsize::new(0);

    /// Return a short socket path below the temporary directory, within the
    /// `sun_path` bound of macOS.
    fn socket_path() -> PathBuf {
        let index = NEXT_SOCKET.fetch_add(1, Ordering::Relaxed);
        env::temp_dir().join(format!("tmcp-{}-{index}.sock", process::id()))
    }

    #[tokio::test]
    async fn a_client_connects_over_a_private_unix_socket() {
        let path = socket_path();
        let handle = Server::new(|| EchoServer)
            .serve_unix(&path)
            .await
            .expect("serve unix");
        let mode = fs::metadata(&path)
            .expect("socket metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600, "only the owner may connect");

        let mut client = Client::new("unix-test", "0.1.0");
        let initialized = client.connect_unix(&path).await.expect("connect unix");
        assert_eq!(initialized.server_info.name, "unix-echo");
        client.ping().await.expect("ping over the socket");
        drop(client);

        handle.stop().await.expect("stop");
        assert!(!path.exists(), "stop removes the socket file");
    }

    #[tokio::test]
    async fn serving_refuses_a_path_that_exists() {
        let path = socket_path();
        fs::write(&path, "taken").expect("occupy the path");
        let result = Server::new(|| EchoServer).serve_unix(&path).await;
        assert!(result.is_err(), "a bind over an existing file fails");
        fs::remove_file(&path).expect("remove the file");
    }
}
