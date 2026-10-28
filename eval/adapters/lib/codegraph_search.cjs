// Ranks CodeGraph symbols for one question, the way codegraph_explore gathers them.
//
//   node codegraph_search.cjs <codegraph library entry> <corpus root> <question> <limit>
//
// codegraph_explore is CodeGraph's one advertised search tool, but it answers with
// a Markdown page of source grouped by file, not a ranked list. Its retrieval is
// findRelevantContext with the options below; this calls that directly and prints
// the search hits (roots, in rank order) first, then the graph neighbours it pulled in.

const [library, root, question, rawLimit] = process.argv.slice(2);
const limit = Number(rawLimit) || 10;
const { default: CodeGraph } = require(library);

// The exact options handleExplore passes in CodeGraph 1.6.2 (dist/mcp/tools.js).
const EXPLORE_OPTIONS = { searchLimit: 8, traversalDepth: 3, maxNodes: 200, minScore: 0.2 };
const SKIPPED_KINDS = new Set(["file", "import", "export"]);

main().catch((err) => {
  console.error(`codegraph: ${err && err.message ? err.message : err}`);
  process.exit(1);
});

async function main() {
  const cg = await CodeGraph.open(root);
  const subgraph = await cg.findRelevantContext(question, EXPLORE_OPTIONS);
  const ordered = [
    ...subgraph.roots.map((id) => subgraph.nodes.get(id)),
    ...subgraph.nodes.values(),
  ];
  const seen = new Set();
  const lines = [];
  for (const node of ordered) {
    if (!node || seen.has(node.id) || SKIPPED_KINDS.has(node.kind)) continue;
    seen.add(node.id);
    const pointer = node.startLine ? `${node.filePath}:L${node.startLine}-L${node.endLine || node.startLine}` : node.filePath;
    lines.push(pointer);
    if (lines.length >= limit) break;
  }
  cg.destroy();
  if (lines.length > 0) process.stdout.write(`${lines.join("\n")}\n`);
}
