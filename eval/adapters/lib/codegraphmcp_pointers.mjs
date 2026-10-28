// Reads a code-graph-mcp semantic_code_search JSON result on stdin and prints
// one pointer per result, in its ranked order.
//
//   ... | node codegraphmcp_pointers.mjs <limit>

const limit = Number(process.argv[2]) || 10;
let input = "";
for await (const chunk of process.stdin) input += chunk;
const start = input.indexOf("{");
if (start < 0) process.exit(0);
const results = JSON.parse(input.slice(start)).results ?? [];

const seen = new Set();
const lines = [];
for (const hit of results) {
  if (!hit.file_path) continue;
  const span = lineSpan(hit);
  const pointer = span ? `${hit.file_path}:L${span[0]}-L${span[1]}` : hit.file_path;
  if (seen.has(pointer)) continue;
  seen.add(pointer);
  lines.push(pointer);
  if (lines.length >= limit) break;
}
if (lines.length > 0) process.stdout.write(`${lines.join("\n")}\n`);

// Compact rows say "line": "a-b"; full rows have start_line and end_line; a
// compressed envelope only names the lines inside its summary text.
function lineSpan(hit) {
  if (hit.start_line) return [hit.start_line, hit.end_line ?? hit.start_line];
  const text = typeof hit.line === "string" ? hit.line : String(hit.summary ?? "").match(/lines (\d+-\d+)/)?.[1];
  const match = text?.match(/^(\d+)-(\d+)$/);
  return match ? [Number(match[1]), Number(match[2])] : null;
}
