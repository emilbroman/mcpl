# Local Pi test rig

A small, single-task agent loop using [Pi](https://pi.dev/)'s
`@earendil-works/pi-agent-core` and `pi-ai`, with the official TypeScript MCP SDK
bridging to MCPL over stdio. This is an experimental test rig, not an official
MCPL client or a coding-agent installation.

The model gets **only `run`**, with its description and input schema taken
unchanged from MCPL's `tools/list`. The system prompt is just
`You are a helpful assistant completing the user's task.` No language tutorial,
server instructions, files, skills, shell tools or downstream tool declarations
are added. One MCPL connection stays open for the entire agent loop, preserving
session-local bindings/handles. Tool calls execute sequentially.

## Run locally

Requires Node.js **22.19+**, npm, Rust and the build prerequisites in the root
README. Run from this directory after building the binary:

```sh
# From the repository root:
cargo build --locked
cd testing
npm ci

# Supply a LiteLLM key via your environment/secret manager, never in a source file.
read -rs -p 'LiteLLM API key: ' LITELLM_API_KEY; echo
export LITELLM_API_KEY

npm start -- --config empty.json \
  'Call the available tool once to inspect downstream availability, then briefly report the result.'
```

`empty.json` has no downstreams and is enough to exercise the current proxy stub.
A successful smoke run prints a `run` call, an MCP result with `stub: true` and
`servers: []`, then the model's final answer. This verifies plumbing, not language
execution. With the Playwright example:

```sh
# Install ahead of time, outside MCPL's downstream startup deadline:
npx --yes @playwright/mcp@0.0.84 --help
npm start -- --config ../examples/playwright.json \
  'Inspect downstream availability and report which server is ready.'
```

`model.json` selects `chatgpt/gpt-6.1-sol` at
`https://litellm.bb3.internal/v1` using OpenAI-compatible chat completions. You need
network access to this endpoint and trust in its CA. Do not disable TLS
verification. Edit this non-secret config to experiment with another endpoint or
model. Its context/output limits are conservative harness settings; zero costs
are placeholders, not a billing estimate. The model credential is read only from
`LITELLM_API_KEY`; the harness excludes that variable from MCPL's child environment.
Other environment variables are inherited so downstream server configuration
works normally. Configuration can launch local processes; only use trusted files.

Options:

- `--config <file>`: required MCPL `mcpServers` configuration.
- `--mcpl <binary>`: defaults to `../target/debug/mcpl`.
- `--max-turns <integer>`: defaults to 12 model turns, to bound accidental loops.
- One quoted positional argument: the user's task, sent verbatim.

Paths are relative to the working directory. Each invocation starts a fresh
session; there is no interactive REPL or session persistence. Ctrl-C/SIGTERM
aborts the loop and closes the MCP connection. A model failure, interruption,
output-token limit, or turn limit exits nonzero. Tool errors are returned to the
model so it can recover. Startup has a 60-second timeout and individual MCP tool
calls have a 120-second timeout.

## Transcript

Stdout contains JSON-line events for the tool declaration/system prompt, each
completed model message, tool call (including the program), full raw MCP result,
and Pi tool result. Text blocks are passed unchanged and in order, including
multi-part previews and errors. Images are forwarded as images; other MCP content
blocks are encoded as JSON text because Pi accepts only text/images. Raw MCP
metadata and structured output remain visible in the transcript. No extra
summary of the tool result is injected into the model context.

To save a clean JSONL transcript without npm's banner:

```sh
set -o pipefail
npm run --silent start -- --config empty.json \
  'Call the available tool once, then report the result.' | tee smoke.jsonl
```

MCPL/downstream diagnostic logs go to stderr. Transcripts can contain sensitive
tool results; `*.jsonl` and `.env*` are ignored here. Do not commit credentials or
private transcripts.

## Checks

```sh
# Build MCPL first as above; integration tests spawn the real local binary.
npm run check
npm test
```

Tests use a local fake OpenAI-compatible streaming endpoint, not a paid model.
They exercise the real Pi → MCP → MCPL loop and the forwarding contract.
The live smoke commands above remain the manual acceptance check against
LiteLLM. They do not constitute the later end-to-end language/Playwright demo.
