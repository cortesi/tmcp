# API Ergonomics Execution Plan

This plan implements ergonomic improvements to the tmcp API in stages. Each stage leaves the
system in a consistent state with all tests passing. Items are ordered to minimize churn and
maximize incremental value.

---

# Stage 1: Quick Wins - Convenience Methods ✓ COMPLETE

Add convenience methods to existing types. These are additive changes with no breaking changes,
providing immediate ergonomic value.

1. [x] Add `text()`, `all_text()`, and `json<T>()` convenience methods to `CallToolResult` in
       `crates/tmcp/src/schema/tools.rs`. The `text()` method returns `Option<&str>` for the first
       text content block. The `all_text()` method concatenates all text blocks. The `json<T>()`
       method deserializes the first text block as JSON. Add unit tests.

2. [x] Add `require_string()`, `require_i64()`, `require_bool()`, and `require<T>()` methods to
       `Arguments` in `crates/tmcp/src/arguments.rs`. These return `Result<T>` with proper
       `Error::InvalidParams` messages when the parameter is missing. Add unit tests.

3. [x] Add `into_params<T: DeserializeOwned>(self) -> Result<T>` to `Arguments` that wraps
       `deserialize()` with a proper `Error::InvalidParams` error message. Add unit tests.

4. [x] Update `examples/server_client_calls.rs` to use the new `require_string()` method instead
       of the verbose `ok_or_else` chains. Verify the example still compiles and runs.

5. [x] Update `examples/basic_client.rs` to use `result.text()` instead of pattern matching on
       `ContentBlock::Text`. Verify the example still compiles and runs.

6. [x] Update `examples/basic_client_stdio.rs` to use `result.text()`.

7. [x] Update `examples/process_spawn.rs` to use `result.text()` and `Arguments::insert()`.

---

# Stage 2: API Cleanup ✓ COMPLETE

Remove redundant API surface and fix naming issues. These are small breaking changes that should
be done early.

1. [x] Remove the standalone `new_server()` function from `crates/tmcp/src/server.rs`. It
       duplicated `Server::new()`. Inlined its implementation directly into `Server::new()`.

2. [x] Rename `call_tool_typed` to `call_tool_json` in `crates/tmcp/src/client.rs` to better
       reflect that it deserializes JSON from the first text content block. Updated documentation
       to clarify this behavior. Also refactored to use `result.text()` from Stage 1.

3. [x] Verified `Cursor` already implements `From<&str>` at `schema/jsonrpc.rs:43`. No changes
       needed.

---

# Stage 3: Builder Improvements for Schema Types ✓ COMPLETE

Add ergonomic builders for commonly constructed types that currently require verbose nesting.

1. [x] Add `SamplingMessage::user_text(text: impl Into<String>) -> Self` constructor in
       `crates/tmcp/src/schema/sampling.rs`. It creates a user role message with text content.

2. [x] Add `SamplingMessage::assistant_text(text: impl Into<String>) -> Self` constructor in the
       same file. It creates an assistant role message with text content.

3. [x] Add `CreateMessageParams::user_message(text: impl Into<String>) -> Self` convenience
       constructor that creates a params struct with a single user text message and reasonable
       defaults (max_tokens: 1024).

4. [x] Add `with_max_tokens(mut self, tokens: i64) -> Self` builder method to `CreateMessageParams`.
       Also added bonus builders: `with_system_prompt()` and `with_temperature()`.

5. [x] Update `examples/server_client_calls.rs` to use the new `CreateMessageParams::user_message()`
       builder instead of the verbose struct construction. Verified the example works.

---

# Stage 4: OneOrMany Ergonomics ✓ COMPLETE

Add iterator and accessor methods to the `OneOrMany<T>` type to eliminate verbose pattern matching.

1. [x] Locate `OneOrMany<T>` definition in `crates/tmcp/src/schema/content.rs`. Added
       `fn iter(&self) -> impl Iterator<Item = &T>` and `iter_mut()` methods.

2. [x] Add `fn first(&self) -> Option<&T>` method to `OneOrMany<T>`.

3. [x] `into_vec(self) -> Vec<T>` already existed. Added documentation.

4. [x] Add `fn len(&self) -> usize` and `fn is_empty(&self) -> bool` methods to `OneOrMany<T>`.

5. [x] Update `examples/client_with_connection.rs` to use the new `iter()` method and
       `SamplingMessage::assistant_text()` builder. Updated tests in `bidi.rs` and
       `client_server_ping.rs` to also use the new ergonomic APIs.

Bonus: Implemented `IntoIterator` for `OneOrMany<T>` and `&OneOrMany<T>` for use in for loops.

---

# Stage 5: Tool Construction Ergonomics ✓ COMPLETE

Add type-safe tool construction from schemars types, bridging the gap between macro-based and
trait-based server implementations.

1. [x] Add `Tool::from_schema<T: schemars::JsonSchema>(name: impl Into<String>) -> Self` constructor
       in `crates/tmcp/src/schema/tools.rs`. Extracts description from schema if present.

2. [x] Add `Tool::with_schema<T: schemars::JsonSchema>(mut self) -> Self` method that replaces the
       input_schema using the type's JSON schema.

3. [x] Created unit tests demonstrating `Tool::from_schema::<MyParams>("tool_name")` usage and
       verifying schema properties are correctly derived.

4. [x] Add `ToolSchema::empty()` as an alias for `ToolSchema::default()` for clarity when a tool
       takes no arguments.

5. [x] Updated `examples/server_client_calls.rs` to demonstrate `Tool::from_schema<T>()` with typed
       parameter structs (`AskLlmParams`, `AskUserParams`) and `into_params::<T>()` for type-safe
       deserialization. Used `ToolSchema::empty()` for tools with no parameters.

6. [x] Updated mcptool's `testserver.rs` to use `Tool::from_schema::<EchoParams>("echo")` instead of
       verbose `ToolSchema::default().with_property().with_required()` construction. Also uses
       `into_params::<EchoParams>()` in the handler for type-safe deserialization.

Bonus: Added `ToolSchema::description()` and `ToolSchema::title()` accessor methods for extracting
metadata from JSON schemas.

---

# Stage 6: Documentation and Naming Clarity ✓ COMPLETE

Improve documentation and fix remaining naming issues.

1. [x] Added comprehensive documentation to `ServerHandler` trait in `crates/tmcp/src/connection.rs`
       explaining the default behavior philosophy:
       - Listing methods (list_tools, list_resources, list_prompts) return empty results
       - Dispatch methods (call_tool, read_resource, get_prompt) return errors
       - Lifecycle methods (on_connect, on_shutdown, notification) are no-ops

2. [x] Documented the `#[mcp_server]` macro vs `ServerHandler` trait trade-offs in
       `crates/tmcp/src/lib.rs` module documentation with code examples for each approach.

3. [x] Documented `Client::with_handler()` type state pattern in `crates/tmcp/src/client.rs`.
       Explains how it transforms `Client<()>` to `Client<C>` at compile time.

4. [x] Enhanced `call_tool()` documentation in `crates/tmcp/src/client.rs` explaining that `()`
       serializes to an empty JSON object `{}` and is the idiomatic way to call parameter-less tools.

---

# Stage 7: ProcessConnection Rename (Breaking) ✓ COMPLETE

Rename `ProcessConnection` for clarity. This is a breaking change for users of `connect_process()`.

1. [x] Renamed `ProcessConnection` to `SpawnedServer` in `crates/tmcp/src/client.rs` with improved
       documentation explaining its purpose.

2. [x] Renamed the `child` field to `process` and `init` field to `server_info` for clarity.
       Added documentation to both fields.

3. [x] Updated `crates/tmcp/src/lib.rs` to re-export `SpawnedServer` instead of `ProcessConnection`.

4. [x] Updated `examples/process_spawn.rs` and `examples/basic_client_stdio.rs` to use the new
       struct name and field names.

5. [x] No CHANGELOG file exists in the project. Breaking change is documented in commit message.

---

# Stage 8: Capabilities Builder (Optional Enhancement)

Create a fluent capabilities builder for `InitializeResult` to replace boolean parameter confusion.

1. [ ] Create `ServerCapabilitiesBuilder` in `crates/tmcp/src/schema/capabilities.rs` with methods:
       - `fn new() -> Self`
       - `fn tools(mut self) -> Self` - enables tools with list_changed
       - `fn resources(mut self) -> ResourcesBuilder` - returns sub-builder
       - `fn prompts(mut self) -> Self` - enables prompts with list_changed
       - `fn logging(mut self) -> Self` - enables logging
       - `fn build(self) -> ServerCapabilities`

2. [ ] Create `ResourcesBuilder` with `with_subscribe(mut self) -> Self` and
       `with_list_changed(mut self) -> Self` methods, returning to parent builder on build.

3. [ ] Add `InitializeResult::with_capabilities_builder(builder: ServerCapabilitiesBuilder) -> Self`
       method as an alternative to the current `with_tools(bool)`, `with_resources(bool, bool)` etc.

4. [ ] Keep existing `with_*` methods for backwards compatibility but consider deprecating them
       in favor of the builder pattern in a future major version.

---

# Future Considerations (Not in this plan)

The following items are noted for future consideration but require more design work or are
larger breaking changes:

- **Transport enum for Client::connect()** - Would unify 8 connect methods but requires careful
  API design for the raw variants and OAuth integration.

- **ToolRegistry helper** - Would eliminate list_tools/call_tool sync issues but adds complexity.
  Consider whether the macro approach is sufficient for most use cases first.

- **Richer error types** - Adding available tools to `ToolNotFound` or structured `InvalidParams`
  would be helpful but requires careful consideration of error handling patterns across the crate.

- **ServerCtx notification helpers** - The `context.notify()` pattern works but could be made more
  discoverable. Consider adding examples to documentation first before API changes.
