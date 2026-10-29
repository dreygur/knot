// Calls one tool on an MCP server over stdio and prints the text of its result.
//
//   node mcp_client_a.mjs <cwd> <tool> <arguments json> -- <server command> [args...]
//
// Sends initialize, notifications/initialized and tools/call, then exits. A tool
// result flagged isError, or a JSON-RPC error, exits 1 with the message on stderr.

import { spawn } from "node:child_process";

const TIMEOUT_MS = Number(process.env.MCP_CLIENT_TIMEOUT_MS ?? 120000);

const [cwd, tool, rawArgs, separator, command, ...commandArgs] = process.argv.slice(2);
if (separator !== "--" || !command) {
  console.error("usage: mcp_client_a.mjs <cwd> <tool> <arguments json> -- <command> [args...]");
  process.exit(2);
}

const server = spawn(command, commandArgs, { cwd, stdio: ["pipe", "pipe", "pipe"] });
let stderr = "";
server.stderr.on("data", (chunk) => { stderr += chunk; });
server.on("error", (err) => fail(`spawn ${command}: ${err.message}`));
server.on("exit", (code) => fail(`server exited ${code} before answering: ${stderr.trim().slice(-2000)}`));

const pending = new Map();
let buffer = "";
server.stdout.on("data", (chunk) => {
  buffer += chunk;
  let newline;
  while ((newline = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, newline).trim();
    buffer = buffer.slice(newline + 1);
    if (!line.startsWith("{")) continue;
    let message;
    try { message = JSON.parse(line); } catch { continue; }
    const waiter = pending.get(message.id);
    if (waiter && (message.result !== undefined || message.error !== undefined)) {
      pending.delete(message.id);
      waiter(message);
    }
  }
});

const timer = setTimeout(() => fail(`timed out after ${TIMEOUT_MS} ms: ${stderr.trim().slice(-2000)}`), TIMEOUT_MS);

const initialized = await request(1, "initialize", {
  protocolVersion: "2025-06-18",
  capabilities: {},
  clientInfo: { name: "knot-eval", version: "1" },
});
if (initialized.error) fail(`initialize: ${initialized.error.message}`);
send({ jsonrpc: "2.0", method: "notifications/initialized" });

const called = await request(2, "tools/call", { name: tool, arguments: JSON.parse(rawArgs) });
if (called.error) fail(`${tool}: ${called.error.message}`);
const text = (called.result.content ?? []).filter((c) => c.type === "text").map((c) => c.text).join("\n");
if (called.result.isError) fail(`${tool} returned an error: ${text}`);

clearTimeout(timer);
server.removeAllListeners("exit");
process.stdout.write(text);
process.stdout.write("\n", () => {
  server.kill();
  process.exit(0);
});

function request(id, method, params) {
  return new Promise((resolve) => {
    pending.set(id, resolve);
    send({ jsonrpc: "2.0", id, method, params });
  });
}

function send(message) {
  server.stdin.write(`${JSON.stringify(message)}\n`);
}

function fail(message) {
  console.error(message);
  server.kill();
  process.exit(1);
}
