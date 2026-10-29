// Reads a trace-mcp get_feature_context JSON result on stdin and prints one
// pointer per item, in its ranked order. The items carry a symbol id but no
// lines, so the lines come from trace-mcp's own index for that symbol.
//
//   ... | node tracemcp_pointers.mjs <index db> <limit>

import { DatabaseSync } from "node:sqlite";

const [dbPath, rawLimit] = process.argv.slice(2);
const limit = Number(rawLimit) || 10;
let input = "";
for await (const chunk of process.stdin) input += chunk;
const start = input.indexOf("{");
if (start < 0) process.exit(0);
const items = JSON.parse(input.slice(start)).items ?? [];

const db = new DatabaseSync(dbPath, { readOnly: true });
const lookup = db.prepare("SELECT line_start, line_end FROM symbols WHERE symbol_id = ?");
const seen = new Set();
const lines = [];
for (const item of items) {
  const file = item.filePath ?? item.file;
  if (!file) continue;
  const row = item.symbolId ? lookup.get(item.symbolId) : undefined;
  const pointer = row && row.line_start ? `${file}:L${row.line_start}-L${row.line_end ?? row.line_start}` : file;
  if (seen.has(pointer)) continue;
  seen.add(pointer);
  lines.push(pointer);
  if (lines.length >= limit) break;
}
if (lines.length > 0) process.stdout.write(`${lines.join("\n")}\n`);
