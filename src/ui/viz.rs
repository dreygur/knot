use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::{Duration, Instant};

use crate::ui::viz_assets::{data_uri_png, font_face_css, FA2_JS, GRAPHOLOGY_JS, LICENSES, LOGO_DARK, LOGO_LIGHT, MARK_DARK_64, MARK_LIGHT_64, SIGMA_JS};
use crate::ui::viz_data::{viz_data, VizData};

const TEMPLATE: &str = include_str!("viz.html");
const PAGE_FILE: &str = "graph.html";
#[cfg(any(target_os = "macos", target_os = "linux"))]
const OPEN_GRACE: Duration = Duration::from_secs(2);
#[cfg(any(target_os = "macos", target_os = "linux"))]
const OPEN_POLL: Duration = Duration::from_millis(50);

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
            "LICENSES" => LICENSES.to_string(),
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

/// Runs `opener path` and gives it `OPEN_GRACE` to fail. `open` and most `xdg-open`
/// setups hand off to the browser and exit at once, so a non-zero exit is the only
/// sign the page was not opened. On bare X, i3 or sway `xdg-open` runs the browser
/// itself and only returns when it closes; still running counts as opened.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run_opener(opener: &str, path: &Path) -> Result<()> {
    let mut child = std::process::Command::new(opener).arg(path).spawn().with_context(|| format!("could not run {opener}"))?;
    let deadline = Instant::now() + OPEN_GRACE;
    while Instant::now() < deadline {
        if let Some(status) = child.try_wait().with_context(|| format!("could not wait for {opener}"))? {
            if !status.success() {
                anyhow::bail!("{opener} exited with {status}");
            }
            return Ok(());
        }
        std::thread::sleep(OPEN_POLL);
    }
    Ok(())
}

/// The Windows opener. Never `cmd /C start`: cmd.exe reads `&`, `|`, `^` and `%` in a
/// repo folder name as its own syntax and would run what follows. explorer.exe takes
/// the path as one argument and opens it in the default browser.
#[cfg(any(target_os = "windows", test))]
fn windows_opener(path: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("explorer.exe");
    cmd.arg(path);
    cmd
}

fn open_browser(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    run_opener("open", path)?;
    #[cfg(target_os = "linux")]
    run_opener("xdg-open", path)?;
    // explorer.exe exits with 1 even when it opened the file, so only a failed spawn counts.
    #[cfg(target_os = "windows")]
    windows_opener(path).spawn().context("could not run explorer.exe")?;
    Ok(())
}

/// Renders the page into `.knot/cache/graph.html`. The write goes through `state_file`,
/// which refuses a symlinked `.knot`, cache folder or file.
pub(crate) fn write_page(repo_root: &Path) -> Result<PathBuf> {
    let graph = crate::ask::load_graph(repo_root)?;
    let learnings = crate::engine::learnings::active_learnings(repo_root).unwrap_or_else(|e| {
        tracing::warn!("knot viz: could not read learnings, showing none: {e}");
        Vec::new()
    });
    let out_path = crate::ask::state_file(&crate::ask::state_dir(repo_root), PAGE_FILE)?;
    fs::write(&out_path, render_page(&viz_data(&graph, &learnings))).context("Failed to write graph HTML")?;
    Ok(out_path)
}

fn open_with(path: &Path, opener: fn(&Path) -> Result<()>) -> bool {
    match opener(path) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("knot viz: could not open a browser: {e}");
            false
        }
    }
}

/// Tries to open the written page in a browser and says whether it did. Returns
/// within `OPEN_GRACE` even when the browser runs in the opener's foreground.
pub(crate) fn open_page(path: &Path) -> bool {
    open_with(path, open_browser)
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
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0, "nothing written through the link");
    }

    #[test]
    fn open_failure_is_reported_honestly() {
        let repo = tempfile::tempdir().unwrap();
        let path = write_page(repo.path()).unwrap();
        assert!(!open_with(&path, no_browser));
        assert!(path.exists(), "the page is still written");
        assert!(open_with(&path, mock_browser));
    }

    #[test]
    fn windows_opener_never_goes_through_cmd() {
        let path = Path::new(r"C:\repos\a&calc|x^%PATH%\.knot\cache\graph.html");
        let cmd = windows_opener(path);
        assert_eq!(cmd.get_program(), "explorer.exe");
        let args: Vec<_> = cmd.get_args().collect();
        assert_eq!(args, [path.as_os_str()], "the path is one argument and nothing else");
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
    fn opener_in_the_foreground_does_not_block() {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            // Bare X, i3 and sway: xdg-open runs the browser itself and returns when it closes.
            let started = std::time::Instant::now();
            assert!(run_opener("sleep", Path::new("5")).is_ok(), "still running means opened");
            assert!(started.elapsed() < std::time::Duration::from_secs(4), "{:?}", started.elapsed());
        }
    }

    #[test]
    fn page_carries_every_licence_notice() {
        let html = render_page(&empty());
        for copyright in [
            "Copyright (c) 2016-2021 Guillaume Plique (Yomguithereal)",
            "Copyright (C) 2013-2025, Alexis Jacomy, Guillaume Plique, Benoît Simard",
            "Copyright (c) 2017-2021 Guillaume Plique (Yomguithereal)",
            "Copyright Joyent, Inc. and other Node contributors.",
            "Copyright (c) 2016 Guillaume Plique (Yomguithereal)",
            "Copyright (c) 2017-2020 Guillaume Plique (Yomguithereal)",
            "Copyright (c) 2018 Guillaume Plique (Yomguithereal)",
            "Copyright OpenJS Foundation and other contributors",
            "Copyright (c) 2017 Evgeny Poberezkin",
            "Copyright 2011 Nicolas Thouvenin",
        ] {
            assert!(html.contains(copyright), "{copyright}");
        }
        assert!(html.starts_with("<!DOCTYPE html>\n<!--"), "the notice opens the page");
        assert!(!LICENSES.contains("--"), "the notice cannot close its own comment");
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
    fn note_hint_is_a_command_that_runs() {
        let t = TEMPLATE;
        assert!(t.contains(r#"knot note --title "&lt;the fact&gt;" --touches "${esc(m.f)}""#), "title and a quoted path");
        assert!(t.contains("No learnings touch this file yet"), "learnings attach by file");
        assert!(!t.contains("#${esc(m.l"), "no symbol fragment");
    }

    #[test]
    fn page_text_stays_inside_the_font_subset() {
        for curly in ['\u{201c}', '\u{201d}', '\u{2018}', '\u{2019}'] {
            assert!(!TEMPLATE.contains(curly), "{curly}");
        }
    }

    #[test]
    fn page_survives_a_bad_hash_and_no_webgl() {
        let t = TEMPLATE;
        assert!(t.contains("This browser has WebGL turned off, so the map can't draw."));
        assert!(t.contains("try {\n    want = location.hash.startsWith(\"#select=\") ? decodeURIComponent("), "the decode is guarded");
    }

    #[test]
    fn picking_a_symbol_shows_its_folder() {
        assert!(TEMPLATE.contains("hidden.delete(m.fo)"));
    }

    #[test]
    fn phone_header_keeps_the_theme_toggle() {
        let phone = &TEMPLATE[TEMPLATE.find("@media (max-width: 760px)").unwrap()..];
        assert!(phone.contains(".search kbd { display: none; }"));
        assert!(phone.contains(".search { min-width: 0; }"));
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
