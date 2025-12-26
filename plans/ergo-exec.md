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

# Stage 2: API Cleanup

Remove redundant API surface and fix naming issues. These are small breaking changes that should
be done early.

1. [ ] Remove the standalone `new_server()` function from `crates/tmcp/src/server.rs`. It
       duplicates `Server::new()`. Update any internal callers to use `Server::new()`. Ensure
       `new_server` is not re-exported from `lib.rs`.

2. [ ] Rename `call_tool_typed` to `call_tool_json` in `crates/tmcp/src/client.rs` to better
       reflect that it deserializes JSON from the first text content block. Update documentation
       to clarify this behavior.

3. [ ] Verify `Cursor` implements `From<&str>`. If not, add the implementation in
       `crates/tmcp/src/schema/mod.rs` or the appropriate location. This allows `"cursor".into()`
       to work directly.

---

# Stage 3: Builder Improvements for Schema Types

Add ergonomic builders for commonly constructed types that currently require verbose nesting.

1. [ ] Add `SamplingMessage::user_text(text: impl Into<String>) -> Self` constructor in
       `crates/tmcp/src/schema/sampling.rs`. It creates a user role message with text content.

2. [ ] Add `SamplingMessage::assistant_text(text: impl Into<String>) -> Self` constructor in the
       same file. It creates an assistant role message with text content.

3. [ ] Add `CreateMessageParams::user_message(text: impl Into<String>) -> Self` convenience
       constructor that creates a params struct with a single user text message and reasonable
       defaults (max_tokens: 1024).

4. [ ] Add `with_max_tokens(mut self, tokens: i64) -> Self` builder method to `CreateMessageParams`.

5. [ ] Update `examples/server_client_calls.rs` to use the new `CreateMessageParams::user_message()`
       builder instead of the verbose struct construction. Verify the example works.

---

# Stage 4: OneOrMany Ergonomics

Add iterator and accessor methods to the `OneOrMany<T>` type to eliminate verbose pattern matching.

1. [ ] Locate `OneOrMany<T>` definition (likely in `crates/tmcp/src/schema/mod.rs` or a content
       module). Add `fn iter(&self) -> impl Iterator<Item = &T>` method.

2. [ ] Add `fn first(&self) -> Option<&T>` method to `OneOrMany<T>`.

3. [ ] Add `fn into_vec(self) -> Vec<T>` method to `OneOrMany<T>`.

4. [ ] Add `fn len(&self) -> usize` and `fn is_empty(&self) -> bool` methods to `OneOrMany<T>`.

5. [ ] Update `examples/client_with_connection.rs` to use the new iterator methods instead of
       manual pattern matching on `OneOrMany`. Verify the example works.

---

# Stage 5: Tool Construction Ergonomics

Add type-safe tool construction from schemars types, bridging the gap between macro-based and
trait-based server implementations.

1. [ ] Add `Tool::from_schema<T: schemars::JsonSchema>(name: impl Into<String>) -> Self` constructor
       in `crates/tmcp/src/schema/tools.rs`. It uses `ToolSchema::from_json_schema::<T>()` and sets
       the tool name. The description can be extracted from the schema's title/description if present.

2. [ ] Add `Tool::with_schema<T: schemars::JsonSchema>(mut self) -> Self` method that replaces the
       input_schema using the type's JSON schema.

3. [ ] Create a simple example or test demonstrating `Tool::from_schema::<MyParams>("tool_name")`
       as an alternative to manual `ToolSchema::default().with_property(...)` construction.

4. [ ] Consider adding `ToolSchema::empty()` as an alias for `ToolSchema::default()` for clarity
       when a tool takes no arguments (the default is `{"type": "object"}`).

---

# Stage 6: Documentation and Naming Clarity

Improve documentation and fix remaining naming issues.

1. [ ] Add documentation to `ServerHandler` trait in `crates/tmcp/src/connection.rs` explaining
       the default behavior philosophy: methods for optional features (list_tools, list_resources)
       return empty results, while dispatch methods (call_tool, read_resource) return errors.

2. [ ] Document the `#[mcp_server]` macro vs `ServerHandler` trait trade-offs in
       `crates/tmcp/src/lib.rs` module documentation. Explain when to use each approach:
       - Macro: simple servers, automatic tool registration, less boilerplate
       - Trait: custom initialization, per-connection state, complex capability negotiation

3. [ ] Document `Client::with_handler()` type state pattern. Explain that it changes the client
       type from `Client<()>` to `Client<H>` and why this is the design choice.

4. [ ] Add a doc comment on `call_tool("tool", ())` explaining that `()` is the idiomatic way to
       call tools with no arguments (it serializes to an empty JSON object).

---

# Stage 7: ProcessConnection Rename (Breaking)

Rename `ProcessConnection` for clarity. This is a breaking change for users of `connect_process()`.

1. [ ] Rename `ProcessConnection` to `SpawnedServer` in `crates/tmcp/src/client.rs`.

2. [ ] Rename the `child` field to `process` and `init` field to `server_info` for clarity.

3. [ ] Update `crates/tmcp/src/lib.rs` to re-export `SpawnedServer` instead of `ProcessConnection`.

4. [ ] Update `examples/process_spawn.rs` to use the new name and field names.

5. [ ] Add a deprecation note in CHANGELOG or migration guide about this rename.

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
