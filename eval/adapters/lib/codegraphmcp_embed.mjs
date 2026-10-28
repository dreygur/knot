// Keeps one code-graph-mcp server session open until its background embedding
// finishes, so later searches run hybrid (vector plus FTS) instead of FTS only.
//
//   node codegraphmcp_embed.mjs <corpus root> <server binary>
//
// The server downloads its model and embeds nodes only while a session is live,
// and each eval query is its own short session, so setup has to wait here.

import { spawn } from "node:child_process";

const POLL_MS = 3000;
const TIMEOUT_MS = 60 * 60 * 1000;

const [root, binary] = process.argv.slice(2);
const server = spawn(binary, ["serve"], { cwd: root, stdio: ["pipe", "pipe", "pipe"] });
let stderr = "";
server.stderr.on("data", (chunk) => { stderr = (stderr + chunk).slice(-4000); });
server.on("exit", (code) => fail(`server exited ${code}: ${stderr.trim()}`));

const pending = new Map();
let nextId = 1;
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
    if (waiter) {
      pending.delete(message.id);
      waiter(message);
    }
  }
});

const started = Date.now();
await request("initialize", { protocolVersion: "2025-06-18", capabilities: {}, clientInfo: { name: "knot-eval", version: "1" } });
send({ jsonrpc: "2.0", method: "notifications/initialized" });
// A query tool call is what brings the index up to date and starts the model download.
await callTool("semantic_code_search", { query: "warm up", compact: true });

let last = "";
while (Date.now() - started < TIMEOUT_MS) {
  const status = JSON.parse(await callTool("get_index_status", {}));
  const line = `${status.embedding_status} ${status.embedding_progress}`;
  if (line !== last) console.error(`[codegraphmcp] ${line}`);
  last = line;
  // The model lock is held while a batch embeds, so the status can read
  // "unavailable" mid run; the vector count is the reliable signal.
  const [done, total] = String(status.embedding_progress).split("/").map(Number);
  const finished = total > 0 && done >= total && status.embedding_status !== "in_progress";
  if (status.embedding_status === "complete" || finished) {
    server.removeAllListeners("exit");
    server.kill();
    process.exit(0);
  }
  if (status.embedding_status === "unavailable" && Date.now() - started > 10 * 60 * 1000) {
    fail("embedding model never became available");
  }
  await new Promise((resolve) => setTimeout(resolve, POLL_MS));
}
fail("timed out waiting for embeddings");

async function callTool(name, args) {
  const reply = await request("tools/call", { name, arguments: args });
  if (reply.error) fail(`${name}: ${reply.error.message}`);
  return (reply.result.content ?? []).filter((c) => c.type === "text").map((c) => c.text).join("\n");
}

function request(method, params) {
  const id = nextId++;
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
