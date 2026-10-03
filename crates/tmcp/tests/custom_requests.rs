//! Custom requests from a server to its client, and requests to a server
//! handler without a transport.
//!
//! Each scenario runs on two routes with the same server handler and the same
//! client handler: across a transport, and in process through
//! `ServerCtx::in_process` and `handle_request`.
#[cfg(test)]
mod tests {
    use std::{
        collections::HashMap,
        sync::{
            Arc, Mutex,
            atomic::{AtomicI64, Ordering},
        },
        time::Duration,
    };

    use async_trait::async_trait;
    use serde::{Deserialize, de::DeserializeOwned};
    use serde_json::{Value, json};
    use tmcp::{
        Arguments, Client, ClientCtx, ClientHandler, CustomRequest, Error, Result, Server,
        ServerCtx, ServerHandle, ServerHandler, handle_request,
        schema::{
            CallToolResponse, CallToolResult, ClientCapabilities, ClientRequest, GetPromptResult,
            Implementation, InitializeResult, ListRootsResult, LoggingLevel, ProtocolVersion,
            RequestId, Root, ServerNotification, TaskMetadata,
        },
        testutils::{connected_client_and_server_with_conn, make_duplex_pair},
    };
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader, duplex},
        sync::mpsc,
        time::{sleep, timeout},
    };
    use tokio_util::sync::CancellationToken;

    /// The largest custom request params that the test client accepts.
    const CLIENT_LIMIT: usize = 4096;

    /// The longest wait for an event of the test client.
    const EVENT_WAIT: Duration = Duration::from_secs(5);

    /// The two routes between a server handler and its client.
    #[derive(Clone, Copy, Debug)]
    enum Route {
        /// A client across an in-memory transport.
        Wire,
        /// A host that drives the server handler in its own process.
        InProcess,
    }

    const ROUTES: [Route; 2] = [Route::Wire, Route::InProcess];

    /// The arguments of the relay tool.
    #[derive(Deserialize)]
    struct RelayArgs {
        /// The method of the custom request.
        method: String,
        /// The params of the custom request.
        #[serde(default)]
        params: Value,
        /// The deadline of the request, in milliseconds.
        timeout_ms: Option<u64>,
        /// The payload limit of the request.
        limit: Option<usize>,
        /// Stop waiting for the result after this many milliseconds.
        abandon_after_ms: Option<u64>,
    }

    /// A server whose tools send requests to the client and report the
    /// outcome.
    #[derive(Clone, Default)]
    struct Relay;

    #[async_trait]
    impl ServerHandler for Relay {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("relay").with_tools(None))
        }

        async fn call_tool(
            &self,
            context: &ServerCtx,
            name: String,
            arguments: Option<Arguments>,
            _task: Option<TaskMetadata>,
        ) -> Result<CallToolResponse> {
            let outcome = match name.as_str() {
                "relay" => {
                    let args = arguments
                        .unwrap_or_default()
                        .deserialize::<RelayArgs>()
                        .map_err(|error| Error::InvalidParams(error.to_string()))?;
                    relay(context, args).await
                }
                "ping" => context.ping().await.map(|()| json!({})),
                "roots" => context
                    .list_roots()
                    .await
                    .and_then(|roots| serde_json::to_value(roots).map_err(Into::into)),
                _ => return Err(Error::ToolNotFound(name)),
            };
            let result = match outcome {
                Ok(value) => {
                    CallToolResult::new().with_structured_content(json!({ "result": value }))
                }
                Err(error) => CallToolResult::error(error_code(&error), error.to_string()),
            };
            Ok(result.into())
        }
    }

    /// Send one custom request with the options of the relay arguments.
    async fn relay(context: &ServerCtx, args: RelayArgs) -> Result<Value> {
        let mut request = CustomRequest::new(args.method, args.params)?;
        if let Some(timeout_ms) = args.timeout_ms {
            request = request.with_timeout(Duration::from_millis(timeout_ms));
        }
        if let Some(limit) = args.limit {
            request = request.with_payload_limit(limit);
        }
        match args.abandon_after_ms {
            Some(abandon_after_ms) => tokio::select! {
                result = context.send_request(request) => result,
                () = sleep(Duration::from_millis(abandon_after_ms)) => Err(Error::Cancelled {
                    request_id: "abandoned".to_owned(),
                }),
            },
            None => context.send_request(request).await,
        }
    }

    /// Return a stable code for the kind of an error.
    fn error_code(error: &Error) -> &'static str {
        match error {
            Error::Timeout { .. } => "timeout",
            Error::Cancelled { .. } => "cancelled",
            Error::PayloadTooLarge { .. } => "payload_too_large",
            Error::MethodNotFound(_) => "method_not_found",
            Error::InvalidParams(_) => "invalid_params",
            Error::InvalidRequest(_) => "invalid_request",
            _ => "other",
        }
    }

    /// A client that answers custom requests and reports what it sees.
    #[derive(Clone)]
    struct Answerer {
        /// Events of the client, in order.
        events: mpsc::UnboundedSender<String>,
    }

    impl Answerer {
        /// Create a client and the receiver of its events.
        fn new() -> (Self, mpsc::UnboundedReceiver<String>) {
            let (events, receiver) = mpsc::unbounded_channel();
            (Self { events }, receiver)
        }

        /// Report one event.
        fn event(&self, event: String) {
            self.events.send(event).expect("event receiver");
        }
    }

    #[async_trait]
    impl ClientHandler for Answerer {
        async fn pong(&self, _context: &ClientCtx) -> Result<()> {
            self.event("pong".to_owned());
            Ok(())
        }

        async fn list_roots(&self, _context: &ClientCtx) -> Result<ListRootsResult> {
            Ok(ListRootsResult {
                roots: vec![Root {
                    uri: "file:///work".to_owned(),
                    name: None,
                    _meta: None,
                }],
                _meta: None,
                _extra: Default::default(),
            })
        }

        async fn custom_request(
            &self,
            context: &ClientCtx,
            method: &str,
            params: Option<Value>,
        ) -> Result<Value> {
            self.event(format!("called {method}"));
            let params = params.unwrap_or(Value::Null);
            match method {
                "test/echo" => {
                    if let Some(delay_ms) = params["delay_ms"].as_u64() {
                        sleep(Duration::from_millis(delay_ms)).await;
                    }
                    Ok(json!({ "echo": params, "request_id": context.request_id() }))
                }
                "test/wait" => {
                    context.cancelled().await;
                    self.event(format!("cancelled {method}"));
                    Ok(json!({}))
                }
                "test/large" => {
                    let size = params["size"].as_u64().expect("size") as usize;
                    Ok(json!({ "data": "x".repeat(size) }))
                }
                "test/fail" => Err(Error::InvalidParams("the answerer refuses".to_owned())),
                _ => Err(Error::MethodNotFound(method.to_owned())),
            }
        }

        fn custom_request_limit(&self) -> usize {
            CLIENT_LIMIT
        }
    }

    /// A server handler and its client on one route.
    enum Host<S, C: ClientHandler + 'static> {
        /// A client across a transport.
        Wire {
            /// The connected client.
            client: Box<Client<C>>,
            /// The running server.
            _server: ServerHandle,
        },
        /// A server handler that the test drives without a transport.
        InProcess {
            /// The server handler.
            server: S,
            /// The connection context of the server.
            connection: ServerCtx,
            /// The next request ID.
            next_id: AtomicI64,
            /// The notifications of the server.
            _notifications: mpsc::Receiver<ServerNotification>,
        },
    }

    impl<S, C> Host<S, C>
    where
        S: ServerHandler + Clone + 'static,
        C: ClientHandler + Clone + 'static,
    {
        /// Connect a client to a server on one route, and initialize.
        async fn start(route: Route, server: S, client: C) -> Self {
            match route {
                Route::Wire => {
                    let (mut client, server) =
                        connected_client_and_server_with_conn(move || server.clone(), client)
                            .await
                            .expect("connect");
                    client
                        .initialize(
                            ClientCapabilities::default(),
                            Implementation::new("test-client", "1.0.0"),
                        )
                        .await
                        .expect("initialize");
                    Self::Wire {
                        client: Box::new(client),
                        _server: server,
                    }
                }
                Route::InProcess => {
                    let (notification_tx, notifications) = mpsc::channel(16);
                    let host = Self::InProcess {
                        server,
                        connection: ServerCtx::in_process(notification_tx, client),
                        next_id: AtomicI64::new(1),
                        _notifications: notifications,
                    };
                    let protocol_version = "2025-11-25".parse().expect("protocol version");
                    let initialized: InitializeResult = host
                        .request(ClientRequest::initialize(
                            protocol_version,
                            ClientCapabilities::default(),
                            Implementation::new("test-client", "1.0.0"),
                        ))
                        .await
                        .expect("initialize");
                    assert_eq!(initialized.protocol_version.to_string(), "2025-11-25");
                    host
                }
            }
        }

        /// Send one request to the server and decode its result.
        async fn request<T>(&self, request: ClientRequest) -> Result<T>
        where
            T: DeserializeOwned + Send + 'static,
        {
            match self {
                Self::Wire { client, .. } => {
                    let (_, pending) = client.request(request).await?;
                    pending.await
                }
                Self::InProcess {
                    server,
                    connection,
                    next_id,
                    ..
                } => {
                    let request_id = RequestId::Number(next_id.fetch_add(1, Ordering::Relaxed));
                    let context = connection.for_request(request_id, CancellationToken::new());
                    handle_request(server, &context, request).await
                }
            }
        }

        /// Call one tool and return its result.
        async fn call(&self, name: &str, args: Value) -> CallToolResult {
            let arguments = Arguments::from_struct(args).expect("arguments");
            self.request(ClientRequest::call_tool(name, Some(arguments), None))
                .await
                .unwrap_or_else(|error| panic!("{name} failed: {error}"))
        }
    }

    /// Return the result that a relay tool reports, and fail on an error.
    fn relayed(result: &CallToolResult) -> &Value {
        assert_ne!(result.is_error, Some(true), "relay failed: {result:?}");
        &result.structured_content.as_ref().expect("structured")["result"]
    }

    /// Return the error code that a relay tool reports.
    fn relay_error(result: &CallToolResult) -> String {
        assert_eq!(result.is_error, Some(true), "relay succeeded: {result:?}");
        result.structured_content.as_ref().expect("structured")["code"]
            .as_str()
            .expect("code")
            .to_owned()
    }

    /// Wait for the next event of the client.
    async fn next_event(events: &mut mpsc::UnboundedReceiver<String>) -> String {
        timeout(EVENT_WAIT, events.recv())
            .await
            .expect("an event in time")
            .expect("an open event channel")
    }

    #[tokio::test]
    async fn custom_requests_correlate_with_their_results() {
        for route in ROUTES {
            let (answerer, _events) = Answerer::new();
            let host = Host::start(route, Relay, answerer).await;

            let (slow, fast) = tokio::join!(
                host.call(
                    "relay",
                    json!({ "method": "test/echo", "params": { "tag": "slow", "delay_ms": 100 } }),
                ),
                host.call(
                    "relay",
                    json!({ "method": "test/echo", "params": { "tag": "fast" } }),
                ),
            );

            let slow = relayed(&slow);
            let fast = relayed(&fast);
            assert_eq!(slow["echo"]["tag"], "slow", "{route:?}");
            assert_eq!(fast["echo"]["tag"], "fast", "{route:?}");
            assert_ne!(slow["request_id"], fast["request_id"], "{route:?}");
        }
    }

    #[tokio::test]
    async fn a_server_that_stops_waiting_cancels_the_request_at_the_client() {
        for route in ROUTES {
            let (answerer, mut events) = Answerer::new();
            let host = Host::start(route, Relay, answerer).await;

            let result = host
                .call(
                    "relay",
                    json!({ "method": "test/wait", "abandon_after_ms": 50 }),
                )
                .await;

            assert_eq!(relay_error(&result), "cancelled", "{route:?}");
            assert_eq!(next_event(&mut events).await, "called test/wait");
            assert_eq!(next_event(&mut events).await, "cancelled test/wait");
            // The connection still carries requests.
            let echo = host
                .call("relay", json!({ "method": "test/echo", "params": {} }))
                .await;
            relayed(&echo);
        }
    }

    #[tokio::test]
    async fn a_custom_request_ends_at_its_deadline() {
        for route in ROUTES {
            let (answerer, mut events) = Answerer::new();
            let host = Host::start(route, Relay, answerer).await;

            let result = host
                .call("relay", json!({ "method": "test/wait", "timeout_ms": 50 }))
                .await;

            assert_eq!(relay_error(&result), "timeout", "{route:?}");
            assert_eq!(next_event(&mut events).await, "called test/wait");
            // The client sees the request cancelled at the deadline.
            assert_eq!(next_event(&mut events).await, "cancelled test/wait");
        }
    }

    #[tokio::test]
    async fn payload_limits_refuse_large_payloads_cleanly() {
        for route in ROUTES {
            let (answerer, mut events) = Answerer::new();
            let host = Host::start(route, Relay, answerer).await;
            let data = "x".repeat(CLIENT_LIMIT);

            // Params over the limit of the request are not sent.
            let result = host
                .call(
                    "relay",
                    json!({ "method": "test/never", "params": { "data": "x".repeat(200) }, "limit": 100 }),
                )
                .await;
            assert_eq!(relay_error(&result), "payload_too_large", "{route:?}");

            // A result over the limit of the request is refused.
            let result = host
                .call(
                    "relay",
                    json!({ "method": "test/large", "params": { "size": 500 }, "limit": 200 }),
                )
                .await;
            assert_eq!(relay_error(&result), "payload_too_large", "{route:?}");
            assert_eq!(next_event(&mut events).await, "called test/large");

            // Params over the limit of the client reach no handler.
            let result = host
                .call(
                    "relay",
                    json!({ "method": "test/never", "params": { "data": data } }),
                )
                .await;
            assert_eq!(relay_error(&result), "invalid_params", "{route:?}");

            // The connection still carries requests, and the client handler
            // never saw `test/never`.
            let echo = host
                .call("relay", json!({ "method": "test/echo", "params": {} }))
                .await;
            relayed(&echo);
            assert_eq!(next_event(&mut events).await, "called test/echo");
        }
    }

    #[tokio::test]
    async fn unknown_custom_methods_are_not_found() {
        for route in ROUTES {
            // A client handler without custom requests.
            let host = Host::start(route, Relay, ()).await;
            let result = host
                .call("relay", json!({ "method": "test/echo", "params": {} }))
                .await;
            assert_eq!(relay_error(&result), "method_not_found", "{route:?}");

            // A client handler that does not know one method.
            let (answerer, _events) = Answerer::new();
            let host = Host::start(route, Relay, answerer).await;
            let result = host
                .call("relay", json!({ "method": "test/unknown" }))
                .await;
            assert_eq!(relay_error(&result), "method_not_found", "{route:?}");
        }
    }

    #[tokio::test]
    async fn client_errors_reach_the_server() {
        for route in ROUTES {
            let (answerer, _events) = Answerer::new();
            let host = Host::start(route, Relay, answerer).await;
            let result = host.call("relay", json!({ "method": "test/fail" })).await;
            assert_eq!(relay_error(&result), "invalid_params", "{route:?}");
            let message = result.structured_content.as_ref().expect("structured")["message"]
                .as_str()
                .expect("message")
                .to_owned();
            assert!(message.contains("the answerer refuses"), "{message}");
        }
    }

    #[tokio::test]
    async fn standard_requests_reach_the_client_beside_custom_requests() {
        for route in ROUTES {
            let (answerer, mut events) = Answerer::new();
            let host = Host::start(route, Relay, answerer).await;

            let ping = host.call("ping", json!({})).await;
            relayed(&ping);
            assert_eq!(next_event(&mut events).await, "pong");

            let roots = host.call("roots", json!({})).await;
            assert_eq!(
                relayed(&roots)["roots"][0]["uri"],
                "file:///work",
                "{route:?}"
            );

            // The default client handler refuses roots as before.
            let host = Host::start(route, Relay, ()).await;
            let roots = host.call("roots", json!({})).await;
            assert_eq!(relay_error(&roots), "other", "{route:?}");
        }
    }

    #[tokio::test]
    async fn a_client_answers_custom_requests_on_the_wire_and_drops_cancelled_ones() {
        let (server_reader, mut server_writer, client_reader, client_writer) = make_duplex_pair();
        let mut server_reader = BufReader::new(server_reader).lines();
        let (answerer, mut events) = Answerer::new();
        let mut client = Client::new("test-client", "1.0.0").with_handler(answerer);
        client
            .connect_stream_raw(client_reader, client_writer)
            .await
            .expect("connect");
        let mut send = async |message: Value| {
            let mut line = serde_json::to_vec(&message).expect("message");
            line.push(b'\n');
            server_writer.write_all(&line).await.expect("write");
        };

        send(json!({
            "jsonrpc": "2.0",
            "id": "s-1",
            "method": "test/echo",
            "params": { "tag": "wire", "_meta": { "vendor/key": 1 } }
        }))
        .await;
        let line = timeout(EVENT_WAIT, server_reader.next_line())
            .await
            .expect("a response in time")
            .expect("read")
            .expect("a response line");
        let response: Value = serde_json::from_str(&line).expect("response");
        assert_eq!(response["id"], "s-1");
        assert_eq!(response["result"]["echo"]["tag"], "wire");
        assert_eq!(response["result"]["echo"]["_meta"]["vendor/key"], 1);
        assert_eq!(response["result"]["request_id"], "s-1");
        assert_eq!(next_event(&mut events).await, "called test/echo");

        send(json!({ "jsonrpc": "2.0", "id": "s-2", "method": "test/wait" })).await;
        assert_eq!(next_event(&mut events).await, "called test/wait");
        send(json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": { "requestId": "s-2", "reason": "test" }
        }))
        .await;
        assert_eq!(next_event(&mut events).await, "cancelled test/wait");
        let late = timeout(Duration::from_millis(200), server_reader.next_line()).await;
        assert!(
            late.is_err(),
            "the client answered a cancelled request: {late:?}"
        );
    }

    /// The number of `hold` calls of the saturation test: more than the
    /// response queue of a connection holds.
    const HOLDS: usize = 80;

    /// A server that abandons a custom request while the queues of its
    /// connection are full.
    ///
    /// Its `hold` calls wait for `release`. Its `abandon` call sends
    /// `test/wait`, waits for `abandon`, fills the notification queue, and
    /// then drops the request.
    #[derive(Clone)]
    struct Saturator {
        /// Events of the server, in order.
        events: mpsc::UnboundedSender<String>,
        /// Ends the wait of each `hold` call.
        release: CancellationToken,
        /// Lets the `abandon` call drop its request.
        abandon: CancellationToken,
    }

    #[async_trait]
    impl ServerHandler for Saturator {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("saturator")
                .with_tools(None)
                .with_logging())
        }

        async fn call_tool(
            &self,
            context: &ServerCtx,
            name: String,
            _arguments: Option<Arguments>,
            _task: Option<TaskMetadata>,
        ) -> Result<CallToolResponse> {
            match name.as_str() {
                "hold" => {
                    self.events.send("held".to_owned()).expect("events");
                    self.release.cancelled().await;
                }
                "abandon" => {
                    let request = CustomRequest::new("test/wait", ())?.without_timeout();
                    let mut send = Box::pin(context.send_request::<Value>(request));
                    tokio::select! {
                        result = &mut send => panic!("the client answered: {result:?}"),
                        () = self.abandon.cancelled() => {}
                    }
                    let mut queued = 0;
                    while context
                        .notify(ServerNotification::logging_message(
                            LoggingLevel::Info,
                            None,
                            json!("fill"),
                        ))
                        .is_ok()
                    {
                        queued += 1;
                    }
                    self.events
                        .send(format!("queued {queued}"))
                        .expect("events");
                    drop(send);
                }
                _ => return Err(Error::ToolNotFound(name)),
            }
            Ok(CallToolResult::new().with_text_content("done").into())
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancellation_reaches_the_client_when_the_queues_are_full() {
        // The buffer from the server to the client holds about two responses,
        // so the connection loop stalls while the test does not read.
        let (server_reader, mut client_writer) = duplex(64 * 1024);
        let (client_reader, server_writer) = duplex(256);
        let mut lines = BufReader::new(client_reader).lines();
        let (events, mut event_rx) = mpsc::unbounded_channel();
        let server = Saturator {
            events,
            release: CancellationToken::new(),
            abandon: CancellationToken::new(),
        };
        let (release, abandon) = (server.release.clone(), server.abandon.clone());
        let _server = ServerHandle::from_stream(
            Server::new(move || server.clone()),
            server_reader,
            server_writer,
        )
        .await
        .expect("start server");
        let mut send = async |message: Value| {
            let mut line = serde_json::to_vec(&message).expect("message");
            line.push(b'\n');
            client_writer.write_all(&line).await.expect("write");
        };
        let call = |id: usize, name: &str| {
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": { "name": name, "arguments": {} }
            })
        };
        let mut next_message = async || -> Value {
            let line = timeout(EVENT_WAIT, lines.next_line())
                .await
                .expect("a message in time")
                .expect("read")
                .expect("a message line");
            serde_json::from_str(&line).expect("message")
        };

        send(json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "test-client", "version": "1.0.0" }
            }
        }))
        .await;
        assert_eq!(next_message().await["id"], 1);
        send(call(2, "abandon")).await;
        let request = next_message().await;
        assert_eq!(request["method"], "test/wait");
        let request_id = request["id"].clone();

        // Each hold call waits, so the loop reads all of them.
        for id in 0..HOLDS {
            send(call(100 + id, "hold")).await;
        }
        for _ in 0..HOLDS {
            assert_eq!(next_event(&mut event_rx).await, "held");
        }
        // The responses fill the transport buffer and then the response
        // queue. Paused time advances only when every task waits, so the
        // sleep ends when the loop has stalled.
        release.cancel();
        sleep(Duration::from_millis(10)).await;
        // The abandon call fills the notification queue, and then drops its
        // request.
        abandon.cancel();
        let queued = next_event(&mut event_rx).await;
        assert_ne!(queued, "queued 0", "the notification queue had space");

        loop {
            let message = next_message().await;
            if message["method"] == "notifications/cancelled" {
                assert_eq!(message["params"]["requestId"], request_id);
                break;
            }
        }
    }

    #[test]
    fn a_custom_request_cannot_take_a_standard_method() {
        assert!(matches!(
            CustomRequest::new("roots/list", ()),
            Err(Error::InvalidRequest(_))
        ));
    }

    /// The arguments of the strict tool.
    #[derive(Deserialize)]
    struct StrictArgs {
        /// A count.
        count: u32,
    }

    /// A server handler with one strict tool and a prompt that refuses its
    /// parameters.
    #[derive(Clone, Default)]
    struct Tools;

    #[async_trait]
    impl ServerHandler for Tools {
        async fn initialize(
            &self,
            _context: &ServerCtx,
            _protocol_version: ProtocolVersion,
            _capabilities: ClientCapabilities,
            _client_info: Implementation,
        ) -> Result<InitializeResult> {
            Ok(InitializeResult::new("tools")
                .with_tools(None)
                .with_prompts(None))
        }

        async fn call_tool(
            &self,
            context: &ServerCtx,
            name: String,
            arguments: Option<Arguments>,
            _task: Option<TaskMetadata>,
        ) -> Result<CallToolResponse> {
            match name.as_str() {
                "strict" => {
                    let args = arguments
                        .unwrap_or_default()
                        .deserialize::<StrictArgs>()
                        .map_err(|error| Error::InvalidParams(error.to_string()))?;
                    Ok(CallToolResult::new()
                        .with_structured_content(json!({ "count": args.count }))
                        .into())
                }
                "wait" => {
                    context.cancelled().await;
                    Ok(CallToolResult::new().with_text_content("cancelled").into())
                }
                _ => Err(Error::ToolNotFound(name)),
            }
        }

        async fn get_prompt(
            &self,
            _context: &ServerCtx,
            name: String,
            _arguments: Option<HashMap<String, String>>,
        ) -> Result<GetPromptResult> {
            Err(Error::InvalidParams(format!("no prompt {name}")))
        }
    }

    /// Middleware: a server handler that records each request and refuses
    /// one tool before the inner handler sees it.
    #[derive(Clone, Default)]
    struct Middleware {
        /// The handler behind the middleware.
        inner: Tools,
        /// The methods that the middleware saw.
        seen: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl ServerHandler for Middleware {
        async fn initialize(
            &self,
            context: &ServerCtx,
            protocol_version: ProtocolVersion,
            capabilities: ClientCapabilities,
            client_info: Implementation,
        ) -> Result<InitializeResult> {
            self.inner
                .initialize(context, protocol_version, capabilities, client_info)
                .await
        }

        async fn handle_request(
            &self,
            context: &ServerCtx,
            request: ClientRequest,
        ) -> Result<Value> {
            self.seen
                .lock()
                .expect("seen")
                .push(request.method().to_owned());
            if matches!(&request, ClientRequest::CallTool { name, .. } if name == "blocked") {
                return Err(Error::InvalidRequest(
                    "the middleware refuses blocked".to_owned(),
                ));
            }
            self.inner.handle_request(context, request).await
        }
    }

    /// Return the outcome of one request as text, for comparison.
    fn outcome(result: Result<Value>) -> String {
        match result {
            Ok(value) => value.to_string(),
            Err(error) => format!("error: {error}"),
        }
    }

    #[tokio::test]
    async fn handle_request_matches_a_transport_with_middleware_and_invalid_params() {
        let tool = |name: &str, args: Value| {
            ClientRequest::call_tool(
                name,
                Some(Arguments::from_struct(args).expect("arguments")),
                None,
            )
        };
        let requests = || {
            vec![
                tool("strict", json!({ "count": 2 })),
                tool("strict", json!({ "count": "two" })),
                tool("blocked", json!({})),
                tool("missing", json!({})),
                ClientRequest::get_prompt("absent", None),
                ClientRequest::ping(),
            ]
        };

        let mut outcomes = Vec::new();
        let mut seen = Vec::new();
        for route in ROUTES {
            let middleware = Middleware::default();
            let host = Host::start(route, middleware.clone(), ()).await;
            let mut route_outcomes = Vec::new();
            for request in requests() {
                route_outcomes.push(outcome(host.request::<Value>(request).await));
            }
            outcomes.push(route_outcomes);
            seen.push(middleware.seen.lock().expect("seen").clone());
        }

        assert_eq!(outcomes[0], outcomes[1], "the routes differ");
        assert_eq!(seen[0], seen[1], "the middleware saw different requests");
        let outcomes = &outcomes[0];
        let valid: Value = serde_json::from_str(&outcomes[0]).expect("tool result");
        assert_eq!(valid["structuredContent"]["count"], 2);
        // Invalid tool parameters become a tool error.
        let invalid: Value = serde_json::from_str(&outcomes[1]).expect("tool error");
        assert_eq!(invalid["isError"], true);
        assert_eq!(invalid["structuredContent"]["code"], "INVALID_INPUT");
        assert!(
            outcomes[2].contains("the middleware refuses blocked"),
            "{}",
            outcomes[2]
        );
        assert!(
            outcomes[3].contains("Unknown tool: missing"),
            "{}",
            outcomes[3]
        );
        assert!(
            outcomes[4].starts_with("error: Invalid parameters: prompts/get"),
            "{}",
            outcomes[4]
        );
        assert_eq!(outcomes[5], "{}");
        assert_eq!(seen[0].len(), 6);
    }

    #[tokio::test]
    async fn handle_request_reports_host_cancellation() {
        let (notification_tx, _notifications) = mpsc::channel(16);
        let connection = ServerCtx::in_process(notification_tx, ());
        let cancel = CancellationToken::new();
        let context = connection.for_request(RequestId::Number(1), cancel.clone());

        let request = ClientRequest::call_tool("wait", None, None);
        let call = handle_request::<_, Value>(&Tools, &context, request);
        let cancel_later = async {
            sleep(Duration::from_millis(20)).await;
            cancel.cancel();
        };
        let (result, ()) = timeout(EVENT_WAIT, async { tokio::join!(call, cancel_later) })
            .await
            .expect("the handler sees the cancellation");

        assert!(
            matches!(result, Err(Error::Cancelled { ref request_id }) if request_id == "1"),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn handle_request_needs_a_request_context() {
        let (notification_tx, _notifications) = mpsc::channel(16);
        let connection = ServerCtx::in_process(notification_tx, ());
        let result = handle_request::<_, Value>(&Tools, &connection, ClientRequest::ping()).await;
        assert!(
            matches!(result, Err(Error::InvalidRequest(_))),
            "{result:?}"
        );
    }
}
