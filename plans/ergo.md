# API Ergonomic Improvements

A ranked list of ergonomic improvements to the tmcp API, focusing on clarity, expressiveness,
elegance, and coherence. Each item identifies a pain point observed in examples and proposes a
solution.

---

## Priority 1: High Impact

### 1.1 ToolSchema Construction is Awkward

**Problem:** Creating tool schemas manually in `server_client_calls.rs` requires verbose JSON
construction:

```rust
Tool::new(
    "ask_llm",
    ToolSchema::default()
        .with_property(
            "prompt",
            json!({
                "type": "string",
                "description": "The prompt to send to the LLM"
            }),
        )
        .with_required("prompt"),
)
```

Compare to the elegant macro approach in `basic_server.rs`:

```rust
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
struct EchoParams {
    /// The message to echo back
    message: String,
}

#[tool]
async fn echo(&self, _context: &ServerCtx, params: EchoParams) -> Result<CallToolResult>
```

**Recommendation:** Add a convenience constructor that accepts a `JsonSchema` type directly:

```rust
// Instead of manual JSON construction:
Tool::new("ask_llm", ToolSchema::default().with_property(...))

// Allow:
Tool::new("ask_llm", AskLlmParams::schema())
// or:
Tool::from_params::<AskLlmParams>("ask_llm")
```

This would let trait-based implementations get the same ergonomics as macro-based ones.


### 1.2 CallToolResult Response Extraction is Verbose

**Problem:** In `basic_client.rs` (lines 102-105), extracting text from a tool result requires
pattern matching:

```rust
if let Some(schema::ContentBlock::Text(text_content)) = result.content.first() {
    info!("Response: {}", text_content.text);
}
```

This pattern appears in multiple examples and is error-prone.

**Recommendation:** Add convenience methods to `CallToolResult`:

```rust
impl CallToolResult {
    /// Get the first text content, if any.
    pub fn text(&self) -> Option<&str> { ... }

    /// Get all text content concatenated.
    pub fn all_text(&self) -> String { ... }

    /// Try to deserialize the first text content as JSON.
    pub fn json<T: DeserializeOwned>(&self) -> Result<T> { ... }
}

// Usage becomes:
if let Some(text) = result.text() {
    info!("Response: {}", text);
}
```


### 1.3 Trait Implementation Requires Verbose Boilerplate

**Problem:** In `timeout_server.rs` and `server_client_calls.rs`, implementing `ServerHandler`
manually requires implementing `initialize`, `list_tools`, AND `call_tool` with matching logic. The
tool definitions in `list_tools` must be kept in sync with the `call_tool` dispatch manually.

```rust
async fn list_tools(...) -> Result<ListToolsResult> {
    Ok(ListToolsResult::new()
        .with_tool(Tool::new("flakey_operation", ...))
        .with_tool(Tool::new("slow_operation", ...)))
}

async fn call_tool(..., name: String, ...) -> Result<CallToolResult> {
    match name.as_str() {
        "flakey_operation" => { ... }
        "slow_operation" => { ... }
        _ => Err(Error::ToolNotFound(name)),
    }
}
```

**Recommendation:** Consider a `ToolRegistry` helper that auto-generates both:

```rust
struct MyServer {
    tools: ToolRegistry,
}

impl MyServer {
    fn new() -> Self {
        let mut tools = ToolRegistry::new();
        tools.register("flakey_operation", FlakeyParams::schema(), |ctx, args| async { ... });
        tools.register("slow_operation", SlowParams::schema(), |ctx, args| async { ... });
        Self { tools }
    }
}

impl ServerHandler for MyServer {
    async fn list_tools(&self, ...) -> Result<ListToolsResult> {
        Ok(self.tools.list())
    }

    async fn call_tool(&self, ctx: &ServerCtx, name: String, args: ...) -> Result<CallToolResult> {
        self.tools.call(ctx, &name, args).await
    }
}
```


### 1.4 Arguments Extraction in ServerHandler is Error-Prone

**Problem:** In `server_client_calls.rs` (lines 166-170), argument extraction requires multiple
unwraps and error construction:

```rust
let args = arguments
    .ok_or_else(|| Error::InvalidParams("Missing arguments".to_string()))?;
let prompt = args
    .get_string("prompt")
    .ok_or_else(|| Error::InvalidParams("Missing prompt parameter".to_string()))?;
```

**Recommendation:** Add a helper method to `Arguments` for ergonomic extraction:

```rust
impl Arguments {
    /// Require a string parameter, returning a proper InvalidParams error if missing.
    pub fn require_string(&self, name: &str) -> Result<String> {
        self.get_string(name)
            .ok_or_else(|| Error::InvalidParams(format!("Missing required parameter: {}", name)))
    }

    /// Deserialize into a typed struct, with proper error messages.
    pub fn into_params<T: DeserializeOwned>(self) -> Result<T> {
        self.deserialize()
            .map_err(|e| Error::InvalidParams(format!("Invalid parameters: {}", e)))
    }
}

// Or even better, allow Arguments to be Option<Arguments> in the signature:
let prompt = arguments.require_string("prompt")?;
```


### 1.5 CreateMessageParams Construction is Extremely Verbose

**Problem:** In `server_client_calls.rs` (lines 174-185), creating a sampling request requires
deep nesting:

```rust
let params = CreateMessageParams {
    messages: vec![SamplingMessage {
        role: Role::User,
        content: schema::MessageContent::Text(schema::TextContent {
            text: prompt,
            annotations: None,
            _meta: None,
        }),
    }],
    max_tokens: 500,
    ..Default::default()
};
```

**Recommendation:** Add builder methods for common patterns:

```rust
impl CreateMessageParams {
    /// Create a simple user message request.
    pub fn user_message(text: impl Into<String>) -> Self {
        Self {
            messages: vec![SamplingMessage::user_text(text)],
            max_tokens: 500,
            ..Default::default()
        }
    }
}

impl SamplingMessage {
    pub fn user_text(text: impl Into<String>) -> Self { ... }
    pub fn assistant_text(text: impl Into<String>) -> Self { ... }
}

// Usage:
let params = CreateMessageParams::user_message(prompt).with_max_tokens(500);
```

---

## Priority 2: Medium Impact

### 2.1 Client Connect Methods Proliferate

**Problem:** The `Client` has 8 different `connect_*` methods:
- `connect_tcp`
- `connect_stdio`
- `connect_http`
- `connect_http_with_oauth`
- `connect_stream`
- `connect_stream_raw`
- `connect_process`
- `connect_process_raw`

The `_raw` variants are confusing (they skip initialization), and the OAuth variant is
transport-specific.

**Recommendation:** Consider a transport enum or builder pattern:

```rust
pub enum Transport {
    Tcp(String),
    Http { endpoint: String, oauth: Option<Arc<OAuth2Client>> },
    Stdio,
    Process(Command),
    Stream { reader: R, writer: W },
}

// Unified API:
client.connect(Transport::Tcp("localhost:3000")).await?;
client.connect(Transport::Http { endpoint: url, oauth: Some(oauth_client) }).await?;

// For raw connections:
client.connect_raw(Transport::Stdio).await?;
```


### 2.2 Server::new vs new_server Duplication

**Problem:** Both `Server::new()` and `new_server()` exist with identical functionality. The
free function `new_server` is redundant.

**Recommendation:** Remove `new_server` and keep only `Server::new()`. Update any internal uses
to go through `Server::new()`.


### 2.3 ProcessConnection Naming is Unclear

**Problem:** `ProcessConnection` (in `client.rs`) holds `child` and `init`, but the name
suggests a connection type rather than a result struct.

```rust
pub struct ProcessConnection {
    pub child: Child,
    pub init: InitializeResult,
}
```

**Recommendation:** Rename to something clearer:

```rust
pub struct SpawnedServer {
    pub process: Child,
    pub server_info: InitializeResult,
}
// or
pub struct ProcessHandle { ... }
```


### 2.4 ServerHandle Notification API Could Be Cleaner

**Problem:** Sending notifications from a server requires calling
`handle.send_server_notification(&notification)`, but the handle is only available after
`serve_tcp/serve_http`. There's no way for the handler itself to send notifications easily.

**Recommendation:** The `ServerCtx` already has notification capability via the broadcast channel.
Document this more clearly and ensure handlers can use `context.notify()` pattern consistently:

```rust
// In handler:
context.notify(ServerNotification::tool_list_changed())?;
```


### 2.5 InitializeResult Builder Has Too Many with_* Methods

**Problem:** `InitializeResult` has many capability methods that could be simplified:

```rust
InitializeResult::new("server-name")
    .with_version("1.0.0")
    .with_tools(true)
    .with_resources(true, true)  // subscribe, list_changed
    .with_prompts(true)
    .with_logging()
```

The `with_resources(true, true)` pattern requires remembering parameter order.

**Recommendation:** Use more descriptive methods or a capabilities builder:

```rust
InitializeResult::new("server-name")
    .with_version("1.0.0")
    .with_capabilities(
        Capabilities::new()
            .tools()
            .resources().with_subscribe().with_list_changed()
            .prompts()
            .logging()
    )
```


### 2.6 OneOrMany Pattern in SamplingMessage is Confusing

**Problem:** In `client_with_connection.rs` (lines 50-66), handling `OneOrMany<T>` requires
verbose pattern matching:

```rust
let last_message_text = params
    .messages
    .last()
    .and_then(|m| match &m.content {
        schema::OneOrMany::One(block) => match block { ... },
        schema::OneOrMany::Many(blocks) => blocks.iter().find_map(|block| ...) ,
    })
```

**Recommendation:** Add iterator/accessor methods to `OneOrMany`:

```rust
impl<T> OneOrMany<T> {
    pub fn iter(&self) -> impl Iterator<Item = &T> { ... }
    pub fn first(&self) -> Option<&T> { ... }
    pub fn into_vec(self) -> Vec<T> { ... }
}
```

---

## Priority 3: Lower Impact / Polish

### 3.1 Error Types Could Have More Context

**Problem:** `Error::ToolNotFound(name)` doesn't include context about available tools.
`Error::InvalidParams` is a catch-all string.

**Recommendation:** Consider richer error types:

```rust
Error::ToolNotFound { name: String, available: Vec<String> }
Error::InvalidParams { param: String, expected: String, got: String }
```


### 3.2 Cursor Type Could Have From<&str>

**Problem:** Creating cursors requires `Cursor::from("string")` rather than just `"string"`.

**Recommendation:** The `Into<Option<Cursor>>` pattern is already used in list methods. Consider
also implementing `From<&str>` for `Cursor` if not already present.


### 3.3 Handler Default Method Behaviors are Inconsistent

**Problem:** Some `ServerHandler` default methods return empty results (e.g., `list_tools` returns
empty list), while others return errors (e.g., `call_tool` returns `ToolNotFound`). This is
intentional but could be documented more clearly.

**Recommendation:** Add documentation explaining the philosophy: "Optional features return empty
by default; required dispatch methods error."


### 3.4 Missing Convenience for Empty Tool Calls

**Problem:** Calling a tool with no arguments requires passing `()`:

```rust
client.call_tool("ping", ()).await?
```

This works but is slightly awkward.

**Recommendation:** Consider an overload or default:

```rust
client.call_tool_no_args("ping").await?
// or document that () is the idiomatic way
```


### 3.5 call_tool_typed Mixes Concerns

**Problem:** `call_tool_typed<R>` both serializes input AND deserializes output, but the name
only hints at the output typing. It also assumes the response is JSON in the first text block.

**Recommendation:** Either rename for clarity or split:

```rust
// Current:
client.call_tool_typed::<Response>("tool", args).await?

// Clearer naming:
client.call_tool_json::<Response>("tool", args).await?

// Or split concerns:
let result = client.call_tool("tool", args).await?;
let typed: Response = result.parse_json()?;
```


### 3.6 Client.with_handler Consumes Self

**Problem:** `Client::new().with_handler(h)` changes the type from `Client<()>` to `Client<H>`,
which works but creates an interesting type state pattern that may surprise users.

**Recommendation:** Document this clearly or consider a different pattern where handlers are
always boxed trait objects.


### 3.7 Macro Limitations Not Documented

**Problem:** The `#[mcp_server]` macro is excellent for simple cases, but `timeout_server.rs`
shows that complex stateful servers need trait implementations. The trade-offs aren't documented.

**Recommendation:** Add documentation (in lib.rs or README) explaining:
- When to use `#[mcp_server]` macro (simple, stateless or simple state)
- When to implement `ServerHandler` directly (complex initialization, per-connection state,
  custom capability negotiation)

---

## Summary by Category

| Category | Items |
|----------|-------|
| **Schema/Type Construction** | 1.1, 1.5, 2.5, 2.6 |
| **Result/Response Handling** | 1.2, 1.4, 3.1 |
| **Handler Ergonomics** | 1.3, 2.4, 3.3, 3.7 |
| **Client API Surface** | 2.1, 2.3, 3.4, 3.5, 3.6 |
| **Naming/Consistency** | 2.2, 3.2 |

---

## Implementation Order Recommendation

1. **Quick wins** (low effort, high value):
   - 1.2 CallToolResult convenience methods
   - 1.4 Arguments helper methods
   - 2.2 Remove new_server duplication
   - 3.2 Cursor From<&str>

2. **Medium effort**:
   - 1.5 CreateMessageParams builders
   - 2.5 Capabilities builder
   - 2.6 OneOrMany helpers
   - 3.5 call_tool_typed rename

3. **Larger refactors** (consider for future versions):
   - 1.1 ToolSchema from type
   - 1.3 ToolRegistry helper
   - 2.1 Transport enum
   - 2.3 ProcessConnection rename (breaking change)
