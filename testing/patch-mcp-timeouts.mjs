import { readFileSync, writeFileSync } from "node:fs";

const root = new URL("./node_modules/@modelcontextprotocol/sdk/", import.meta.url);
const { version } = JSON.parse(readFileSync(new URL("package.json", root), "utf8"));
if (version !== "1.32.2") throw new Error(`Recheck MCP timeout patch for SDK ${version}`);

const original = "    _setupTimeout(messageId, timeout, maxTotalTimeout, onTimeout, resetTimeoutOnProgress = false) {\n        this._timeoutInfo.set(messageId, {";
const patched = original.replace("        this._timeoutInfo", "        if (timeout === 0) return;\n        this._timeoutInfo");

// Upstream always schedules a timer; MCPL needs an explicit no-deadline mode.
for (const format of ["esm", "cjs"]) {
  const path = new URL(`dist/${format}/shared/protocol.js`, root);
  const source = readFileSync(path, "utf8");
  if (source.split(patched).length === 2) continue;
  if (source.split(original).length !== 2) {
    throw new Error(`MCP timeout patch anchor changed in ${path}`);
  }
  writeFileSync(path, source.replace(original, patched));
}
