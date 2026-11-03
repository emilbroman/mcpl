# Browser proof-of-concept: evidence and limitations

Manual experiment, 2026-11-03. Runtime and harness revision:
[`3cf212da7ff604724b739751f0d9803f8c7b2414`](https://github.com/emilbroman/mcpl/commit/3cf212da7ff604724b739751f0d9803f8c7b2414).
No language, runtime, tool description, or agent-loop changes were made for this
experiment. The shipped surface is **run/help/show**, superseding the original
project ticket's older run-only wording.

## Verdict boundary

**All eight fresh model sessions answered correctly with no human intervention
mid-run.** Three clean-environment baseline runs kept intermediate page data out
of context, but delegated filtering/counting to browser JavaScript. A separately
labeled clean-environment diagnostic run performed the filtering/counting in
MCPL and disclosed only the final matches/count (plus boolean confirmations).

This demonstrates end-to-end proxying, progressive discovery, session bindings,
bounded previews, and MCPL composition capability. It does **not** demonstrate
that models naturally prefer MCPL composition, or that disclosure always stays
small during recovery. The diagnostic prompt explicitly requests that strategy;
it is not equivalent to the baseline. This is one model, one static local page,
and one off-the-shelf MCP, not a general reliability benchmark or safety audit.

**User verdict: pending.** The evidence is published for the user's assessment;
publication alone does not close the project ticket's verdict acceptance gate.

## Setup and ground truth

- Pi harness from `testing/`, actual `chatgpt/gpt-6.1-sol` through LiteLLM.
- Generic system prompt: `You are a helpful assistant completing the user's task.`
- Only MCPL's three tools, descriptions/schemas forwarded unchanged. No shell,
  filesystem access, skills, language examples, or raw Playwright schema dump.
- Official `@playwright/mcp@0.0.84`, Chromium 143.0.7499.40, Node 24.11.1.
- Headless, isolated profiles, explicit executable path, no nested Chromium
  sandbox in this container. Action/navigation/idle timeouts set to zero;
  MCPL/client use no operation deadlines. This is not OS isolation evidence.
- Static [fixture](../../demo/links.html): 80 anchors, seven case-sensitive `Rust`
  matches; lowercase `rust`, uppercase `RUST`, and href-only `Rust` distractors.
  [Ground truth](../../demo/expected.json) was never passed to the model or served
  by the HTTP fixture server. Only the HTML was served at port 80.
- [Reproduction instructions and artifact checks](../../demo/README.md).

Every baseline received exactly:

> Open http://127.0.0.1/links.html, list all link texts containing the case-sensitive substring "Rust", and tell me how many matching links there are.

Diagnostic sessions added:

> Use the browser only to navigate and extract data; do the filtering and counting in MCPL. Do not display the full page, snapshot, or unfiltered link collection: only bring the matching texts and count into your context.

Baseline turn cap: 12; diagnostic cap: 20. All finished before the cap. Each
invocation started a fresh MCPL session and model conversation; no transcript
or fix was supplied to subsequent invocations. Full prompts, timestamps with
Stockholm offsets, exits, revision and font configuration are in `*.meta.json`.
All exits were zero and all stderr logs were empty.

## Runs

“Data chars” counts raw MCPL result text for run/show, once per result, excluding
help and model-authored source. It is not token usage or raw downstream size.
“Tokens” sums the provider-reported total over all model requests, including
repeated context; it is **not** peak context size or cost. Zero cost fields in
raw events are harness placeholders. The summary retains input/output/cache
components separately (diagnostic 8 includes 2,048 cache-read tokens).

| Transcript | Environment / prompt | Turns | help/run/show | MCPL errors | Data chars | Tokens | Outcome |
|---|---|---:|---|---:|---:|---:|---|
| [run-1](run-1.jsonl) | No font config / baseline | 8 | 2/3/2 | 0 | 36,169 | 23,813 | Correct; full snapshot shown |
| [run-2](run-2.jsonl) | No font config / baseline | 8 | 2/3/2 | 0 | 36,169 | 23,746 | Correct; full snapshot shown |
| [run-3](run-3.jsonl) | No font config / baseline | 9 | 2/4/2 | 0 | 11,792 | 25,417 | Correct; unfiltered links shown |
| [diagnostic-4](diagnostic-4.jsonl) | No font config / MCPL requested | 8 | 2/4/1 | 1 | 1,166 | 17,942 | Correct; MCPL aggregation, self-recovery |
| [run-5](run-5.jsonl) | Fonts configured / baseline | 6 | 2/2/1 | 0 | 991 | 11,175 | Correct; browser-JS aggregation |
| [run-6](run-6.jsonl) | Fonts configured / baseline | 6 | 2/2/1 | 0 | 901 | 11,134 | Correct; browser-JS aggregation |
| [run-7](run-7.jsonl) | Fonts configured / baseline | 6 | 2/2/1 | 0 | 991 | 11,175 | Correct; browser-JS aggregation |
| [diagnostic-8](diagnostic-8.jsonl) | Fonts configured / MCPL requested | 6 | 2/2/1 | 0 | 411 | 11,507 | Correct; MCPL aggregation, no full intermediates |

All eight final answers list exactly these labels in order, with count **7**:

1. Rust ownership guide
2. Async Rust patterns
3. Rust on embedded devices
4. Learning Rust macros
5. Rust error handling
6. Web services in Rust
7. Rust release notes

## Successful MCPL composition

Diagnostic 8 first discovered servers and Playwright tools through help. Its
first run navigated and extracted data inside lexical locals, returning only
`true` confirmations. It unnecessarily discarded that first extraction and
called the browser again in its second run. The second program, line-wrapped
here for readability (raw original in the transcript):

```text
matches = (
  raw = text(playwright.browser_evaluate({
    function: "() => Array.from(document.querySelectorAll('a[href]'), a => a.innerText)"
  }));
  result = trim(split(split(raw, "### Result")[1], "###")[0]);
  links = parse_json(result);
  filter(links, (s) => contains(s, "Rust"))
);
count = len(matches);
```

Returned:

```text
matches: any[] (7 items) = ["Rust ownership guide", "Async Rust patterns", "Rust on embedded devices", "Learning Rust macros", "Rust error handling", …+2]
count: number (1 value) = 7
```

`show(expression: "matches")` then returned only the seven strings (168 chars).
The full unfiltered link collection and snapshot never entered this model's
context. All required operations already existed; no missing builtin blocked
completion. The result is `any[]` because Playwright provides no output schema
and `parse_json` is gradual, not evidence of rich output-schema-derived typing.

## Friction and recovery

1. **Browser environment, not MCPL semantics.** In the first four runs Chromium
   had no usable font configuration: `innerText` was empty although textContent
   and accessibility snapshots contained the text. The initial three baselines
   all tried `innerText`, received zero matches, and recovered themselves: two
   inspected full snapshots and one switched to `textContent` after dumping
   the link array. After those completed, operator probes isolated the cause:
   first-link bounds changed from 0×0 to 387×16 and body innerText from zero to
   24,699 characters with explicit fontconfig + DejaVu fonts. See the separate
   [before/after probes](environment-probes.json). Runs 5–8 used working fonts;
   the earlier runs are retained, not counted as clean baseline failures.
2. **Strategy selection.** Every baseline put filtering/counting inside browser
   JavaScript. Clean baselines succeeded efficiently but only prove MCPL as a
   proxy/discovery/disclosure layer. Explicit strategy constraints were needed
   here to exercise MCPL data manipulation. Diagnostic 8 also performed one
   unnecessary repeated extraction because its first local was discarded.
3. **Text-wrapped JSON.** Playwright returns Markdown containing JSON and code
   as `Content[]`. Diagnostic 4 guessed a double-newline section separator and
   authored an executable but failing program. The error was:
   `runtime error: invalid JSON: trailing characters at line 83 column 1`, with
   source location `1:150`, a caret spanning parse_json, and hint
   `check the value and retry the expression`. It changed `\n\n###` to `\n###`
   without human help, then switched to textContent after the font-confounded
   zero result. No syntax or static type error occurred in any model run. The
   generic hint did not explain the wrapper format; the model inferred it.
4. **Preview size.** Navigation previews (~179 chars) show mostly boilerplate.
   Browser aggregation previews (~189 chars) cut the text before count, so the
   model shows the final Content[] string, including generated browser code.
   MCPL-native aggregation gives a more useful five-item preview plus count;
   one show call reveals the two omitted matches. This is successful progressive
   disclosure, but structured text inside one string is less useful to preview.
5. **Full disclosure during recovery.** Initial runs 1–2 requested and received
   a 35,213-character full snapshot; run 3 showed a 10,633-character unfiltered
   result. These are allowed show requests, not cap bypasses. The small automatic
   preview alone does not prevent the model from choosing a full dump.
6. **Snapshot files.** This Playwright version's navigation response links to a
   snapshot file; full snapshots can instead be requested with browser_snapshot.
   Do not attribute all navigation-response reduction to MCPL. That file is not
   automatically available to the model through a filesystem tool. A harmless
   missing favicon generated one browser console error; no MCP error resulted.
7. **Discovery is on demand, not free.** Each session used help() (517 chars) and
   help("playwright") (5,029 chars). None requested individual builtin docs.
   This avoids initial schemas but still loads the whole server's signature list
   after discovery. Help text is excluded from the table's data-character counts.

## Description/context budget

Exact description sizes: run **1,473 Unicode characters / 1,479 UTF-8 bytes**;
help 377 bytes; show 416 bytes. Run is approximately **364 tokens**, or **0.284%**
of the harness's configured 128,000-token context window. All three descriptions
are approximately 526 tokens; serialized declarations including schemas are
approximately 694 tokens. These use `js-tiktoken@1.0.21` with `o200k_base`, a
**proxy tokenizer, not verified model-native accounting**. Serialization also
is not provider chat framing. The first baseline request reported 705 input
tokens for the entire request, not an attributable run-only count. Exact
measurements and method are in [description-budget.json](description-budget.json).

## Proposed follow-ups

Tracked as separate project tickets, not implemented in this experiment:

- **Test whether MCPL composition emerges without task-specific prompting:**
  compare discoverable guidance and a second MCP without a code-execution escape
  hatch; distinguish manipulation capability from preferred model strategy.
- **Choose how to compose structured data embedded in text-only MCP results:**
  decide on docs, generic extraction, adapters or upstream structured output;
  avoid silently hard-coding Playwright Markdown into the language.
- **Evaluate disclosure defaults for text-wrapped tool results:** compare useful
  previews and bounded inspection against extra round trips and accidental
  intermediate dumps, preserving intentional show behavior.

## Evidence integrity and checks

Raw JSONL contains session declarations, the user prompt, model messages,
programs, unmodified previews/errors/final answers, raw MCP responses and Pi
forwarded results. A response can occur more than once in that file; model
context is not its byte size. No credentials or private browsing data are in
these transcripts. [summary.json](summary.json) is generated by the stdlib-only
[summarizer](../../demo/summarize.py), not another model's interpretation.

Validation before publication: Rust formatting and all 183 existing Rust tests;
fresh npm install, TypeScript check and all nine harness tests; two Python
artifact tests; independent HTML parsing against expected labels/count; exact
ordered final-answer labels checked across all eight transcripts; diff checks.
No MCPL, Playwright MCP or Chromium processes remained after the agent runs.
The target is this reproducible local experimental artifact, not a hosted
MCPL deployment. No language fixes were made under this ticket.
