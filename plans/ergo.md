# tmcp ergonomics + correctness review (2025-12-24)

Scope:
- Public API surface: `crates/tmcp`, `crates/tmcp-macros`
- Usage ergonomics: `examples/*` and the README example

## Errors (correctness risks / protocol footguns)

1) Make initialization behavior consistent across all connect paths.
   - Evidence: `Client::connect_tcp/connect_http/connect_stdio` auto-call `init`, but
     `connect_stream` and `connect_process` do not. (`crates/tmcp/src/client.rs:117-226`)
     The examples show manual `init` in stdio/process flows but not in stream flows
     (`examples/basic_client_stdio.rs:35-41`, `examples/process_spawn.rs`,
     `examples/stream_connection.rs:35-41`).
   - Risk: users can accidentally skip `initialize`, violating MCP protocol or getting
     hard-to-debug failures.
   - Recommendation: either (a) make every `connect_*` call `init` and return
     `InitializeResult`, or (b) rename raw variants to `connect_*_raw`/`connect_uninitialized`
     and add `connect_*_and_init` helpers; update examples accordingly.

2) Ensure client handler hooks cannot send notifications before `initialize` completes.
   - Evidence: `Client::start_message_handler` calls `ClientHandler::on_connect` before
     `Client::init` is executed (`crates/tmcp/src/client.rs:246-270`). The example handler
     sends `ClientNotification::Initialized` inside `on_connect`
     (`examples/client_with_connection.rs:17-22`).
   - Risk: clients can emit `notifications/initialized` before `initialize` finishes, which
     violates MCP ordering and can confuse servers.
   - Recommendation: add an `on_initialized` hook or move the `on_connect` callback to run
     after `init` succeeds; or make `connect_*` perform init and call hooks in order.

3) Align advertised server capabilities with actual runtime configuration.
   - Evidence: `Server::with_capabilities` only affects `ServerHandle` notification gating
     (`crates/tmcp/src/server.rs:74-185`), but the handshake capabilities are determined by
     `ServerHandler::initialize` (macro default or manual). The macro default ignores
     `Server::capabilities` entirely (`crates/tmcp-macros/src/lib.rs:372-401`).
   - Risk: clients receive incorrect capability info; `ServerHandle::send_server_notification`
     may suppress notifications that the client was told to expect (or vice versa).
   - Recommendation: make `Server` own the authoritative capability config and inject it into
     `initialize` responses, or pass a shared `ServerConfig` into the generated initialize
     implementation.

## Warnings (ergonomics + inconsistency)

4) Requiring `ServerAPI` to call client methods is counterintuitive.
   - Evidence: all client examples must import `ServerAPI` to call `client.list_tools()` etc.
     (`examples/basic_client.rs:5`, `examples/basic_client_stdio.rs:11`).
   - Ergonomic cost: users must remember a trait named after the *server* to use the client.
   - Recommendation: add inherent methods on `Client`, or export a `tmcp::prelude::*` that
     includes `ServerAPI`. Consider renaming traits to `ClientRequests` / `ServerRequests` or
     similar to match call sites.

5) Notification method names are inconsistent between contexts.
   - Evidence: `ClientCtx::send_notification` vs `ServerCtx::notify`
     (`crates/tmcp/src/context.rs:40-45` and `235-240`).
   - Recommendation: unify naming (`notify` or `send_notification`) for both contexts.

6) `Server::default()` can start without a handler and fail only after a client sends a message.
   - Evidence: `Server` stores `connection_factory: Option<F>` and `serve_*` does not validate
     that it is set (`crates/tmcp/src/server.rs:23-157`).
   - Risk: runtime errors that look like protocol failures rather than configuration errors.
   - Recommendation: make the handler mandatory via a constructor or typestate, or error early
     in `serve_*` when no handler is configured.

7) Tool-not-found errors are inconsistent and only partially mapped to JSON-RPC error codes.
   - Evidence: macro-generated `call_tool` returns `Error::MethodNotFound`, while the
     `ServerHandler` default returns `Error::ToolExecutionFailed` (`crates/tmcp-macros/src/lib.rs:307-318`,
     `crates/tmcp/src/connection.rs:116-127`). `Error::to_jsonrpc_response` only maps a subset
     of errors (`crates/tmcp/src/error.rs:141-162`).
   - Recommendation: standardize on `ToolNotFound`/`MethodNotFound` for unknown tools and map
     tool execution errors consistently to spec error codes (or include `data` with tool name).

8) `#[mcp_server]` default metadata is static and can be wrong.
   - Evidence: macro hard-codes `with_version("0.1.0")` and always sets `with_tools(false)`
     regardless of tool count (`crates/tmcp-macros/src/lib.rs:377-388`).
   - Recommendation: default to `env!("CARGO_PKG_VERSION")` and skip tools capability when no
     tools are registered; avoid `with_description("")` by only setting descriptions when docs
     are non-empty.

9) `ToolSchema` drops most JSON Schema information when deriving from `schemars`.
   - Evidence: `ToolSchema::from_json_schema` only captures `type`, `properties`, and
     `required`, losing `description`, `enum`, `format`, nested constraints, etc.
     (`crates/tmcp/src/schema/tools.rs:260-306`).
   - Risk: clients see weaker schemas than intended.
   - Recommendation: represent schema as `serde_json::Value` (or `schemars::schema::RootSchema`)
     in `ToolSchema`, or add an alternate `ToolSchema::from_json_schema_full` and a
     `ToolSchema::from_value` path.

10) HTTP transport protocol version is duplicated and could drift.
    - Evidence: `MCP_PROTOCOL_VERSION` is a string literal in `http.rs` while protocol
      versions also exist in schema constants (`crates/tmcp/src/http.rs:41-44`).
    - Recommendation: derive the HTTP header value from `schema::LATEST_PROTOCOL_VERSION` or a
      single shared constant to prevent mismatch.

## Suggestions (quality-of-life)

11) Provide typed tool-call convenience helpers on `Client`.
    - Example goal: `client.call_tool_typed("echo", params).await?` returning a deserialized
      type, or `Arguments::try_from(params)` to reduce boilerplate.

12) Offer a `ServerBuilder`/`ClientBuilder` for name/version/capabilities/timeouts.
    - This would keep configuration centralized and reduce `with_*` chaining, while allowing
      `HttpClientTransport` configuration (timeouts, headers, proxy settings).

13) Return a handle for `serve_tcp` so servers can be shut down gracefully.
    - `serve_http` already returns `ServerHandle`; a TCP equivalent would make cleanup and
      tests cleaner (`crates/tmcp/src/server.rs:114-171`).

14) Remove the `Clone` bound from `ClientHandler` by storing `Arc<dyn ClientHandler>` in
    `Client`, or add `Client::with_handler_arc` to reduce boilerplate for stateful handlers.

15) Add example(s) showing server-initiated client API calls (`ServerCtx` + `ClientAPI`).
    - Today examples focus on tools; a minimal example of `ctx.list_roots()` or
      `ctx.create_message()` would clarify the role of `ClientAPI` and reduce confusion.
