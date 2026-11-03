import { Agent, type AgentTool } from "@earendil-works/pi-agent-core";
import type { Model } from "@earendil-works/pi-ai";
import { streamSimple } from "@earendil-works/pi-ai/api/openai-completions";
import { Client } from "@modelcontextprotocol/sdk/client/index.js";
import { StdioClientTransport } from "@modelcontextprotocol/sdk/client/stdio.js";
import { CallToolResultSchema, type Tool } from "@modelcontextprotocol/sdk/types.js";
import { parseArgs } from "node:util";
import { pathToFileURL } from "node:url";
import modelConfig from "./model.json" with { type: "json" };

export const model = modelConfig as Model<"openai-completions">;
export const systemPrompt = "You are a helpful assistant completing the user's task.";
type Emit = (event: Record<string, unknown>) => void;

export const toolNames = ["run", "help", "show"] as const;

export function bridgeTool(client: Client, tool: Tool, emit: Emit): AgentTool {
  if (!toolNames.some((name) => name === tool.name) || typeof tool.description !== "string") {
    throw new Error("MCPL must advertise run, help and show with descriptions");
  }
  return {
    name: tool.name,
    label: tool.name,
    description: tool.description,
    parameters: tool.inputSchema,
    async execute(_id, args, signal) {
      const result = CallToolResultSchema.parse(await client.callTool(
        { name: tool.name, arguments: args as Record<string, unknown> },
        CallToolResultSchema,
        { signal, timeout: 0 },
      ));
      emit({ type: "mcp_result", toolCallId: _id, result });
      return {
        content: result.content.map((block) => {
          if (block.type === "text" || block.type === "image") return block;
          // Pi accepts text/images; preserve other MCP blocks as JSON, not summaries.
          return { type: "text" as const, text: JSON.stringify(block) };
        }),
        details: result,
        isError: result.isError ?? false,
      };
    },
  };
}

export interface RunOptions {
  binary: string;
  config: string;
  prompt: string;
  apiKey: string;
  model?: Model<"openai-completions">;
  maxTurns?: number;
  signal?: AbortSignal;
  emit?: Emit;
}

export async function run(options: RunOptions): Promise<void> {
  const emit = options.emit ?? ((event) => console.log(JSON.stringify(event)));
  const maxTurns = options.maxTurns ?? 12;
  if (!Number.isSafeInteger(maxTurns) || maxTurns < 1) {
    throw new Error("max-turns must be a positive integer");
  }
  if (!options.apiKey.trim()) throw new Error("Set LITELLM_API_KEY in the environment");
  options.signal?.throwIfAborted();
  const client = new Client({ name: "mcpl-testing", version: "0.1.0" });
  const transport = new StdioClientTransport({
    command: options.binary,
    args: ["--config", options.config],
    stderr: "inherit",
    // Inherit downstream configuration, but never give it the model credential.
    env: Object.fromEntries(Object.entries(process.env).filter(
      (entry): entry is [string, string] => entry[1] !== undefined && entry[0] !== "LITELLM_API_KEY",
    )),
  });
  let agent: Agent | undefined;
  const abort = () => { agent?.abort(); void client.close(); };
  options.signal?.addEventListener("abort", abort, { once: true });
  try {
    await client.connect(transport, { signal: options.signal, timeout: 0 });
    const discovery = await client.listTools({}, { signal: options.signal, timeout: 0 });
    if (discovery.nextCursor || discovery.tools.length !== toolNames.length
      || !toolNames.every((name) => discovery.tools.some((tool) => tool.name === name))) {
      throw new Error("Expected MCPL to expose exactly run, help and show");
    }
    const tools = discovery.tools.map((tool) => bridgeTool(client, tool, emit));
    emit({ type: "session", model: (options.model ?? model).id, systemPrompt, tools: discovery.tools });
    let turns = 0;
    let limited = false;
    agent = new Agent({
      initialState: { model: options.model ?? model, systemPrompt, tools },
      streamFn: (selected, context, settings) => streamSimple(
        selected as Model<"openai-completions">, context, { ...settings, maxTokens: 8192 },
      ),
      getApiKey: () => options.apiKey,
      toolExecution: "sequential",
      finishTurn: ({ message }) => {
        turns += 1;
        if (turns >= maxTurns && message.stopReason === "toolUse") {
          limited = true;
          return { action: "end" };
        }
      },
    });
    agent.subscribe((event) => {
      if (event.type === "message_end") emit({ type: "message", message: event.message });
      if (event.type === "tool_execution_start") emit({ type: "tool_call", toolCallId: event.toolCallId, name: event.toolName, arguments: event.args });
      if (event.type === "tool_execution_end") emit({ type: "tool_result", toolCallId: event.toolCallId, result: event.result, isError: event.isError });
    });
    options.signal?.throwIfAborted();
    await agent.prompt(options.prompt);
    options.signal?.throwIfAborted();
    if (agent.state.errorMessage) throw new Error(agent.state.errorMessage);
    if (limited) throw new Error(`Stopped after ${maxTurns} model turns; increase --max-turns if needed`);
    const last = agent.state.messages.at(-1);
    if (last?.role === "assistant" && last.stopReason === "length") {
      throw new Error("Model output reached its token limit");
    }
  } finally {
    options.signal?.removeEventListener("abort", abort);
    await client.close();
    await transport.close();
  }
}

async function main() {
  const { values, positionals } = parseArgs({
    allowPositionals: true,
    options: {
      mcpl: { type: "string", default: "../target/debug/mcpl" },
      config: { type: "string" },
      "max-turns": { type: "string", default: "12" },
      help: { type: "boolean" },
    },
  });
  if (values.help) {
    console.log('Usage: npm start -- --config <mcp-config.json> [--mcpl <binary>] [--max-turns 12] "Your task"');
    return;
  }
  if (!values.config || positionals.length !== 1 || !positionals[0]?.trim()) {
    throw new Error('Provide --config <mcp-config.json> and one quoted task; see --help');
  }
  const controller = new AbortController();
  const interrupt = () => controller.abort(new Error("Interrupted"));
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", interrupt);
  try {
    await run({
      binary: values.mcpl!, config: values.config, prompt: positionals[0],
      apiKey: process.env.LITELLM_API_KEY ?? "", maxTurns: Number(values["max-turns"]),
      signal: controller.signal,
    });
  } finally {
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", interrupt);
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error: unknown) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exitCode = 1;
  });
}
