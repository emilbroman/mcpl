# Manual browser demo

This is a local, manual experiment, not an automated model acceptance test.
The real Pi loop gives `chatgpt/gpt-6.1-sol` only MCPL's discovered `run`, `help`
and `show` tools and their unchanged descriptions. The generic system prompt and
harness are unchanged. Playwright's tools are discovered progressively through
MCPL; the model has no shell, filesystem tool, or direct browser tool.

`links.html` is a deterministic, static page with 80 links, seven matching the
case-sensitive substring `Rust`. It includes lowercase/uppercase and href-only
distractors. `expected.json` is independent ground truth, not model context.
The HTML's editorial notes make a full accessibility snapshot substantial.

## Reproduce

Build MCPL and install the harness dependencies as described in
[the testing README](../README.md). Install a compatible Chromium binary and set
`LITELLM_API_KEY` through your secret manager. Do not put the key in configuration
or transcripts. The recorded experiment used Node 24, Chromium 143.0.7499.40,
and `@playwright/mcp@0.0.84`.

Chromium needs working fonts, not just a launchable executable. In the BB3 Nix
sandbox the initial runs had zero-size text and empty `innerText` until fonts
were configured. For this environment, install `nixpkgs#dejavu_fonts` and use:

```sh
cat > /tmp/mcpl-demo-fonts.conf <<EOF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "urn:fontconfig:fonts.dtd">
<fontconfig>
  <dir>$HOME/.nix-profile/share/fonts</dir>
  <cachedir>/tmp/mcpl-demo-font-cache</cachedir>
</fontconfig>
EOF
export FONTCONFIG_FILE=/tmp/mcpl-demo-fonts.conf
```

An ordinary desktop with working system fonts does not need this override.

From the repository root, serve **only** the HTML in one terminal:

```sh
mkdir -p /tmp/mcpl-demo-www
cp testing/demo/links.html /tmp/mcpl-demo-www/
python3 -m http.server 80 --bind 0.0.0.0 --directory /tmp/mcpl-demo-www
```

In another terminal, generate a machine-local configuration (requires `jq`):

```sh
CHROMIUM_BIN=$(command -v chromium)
test -n "$CHROMIUM_BIN"
jq --arg chromium "$CHROMIUM_BIN" \
  '.mcpServers.playwright.args += ["--executable-path", $chromium,
    "--no-sandbox", "--isolated", "--timeout-action", "0",
    "--timeout-navigation", "0", "--idle-timeout", "0"]' \
  examples/playwright.json > /tmp/mcpl-demo-config.json
```

`--no-sandbox` is for the containerized experiment, where Chromium's nested
sandbox is unavailable. Omit it on a host that supports the browser sandbox;
this demonstration is not a security boundary. The timeout options disable
Playwright's action/navigation/idle deadlines; MCPL and the MCP client already
use no operation deadlines. Profiles are isolated per invocation. Playwright
may write snapshots to `.playwright-mcp/`; those files are not model context and
are not automatically readable through MCPL.

Run three **separate invocations**, with no mid-run corrections:

```sh
cd testing
mkdir -p /tmp/mcpl-demo-transcripts
for run in 1 2 3; do
  npm run --silent start -- --config /tmp/mcpl-demo-config.json \
    'Open http://127.0.0.1/links.html, list all link texts containing the case-sensitive substring "Rust", and tell me how many matching links there are.' \
    > "/tmp/mcpl-demo-transcripts/run-$run.jsonl"
done
python3 demo/summarize.py /tmp/mcpl-demo-transcripts/run-*.jsonl
```

The harness defaults to 12 model turns, not a time limit. A final answer is not
necessarily correct: compare its text and count to `expected.json`. Do not pass
the expected data, language examples, or observed fixes to the model while a run
is underway. Record failures as well as successes. The actual experiment's raw
events, timing/exit metadata, observations and caveats live under
[`testing/transcripts/browser-demo/`](../transcripts/browser-demo/).

## Local artifact checks

```sh
python3 -m unittest discover -s testing/demo -p 'test_*.py'
python3 testing/demo/summarize.py testing/transcripts/browser-demo/run-*.jsonl
```

The summary validates the fixture and extracts counts, programs, errors, final
answers and reported model usage; it does not judge semantic correctness. Raw
JSONL intentionally includes repeated representations of MCP responses for
traceability; do not equate its file size with model context size.
