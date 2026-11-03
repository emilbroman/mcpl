# Playwright MCP discovery fixture

`playwright-tools.json` is the complete, unmodified `result` object from an actual
stdio MCP `tools/list` response (pretty-printed; JSON-RPC envelope omitted).
Captured on 2026-11-03 using Node v22.21.1, npm 10.9.4, and the pinned npm package
`@playwright/mcp@0.0.84`. Its pinned Playwright dependency and advertised server
version are `1.65.0-alpha-2026-10-30`; that is not the npm MCP package version.
The negotiated MCP protocol version was `2025-06-18`.

Reproduce from the repository root (requires Python 3, Node, npm, and npm registry
access on the first run):

```sh
npm_config_cache=/workspace/.npm-cache python3 tests/fixtures/capture-playwright-tools.py
```

The script starts `npx --yes @playwright/mcp@0.0.84 --headless`, exchanges
`initialize`, `notifications/initialized`, and `tools/list`, saves the result, and
stops the server. It does not call any tools or launch a browser. The fixture
contains only public discovery metadata: 25 tools with descriptions, annotations,
and draft-2020-12 object input schemas; no tool advertises an `outputSchema`.

The two-tool fixture in `docs/language.md` Appendix A is illustrative, not this
server's live index. In particular, real `browser_snapshot` accepts optional
`target: string`, `filename: string`, `depth: number`, and `boxes: boolean`
(`additionalProperties: false`), rather than an empty input record.
`browser_navigate` does match the illustrative required `url: string` input.
