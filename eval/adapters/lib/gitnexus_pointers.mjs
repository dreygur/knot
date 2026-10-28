// Reads `gitnexus query` JSON on stdin and prints one pointer per line in the
// order GitNexus presents results: symbols of each ranked execution flow, then
// the standalone definitions. File level hits have no lines and print bare.
//
//   gitnexus query ... | node gitnexus_pointers.mjs <limit>

const limit = Number(process.argv[2]) || 10;
let input = "";
for await (const chunk of process.stdin) input += chunk;
const start = input.indexOf("{");
if (start < 0) process.exit(0);
const result = JSON.parse(input.slice(start));

const seen = new Set();
const lines = [];
for (const hit of [...(result.process_symbols ?? []), ...(result.definitions ?? [])]) {
  if (!hit.filePath) continue;
  const pointer = hit.startLine ? `${hit.filePath}:L${hit.startLine}-L${hit.endLine ?? hit.startLine}` : hit.filePath;
  if (seen.has(pointer)) continue;
  seen.add(pointer);
  lines.push(pointer);
  if (lines.length >= limit) break;
}
if (lines.length > 0) process.stdout.write(`${lines.join("\n")}\n`);
