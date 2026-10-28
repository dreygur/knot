// Throwaway: lays out Knot's real call graph once, so the prototype page only renders.
const fs = require("fs");
const Graph = require("graphology");
const fa2 = require("graphology-layout-forceatlas2");
const { pagerank } = require("graphology-metrics/centrality");

const raw = JSON.parse(fs.readFileSync(__dirname + "/raw.json", "utf8"));
const g = new Graph({ type: "directed", multi: false });

const folderOf = (file) => {
  const parts = file.split("/");
  return parts.length > 2 ? parts.slice(0, 2).join("/") : parts.length === 2 ? parts[0] : "(root)";
};
const kindOf = (sig) => {
  const m = sig.match(/\b(fn|struct|enum|trait|impl|mod|const|type|class|function|interface|def)\b/);
  return m ? ({ function: "fn", def: "fn", class: "struct", interface: "trait" }[m[1]] || m[1]) : "symbol";
};

const fileOf = (n) => n.id.split("::")[0];
const folders = [...new Set(raw.nodes.map((n) => folderOf(fileOf(n))))].sort();
raw.nodes.forEach((n) => {
  const file = fileOf(n);
  const tail = ` (${file}:L`;
  const cut = n.title.lastIndexOf(tail);
  const sig = n.title.slice(0, cut);
  const line = n.title.slice(cut + tail.length, -1);
  const folder = folderOf(file);
  const fi = folders.indexOf(folder);
  const a = (fi / folders.length) * Math.PI * 2;
  g.addNode(n.id, {
    label: n.label, file, line: +line, sig: sig.trim(), folder, kind: kindOf(sig),
    x: Math.cos(a) * 100 + Math.random() * 20, y: Math.sin(a) * 100 + Math.random() * 20,
  });
});
raw.edges.forEach((e) => { if (g.hasNode(e.from) && g.hasNode(e.to) && e.from !== e.to && !g.hasEdge(e.from, e.to)) g.addEdge(e.from, e.to); });

const pr = pagerank(g, { getEdgeWeight: null });

// Lay out with a hidden anchor per folder, tied to its members, so each folder settles
// into its own region and calls between folders read as bridges. Anchors are removed after.
const L = new Graph({ type: "undirected" });
g.forEachNode((id, a) => L.addNode(id, { x: a.x, y: a.y }));
g.forEachEdge((e, a, s, t) => { if (!L.hasEdge(s, t)) L.addEdge(s, t, { weight: 1 }); });
folders.forEach((f, i) => {
  const ang = (i / folders.length) * Math.PI * 2;
  L.addNode("@" + f, { x: Math.cos(ang) * 400, y: Math.sin(ang) * 400 });
});
g.forEachNode((id, a) => L.addEdge(id, "@" + a.folder, { weight: 1.5 }));
fa2.assign(L, { iterations: 1200, getEdgeWeight: "weight", settings: { ...fa2.inferSettings(L), gravity: 0.4, scalingRatio: 3, barnesHutOptimize: true, edgeWeightInfluence: 1, linLogMode: true, outboundAttractionDistribution: true } });
L.forEachNode((id, a) => { if (!id.startsWith("@")) g.mergeNodeAttributes(id, { x: a.x, y: a.y }); });

const out = {
  folders,
  nodes: g.mapNodes((id, a) => ({
    id, l: a.label, f: a.file, ln: a.line, s: a.sig, fo: folders.indexOf(a.folder), k: a.kind,
    x: +a.x.toFixed(2), y: +a.y.toFixed(2), pr: +pr[id].toFixed(6),
  })),
  edges: g.mapEdges((e, a, s, t) => [s, t]),
};
fs.writeFileSync(__dirname + "/graph.json", JSON.stringify(out));
console.log(out.nodes.length, "nodes", out.edges.length, "edges", folders.length, "folders:", folders.join(", "));
