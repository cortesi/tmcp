# TMCP Code Review: Staged Execution Checklist

Comprehensive review of ergonomics, design, and implementation correctness issues, ordered by priority.
Each stage groups related fixes that can be completed together while maintaining a consistent codebase.

---

## Stage 1: Critical Implementation Correctness

These issues could cause silent failures, data loss, or incorrect behavior in production.

1. [x] **Fix RequestId handling inconsistency** (`request_handler.rs`, `schema/jsonrpc.rs`)
   - Added `RequestId::to_key()` method to normalize both string and numeric IDs
   - Added `Display` impl for `RequestId` for better logging
   - Updated `handle_response()` and `handle_error()` to use `to_key()` for ID extraction
   - Both string and numeric IDs are now handled correctly
   - Added unit tests for ID conversion

2. [x] **Add request timeout and cleanup for pending requests** (`request_handler.rs`)
   - Added `DEFAULT_TIMEOUT_MS` constant (30 seconds)
   - Added `timeout_ms` field to `RequestHandler` with `with_timeout()` builder method
   - Integrated `tokio::time::timeout` into the request/response flow
   - Added `Error::Timeout` variant with request_id and timeout_ms fields
   - Pending requests are automatically cleaned up on timeout
   - Added `pending_count()` method for monitoring

3. [x] **Implement cancellation token propagation** (`request_handler.rs`)
   - Integrated `tokio_util::sync::CancellationToken` into `RequestHandler`
   - Each request gets a child token from the global cancellation token
   - Added `cancellation_token()`, `cancel_all()`, and `cancel_request()` methods
   - Added `Error::Cancelled` variant with request_id field
   - Request handling uses `tokio::select!` to respond to cancellation immediately
   - Added unit tests for cancellation functionality

---

## Stage 2: API Ergonomics (High Impact)

These changes significantly improve developer experience and prevent common mistakes.

4. [x] **Rename `with_connection` to clarify it takes a factory** (`server.rs:45-66`)
   - Renamed `with_connection` to `with_handler` for both Server and Client
   - Renamed `with_connection_factory` to `with_handler_factory` (internal)
   - Client now uses builder pattern: `Client::new(...).with_handler(...)` instead of `new_with_connection`
   - Updated all examples and tests to use new names

5. [x] **Improve `is_error(bool)` method ergonomics on CallToolResult** (`schema/tools.rs:89-92`)
   - Replaced `as_error(bool)` with `mark_as_error()` (no parameter)
   - Success is now the default — no need to call anything for success cases
   - Only call `mark_as_error()` when the tool result represents an error
   - Removed all redundant `.as_error(false)` calls from examples and tests

6. [x] **Rename ServerConn/ClientConn traits** (`connection.rs`)
   - Renamed `ServerConn` to `ServerHandler`
   - Renamed `ClientConn` to `ClientHandler`
   - Updated all examples, tests, and macro-generated code
   - Updated documentation to reflect new naming

7. [x] **Make Arguments::set() more ergonomic** (`arguments.rs`)
   - Added `insert()` method that takes `impl Into<Value>` for infallible chaining
   - Common types (strings, numbers, bools) can now be chained without `?`
   - Kept `set()` for complex types that need serialization

---

## Stage 3: Remove Code Duplication

Simplify the codebase by removing redundant types.

8. [ ] **Remove duplicate standalone request/response structs** (`schema/requests.rs:478-916`)
   - Duplicates: `InitializeRequest` vs `ClientRequest::Initialize`, `PingRequest` vs `ClientRequest::Ping`,
     `ListToolsRequest` vs `ClientRequest::ListTools`, etc.
   - The enum variants (`ClientRequest`, `ServerRequest`) are used for routing
   - The standalone structs appear unused in actual code paths
   - Audit all usages of standalone structs — if unused, remove them
   - If some are needed for external serialization, document why and consolidate

9. [ ] **Reconsider `_meta` field approach** (`tmcp-macros/src/lib.rs`, various schema types)
   - `#[with_meta]` macro adds `_meta: Option<HashMap<String, Value>>` to many types
   - Pollutes constructors: must always write `_meta: None`
   - Consider: `WithMeta<T>` wrapper type for cases that need metadata
   - OR: Add `_meta` only to types the MCP spec requires it on
   - Audit which types actually need `_meta` per spec

---

## Stage 4: Error Handling Improvements

Better error types make the library easier to use correctly.

10. [ ] **Create layered error hierarchy** (`error.rs`)
    - Current `Error` enum mixes I/O, protocol, and application errors
    - Create subcategories:
      - `TransportError` — I/O, connection, timeout issues
      - `ProtocolError` — JSON-RPC format, invalid messages, unknown methods
      - `ApplicationError` — tool failures, resource not found, invalid params
    - Main `Error` enum can wrap these or use `#[from]` for conversion
    - Users can then match on error category without knowing all variants

11. [ ] **Improve error context throughout codebase**
    - Many errors use generic strings: `Error::Protocol("Response channel closed")`
    - Add context: which request failed, what was expected vs received
    - Consider using `thiserror` more extensively with structured fields
    - Example: `ProtocolError::ResponseChannelClosed { request_id: String }`

---

## Stage 5: Documentation

Fill documentation gaps in public API.

12. [ ] **Remove `#[allow(missing_docs)]` from schema module** (`schema/mod.rs:1`)
    - Currently entire schema module has no doc requirement
    - Add doc comments to all public types, especially:
      - `ElicitSchema`, `ElicitParams`, `ElicitResult`
      - `PrimitiveSchemaDefinition` and its variants
      - `ResourceLink`, `ResourceContents`
      - All notification and request types

13. [ ] **Document the Cursor type and pagination pattern**
    - `Cursor` is just a string wrapper with no documentation on format
    - Document: what format servers should use, how clients should treat it
    - Add examples of pagination in list_tools, list_resources, etc.

14. [ ] **Add module-level documentation to core modules**
    - `transport.rs` — explain transport abstraction, how to implement custom transports
    - `connection.rs` — explain ServerConn/ClientConn lifecycle, when methods are called
    - `context.rs` — explain context usage, lifetime constraints, notification patterns
    - `request_handler.rs` — explain request correlation, response routing

---

## Stage 6: Architectural Improvements

Larger refactors that improve long-term maintainability.

15. [ ] **Add feature flags for optional components** (`Cargo.toml`)
    - Current: all features compiled unconditionally (HTTP, OAuth, all transports)
    - Add features:
      - `default = ["tcp"]` — minimal for subprocess servers
      - `http` — HTTP/SSE transport, pulls in axum/reqwest
      - `oauth` — OAuth2 support, pulls in oauth2 crate
      - `full` — everything
    - Gate code with `#[cfg(feature = "...")]`
    - Update CI to test feature combinations

16. [ ] **Consider GATs for Transport trait** (`transport.rs:32-39`)
    - Current: `fn framed(self: Box<Self>) -> Result<Box<dyn TransportStream>>`
    - With GATs: `type Stream: TransportStream; async fn connect(&mut self) -> Result<Self::Stream>`
    - Cleaner ownership, better type inference, less boxing
    - This is a breaking change — evaluate carefully

17. [ ] **Clarify RequestHandler ownership model** (`request_handler.rs`, `context.rs`, `client.rs`)
    - `RequestHandler` is cloned and shared between multiple owners
    - Document: which instance is authoritative, when cloning is safe
    - Consider: single owner with `Arc<RequestHandler>` passed as immutable refs
    - OR: document current design and why it's correct

18. [ ] **Add graceful shutdown protocol** (`server.rs`, `client.rs`)
    - Currently: shutdown just closes connection, pending requests dropped
    - Implement: send shutdown notification, wait for pending requests, then close
    - Add `shutdown_timeout` configuration
    - Ensure both sides handle peer-initiated shutdown gracefully

---

## Stage 7: Testing Improvements

Better test infrastructure and coverage.

19. [ ] **Fix flaky timeout in testutils** (`testutils.rs:121`)
    - Current: `timeout(Duration::from_millis(10), server.stop()).await.ok()`
    - 10ms is too short for CI machines under load
    - Change to 1000ms or make configurable
    - Don't swallow errors with `.ok()` — at least log them

20. [ ] **Add request ID test coverage**
    - Test: client sends request, server responds with same ID
    - Test: server sends request to client, client responds correctly
    - Test: numeric IDs work end-to-end (once fixed)
    - Test: mismatched ID is handled as error

21. [ ] **Add timeout/cancellation test coverage**
    - Test: request times out, caller gets `Error::Timeout`
    - Test: cancelled request stops processing
    - Test: server handles client disconnect mid-request
    - Test: client handles server disconnect mid-request

22. [ ] **Expand testutils with common server fixtures**
    - Add `EchoServer` — simple echo tool for basic tests
    - Add `SlowServer` — artificial delays for timeout testing
    - Add `FailingServer` — returns errors for error handling tests
    - Document how to use these in downstream crates

---

## Stage 8: Minor Cleanups

Small fixes that improve code quality.

23. [ ] **Fix doc comment placement in schema types**
    - Many places have doc comment after attribute instead of before
    - Example in `schema/tools.rs:14-16`: `#[serde(...)] /// Doc` should be `/// Doc #[serde(...)]`
    - Run through all schema files and fix ordering

24. [ ] **Use derive(Default) consistently** (`schema/tools.rs:101-105`)
    - `CallToolResult` has manual `Default` impl that just calls `new()`
    - Replace with `#[derive(Default)]` where possible
    - For types with `_meta` field, may need `#[default]` attribute on field

25. [ ] **Make re-exports explicit in lib.rs** (`lib.rs:96`, `schema/mod.rs:29-41`)
    - Current: `pub use api::*`, `pub use capabilities::*` — exports everything
    - List explicit public items: `pub use api::{ServerAPI, ClientAPI};`
    - Keeps public API surface intentional and documented
    - Makes it easier to track breaking changes

26. [ ] **Remove internal macros module wrapper** (`lib.rs:106-110`)
    - `mod macros { pub use ::tmcp_macros::*; }` alongside direct re-exports is confusing
    - Remove the wrapper, use `tmcp_macros::` directly where needed internally
    - Public API already re-exports `mcp_server` and `tool` at crate root

27. [ ] **Consider splitting large schema/requests.rs file**
    - File is 1000+ lines mixing many concepts
    - Options:
      - Split by domain: `schema/tools.rs`, `schema/resources.rs`, `schema/prompts.rs` (partially done)
      - Move standalone request structs to `schema/messages.rs`
      - Keep enums in `schema/requests.rs`
    - Re-export from `schema/mod.rs` to maintain API

---

## Completion Criteria

After all stages complete:
- [ ] All tests pass: `cargo nextest run --all --all-features`
- [ ] No clippy warnings: `cargo clippy --all --all-targets --all-features`
- [ ] Documentation builds: `cargo doc --all-features --no-deps`
- [ ] Examples still work: run each example in `examples/` directory
- [ ] Format check passes: `cargo +nightly fmt --all --check`
