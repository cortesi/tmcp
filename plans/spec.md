# MCP 2025-11-25 alignment checklist

Align tmcp with the MCP 2025-11-25 specification. This checklist enumerates concrete
implementation gaps and updates, with source references for each recommendation.

Findings (spec audit complete; references inline):
- Streamable HTTP expects clients to advertise `Accept: application/json, text/event-stream`
  and handle both response types. `crates/tmcp/src/http.rs` only sends `application/json` on
  POST. Ref: https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
- Streamable HTTP requires session-based SSE with `Mcp-Session-Id`, SSE `id` fields, and
  GET resume via `Last-Event-ID`. The client attempts SSE before it has a session id and
  never retries; the server emits SSE without `id` or resume handling. Ref:
  https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
- Streamable HTTP requires Origin validation with 403 for invalid origins. The server uses
  permissive CORS and does not validate Origin. Ref:
  https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
- Authorization requires Protected Resource Metadata discovery (RFC 9728), using
  `WWW-Authenticate` and `.well-known` endpoints. `crates/tmcp/src/auth/*` only discovers
  RFC 8414 auth server metadata and lacks protected resource metadata and challenge handling.
  Ref: https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization
- Authorization requires OpenID Connect discovery; it is not implemented. Ref:
  https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization
- Authorization requires Client ID Metadata Documents for HTTPS client ids; not implemented.
  Ref: https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization
- Tool schema dialect defaults to JSON Schema 2020-12 when `$schema` is missing; tmcp
  generates draft-07 and strips `$schema`. Ref:
  https://modelcontextprotocol.io/specification/2025-11-25/schema
- README claims MCP 2025-06-18 and omits Streamable HTTP despite the 2025-11-25 spec
  and existing HTTP transport. Ref:
  https://modelcontextprotocol.io/specification/2025-11-25/changelog

1. Stage One: Streamable HTTP compliance

Bring HTTP transport behavior into alignment with Streamable HTTP requirements.

1. [x] Send POST `Accept` header with both `application/json` and `text/event-stream`, and
   handle both response content-types. Ref:
   https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
2. [x] Establish SSE after session id is known, and support GET-based resume using
   `Last-Event-ID` plus server-sent `id` fields. Ref:
   https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
3. [x] Validate `Origin` on all HTTP requests and return HTTP 403 on invalid origins. Ref:
   https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
4. [x] Avoid broadcasting a single JSON-RPC message across multiple SSE streams; route each
   message to exactly one stream per spec. Ref:
   https://modelcontextprotocol.io/specification/2025-11-25/basic/transports

2. Stage Two: Authorization compliance

Implement required OAuth discovery flows and metadata handling.

1. [x] Implement OAuth 2.0 Protected Resource Metadata discovery (RFC 9728), including
   `WWW-Authenticate` handling and `.well-known` fallback. Ref:
   https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization
2. [x] Support auth server discovery via RFC 8414 and OpenID Connect Discovery. Ref:
   https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization
3. [x] Add Client ID Metadata Document support for HTTPS client IDs and document the flow.
   Ref: https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization

3. Stage Three: Tool errors + schema dialect

Align tool error reporting and schema dialect behavior with the spec.

1. [x] Return input validation failures as tool execution errors (`isError: true`) rather
   than JSON-RPC InvalidParams in tool call handling. Ref:
   https://modelcontextprotocol.io/specification/2025-11-25/changelog
2. [x] Ensure tool schemas default to JSON Schema 2020-12 when `$schema` is absent or include
   an explicit dialect, and document supported dialects. Ref:
   https://modelcontextprotocol.io/specification/2025-11-25/basic
3. [x] Add tests covering tool validation error surfaces and schema dialect behavior. Ref:
   https://modelcontextprotocol.io/specification/2025-11-25/basic

4. Stage Four: Verification

Run linting, formatting, and tests to verify changes.

1. [x] Run `cargo clippy -q --fix --all --all-targets --all-features --allow-dirty --tests
   --examples`.
2. [x] Run `cargo +nightly fmt --all -- --config-path ./rustfmt-nightly.toml`.
3. [x] Run `cargo nextest run --all --all-features` (fallback to `cargo test` if needed).

5. Stage Five: Verification (post Stage Three)

Re-run linting, formatting, and tests after the remaining stage completes.

1. [x] Run `cargo clippy -q --fix --all --all-targets --all-features --allow-dirty --tests
   --examples`.
2. [x] Run `cargo +nightly fmt --all -- --config-path ./rustfmt-nightly.toml`.
3. [x] Run `cargo nextest run --all --all-features` (fallback to `cargo test` if needed).
