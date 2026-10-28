use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ui::viz_assets::{data_uri_png, font_face_css, FA2_JS, GRAPHOLOGY_JS, LOGO_DARK, LOGO_LIGHT, MARK_DARK_64, MARK_LIGHT_64, SIGMA_JS};
use crate::ui::viz_data::{viz_data, VizData};

const TEMPLATE: &str = include_str!("viz.html");

/// Replace each `{{NAME}}` in `template` with `value(NAME)`, in one pass, so text that
/// was inserted is never scanned for slots. Unknown names are left as they are.
fn fill(template: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        match after.find("}}").and_then(|close| value(&after[..close]).map(|v| (close, v))) {
            Some((close, v)) => {
                out.push_str(&v);
                rest = &after[close + 2..];
            }
            None => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The whole `knot viz` page: one offline file with the scripts, fonts, images and
/// data inlined.
pub fn render_page(data: &VizData) -> String {
    // Symbol text comes from the indexed repo; escaping <, > and & keeps a signature
    // containing `</script>` from ending the inline script. JSON reads the escapes back.
    let json = serde_json::to_string(data)
        .expect("VizData holds only strings, numbers and lists, which always serialize")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026");
    let slot = |name: &str| -> Option<String> {
        Some(match name {
            "FONTS_CSS" => font_face_css(),
            "GRAPHOLOGY" => GRAPHOLOGY_JS.to_string(),
            "SIGMA" => SIGMA_JS.to_string(),
            "FA2" => FA2_JS.to_string(),
            "DATA" => json.clone(),
            "LOGO_LIGHT" => data_uri_png(LOGO_LIGHT),
            "LOGO_DARK" => data_uri_png(LOGO_DARK),
            "MARK_LIGHT" => data_uri_png(MARK_LIGHT_64),
            "MARK_DARK" => data_uri_png(MARK_DARK_64),
            _ => return None,
        })
    };
    fill(TEMPLATE, slot)
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

pub fn render_and_launch_viz(repo_root: &Path) -> Result<PathBuf> {
    let graph = crate::ask::load_graph(repo_root)?;
    let learnings = crate::engine::learnings::active_learnings(repo_root).unwrap_or_else(|e| {
        tracing::warn!("knot viz: could not read learnings, showing none: {e}");
        Vec::new()
    });
    let out_path = repo_root.join(".knot").join("graph.html");
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&out_path, render_page(&viz_data(&graph, &learnings))).context("Failed to write graph HTML")?;
    let _ = open_browser(&out_path);
    Ok(out_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::resolve::CodeGraph;

    fn empty() -> VizData {
        viz_data(&CodeGraph::default(), &[])
    }

    fn symbol(id: &str, signature: &str) -> crate::ast::types::SymbolNode {
        crate::ast::types::SymbolNode {
            id: id.into(),
            file: id.split("::").next().unwrap_or("").into(),
            name: id.rsplit("::").next().unwrap_or("").into(),
            kind: crate::ast::types::SymbolKind::Function,
            signature: signature.into(),
            span: (1, 1),
            crux: String::new(),
            hash: String::new(),
            outgoing_calls: vec![],
        }
    }

    #[test]
    fn page_is_offline() {
        let html = render_page(&empty());
        assert!(!html.contains("src=\"http"));
        assert!(!html.contains("href=\"http"));
        assert!(!html.contains("@import url(http"));
    }

    #[test]
    fn page_has_logo_and_both_favicons() {
        let html = render_page(&empty());
        assert!(html.contains("rel=\"icon\""));
        assert!(html.contains("prefers-color-scheme: dark"));
        assert!(html.contains("prefers-color-scheme: light"));
        let logo = data_uri_png(LOGO_LIGHT);
        assert!(html.contains(&logo[..80]));
    }

    #[test]
    fn page_uses_knot_fonts_only() {
        let html = render_page(&empty());
        assert!(html.contains("Knot Sans"));
        assert!(!html.contains("Plex"));
    }

    #[test]
    fn layout_runs_off_the_main_frame() {
        let html = render_page(&empty());
        assert!(html.contains("new Worker(") || html.contains("requestAnimationFrame"));
        assert!(html.contains("graphologyLibrary.layoutForceAtlas2"));
        assert!(html.contains("self.DOMParser"), "the library needs DOMParser to load in a worker");
        assert!(html.contains("job.step(FRAME_STEP)"), "the fallback steps a few iterations at a time");
        assert!(html.contains("localStorage.getItem(key)"), "a finished layout is reused");
    }

    #[test]
    fn empty_page_renders() {
        let html = render_page(&empty());
        assert!(html.contains("No symbols indexed yet"));
        assert!(!html.contains("{{"), "every slot is filled");
    }

    #[test]
    fn names_are_escaped_in_the_inspector() {
        let t = TEMPLATE;
        assert!(t.contains("const esc = (s) =>"));
        assert!(t.contains("<span class=\"n\">${esc(m.l)}"), "names in caller and callee lists");
        assert!(t.contains("esc(L.title)"), "learning titles");
        for field in ["iName", "iPath", "iSig"] {
            assert!(t.contains(&format!("$(\"{field}\").textContent")), "{field} is set as text");
        }
    }

    #[test]
    fn inserted_text_is_not_rescanned_for_slots() {
        let mut graph = CodeGraph::default();
        graph.symbols.insert("a.rs::x".into(), symbol("a.rs::x", "fn x() { \"{{SIGMA}}\" }"));
        let html = render_page(&viz_data(&graph, &[]));
        assert!(html.contains("{{SIGMA}}"), "a slot name inside data stays as text");
    }

    #[test]
    fn test_symbol_text_cannot_close_the_script_tag() {
        let mut graph = CodeGraph::default();
        graph.symbols.insert(
            "evil.ts::x".into(),
            symbol("evil.ts::x", "function x() { return '</script><script>alert(1)</script>' }"),
        );
        let html = render_page(&viz_data(&graph, &[]));
        assert_eq!(
            html.matches("</script>").count(),
            TEMPLATE.matches("</script>").count(),
            "only the page's own script tags may close"
        );
        assert!(!html.contains("<script>alert"));
    }
}
