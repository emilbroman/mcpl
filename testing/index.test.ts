import assert from "node:assert/strict";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { test, type TestContext } from "node:test";
import { fileURLToPath } from "node:url";
import type { Client } from "@modelcontextprotocol/sdk/client/index.js";
import type { Tool } from "@modelcontextprotocol/sdk/types.js";
import { bridgeRun, model, run, systemPrompt } from "./index.js";

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
const program = "inspect downstream tools";
const finalAnswer = "The MCPL stub returned no downstream servers.";

test("bridgeRun preserves discovery, text blocks, error flag and raw result details", async () => {
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
  const bridge = bridgeRun(client, tool, (event) => events.push(event));
  assert.equal(bridge.name, "run");
  assert.equal(bridge.description, tool.description);
  assert.strictEqual(bridge.parameters, tool.inputSchema);
  const args = { program: "an exact program" };
  const signal = new AbortController().signal;
  const result = await bridge.execute("call-1", args, signal);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0]?.[0], { name: "run", arguments: args });
  assert.deepEqual(calls[0]?.[2], { signal, timeout: 120_000 });
  assert.deepEqual(result.content, raw.content);
  assert.equal(result.isError, true);
  assert.deepEqual(result.details, raw);
  assert.deepEqual(events, [{ type: "mcp_result", toolCallId: "call-1", result: raw }]);
});

test("bridgeRun defaults a missing error flag to false", async () => {
  const raw = { content: [{ type: "text", text: "ok" }] };
  const client = { async callTool() { return raw; } } as unknown as Client;
  const result = await bridgeRun(client, tool, () => {}).execute("call-2", {});
  assert.equal(result.isError, false);
  assert.deepEqual(result.details, raw);
});

test("bridgeRun rejects another tool or a missing description", () => {
  const client = {} as Client;
  for (const invalid of [
    { ...tool, name: "other" },
    { name: "run", inputSchema: tool.inputSchema },
  ]) {
    assert.throws(() => bridgeRun(client, invalid, () => {}), /advertise run with a description/);
  }
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
        send({ tool_calls: [{ index: 0, id: "call_mcpl", type: "function", function: {
          name: "run", arguments: JSON.stringify({ program }),
        } }] });
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

test("real MCPL run reaches Pi and the transcript without added language instructions", { timeout: 20_000 }, async (t) => {
  const { requests, baseUrl } = await fakeModel(t);
  const events: Record<string, unknown>[] = [];
  await run({
    binary, config, prompt: "Inspect downstream tools", apiKey: "test",
    model: { ...model, baseUrl }, signal: AbortSignal.timeout(15_000),
    emit: (event) => events.push(event),
  });
  assert.equal(requests.length, 2);
  const session = events.find((event) => event.type === "session");
  assert.ok(session);
  const discovered = session.tool as Tool;
  assert.equal(discovered.name, "run");
  assert.ok(discovered.description);
  assert.equal(systemPrompt, "You are a helpful assistant completing the user's task.");
  assert.equal(session.systemPrompt, systemPrompt);
  for (const request of requests) {
    assert.equal(request.model, model.id);
    assert.equal(request.tools.length, 1);
    assert.equal(request.tools[0]?.type, "function");
    assert.equal(request.tools[0]?.function.name, "run");
    assert.equal(request.tools[0]?.function.description, discovered.description);
    assert.deepEqual(request.tools[0]?.function.parameters, discovered.inputSchema);
    assert.deepEqual(request.messages.filter((message) => ["system", "developer"].includes(message.role)), [
      { role: "system", content: systemPrompt },
    ]);
  }
  assert.deepEqual(requests[0]?.messages, [
    { role: "system", content: systemPrompt },
    { role: "user", content: [{ type: "text", text: "Inspect downstream tools" }] },
  ]);
  assert.deepEqual(events.find((event) => event.type === "tool_call"), {
    type: "tool_call", toolCallId: "call_mcpl", name: "run", arguments: { program },
  });
  const rawEvent = events.find((event) => event.type === "mcp_result");
  assert.ok(rawEvent);
  assert.equal(rawEvent.toolCallId, "call_mcpl");
  const raw = rawEvent.result as { content: { type: string; text: string }[]; structuredContent: unknown };
  const stub = {
    stub: true,
    message: "Program not executed; MCPL language support is not implemented yet.",
    servers: [],
  };
  assert.deepEqual(raw.structuredContent, stub);
  assert.deepEqual(JSON.parse(raw.content.map((block) => block.text).join("\n")), stub);
  const resultEvent = events.find((event) => event.type === "tool_result");
  assert.ok(resultEvent);
  assert.deepEqual((resultEvent.result as { details: unknown }).details, raw);
  const toolMessage = requests[1]?.messages.find((message) => message.role === "tool");
  assert.ok(toolMessage);
  assert.equal(toolMessage.tool_call_id, "call_mcpl");
  assert.equal(typeof toolMessage.content, "string");
  assert.deepEqual(JSON.parse(toolMessage.content as string), stub);
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
