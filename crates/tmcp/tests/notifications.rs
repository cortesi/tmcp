//! Notification integration tests.

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tmcp::{
        ClientCtx, ClientHandler, Result, ServerCtx, ServerHandler, schema,
        testutils::{connected_client_and_server_with_conn, shutdown_client_and_server},
    };
    use tokio::{
        sync::oneshot,
        time::{Duration, sleep, timeout},
    };
    use tracing_subscriber::fmt;

    #[derive(Clone)]
    struct NotificationRecorder {
        tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    }

    #[async_trait]
    impl ClientHandler for NotificationRecorder {
        async fn notification(
            &self,
            _context: &ClientCtx,
            notification: schema::ServerNotification,
        ) -> Result<()> {
            tracing::info!("Client received notification: {:?}", notification);
            if matches!(notification, schema::ServerNotification::ToolListChanged)
                && let Some(tx) = self.tx.lock().unwrap().take()
            {
                tx.send(()).ok();
            }
            Ok(())
        }
    }

    // Create tmcp server that sends notifications.
    struct NotifyingServer {
        sent_notification: Arc<Mutex<bool>>,
    }

    #[async_trait]
    impl ServerHandler for NotifyingServer {
        async fn on_connect(&self, context: &ServerCtx, _remote_addr: &str) -> Result<()> {
            // Send a notification after connection.
            let sent_notification = self.sent_notification.clone();
            let context = context.clone();
            tokio::spawn(async move {
                sleep(Duration::from_millis(500)).await;
                // Send roots list changed notification.
                match context.notify(schema::ServerNotification::ToolListChanged) {
                    Ok(_) => {
                        tracing::info!("Server sent roots_list_changed notification");
                        *sent_notification.lock().unwrap() = true;
                    }
                    Err(e) => {
                        tracing::error!("Failed to send notification: {:?}", e);
                    }
                }
            });
            Ok(())
        }

        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: String,
            _capabilities: schema::ClientCapabilities,
            _client_info: schema::Implementation,
        ) -> Result<schema::InitializeResult> {
            Ok(schema::InitializeResult::new("notifying-server").with_version("0.1.0"))
        }
    }

    #[tokio::test]
    async fn test_server_to_client_notifications() {
        fmt::try_init().ok();

        let (tx_notif, rx_notif) = oneshot::channel::<()>();

        // Track if notification was sent.
        let sent_notification = Arc::new(Mutex::new(false));

        // Create connected client and server.
        let (mut client, server_handle) = connected_client_and_server_with_conn(
            {
                let sent_notification = sent_notification.clone();
                move || {
                    Box::new(NotifyingServer {
                        sent_notification: sent_notification.clone(),
                    })
                }
            },
            NotificationRecorder {
                tx: Arc::new(Mutex::new(Some(tx_notif))),
            },
        )
        .await
        .expect("Failed to connect client and server");

        // Initialize the connection to trigger on_connect.
        client.init().await.expect("Failed to initialize");

        // Wait for the notification.
        let result = timeout(Duration::from_secs(3), rx_notif).await;

        // Verify notification was sent by server.
        assert!(
            *sent_notification.lock().unwrap(),
            "Server did not send notification"
        );

        // Verify notification was received by client.
        assert!(result.is_ok(), "Timeout waiting for notification");
        assert!(result.unwrap().is_ok(), "Failed to receive notification");

        // Cleanup.
        shutdown_client_and_server(client, server_handle).await;
    }
}
