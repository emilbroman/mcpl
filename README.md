# MCPL

Model Context Protocol Language: a statically typed, data-manipulation-oriented
scripting proxy for MCP servers. MCPL discovers downstream tools and derives their
types from JSON Schema. Agents compose tool calls and process results in programs
without sending every intermediate value through their context.

The upstream MCP server exposes exactly three tools:

- `run(program)` checks and executes a program, retaining typed session bindings.
- `help(topic?)` discovers servers, tool signatures/descriptions, and builtins.
- `show(expression, depth?, max_items?, max_string?, max_output?)` evaluates one
  expression and displays its value without creating a binding.

Downstream schemas are not flattened into upstream tools. Use `help` to discover
what is available, then call tools from MCPL as `server.tool({...args})`.

## Language and library APIs

See the [MCPL v0.1 language specification](docs/language.md),
[schema-derived types](docs/types.md), and [parser/type checker API](docs/frontend.md).
The `mcpl::frontend` Rust API performs preflight only; the server integrates it
with `mcpl::runtime::Evaluator` and the connection-local `SessionState` to execute
checked programs. `Registry` owns downstream discovery and validated calls.

See [runtime integration and accounting](docs/runtime.md) for execution, memory,
cancellation, and disclosure details.

## Build and test

Requires Rust 1.88+ and a native C toolchain. Node.js/npm are only needed for the
Playwright example and the optional Pi test rig.

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
not supported.

Server keys must also be valid MCPL symbols and must not collide with reserved
builtin names. If a key cannot be imported into the static environment, `run`
and `show` return a type diagnostic asking you to rename it in the configuration;
`help` remains available. Dotted and hyphenated names are supported; string
indexing such as `server["tool.name"]` selects an exact tool key.

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

### Execute and inspect

An empty downstream configuration (`{"mcpServers": {}}`) is enough to try the
language. Call the MCP `run` tool with:

```json
{"program":"xs = [1, 2, 3]; map(xs, (x) => x * 2);"}
```

Its text result is:

```text
xs: number[] (3 items) = [1, 2, 3]
res1: number[] (3 items) = [2, 4, 6]
```

A later `run` with `{"program":"res1[0];"}` returns:

```text
res2: number (1 value) = 2
```

Every statement ends in `;` and creates an immutable named binding. An unnamed
statement gets a session-wide `resN` name. Operators require surrounding spaces
because names can contain hyphens. Iterate with builtins such as `map`, `filter`,
and `reduce`; there are no loops or implicit coercions. Programs are fully
checked before execution. Static errors execute nothing; runtime errors stop at
the failing statement and retain earlier successful bindings and tool effects.
Errors are returned as MCP tool errors (`isError: true`) with source diagnostics.

Call `help` with `{}` for the server/builtin overview, `{"topic":"map"}` for a
builtin, or `{"topic":"playwright.browser_navigate"}` for a tool's signature and
description. Help does not evaluate code or create bindings.

Call `show` with `{"expression":"res1"}` to display `[2, 4, 6]` without allocating
a new name. Its optional disclosure controls adjust truncation under hard caps;
output is at most 64 KiB. Functions, servers and tools display their type.
**`show` is not read-only:** expressions may call downstream tools. Binding lookups
in both `run` and `show` count as use for LRU eviction. `run` previews and the total
binding listing are bounded (8 KiB total); omitted bindings are still stored.

### Inspect with a real MCP client

The included example spawns the binary, verifies that `tools/list` contains
`run`, `help`, and `show`, calls `help`, prints the result and closes the connection.
It is also a manual Playwright discovery smoke test:

```sh
cargo build --locked
# Optionally pre-fetch the package:
npx --yes @playwright/mcp@0.0.84 --help
cargo run --locked --example inspect -- ./target/debug/mcpl ./examples/playwright.json
```

Discovery does not launch a browser. Actually invoking browser tools requires
Playwright's browser dependencies as usual.

### Try an agent loop

The experimental [Pi test rig](testing/README.md) connects a model through
LiteLLM to this binary over MCP, forwards `run`, `help`, and `show` with their
discovered descriptions, and prints a reviewable transcript. It lives under
`testing/` and requires Node.js 22.19+ plus a model API key supplied through the
environment.

## Runtime and proxy behaviour

- Downstreams connect concurrently. There are **no wall-clock deadlines** for
  connection, discovery, calls, or shutdown: slow or stuck downstreams can wait
  indefinitely. Failed connections become `unavailable` without preventing
  healthy servers from working. Invalid configuration is a startup error.
- Discovery happens once at startup, including pagination (at most 100 pages
  and 10,000 tools per server). Duplicate tool names and repeated cursors fail
  that server's discovery. Refresh, reconnection and `tools/list_changed` are
  not implemented. Disconnections update availability; cached schemas remain.
- Downstream calls within an evaluation are sequential. Arguments are validated
  against `inputSchema`. With `outputSchema`, MCPL uses `structuredContent`, or
  parses JSON from exactly one text block if structured content is absent, then
  validates the selected value. Without an output schema, tools return
  `Content[]`; `text(result)` extracts their text. A tool's `isError` response,
  invalid schema/output, or transport failure stops evaluation.
- JSON Schema remote/file reference retrieval is disabled. Raw schemas remain
  available even when validation cannot compile them; calls then return explicit
  schema errors. Derived static types are best-effort, not a substitute for
  runtime schema validation.
- Each evaluation gets 10,000,000 fuel units and at most 50 downstream calls;
  waiting for I/O consumes no fuel. Source is limited to 64 KiB, collections to
  100,000 elements, strings to 16 MiB, and runtime value nesting to 64. Parsing
  and checking have additional deterministic depth/work limits.
- Session values use a 256 MiB logical memory budget with least-recently-used
  eviction. This is not an operating-system RSS cap. Evicted names retain their
  static types and remain reserved; reading one gives an eviction diagnostic,
  not permission to reuse the name. Resource limits also apply to `show`.
- Create a new `ProxyServer` for each upstream connection: its `SessionState`
  owns that connection's values, type environment, and automatic-name counter.
  The registry may be shared, but session state must not be. The stdio binary
  serves one connection and closes downstream clients on disconnect or Ctrl-C.
- `Registry::snapshot()` exposes original names, descriptions, schemas and
  annotations. `Registry::call_tool()` is a Rust integration API, not an extra
  upstream tool; it preserves MCP results and distinguishes validation,
  availability, protocol, and tool-reported errors.

Configuration is trusted: stdio entries can run arbitrary local programs, and HTTP
entries can reach the local network. Fuel and size limits are resource guardrails,
**not an OS sandbox** or a way to undo downstream side effects. There is no hosted
deployment or upstream HTTP listener in this proof of concept.
