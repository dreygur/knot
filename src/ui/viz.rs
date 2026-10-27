use anyhow::{Context, Result};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

use crate::ast::pagerank::top_hubs;
use crate::ast::resolve::CodeGraph;

fn generate_graph_html(graph_data_json: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <title>Knot Code Graph Visualizer</title>
  <script type="text/javascript" src="https://unpkg.com/vis-network@9.1.9/standalone/umd/vis-network.min.js"></script>
  <style>
    body {{ margin: 0; padding: 0; background-color: #0f172a; color: #f8fafc; font-family: system-ui, sans-serif; }}
    #header {{ padding: 12px 20px; background-color: #1e293b; border-bottom: 1px solid #334155; display: flex; justify-content: space-between; align-items: center; }}
    #network {{ width: 100vw; height: calc(100vh - 54px); }}
    .badge {{ background: #3b82f6; padding: 4px 8px; border-radius: 4px; font-size: 12px; font-weight: bold; }}
  </style>
</head>
<body>
  <div id="header">
    <div style="font-weight: bold; font-size: 16px;">Knot Code Graph & Centrality Visualizer</div>
    <div><span class="badge">Live Graph</span></div>
  </div>
  <div id="network"></div>
  <script>
    const data = {graph_data_json};
    const container = document.getElementById('network');
    const options = {{
      nodes: {{
        shape: 'dot',
        size: 16,
        font: {{ size: 12, color: '#f8fafc' }},
        borderWidth: 2,
        shadow: true
      }},
      edges: {{
        width: 1.5,
        color: {{ color: '#64748b', highlight: '#38bdf8' }},
        arrows: {{ to: {{ enabled: true, scaleFactor: 0.6 }} }},
        smooth: {{ type: 'continuous' }}
      }},
      physics: {{
        stabilization: false,
        barnesHut: {{ gravitationalConstant: -3000, springLength: 100 }}
      }}
    }};
    new vis.Network(container, data, options);
  </script>
</body>
</html>"#
    )
}

fn open_browser(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd").args(["/C", "start", &path.to_string_lossy()]).spawn()?;
    }
    Ok(())
}

pub fn export_graph_html(graph: &CodeGraph, output_path: &Path) -> Result<()> {
    let hubs = top_hubs(graph, 10);
    let top_hub_ids: std::collections::HashSet<&str> = hubs.iter().map(|(s, _)| s.id.as_str()).collect();

    let mut nodes = Vec::new();
    let mut edges = Vec::new();

    for sym in graph.symbols.values() {
        let is_hub = top_hub_ids.contains(sym.id.as_str());
        let color = if is_hub { "#ef4444" } else { "#3b82f6" };
        let size = if is_hub { 24 } else { 14 };

        nodes.push(json!({
            "id": sym.id,
            "label": sym.name,
            "title": format!("{} ({}:L{})", sym.signature, sym.file, sym.span.0),
            "color": color,
            "size": size,
        }));
    }

    for (caller_id, callee_ids) in &graph.callees {
        for callee_id in callee_ids {
            edges.push(json!({
                "from": caller_id,
                "to": callee_id,
            }));
        }
    }

    let payload = json!({
        "nodes": nodes,
        "edges": edges,
    });

    // Symbol text comes from the indexed repo; escaping <, > and & keeps a signature
    // containing `</script>` from ending the inline script. JSON reads the escapes back.
    let safe_json = payload
        .to_string()
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    let html_content = generate_graph_html(&safe_json);
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)?;
    }

    fs::write(output_path, html_content)?;
    Ok(())
}

pub fn render_and_launch_viz(repo_root: &Path) -> Result<PathBuf> {
    let graph = crate::ask::load_graph(repo_root)?;
    let out_path = repo_root.join(".knot").join("graph.html");

    export_graph_html(&graph, &out_path).context("Failed to export graph HTML")?;
    let _ = open_browser(&out_path);
    Ok(out_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_export_graph_html() -> Result<()> {
        let dir = tempdir()?;
        let graph = CodeGraph::default();
        let out_file = dir.path().join("test_graph.html");

        export_graph_html(&graph, &out_file)?;
        assert!(out_file.exists());

        let html = fs::read_to_string(&out_file)?;
        assert!(html.contains("Knot Code Graph Visualizer"));
        assert!(html.contains("vis.Network"));

        Ok(())
    }

    #[test]
    fn test_symbol_text_cannot_close_the_script_tag() -> Result<()> {
        let dir = tempdir()?;
        let mut graph = CodeGraph::default();
        graph.symbols.insert(
            "evil.ts::x".into(),
            crate::ast::types::SymbolNode {
                id: "evil.ts::x".into(),
                file: "evil.ts".into(),
                name: "x".into(),
                kind: crate::ast::types::SymbolKind::Function,
                signature: "function x() { return '</script><script>alert(1)</script>' }".into(),
                span: (1, 1),
                crux: String::new(),
                hash: String::new(),
                outgoing_calls: vec![],
            },
        );
        let out_file = dir.path().join("g.html");
        export_graph_html(&graph, &out_file)?;

        let html = fs::read_to_string(&out_file)?;
        assert_eq!(html.matches("</script>").count(), 2, "only the page's own script tags may close");
        assert!(!html.contains("<script>alert"));
        Ok(())
    }
}
