use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

use crate::ui::viz_assets::{data_uri_png, font_face_css, FA2_JS, GRAPHOLOGY_JS, LOGO_DARK, LOGO_LIGHT, MARK_DARK_64, MARK_LIGHT_64, SIGMA_JS};
use crate::ui::viz_data::{viz_data, VizData};

const TEMPLATE: &str = include_str!("viz.html");
const PAGE_FILE: &str = "graph.html";

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

/// Runs `opener path` and waits: `open` and `xdg-open` hand off to the browser and
/// return quickly, and their exit code is the only sign the page was not opened.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_opener(opener: &str, path: &Path) -> Result<()> {
    let status = std::process::Command::new(opener).arg(path).status().with_context(|| format!("could not run {opener}"))?;
    if !status.success() {
        anyhow::bail!("{opener} exited with {status}");
    }
    Ok(())
}

fn open_browser(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    run_opener("open", path)?;
    #[cfg(target_os = "linux")]
    run_opener("xdg-open", path)?;
    #[cfg(target_os = "windows")]
    std::process::Command::new("cmd").args(["/C", "start", &path.to_string_lossy()]).spawn().context("could not run start")?;
    Ok(())
}

/// Renders the page into `.knot/cache/graph.html`. The write goes through `state_file`,
/// which refuses a symlinked `.knot`, cache folder or file.
fn write_page(repo_root: &Path) -> Result<PathBuf> {
    let graph = crate::ask::load_graph(repo_root)?;
    let learnings = crate::engine::learnings::active_learnings(repo_root).unwrap_or_else(|e| {
        tracing::warn!("knot viz: could not read learnings, showing none: {e}");
        Vec::new()
    });
    let out_path = crate::ask::state_file(&crate::ask::state_dir(repo_root), PAGE_FILE)?;
    fs::write(&out_path, render_page(&viz_data(&graph, &learnings))).context("Failed to write graph HTML")?;
    Ok(out_path)
}

fn launch_viz(repo_root: &Path, opener: fn(&Path) -> Result<()>) -> Result<(PathBuf, bool)> {
    let out_path = write_page(repo_root)?;
    let opened = match opener(&out_path) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("knot viz: could not open a browser: {e}");
            false
        }
    };
    Ok((out_path, opened))
}

/// Writes the page and tries to open it. The bool says whether a browser opened; the
/// page exists either way.
pub fn render_and_launch_viz(repo_root: &Path) -> Result<(PathBuf, bool)> {
    launch_viz(repo_root, open_browser)
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

    fn no_browser(_: &Path) -> Result<()> {
        anyhow::bail!("mock opener: no browser")
    }

    fn mock_browser(_: &Path) -> Result<()> {
        Ok(())
    }

    #[test]
    fn page_is_written_into_the_cache() {
        let repo = tempfile::tempdir().unwrap();
        let path = write_page(repo.path()).unwrap();
        assert!(path.ends_with(".knot/cache/graph.html"), "{}", path.display());
        assert!(fs::read_to_string(&path).unwrap().contains("Knot Sans"));
        assert!(!repo.path().join(".knot").join("graph.html").exists());
    }

    #[test]
    #[cfg(unix)]
    fn symlinked_cache_is_refused() {
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(repo.path().join(".knot")).unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join(".knot").join("cache")).unwrap();

        assert!(write_page(repo.path()).is_err());
        assert!(launch_viz(repo.path(), mock_browser).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0, "nothing written through the link");
    }

    #[test]
    fn open_failure_is_reported_honestly() {
        let repo = tempfile::tempdir().unwrap();
        let (path, opened) = launch_viz(repo.path(), no_browser).unwrap();
        assert!(!opened);
        assert!(path.exists(), "the page is still written");

        let (_, opened) = launch_viz(repo.path(), mock_browser).unwrap();
        assert!(opened);
    }

    #[test]
    fn missing_opener_is_an_error() {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let err = run_opener("knot-no-such-opener", Path::new("graph.html")).unwrap_err();
            assert!(err.to_string().contains("knot-no-such-opener"), "{err}");
            assert!(run_opener("false", Path::new("graph.html")).is_err(), "non-zero exit is a failure");
            assert!(run_opener("true", Path::new("graph.html")).is_ok());
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
