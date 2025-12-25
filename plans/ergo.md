# tmcp ergonomics + correctness review (2025-12-26)

Scope:
- Public API surface: `crates/tmcp`, `crates/tmcp-macros`
- Usage ergonomics: `examples/*` and the README example

## Critical (highest priority - must fix)

1) Trait naming is backwards from the user's perspective.
   - Evidence: `ServerAPI` contains methods clients call on servers (`list_tools`, `call_tool`, etc.)
     and `ClientAPI` contains methods servers call on clients (`create_message`, `list_roots`).
     But users import `ServerAPI` to use the *client* (`crates/tmcp/src/api.rs:19-109`).
   - Ergonomic cost: Confusing and counterintuitive. Users must remember a trait named after the
     *opposite* role to use their client/server.
   - Recommendation: Rename to `McpClient` / `McpServer` traits, or move methods to inherent impls
     on `Client`/`ServerCtx`. Alternative: export a `tmcp::prelude::*` that includes both traits.

   ```rust
   // BEFORE: Confusing - why do I need "ServerAPI" to use a client?
   use tmcp::{Client, ServerAPI};

   let mut client = Client::new("my-client", "1.0.0");
   client.connect_tcp("localhost:3000").await?;
   client.list_tools(None).await?;  // requires ServerAPI import

   // AFTER (Option A): Inherent methods on Client - no trait import needed
   use tmcp::Client;

   let mut client = Client::new("my-client", "1.0.0");
   client.connect_tcp("localhost:3000").await?;
   client.list_tools(None).await?;  // just works

   // AFTER (Option B): Renamed traits that match mental model
   use tmcp::{Client, McpClient};  // "I'm a client, I use McpClient"

   let mut client = Client::new("my-client", "1.0.0");
   client.connect_tcp("localhost:3000").await?;
   client.list_tools(None).await?;
   ```

2) `call_tool` requires verbose `Arguments` construction.
   - Evidence: Every example must do `Arguments::from_struct(params)?` before calling
     (`examples/basic_client.rs:99-100`, `examples/basic_client_stdio.rs:66-67`).
   - Ergonomic cost: Two lines and error handling for what should be a one-liner.
   - Recommendation: Accept `impl Serialize` directly in `call_tool` signature, or add
     `call_tool_typed<T: Serialize>(&mut self, name, params, task)` helper.

   ```rust
   // BEFORE: Verbose two-step process
   let params = EchoParams { message: "Hello".to_string() };
   let args = Arguments::from_struct(params)?;
   let result = client.call_tool("echo", Some(args), None).await?;

   // Extract response manually
   if let Some(ContentBlock::Text(text)) = result.content.first() {
       println!("{}", text.text);
   }

   // AFTER: Direct serializable input, optional typed output
   let params = EchoParams { message: "Hello".to_string() };
   let result = client.call_tool("echo", params, None).await?;

   // Or with typed response parsing
   let response: EchoResponse = client.call_tool_typed("echo", params, None).await?;
   println!("{}", response.echoed);
   ```

3) `#[mcp_server]` default capabilities are incorrect.
   - Evidence: Macro always emits `.with_tools(false)` regardless of tool count
     (`crates/tmcp-macros/src/lib.rs:382-383`). A server with tools incorrectly advertises
     no tools capability.
   - Risk: Clients may not request tools from a server that has them.
   - Recommendation: Emit `.with_tools(true)` when at least one `#[tool]` method exists.

   ```rust
   // BEFORE: Server has tools but advertises it doesn't
   #[mcp_server]
   impl MyServer {
       #[tool]
       async fn echo(&self, ctx: &ServerCtx, p: EchoParams) -> Result<CallToolResult> { ... }
   }
   // Generated: InitializeResult::new("my_server").with_tools(false)  // BUG!
   // Client sees: capabilities.tools = None, may skip calling list_tools

   // AFTER: Macro detects tools and sets capability correctly
   #[mcp_server]
   impl MyServer {
       #[tool]
       async fn echo(&self, ctx: &ServerCtx, p: EchoParams) -> Result<CallToolResult> { ... }
   }
   // Generated: InitializeResult::new("my_server").with_tools(true)
   // Client sees: capabilities.tools = Some(...), knows to call list_tools
   ```

## High (significant ergonomics/correctness issues)

4) Notification method names are inconsistent between contexts.
   - Evidence: `ClientCtx::send_notification` vs `ServerCtx::notify`
     (`crates/tmcp/src/context.rs:43-48` and `255-259`).
   - Recommendation: Unify naming (`notify` for both, or `send_notification` for both).

   ```rust
   // BEFORE: Inconsistent naming
   // In ClientHandler:
   context.send_notification(ClientNotification::initialized())?;

   // In ServerHandler:
   context.notify(ServerNotification::tool_list_changed())?;

   // AFTER: Consistent naming (using `notify`)
   // In ClientHandler:
   context.notify(ClientNotification::initialized())?;

   // In ServerHandler:
   context.notify(ServerNotification::tool_list_changed())?;
   ```

5) Align advertised server capabilities with actual runtime configuration.
   - Evidence: `Server::with_capabilities` only affects `ServerHandle` notification gating
     (`crates/tmcp/src/server.rs:76-79`), but the handshake capabilities are determined by
     `ServerHandler::initialize`. The macro default ignores `Server::capabilities` entirely.
   - Risk: Clients receive incorrect capability info; `ServerHandle::send_server_notification`
     may suppress notifications that the client was told to expect (or vice versa).
   - Recommendation: Make `Server` own the authoritative capability config and inject it into
     `initialize` responses, or pass capabilities from `Server` into macro-generated initialize.

   ```rust
   // BEFORE: Capabilities set in two places, can be inconsistent
   let server = Server::default()
       .with_handler(MyServer::default)
       .with_capabilities(ServerCapabilities::default().with_tools(Some(true)));
   // But MyServer::initialize returns different capabilities!
   // ServerHandle filters notifications based on Server::capabilities
   // Client sees capabilities from ServerHandler::initialize
   // These can disagree, causing confusion

   // AFTER: Single source of truth
   let server = Server::new(MyServer::default)
       .with_capabilities(ServerCapabilities::default()
           .with_tools(Some(true))
           .with_logging(Some(LoggingCapability::default())));
   // Macro-generated initialize uses Server's capabilities
   // ServerHandle uses same capabilities for filtering
   // Client sees consistent behavior
   ```

6) Tool-not-found errors are inconsistent and only partially mapped to JSON-RPC error codes.
   - Evidence: Macro-generated `call_tool` returns `Error::MethodNotFound`, while the
     `ServerHandler` default returns `Error::ToolExecutionFailed` (`crates/tmcp-macros/src/lib.rs:317`,
     `crates/tmcp/src/connection.rs:167-171`).
   - Recommendation: Standardize on `Error::ToolNotFound` for unknown tools everywhere, and ensure
     it maps to the proper JSON-RPC error code.

   ```rust
   // BEFORE: Different errors for same condition
   // Macro-generated:
   _ => Err(tmcp::Error::MethodNotFound(format!("Unknown tool: {}", name)))

   // ServerHandler default:
   Err(Error::ToolExecutionFailed { tool: name, message: "Tool not found".to_string() })

   // AFTER: Consistent error type
   // Both paths:
   Err(Error::ToolNotFound(name))

   // And in error.rs, map to proper JSON-RPC code:
   Self::ToolNotFound(name) => (METHOD_NOT_FOUND, format!("Tool not found: {name}"))
   ```

7) `Server::default()` can start without a handler and fail only at runtime.
   - Evidence: `Server` stores `connection_factory: Option<F>` and `serve_*` does not validate
     that it is set (`crates/tmcp/src/server.rs:25-38`). First failure occurs when a message
     arrives (line 297-299).
   - Risk: Runtime errors that look like protocol failures rather than configuration errors.
   - Recommendation: Either make the handler mandatory via a constructor, or fail early in
     `serve_*` when no handler is configured with a clear error message.

   ```rust
   // BEFORE: Compiles but fails at runtime with confusing error
   let server = Server::default();  // No handler!
   server.serve_tcp("127.0.0.1:3000").await?;
   // Later, when client connects: "No connection factory provided"

   // AFTER (Option A): Handler required in constructor
   let server = Server::new(MyHandler::default);  // Can't forget handler
   server.serve_tcp("127.0.0.1:3000").await?;

   // AFTER (Option B): Early validation with clear error
   let server = Server::default();
   server.serve_tcp("127.0.0.1:3000").await?;
   // Error: "Server has no handler configured. Use .with_handler() before serving."
   ```

8) `#[mcp_server]` version is static and should use crate version.
   - Evidence: Macro hard-codes `with_version("0.1.0")` (`crates/tmcp-macros/src/lib.rs:381`).
   - Recommendation: Default to `env!("CARGO_PKG_VERSION")` so server versions match the crate.

   ```rust
   // BEFORE: All servers report "0.1.0" regardless of actual version
   // In Cargo.toml: version = "2.3.4"
   // Client sees: server_info.version = "0.1.0"  // Wrong!

   // AFTER: Macro uses crate version automatically
   // In Cargo.toml: version = "2.3.4"
   // Generated code: .with_version(env!("CARGO_PKG_VERSION"))
   // Client sees: server_info.version = "2.3.4"  // Correct!

   // Users can still override:
   #[mcp_server(initialize_fn = custom_init)]
   impl MyServer {
       async fn custom_init(...) -> Result<InitializeResult> {
           Ok(InitializeResult::new("my_server").with_version("custom-1.0"))
       }
   }
   ```

## Medium (nice-to-have ergonomics improvements)

9) HTTP transport protocol version is duplicated and could drift.
   - Evidence: `MCP_PROTOCOL_VERSION` is a string literal in `http.rs` while protocol versions
     also exist in schema constants (`crates/tmcp/src/http.rs:42`,
     `crates/tmcp/src/schema/jsonrpc.rs:15`).
   - Recommendation: Derive the HTTP header value from `schema::LATEST_PROTOCOL_VERSION` or a
     single shared constant to prevent mismatch.

   ```rust
   // BEFORE: Two separate constants that can drift
   // http.rs:
   const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

   // schema/jsonrpc.rs:
   pub const LATEST_PROTOCOL_VERSION: &str = "2025-11-25";

   // AFTER: Single source of truth
   // http.rs:
   use crate::schema::LATEST_PROTOCOL_VERSION;
   // Use LATEST_PROTOCOL_VERSION for HTTP headers
   ```

10) `ToolSchema` drops most JSON Schema information when deriving from `schemars`.
    - Evidence: `ToolSchema::from_json_schema` only captures `$schema`, `type`, `properties`, and
      `required`, losing `description`, `enum`, `format`, `additionalProperties`, nested
      constraints, etc. (`crates/tmcp/src/schema/tools.rs:331-366`).
    - Risk: Clients see weaker schemas than intended; field descriptions not transmitted.
    - Recommendation: Store schema as `serde_json::Value` directly, or provide
      `ToolSchema::from_json_schema_full` that preserves the complete schema.

    ```rust
    // BEFORE: Rich schema information is lost
    #[derive(JsonSchema)]
    struct SearchParams {
        /// The search query string
        query: String,
        #[serde(default)]
        /// Maximum results (1-100)
        #[validate(range(min = 1, max = 100))]
        limit: Option<u32>,
    }
    // Client receives: { "type": "object", "properties": { "query": {...}, "limit": {...} } }
    // Lost: field descriptions, validation constraints, defaults

    // AFTER: Full schema preserved
    // Client receives: {
    //   "type": "object",
    //   "properties": {
    //     "query": { "type": "string", "description": "The search query string" },
    //     "limit": { "type": "integer", "description": "Maximum results (1-100)",
    //                "minimum": 1, "maximum": 100, "default": null }
    //   },
    //   "required": ["query"]
    // }
    ```

11) Requiring `Clone` on `ClientHandler` forces Arc-wrapping for state.
    - Evidence: `ClientHandler: Send + Sync + Clone` (`crates/tmcp/src/connection.rs:22`).
    - Ergonomic cost: Users must wrap any handler state in `Arc<Mutex<_>>` or similar.
    - Recommendation: Store `Arc<dyn ClientHandler>` internally, or add `with_handler_arc`
      constructor to reduce boilerplate for stateful handlers.

    ```rust
    // BEFORE: Must wrap state in Arc to satisfy Clone
    #[derive(Clone)]
    struct MyHandler {
        state: Arc<Mutex<HandlerState>>,  // Forced Arc wrapper
        config: Arc<Config>,               // Everything needs Arc
    }

    let client = Client::new("client", "1.0")
        .with_handler(MyHandler {
            state: Arc::new(Mutex::new(HandlerState::new())),
            config: Arc::new(config),
        });

    // AFTER: Handler stored as Arc internally, Clone not required
    struct MyHandler {
        state: Mutex<HandlerState>,  // No Arc needed
        config: Config,              // Direct ownership
    }

    let client = Client::new("client", "1.0")
        .with_handler(MyHandler {
            state: Mutex::new(HandlerState::new()),
            config,
        });
    // Or: .with_handler_arc(Arc::new(my_existing_handler))
    ```

## Suggestions (quality-of-life)

12) Provide typed tool-call convenience helpers on `Client`.
    - Example goal: `client.call_tool_typed::<EchoParams, EchoResult>("echo", params).await?`
      returning a deserialized type, reducing boilerplate around Arguments and content parsing.

    ```rust
    // BEFORE: Manual serialization and deserialization
    let params = SearchParams { query: "rust".into(), limit: Some(10) };
    let args = Arguments::from_struct(params)?;
    let result = client.call_tool("search", Some(args), None).await?;

    let response: SearchResult = if let Some(ContentBlock::Text(t)) = result.content.first() {
        serde_json::from_str(&t.text)?
    } else {
        return Err(Error::InvalidResponse);
    };

    // AFTER: One-liner with type inference
    let params = SearchParams { query: "rust".into(), limit: Some(10) };
    let response: SearchResult = client.call_tool_typed("search", params).await?;
    ```

13) Offer a `ClientBuilder`/`ServerBuilder` for name/version/capabilities/timeouts.
    - This would keep configuration centralized and reduce `with_*` chaining.

    ```rust
    // BEFORE: Chained with_* methods
    let client = Client::new("my-client", "1.0.0")
        .with_capabilities(ClientCapabilities::default()
            .with_roots(Some(RootsCapability { list_changed: Some(true) }))
            .with_sampling(Some(SamplingCapability::default())))
        .with_handler(MyHandler::new());

    // AFTER: Builder pattern with grouped configuration
    let client = Client::builder()
        .name("my-client")
        .version("1.0.0")
        .capabilities(|c| c
            .roots(true)
            .sampling(true))
        .handler(MyHandler::new())
        .build();
    ```

14) Return a handle for `serve_tcp` so servers can be shut down gracefully.
    - `serve_http` already returns `ServerHandle`; a TCP equivalent would make cleanup and
      tests cleaner (`crates/tmcp/src/server.rs:117-159`).

    ```rust
    // BEFORE: serve_tcp blocks forever, no way to stop
    server.serve_tcp("127.0.0.1:3000").await?;  // Never returns

    // AFTER: Returns handle like serve_http
    let handle = server.serve_tcp("127.0.0.1:3000").await?;

    // Later, in signal handler or test:
    handle.stop().await?;
    ```

15) Add example(s) showing server-initiated client API calls (`ServerCtx` + `ClientAPI`).
    - Today examples focus on tools; a minimal example of `ctx.list_roots()` or
      `ctx.create_message()` would clarify the role of `ClientAPI` and reduce confusion.

    ```rust
    // NEW EXAMPLE: Server calling client methods
    use tmcp::{ServerCtx, ClientAPI, schema::*};

    #[tool]
    async fn smart_tool(&self, ctx: &ServerCtx, params: Params) -> Result<CallToolResult> {
        // Server asks client to create a message (LLM sampling)
        let response = ctx.create_message(CreateMessageParams {
            messages: vec![...],
            max_tokens: 1000,
            ..Default::default()
        }).await?;

        // Server asks client for filesystem roots
        let roots = ctx.list_roots().await?;

        Ok(CallToolResult::new().with_text_content(response.message.text()))
    }
    ```

16) Add a `tmcp::prelude` module.
    - Include common imports: `Client`, `Server`, `ServerHandle`, `Result`, `Error`, `McpClient`
      (renamed `ServerAPI`), `McpServer` (renamed `ClientAPI`), `schema::*`, `schemars`.
    - Reduces typical import boilerplate from 5+ lines to `use tmcp::prelude::*`.

    ```rust
    // BEFORE: Multiple import lines needed
    use tmcp::{Client, Server, ServerHandle, Result, Error, ServerAPI, ClientAPI};
    use tmcp::schema::{CallToolResult, ContentBlock, Tool, ToolSchema};
    use tmcp::schemars;

    // AFTER: Single prelude import
    use tmcp::prelude::*;
    ```

## Completed (removed from active list)

- ~~Make initialization behavior consistent across all connect paths~~
  - COMPLETED: `connect_*` methods now all auto-initialize; `connect_*_raw` variants exist for
    manual initialization flows. Naming is now consistent and clear.

  ```rust
  // Current API (already good):
  client.connect_tcp("localhost:3000").await?;       // Connects AND initializes
  client.connect_stream(reader, writer).await?;      // Connects AND initializes
  client.connect_process(command).await?;            // Connects AND initializes

  // Raw variants for manual control:
  client.connect_stream_raw(reader, writer).await?;  // Connect only
  client.connect_process_raw(command).await?;        // Connect only
  client.init().await?;                              // Initialize manually
  ```

- ~~Ensure client handler hooks cannot send notifications before `initialize` completes~~
  - COMPLETED: `on_connect` is now called after `init()` succeeds, at the end of the
    initialization handshake (`crates/tmcp/src/client.rs:460`).

  ```rust
  // Current behavior (already correct):
  impl ClientHandler for MyHandler {
      async fn on_connect(&self, ctx: &ClientCtx) -> Result<()> {
          // This is called AFTER initialization handshake completes
          // Safe to send notifications here
          ctx.send_notification(ClientNotification::initialized())?;
          Ok(())
      }
  }
  ```
