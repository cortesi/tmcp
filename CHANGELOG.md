
# v0.5.0

- Add production OAuth client/server support, including dynamic callbacks, refresh/retry,
  revocation metadata, JWT validation, and sensitive auth header handling.
- Add configurable streamable HTTP mounts, session lifecycle/replay handling, CORS policy,
  and per-session MCP connections.
- Add MCP API inspection/rendering, task tool calls, progress/logging helpers, and bridge
  request primitives.
- Add resource handler derives and harden macro codegen for receivers, generics, attrs, and tasks.
- Fix MCP wire-format/schema extension handling, codec lifecycle ordering, and client
  SSE/concurrency issues.
- Reduce hot-path cloning/JSON round-trips and expand protocol/compile-fail coverage.

# v0.4.0

- Add streamable HTTP alignment, auth discovery support, and progressive discovery for tools.
- Add flat tool argument support in macros plus new helpers for content blocks, initialization,
  and JSON text tool results.
- Fix disconnect draining, HTTP request stalls, schema and notification metadata propagation, and
  codec handling for empty lines.
- Continue internal cleanup and documentation improvements across request handling and transport
  layers.

# v0.3.0

- Initial public release
