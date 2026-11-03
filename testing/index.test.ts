import assert from "node:assert/strict";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { test, type TestContext } from "node:test";
import { fileURLToPath } from "node:url";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { InMemoryTransport } from "@modelcontextprotocol/sdk/inMemory.js";
import { LATEST_PROTOCOL_VERSION, type JSONRPCMessage, type Tool } from "@modelcontextprotocol/sdk/types.js";
import { bridgeTool, model, run, systemPrompt } from "./index.js";

const tool: Tool = {
  name: "run",
  description: "Exact MCPL description.\n  Preserve whitespace and punctuation!",
  inputSchema: {
    type: "object",
    properties: { program: { type: "string", description: "Source text" } },
    required: ["program"],
    additionalProperties: false,
  },
};
const binary = fileURLToPath(new URL("../target/debug/mcpl", import.meta.url));
const config = fileURLToPath(new URL("./empty.json", import.meta.url));
const program = "numbers = [1, 2, 3]; total = reduce(numbers, (sum, item) => sum + item, 0);";
const finalAnswer = "The total is 6, and no downstream servers are configured.";

for (const name of ["run", "help", "show"]) test(`bridgeTool preserves ${name} discovery and raw result details`, async () => {
  const discovered = { ...tool, name };
  const raw = {
    content: [
      { type: "text" as const, text: "First block\n  verbatim" },
      { type: "text" as const, text: '{"handle":"opaque","preview":[1,2]}' },
    ],
    structuredContent: { handle: "opaque", preview: [1, 2] },
    isError: true,
    _meta: { origin: "test" },
  };
  const calls: unknown[][] = [];
  const client = {
    async callTool(...args: unknown[]) { calls.push(args); return raw; },
  } as unknown as Client;
  const events: Record<string, unknown>[] = [];
  const bridge = bridgeTool(client, discovered, (event) => events.push(event));
  assert.equal(bridge.name, name);
  assert.equal(bridge.description, tool.description);
  assert.strictEqual(bridge.parameters, tool.inputSchema);
  const args = { program: "an exact program" };
  const signal = new AbortController().signal;
  const result = await bridge.execute("call-1", args, signal);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0]?.[0], { name, arguments: args });
  assert.deepEqual(calls[0]?.[2], { signal, timeout: 0 });
  assert.deepEqual(result.content, raw.content);
  assert.equal(result.isError, true);
  assert.deepEqual(result.details, raw);
  assert.deepEqual(events, [{ type: "mcp_result", toolCallId: "call-1", result: raw }]);
});

test("bridgeTool defaults a missing error flag to false", async () => {
  const raw = { content: [{ type: "text", text: "ok" }] };
  const client = { async callTool() { return raw; } } as unknown as Client;
  const result = await bridgeTool(client, tool, () => {}).execute("call-2", {});
  assert.equal(result.isError, false);
  assert.deepEqual(result.details, raw);
});

test("bridgeTool rejects another tool or a missing description", () => {
  const client = {} as Client;
  for (const invalid of [
    { ...tool, name: "other" },
    { name: "run", inputSchema: tool.inputSchema },
  ]) {
    assert.throws(() => bridgeTool(client, invalid, () => {}), /advertise run, help and show with descriptions/);
  }
});

test("patched SDK leaves initialize/list/call pending without deadlines and permits cancellation", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const [transport, peer] = InMemoryTransport.createLinkedPair();
  const client = new Client({ name: "no-deadline-test", version: "1" });
  t.after(() => client.close());
  const messages: JSONRPCMessage[] = [];
  peer.onmessage = (message) => { messages.push(message); };
  await peer.start();
  const controller = new AbortController();
  const options = { timeout: 0, signal: controller.signal };
  const nextTick = () => new Promise<void>((resolve) => setImmediate(resolve));
  async function assertPending(promise: Promise<unknown>) {
    let settled = false;
    void promise.then(() => { settled = true; }, () => { settled = true; });
    await nextTick();
    t.mock.timers.tick(1_000_000);
    await nextTick();
    assert.equal(settled, false);
  }
  async function respond(method: string, result: Record<string, unknown>) {
    const request = messages.find((message) => "method" in message && message.method === method);
    assert.ok(request && "id" in request && request.id !== undefined);
    await peer.send({ jsonrpc: "2.0", id: request.id, result });
  }
  const connection = client.connect(transport, options);
  await assertPending(connection);
  await respond("initialize", {
    protocolVersion: LATEST_PROTOCOL_VERSION, capabilities: { tools: {} },
    serverInfo: { name: "test", version: "1" },
  });
  await connection;
  const discovery = client.listTools({}, options);
  await assertPending(discovery);
  await respond("tools/list", { tools: [tool] });
  await discovery;
  const call = bridgeTool(client, tool, () => {}).execute("pending", { program }, controller.signal);
  await assertPending(call);
  controller.abort(new Error("manual cancellation"));
  await assert.rejects(call, /manual cancellation/);
  assert.ok(messages.some((message) => "method" in message && message.method === "notifications/cancelled"));
});

interface CompletionRequest {
  model: string;
  messages: { role: string; content?: unknown; tool_call_id?: string }[];
  tools: { type: string; function: { name: string; description: string; parameters: unknown } }[];
}

async function fakeModel(t: TestContext, mode: "complete" | "error" | "loop" = "complete") {
  const requests: CompletionRequest[] = [];
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, "/v1/chat/completions");
      assert.equal(request.method, "POST");
      assert.equal(request.headers.authorization, "Bearer test");
      const chunks: Buffer[] = [];
      for await (const chunk of request) chunks.push(Buffer.from(chunk));
      requests.push(JSON.parse(Buffer.concat(chunks).toString()) as CompletionRequest);
      if (mode === "error") {
        response.writeHead(400, { "content-type": "application/json" });
        response.end(JSON.stringify({ error: { message: "fake model failure", type: "invalid_request_error" } }));
        return;
      }
      response.writeHead(200, { "content-type": "text/event-stream" });
      const toolTurn = requests.length === 1 || mode === "loop";
      const send = (delta: unknown, finish_reason: string | null = null) => {
        response.write(`data: ${JSON.stringify({
          id: `completion-${requests.length}`, object: "chat.completion.chunk",
          created: 1, model: model.id,
          choices: [{ index: 0, delta, finish_reason }],
        })}\n\n`);
      };
      send({ role: "assistant", content: "" });
      if (toolTurn) {
        send({ tool_calls: [
          { name: "help", args: {} },
          { name: "run", args: { program } },
          { name: "show", args: { expression: "total" } },
        ].map(({ name, args }, index) => ({
          index, id: `call_${name}`, type: "function",
          function: { name, arguments: JSON.stringify(args) },
        })) });
      } else {
        send({ content: finalAnswer });
      }
      send({}, toolTurn ? "tool_calls" : "stop");
      response.end("data: [DONE]\n\n");
    } catch (error) {
      response.writeHead(500, { "content-type": "application/json" });
      response.end(JSON.stringify({ error: { message: String(error) } }));
    }
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise<void>((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  });
  return { requests, baseUrl: `http://127.0.0.1:${(server.address() as AddressInfo).port}/v1` };
}

test("real MCPL run/help/show reach Pi verbatim and execute sequentially in one session", { timeout: 20_000 }, async (t) => {
  const { requests, baseUrl } = await fakeModel(t);
  const events: Record<string, unknown>[] = [];
  await run({
    binary, config, prompt: "Inspect downstream tools and sum three numbers", apiKey: "test",
    model: { ...model, baseUrl }, signal: AbortSignal.timeout(15_000),
    emit: (event) => events.push(event),
  });
  assert.equal(requests.length, 2);
  const session = events.find((event) => event.type === "session");
  assert.ok(session);
  const discovered = session.tools as Tool[];
  assert.deepEqual(discovered.map((tool) => tool.name).sort(), ["help", "run", "show"]);
  assert.equal(systemPrompt, "You are a helpful assistant completing the user's task.");
  assert.equal(session.systemPrompt, systemPrompt);
  for (const request of requests) {
    assert.equal(request.model, model.id);
    assert.equal(request.tools.length, 3);
    assert.deepEqual(request.tools, discovered.map((tool) => ({
      type: "function",
      function: { name: tool.name, description: tool.description, parameters: tool.inputSchema },
    })));
    assert.deepEqual(request.messages.filter((message) => ["system", "developer"].includes(message.role)), [
      { role: "system", content: systemPrompt },
    ]);
  }
  assert.deepEqual(requests[0]?.messages, [
    { role: "system", content: systemPrompt },
    { role: "user", content: [{ type: "text", text: "Inspect downstream tools and sum three numbers" }] },
  ]);
  assert.deepEqual(events.filter((event) => event.type === "tool_call"), [
    { type: "tool_call", toolCallId: "call_help", name: "help", arguments: {} },
    { type: "tool_call", toolCallId: "call_run", name: "run", arguments: { program } },
    { type: "tool_call", toolCallId: "call_show", name: "show", arguments: { expression: "total" } },
  ]);
  assert.deepEqual(events.filter((event) => ["tool_call", "tool_result"].includes(event.type as string))
    .map((event) => [event.type, event.toolCallId]), [
    ["tool_call", "call_help"], ["tool_result", "call_help"],
    ["tool_call", "call_run"], ["tool_result", "call_run"],
    ["tool_call", "call_show"], ["tool_result", "call_show"],
  ]);
  const rawEvents = events.filter((event) => event.type === "mcp_result");
  assert.equal(rawEvents.length, 3);
  const texts = rawEvents.map((rawEvent) => {
    const raw = rawEvent.result as { content: { type: string; text: string }[]; isError?: boolean };
    assert.notEqual(raw.isError, true, JSON.stringify(raw));
    const resultEvent = events.find((event) => event.type === "tool_result" && event.toolCallId === rawEvent.toolCallId);
    assert.ok(resultEvent);
    assert.deepEqual((resultEvent.result as { details: unknown }).details, raw);
    const text = raw.content.map((block) => block.text).join("\n");
    const toolMessage = requests[1]?.messages.find((message) => message.role === "tool" && message.tool_call_id === rawEvent.toolCallId);
    assert.ok(toolMessage);
    assert.equal(toolMessage.content, text);
    return text;
  });
  assert.match(texts[0]!, /Servers\n  \(none configured\)/);
  assert.match(texts[1]!, /numbers:/);
  assert.match(texts[1]!, /total:.* = 6/);
  assert.equal(texts[2], "6");
  const messages = events.filter((event) => event.type === "message").map((event) => event.message);
  assert.ok(messages.some((message) => JSON.stringify(message).includes('"toolCall"')));
  assert.ok(messages.some((message) => JSON.stringify(message).includes(finalAnswer)));
});

test("run rejects a model error", { timeout: 20_000 }, async (t) => {
  const { requests, baseUrl } = await fakeModel(t, "error");
  await assert.rejects(run({
    binary, config, prompt: "Inspect tools", apiKey: "test", model: { ...model, baseUrl },
    signal: AbortSignal.timeout(15_000), emit: () => {},
  }), /fake model failure/);
  assert.equal(requests.length, 1);
});

test("run rejects when model tool calls reach max-turns", { timeout: 20_000 }, async (t) => {
  const { requests, baseUrl } = await fakeModel(t, "loop");
  await assert.rejects(run({
    binary, config, prompt: "Inspect tools", apiKey: "test", model: { ...model, baseUrl },
    maxTurns: 1, signal: AbortSignal.timeout(15_000), emit: () => {},
  }), /Stopped after 1 model turns/);
  assert.equal(requests.length, 1);
});
