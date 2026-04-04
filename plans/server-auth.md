# Server-Side OAuth 2.1 Auth

Add server-side OAuth 2.1 resource server support to tmcp. This enables MCP servers built
with tmcp to authenticate clients using bearer tokens, serve Protected Resource Metadata
(RFC 9728), and return proper 401/403 challenges — as required by the MCP authorization
specification.

The work is split into four stages: making the HTTP transport extensible, propagating
per-request context to handlers, building auth primitives, and wiring up a high-level
convenience API.

## Compatibility

Both ChatGPT and Anthropic products connect to remote MCP servers over streamable HTTP with
OAuth 2.1. This plan delivers exactly that combination.

| Product            | Transport        | Auth       | Works after this plan? |
|--------------------|------------------|------------|------------------------|
| ChatGPT (web/app)  | Streamable HTTP  | OAuth 2.1  | Yes                    |
| Claude.ai (web)    | HTTP/SSE         | OAuth 2.1  | Yes                    |
| Claude Code        | stdio + HTTP     | OAuth/env  | Yes (HTTP mode)        |
| Claude Desktop     | Streamable HTTP  | OAuth 2.1  | Yes                    |

## Design Decisions

**Token validation model**: Provide a `TokenValidator` trait so users can bring any
validation strategy (JWT/JWKS, token introspection, database lookup). Ship a built-in JWT
validator with JWKS discovery as the common-case default. The validator is stored as
`Arc<dyn TokenValidator>` throughout (in `AuthConfig`, `BearerAuthLayer`, and middleware) so
Tower services can clone cheaply.

**Scope model**: Scopes are server-defined strings. The auth middleware validates token
authenticity (signature, expiration, audience) and extracts granted scopes into `AuthInfo`.
It does NOT enforce required scopes — that is left to handler code, which checks
`ctx.extensions().get::<AuthInfo>()` and decides per-tool whether the granted scopes are
sufficient. This keeps the framework non-prescriptive and avoids coupling route-level scope
configuration into the middleware.

**Transport scope**: Auth applies to the HTTP transport only. Stdio is inherently
authenticated by the process launcher. TCP could be added later using the same extension
mechanism.

**Extensibility approach**: Expose an `HttpBuilder` that accepts router-transformation
closures for middleware and a separate `Router` for additional routes. Closures compose on
repeated `.with_middleware()` calls (each wraps the previous transformation). The composed
transformation wraps only the MCP endpoint routes (POST `/`, GET `/`); additional routes are
merged afterward so they are not affected by auth or other user middleware. The transformation
closure receives `Router<()>` (applied after `.with_state()`), so users never need to know
about internal state types.

**Reuse existing types**: The `ProtectedResourceMetadata` struct already exists in
`auth::discovery` (used on the client side). Server-side auth reuses it rather than
duplicating it.

**Transport envelope**: Propagating per-request context from HTTP middleware to
`ServerHandler` requires the transport abstraction to carry metadata alongside messages.
`TransportStream` is extended to yield `IncomingMessage` (wrapping `JSONRPCMessage` +
optional `http::Extensions`) instead of bare `JSONRPCMessage`. Non-HTTP transports return
empty extensions.

---

## 1. Stage One: HTTP Transport Extensibility

Open up `HttpServerTransport` so users can add middleware layers and additional routes to
the Axum router built in `start()`. This is the foundation for everything that follows.

1. [x] Add an `HttpBuilder` struct returned by a new `Server::http(addr)` method. Provide
       chainable methods: `.with_middleware(impl FnOnce(Router) -> Router)` wraps the MCP
       routes, `.with_routes(Router)` merges additional routes outside the middleware scope.
       `.serve()` consumes the builder (including the `Server`) and starts the server.

       `.with_middleware()` **composes** on repeated calls: each invocation wraps the
       previous transformation, so user middleware and `.with_auth()` (which calls
       `.with_middleware()` internally) coexist without overwriting each other. The builder
       stores a single composed closure internally.

       The closure approach sidesteps Tower's `Layer<S>` generics — users call
       `router.layer(my_layer)` inside the closure, which Axum resolves with concrete types.
       (`server.rs`, new `HttpBuilder` struct)

2. [x] Modify `HttpServerTransport::start()` to accept an optional router transformation
       and optional additional routes. The construction order is a **correctness
       requirement**:

       1. Build the base MCP routes (POST `/`, GET `/`)
       2. Apply `.with_state(state)` — producing `Router<()>`
       3. Apply the middleware transformation — closures receive `Router<()>` and do not
          need to know about `HttpServerState`
       4. Merge additional routes — these bypass user middleware (so PRM is public)
       5. Apply `CorsLayer::permissive()` as the outermost layer

       CORS must be the outermost layer (moved from its current position inside the base
       router at `http.rs:751`) for two reasons: (a) in Axum 0.8
       `router.layer(L).merge(R)` does not apply `L` to `R`, so merged routes like PRM
       would lack CORS if it stayed on the base router — breaking browser-based PRM
       discovery for the compatibility targets in the matrix; and (b) CORS must run
       outside auth middleware so that OPTIONS preflight requests succeed without a
       bearer token (the CORS layer intercepts OPTIONS before auth ever sees it).
       (`http.rs`, router construction at line ~748)

3. [x] Keep `serve_http(addr)` as a convenience that delegates to the builder with no
       middleware or additional routes, so existing code is unaffected.
       (`server.rs:220-228`)

4. [x] Add an integration test: create an HTTP server with a custom middleware (e.g.
       `axum::middleware::from_fn` that sets a response header) and an additional route
       (e.g. GET `/custom` returning 200). Verify: (a) middleware affects MCP endpoint
       responses, (b) the additional route works and is NOT affected by the middleware,
       (c) two `.with_middleware()` calls compose correctly (both layers present).
       (`tests/http_integration.rs`)

---

## 2. Stage Two: Request Context Propagation

Carry per-request context from the HTTP layer (middleware-injected state) through to
`ServerHandler` methods via `ServerCtx`. This requires two things: widening the transport
abstraction to carry metadata alongside messages, and adding an extension map to `ServerCtx`.

The extension map uses `http::Extensions` (a type-safe `AnyMap` — the `http` crate is
already a dependency). Non-HTTP transports provide empty extensions by default.

1. [x] Introduce an `IncomingMessage` struct in `transport.rs`:

       ```
       pub struct IncomingMessage {
           pub message: JSONRPCMessage,
           pub extensions: http::Extensions,
       }
       ```

       Change `TransportStream` from `Stream<Item = Result<JSONRPCMessage>>` to
       `Stream<Item = Result<IncomingMessage>>`. The `Sink` side remains
       `Sink<JSONRPCMessage>` (outbound messages don't carry extensions).

       Update all `TransportStream` implementations. `JsonRpcCodec` still decodes
       `JSONRPCMessage` (`codec.rs:18-21`) — changing its output type would be invasive and
       unnecessary. Instead, replace the blanket `TransportStream for Framed<T, JsonRpcCodec>`
       impl (`transport.rs:101-102`) with a `FramedTransport<T>` wrapper that adapts the
       stream output:
       - `FramedTransport<T>` wraps `Framed<T, JsonRpcCodec>`. Its `Stream` impl maps each
         decoded `JSONRPCMessage` into `IncomingMessage` with empty extensions. Its `Sink`
         impl delegates to the inner `Framed` unchanged. Implement `TransportStream` for
         `FramedTransport<T>` in place of the old blanket impl.
       - All codec-based transports (`StdioTransport`, `TcpClientTransport`,
         `StreamTransport`) change from `Box::new(Framed::new(s, JsonRpcCodec))` to
         `Box::new(FramedTransport::new(s))` in their `Transport::connect` impls.
       - `HttpServerStream` — carry extensions from the widened channel (see item 2).
       - `TestTransportStream` — wrap with empty extensions.
       (`transport.rs`, `http.rs`)

2. [x] Widen the internal HTTP channel from `(JSONRPCMessage, String)` to
       `(JSONRPCMessage, String, http::Extensions)`. Refactor `handle_post` to take a raw
       `axum::extract::Request` instead of individual extractors, since we need access to
       both the JSON body and request extensions:

       ```rust
       async fn handle_post(
           State(state): State<HttpServerState>,
           request: axum::extract::Request,
       ) -> Response {
           let (parts, body) = request.into_parts();
           let headers = parts.headers;
           let extensions = parts.extensions;
           let message: JSONRPCMessage = // parse body via axum::body::to_bytes + serde
       ```

       This replaces the current `HeaderMap` + `Json<T>` extractors (http.rs:1025-1029).
       The refactored handler must re-implement three things that `Json<T>` provided
       automatically: (1) Content-Type validation — reject non-`application/json` with
       415, (2) body size limit — pass an explicit max to `axum::body::to_bytes` (the
       `Json` extractor enforces `DefaultBodyLimit`, typically 2MB), and (3) structured
       JSON parse error responses — return 400 with a description, not a bare 500.
       Forward the extensions through the channel alongside the message and session ID.
       `HttpServerStream::poll_next()` stores the session ID in `request_sessions` (as
       before) and yields `IncomingMessage` with the extensions. `handle_get` establishes
       an SSE stream without forwarding messages, so it does not propagate extensions — auth
       is validated at the HTTP layer for GET.
       (`http.rs`: `HttpServerState`, `HttpServerStream`, `handle_post`)

3. [x] Add an `extensions: http::Extensions` field to `ServerCtx`. `http::Extensions`
       implements `Clone` (http 1.x requires `T: Clone + Send + Sync + 'static` for
       `insert`), so `ServerCtx`'s `#[derive(Clone)]` continues to work. Expose a public
       `extensions(&self) -> &http::Extensions` accessor. Add a `pub(crate)
       with_extensions(http::Extensions)` method following the existing `with_request_id`
       pattern. Initialize with `Extensions::new()` in `ServerCtx::new()` so stdio and TCP
       paths work unchanged. (`context.rs`)

4. [x] Update all `TransportStream` consumers to destructure `IncomingMessage`:
       - **Server** (`server.rs`): Chain `.with_extensions(incoming.extensions)` once at
         the main loop level (`server.rs:302`) to create a per-message `ServerCtx`. Pass
         this context into the existing dispatch arms — `handle_initialize_request`,
         `handle_message_with_connection`, etc. all already take `&ServerCtx`, so no
         intermediate function signatures need to change. Responses (line 339) can
         discard extensions.
       - **Client** (`client.rs`): `start_message_handler` (`client.rs:390`) currently
         matches directly on `JSONRPCMessage` from `rx.next()`. Destructure to
         `incoming.message` and discard extensions — the client side does not use
         per-request context from HTTP middleware.
       - Any other transport consumers (test helpers, examples) that match on the stream
         item must also destructure `IncomingMessage`.

5. [x] Add an integration test: create a middleware (via `axum::middleware::from_fn`) that
       injects a custom type into request extensions, then verify the `ServerHandler` can
       retrieve it from `ctx.extensions().get::<MyType>()` inside `call_tool`.
       (`tests/http_integration.rs`)

---

## 3. Stage Three: Auth Primitives

Build the types and utilities needed for OAuth 2.1 resource server support. These are
standalone, composable pieces — no wiring yet.

1. [x] Add `auth::server` submodule (`auth/server.rs` or `auth/server/mod.rs`). Re-export
       from `auth/mod.rs`. (`auth/mod.rs`)

2. [x] Add a `protected_resource_handler()` function that takes a
       `ProtectedResourceMetadata` and an `endpoint_path` (the HTTP path at which MCP
       endpoints are externally reachable, e.g., `"/"` or `"/mcp"`). Returns an Axum
       `Router` serving the PRM JSON document. The `endpoint_path` parameter — not
       `metadata.resource` — determines the well-known routes: always register
       `/.well-known/oauth-protected-resource`, plus a path-suffixed variant
       (`/.well-known/oauth-protected-resource{endpoint_path}`) when `endpoint_path` is
       not `"/"`. This separation is necessary because `metadata.resource` is a public
       identifier URL (e.g., `https://example.com/api`) that may not match the local
       serving path (especially behind reverse proxies), while clients derive PRM
       discovery URLs from the endpoint URL path they connect to
       (`auth/discovery.rs:508-533`). (`auth/server.rs`)

3. [x] `WwwAuthenticate` builder: construct `WWW-Authenticate: Bearer ...` header values
       for 401 challenges. The `resource_metadata` parameter uses the well-known path
       derived from `endpoint_path` as a **relative URI** (e.g.,
       `/.well-known/oauth-protected-resource`). The client-side discovery code already
       resolves relative `resource_metadata` values against the endpoint URL via
       `resolve_relative_url` (`auth/discovery.rs:307-313, 481-482`), so relative paths
       produce the correct absolute URL even behind reverse proxies — the server never
       needs to know its public URL. Also provide a 403 variant (with
       `error="insufficient_scope"`, `scope="..."`) for handler code that needs
       RFC-compliant scope errors. In addition to the raw `HeaderValue` builders, provide
       an `insufficient_scope_response(required_scope)` helper that returns a complete
       `axum::response::Response` (403 status + `WWW-Authenticate` header + JSON body
       describing the required scope) so handlers don't have to assemble the response
       manually. (`auth/server.rs`)

4. [x] `AuthInfo` struct: the validated identity extracted from a token. Fields: `subject`
       (String), `scopes` (HashSet<String>), `audiences` (Vec<String>), `extra`
       (serde_json::Value for custom claims). `audiences` is a `Vec` because RFC 7519
       §4.1.3 allows the JWT `aud` claim to be either a single string or an array. Must
       derive `Clone + Debug + Send + Sync` for storage in `http::Extensions` and
       logging. Add convenience methods: `has_scope(&self, scope: &str) -> bool`,
       `has_all_scopes(&self, scopes: &[&str]) -> bool`. These save every handler from
       writing `auth_info.scopes.contains(...)` boilerplate. This is what middleware
       injects into request extensions and handlers read from `ctx.extensions()`.
       (`auth/server.rs`)

5. [x] `TokenValidator` trait with a single async method:
       `async fn validate(&self, token: &str) -> Result<AuthInfo, AuthError>`.
       `AuthError` enum: `Invalid`, `Expired`, `Unavailable(String)`. The `Unavailable`
       variant covers infrastructure failures (JWKS fetch timeout, IdP unreachable) where
       the token may be valid but is unverifiable — the middleware maps this to 503 rather
       than 401, giving clients correct retry semantics. Use `#[async_trait]` for
       trait-object compatibility (matching the project's existing pattern for
       `ServerHandler`, `ClientHandler`, and `Transport`). (`auth/server.rs`)

6. [x] `JwtValidator` struct implementing `TokenValidator`. Add `jsonwebtoken` as a
       dependency. Provide two constructors:
       - `new(issuer, audiences, jwks_url)` — fetches and caches JWKS from a URL
         (the common case for production with an IdP).
       - `from_jwk_set(issuer, audiences, jwk_set)` — accepts a pre-loaded `JwkSet`
         (for unit tests without a mock HTTP server, static-key deployments, and
         air-gapped environments).

       Split into three sub-parts:

       a. **JWKS fetching and caching**: Use `reqwest` to fetch the JWKS document from
          `jwks_url`, parse it into `jsonwebtoken::jwk::JwkSet`, and cache the keyset
          with a configurable TTL. On unknown `kid`, refetch the JWKS before failing —
          this handles key rotation between TTL refreshes. Return
          `AuthError::Unavailable` on fetch failures. Concurrent refetches for the same
          cache miss must be coalesced (e.g., `tokio::sync::Notify` or a shared future)
          to prevent a thundering herd when many requests arrive right after key
          rotation.

       b. **Key selection**: Given a JWT header's `kid` claim, select the matching `Jwk`
          from the cached keyset and convert it to a `DecodingKey` via
          `DecodingKey::from_jwk`. Handle missing `kid` (single-key keysets) and unknown
          `kid` (trigger refetch from part a).

       c. **Token decode and validation**: Use `jsonwebtoken::decode` with the selected key,
          `Validation` configured for issuer and audience, and map the decoded claims into
          `AuthInfo`. Map signature/expiration failures to the appropriate `AuthError`
          variant.

       (`auth/server/jwt.rs`)

7. [x] Unit tests for all primitives: PRM handler returns correct JSON at both root and
       path-suffixed well-known URLs, `WwwAuthenticate` header formatting, `JwtValidator`
       with test keys (generate RSA keypair in test, sign a JWT, validate it; test expired
       token; test unknown `kid` triggers JWKS refetch).
       (`auth/server.rs` or `tests/auth_server_test.rs`)

---

## 4. Stage Four: Auth Middleware and High-Level API

Wire the primitives into an Axum middleware and provide a convenient builder method on
`HttpBuilder`.

1. [x] `BearerAuthLayer` / `BearerAuthMiddleware`: a Tower layer+service that extracts
       `Authorization: Bearer <token>` from the request, calls the `TokenValidator`, and on
       success inserts `AuthInfo` into request extensions. Error responses follow RFC 6750
       §3/§3.1:
       - **Missing credentials** (no `Authorization` header) → 401 with bare
         `WWW-Authenticate: Bearer resource_metadata="..."`. Per §3.1, SHOULD NOT
         include error code or other error information.
       - **Invalid or expired token** → 401 with `WWW-Authenticate: Bearer
         error="invalid_token", error_description="...", resource_metadata="..."`.
       - **Unavailable** (infrastructure failure) → 503.
       The middleware does not enforce scopes — that is handler responsibility. The layer
       stores `Arc<dyn TokenValidator>` so the service is `Clone`.
       (`auth/server/middleware.rs`)

2. [x] Add `.with_auth(config)` method to `HttpBuilder` (from Stage 1). `AuthConfig`
       struct holds: `ProtectedResourceMetadata`, `Arc<dyn TokenValidator>`, and
       `endpoint_path` (defaults to `"/"`).
       `.with_auth()` automatically:
       - Uses `.with_middleware()` to wrap MCP routes with the `BearerAuthLayer`
         (composes with any previously added middleware)
       - Uses `.with_routes()` to add the PRM well-known endpoint router
       Because Stage 1 enforces middleware-then-merge ordering, the PRM endpoint is
       automatically public (not behind auth). (`server.rs`, `HttpBuilder`)

3. [x] End-to-end integration test using `reqwest::Client`, covering both POST and GET
       (SSE) routes — `handle_get` (`http.rs:1182`) is a separate code path from
       `handle_post`, and the client transport sends bearer tokens on SSE handshake
       (`http.rs:474-482`):
       (a) POST without `Authorization` header — verify 401 with bare
           `WWW-Authenticate: Bearer resource_metadata="..."` (no error params per
           RFC 6750 §3.1).
       (b) POST with valid JWT — verify request succeeds and the handler sees `AuthInfo`
           in `ctx.extensions()` with correct subject, audiences, and scopes.
       (c) POST with expired JWT — verify 401 with `error="invalid_token"`.
       (d) GET `/` with valid JWT + `Mcp-Session-Id` — verify SSE stream established.
       (e) GET `/` without `Authorization` header — verify 401 with
           `WWW-Authenticate` challenge.
       Additionally verify: `GET /.well-known/oauth-protected-resource` returns the PRM
       document without requiring authentication.
       (`tests/http_auth_integration.rs`)

4. [x] Add an `oauth_server` example showing the complete setup: server with
       `.with_auth()`, PRM, JWT validation. (`examples/oauth_server.rs`)

5. [x] Update `auth/mod.rs` module docs to cover both client-side and server-side auth,
       with a usage overview. (`auth/mod.rs`)
