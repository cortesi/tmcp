# tmcp Ergonomics Execution Plan

This is the staged execution plan for the tmcp ergonomics and correctness review. Each stage
contains related changes and leaves the system in a consistent, testable state.

**Breaking changes are allowed** - we prioritize the cleanest API over backwards compatibility.

Source: [./ergo.md](./ergo.md)


## 1. Stage One: Critical Correctness Fixes ✓

These items fix bugs or incorrect behavior that could cause runtime failures or protocol
violations. Must be fixed before other changes.

1. [x] **Fix `#[mcp_server]` default capabilities** (ergo.md item 3)
   - Location: `crates/tmcp-macros/src/lib.rs:382-383`
   - Problem: Macro always emits `.with_tools(false)` regardless of whether `#[tool]` methods exist
   - Fix: Count `#[tool]` methods during macro expansion; emit `.with_tools(true)` when count > 0
   - Also: Fixed macro to use `env!("CARGO_PKG_VERSION")` for version instead of hardcoded "0.1.0"
   - Test: Create server with tools, verify `initialize` response has `capabilities.tools = Some(...)`

2. [x] **Standardize tool-not-found errors** (ergo.md item 6)
   - Locations: `crates/tmcp-macros/src/lib.rs:317`, `crates/tmcp/src/connection.rs:167-171`
   - Problem: Macro returns `Error::MethodNotFound`, ServerHandler default returns
     `Error::ToolExecutionFailed` for the same condition
   - Fix: Updated both locations to use existing `Error::ToolNotFound(String)` variant
   - Test: Call unknown tool, verify consistent error type and JSON-RPC error code

3. [x] **Require handler in Server constructor** (ergo.md item 7)
   - Location: `crates/tmcp/src/server.rs:25-38, 297-299`
   - Problem: `Server::default()` compiles but fails at runtime with confusing error when client
     connects
   - Fix: Added `Server::new(handler)` constructor; removed `Default` impl and `with_handler()`
   - Added `Server::from_factory()` (crate-internal) for pre-boxed handler factories
   - Test: Verify `Server::new(MyHandler::default)` is the only way to construct; old patterns
     fail to compile


## 2. Stage Two: API Naming Consistency ✓

These changes fix confusing naming that makes the API harder to learn and use correctly.

1. [x] **Move API methods to inherent impls** (ergo.md item 1)
   - Location: `crates/tmcp/src/api.rs:19-109`
   - Problem: `ServerAPI` contains methods clients call on servers, but users import it to use the
     `Client` struct - naming is backwards from user's perspective
   - Fix: Moved all methods from `ServerAPI` trait to inherent impl on `Client`; moved all methods
     from `ClientAPI` trait to inherent impl on `ServerCtx`; deleted both traits and api.rs
   - Also: Removed unused `request_handler` field and `request` method from `ClientCtx` since it was
     only used by the deleted `impl ServerAPI for ClientCtx`
   - Result: `use tmcp::Client` is all users need - no confusing trait imports
   - Updated all examples, tests, and mcptool
   - Test: All examples compile with just `use tmcp::Client`

2. [x] **Unify notification method names to `notify`** (ergo.md item 4)
   - Location: `crates/tmcp/src/context.rs` (ClientCtx and ServerCtx)
   - Problem: `ClientCtx::send_notification` vs `ServerCtx::notify` - inconsistent naming
   - Fix: Renamed `ClientCtx::send_notification` to `notify`
   - Updated all usages in examples, tests, and mcptool
   - Test: Notification sending works from both contexts


## 3. Stage Three: Capabilities & Configuration Alignment ✓

These changes ensure configuration is consistent and doesn't require duplication.

1. [x] **Remove `Server::with_capabilities`, make handler authoritative** (ergo.md item 5)
   - Locations: `crates/tmcp/src/server.rs:76-79`, macro-generated initialize
   - Problem: `Server::with_capabilities` only affects `ServerHandle` notification gating, but
     handshake capabilities come from `ServerHandler::initialize` - can be inconsistent
   - Fix: Remove `Server::with_capabilities` entirely; `ServerHandle` should read capabilities from
     the initialize response that was already sent; handler is single source of truth
   - Implementation: Added `Arc<RwLock<ServerCapabilities>>` to `ServerHandle`, captured from
     handler's initialize response via new `handle_initialize_request` function. Removed
     `capabilities` field and `with_capabilities()` method from `Server` struct.
   - Test: Verify ServerHandle uses capabilities from initialize handshake

2. [x] **Use crate version in `#[mcp_server]` generated code** (ergo.md item 8)
   - Location: `crates/tmcp-macros/src/lib.rs:381`
   - Problem: Macro hard-codes `.with_version("0.1.0")` instead of using actual crate version
   - Fix: Generate `.with_version(env!("CARGO_PKG_VERSION"))` in macro output
   - Note: Already fixed in Stage 1 (item 1 noted the version fix)
   - Test: Build server, verify version matches Cargo.toml

3. [x] **Deduplicate protocol version constant** (ergo.md item 9)
   - Locations: `crates/tmcp/src/http.rs:42`, `crates/tmcp/src/schema/jsonrpc.rs:15`
   - Problem: `MCP_PROTOCOL_VERSION` duplicated, could drift
   - Fix: Remove `http.rs` constant; use `schema::LATEST_PROTOCOL_VERSION` everywhere
   - Test: Verify HTTP headers use correct protocol version


## 4. Stage Four: Ergonomics Improvements ✓

These changes reduce boilerplate and make common operations easier.

1. [x] **Accept `impl Serialize` in `call_tool`** (ergo.md item 2)
   - Location: `crates/tmcp/src/client.rs` (now inherent impl after Stage 2)
   - Problem: Every call_tool requires `Arguments::from_struct(params)?` boilerplate
   - Fix: Changed `call_tool` to accept `impl Serialize + Send`; conversion done internally
   - Added `call_tool_with_task` for passing task metadata
   - Added `call_tool_typed<R: DeserializeOwned>` for typed responses
   - Test: Call tool with struct directly, verify serialization works

2. [x] **Store full JSON Schema in ToolSchema** (ergo.md item 10)
   - Location: `crates/tmcp/src/schema/tools.rs`
   - Problem: `ToolSchema::from_json_schema` dropped description, enum, format, constraints, etc.
   - Fix: Changed `ToolSchema` to transparent wrapper around `serde_json::Value`; added getter
     methods `schema_type()`, `properties()`, `required()` for backwards compatibility
   - `from_json_schema<T>()` now preserves full schema including descriptions
   - Updated all usages to builder pattern: `ToolSchema::default().with_property(...).with_required(...)`
   - Test: Derive JsonSchema with field descriptions, verify they appear in tool listing

3. [x] **Remove Clone requirement from ClientHandler** (ergo.md item 11)
   - Location: `crates/tmcp/src/connection.rs:22`, `crates/tmcp/src/client.rs`
   - Problem: `ClientHandler: Clone` forces users to Arc-wrap all state
   - Fix: Store handler in `Arc<C>` internally; removed Clone from trait bounds
   - Removed now-unnecessary manual Clone implementations from test handlers
   - Test: Create stateful handler without Clone, verify it compiles and works


## 5. Stage Five: Quality of Life Additions

These are additive changes that improve developer experience.

1. [x] **Add typed tool-call helper** (ergo.md item 12)
   - Location: `crates/tmcp/src/client.rs`
   - Note: Already implemented as part of Stage 4 item 1
   - Added: `client.call_tool_typed::<R>(name, params).await?` returning deserialized `R`
   - Handles: Serialization, Arguments construction, result parsing, error mapping
   - Test: Call echo tool with typed params and response

2. [x] **Add `serve_tcp` shutdown handle** (ergo.md item 14)
   - Location: `crates/tmcp/src/server.rs`
   - Problem: `serve_tcp` blocked forever with no graceful shutdown
   - Fix: Added `TcpServerHandle` struct with `stop()` method; `serve_tcp` now returns handle
   - Uses `CancellationToken` and `tokio::select!` to check for shutdown in accept loop
   - Updated `basic_server.rs` and `timeout_server.rs` examples to use graceful shutdown
   - Test: Start server, shut down via handle, verify clean exit

3. [x] **Add server-initiated client API example** (ergo.md item 15)
   - Location: `examples/server_client_calls.rs`
   - Added example demonstrating `ServerCtx` methods: `ping()`, `list_roots()`,
     `create_message()`, `elicit()`
   - Each capability exposed as a callable tool
   - Includes comments explaining when/why server would call client

4. [ ] **Add `tmcp::prelude` module** (ergo.md item 16)
   - Location: New `crates/tmcp/src/prelude.rs`
   - Include: `Client`, `Server`, `ServerHandle`, `Result`, `Error`, common schema types, `schemars`
   - Update examples to use `use tmcp::prelude::*`
   - Test: Verify examples compile with prelude import only


## Notes

- Run `cargo clippy` and `cargo test` after each stage
- Run `cargo +nightly fmt` before any commits
- Update examples and README as APIs change
- Items 17-18 from ergo.md are already completed (marked in source)
- Builder pattern (ergo.md item 13) skipped - current `with_*` chaining is already idiomatic Rust
