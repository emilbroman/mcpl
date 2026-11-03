# MCPL

Model Context Protocol Language: a statically typed, data-manipulation-oriented
scripting proxy for MCP servers. This first slice implements the **proxy core**,
not the language. Its single `run` tool accepts a `program` string but does not
execute it: it returns discovered server names, tool names, availability and
connection errors. Downstream tool schemas are not exposed as upstream tools.

## Build and test

Requires Rust 1.88+ and a native C toolchain. Node.js/npm are only needed for the
Playwright example.

```sh
cargo build --locked
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

The official `rmcp` SDK is pinned to 3.8.0; the lockfile is checked in. Tests use
in-process MCP servers and do not need network access or Playwright.

## Configure and run

Create a JSON file in the standard `mcpServers` shape:

```json
{
  "mcpServers": {
    "playwright": {
      "command": "npx",
      "args": ["--yes", "@playwright/mcp@0.0.84", "--headless"],
      "env": {}
    },
    "remote": {
      "url": "http://127.0.0.1:8080/mcp"
    }
  }
}
```

Each entry supports either `command` plus optional `args`/`env` for stdio, or
`url` for Streamable HTTP. The child inherits the proxy's environment and working
directory; `env` overrides individual variables. Commands are executed directly,
not through a shell, and there is no variable interpolation. Server keys and tool
names are retained verbatim. Unknown fields, ambiguous transports, missing root
`mcpServers`, null fields and empty names/commands/URLs are rejected with a config
error. HTTP headers/authentication, legacy SSE, and alternate config formats are
not supported yet.

```sh
./target/debug/mcpl --config ./config.json
# Alternatively:
MCPL_CONFIG=./config.json ./target/debug/mcpl
```

MCPL speaks MCP over **stdio**: launch it from an MCP client rather than expecting
an interactive terminal. Only protocol messages go to stdout; logs and downstream
stderr go to stderr (`RUST_LOG=mcpl=debug` enables extra logging). A typical host
configuration for MCPL itself is:

```json
{
  "mcpServers": {
    "mcpl": {
      "command": "/absolute/path/to/mcpl",
      "args": ["--config", "/absolute/path/to/config.json"]
    }
  }
}
```

### Inspect with a real MCP client

The included example spawns the binary, lists its tools, calls `run`, prints the
result and closes the connection. It is also the manual Playwright smoke test:

```sh
cargo build --locked
# Pre-fetch the package so npm installation does not consume the startup deadline:
npx --yes @playwright/mcp@0.0.84 --help
cargo run --locked --example inspect -- ./target/debug/mcpl ./examples/playwright.json
```

`tools/list` must contain exactly `run`. Its result has both JSON text content and
`structuredContent` with this shape (the Playwright tool list is abbreviated):

```json
{
  "stub": true,
  "message": "Program not executed; MCPL language support is not implemented yet.",
  "servers": [
    {
      "name": "playwright",
      "status": "ready",
      "tools": ["browser_click", "browser_navigate", "browser_snapshot"],
      "error": null
    }
  ]
}
```

Discovery does not launch a browser. Actually invoking browser tools in a future
language runtime requires Playwright's browser dependencies as usual.

### Try an agent loop

The experimental [Pi test rig](testing/README.md) connects a model through
LiteLLM to this binary over MCP, exposes only `run` using its discovered
description, and prints a reviewable transcript. It lives under `testing/` and
requires Node.js 22.19+ plus a model API key supplied through the environment.

## Behaviour and extension points

- Downstreams connect concurrently, with a 30-second deadline per server covering
  connection and discovery. A failed server appears as `unavailable`; it does not
  prevent healthy servers or the proxy from working. Invalid configuration itself
  is a startup error.
- Discovery happens once, at startup, including pagination (bounded to 100 pages
  and 10,000 tools per server). Duplicate tool names or repeated cursors fail that
  server's discovery. `tools/list_changed`, reconnection, and refresh are not yet
  implemented. Disconnections update availability; cached schemas remain visible.
- `Registry::snapshot()` retains each tool's name, description, raw JSON Schema
  `inputSchema`/optional `outputSchema`, and annotations, keyed by original server
  and tool names. Servers are never flattened into one shared tool namespace.
- `Registry::call_tool()` is a Rust integration API for the forthcoming interpreter,
  **not another exposed MCP tool**. It accepts a deadline duration, validates
  arguments and successful structured output with `jsonschema`, and preserves the
  full MCP result. Unknown names, unavailable connections, protocol/transport
  errors, deadlines, invalid arguments/results/schemas, and tool-reported errors
  are distinct error variants. Tool-reported and invalid-output errors retain the
  original result. Dropping a pending SDK tool call sends MCP cancellation; this
  cannot undo side effects in the downstream server.
- JSON Schema remote/file reference retrieval is disabled. Raw schemas are retained
  even when validation cannot compile them; those calls return an explicit schema
  error. General language/resource guardrails belong to the interpreter work, not
  this initial proxy.
- `ProxyServer::new()` creates a connection-local `SessionState` slot for the future
  handle store. Create a new server handler for each upstream connection. The stdio
  binary serves one connection, then shuts down downstream clients on disconnect
  or Ctrl-C. The registry can be shared; session state must not be.
- `server::run_tool()` and `RUN_DESCRIPTION` centralize the stub's public contract
  for replacement by the language runtime.

Configuration is trusted: stdio entries can run arbitrary local programs, and HTTP
entries can reach the local network. This proxy is not an OS sandbox. There is no
hosted deployment or upstream HTTP listener in this proof of concept.
