//! Put the card's PNG on the system clipboard.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

/// Copy the PNG at `path` to the clipboard with the OS's own tool. The path is handed
/// over as an argument or an environment variable, never spliced into a shell string.
pub fn copy_png(path: &Path) -> Result<()> {
    if cfg!(target_os = "macos") {
        let script = format!("set the clipboard to (read (POSIX file \"{}\") as «class PNGf»)", applescript_escape(&path.to_string_lossy()));
        run(Command::new("osascript").arg("-e").arg(script))
    } else if cfg!(windows) {
        let script = "Add-Type -AssemblyName System.Windows.Forms; \
            [Windows.Forms.Clipboard]::SetImage([Drawing.Image]::FromFile($env:KNOT_CARD_PATH))";
        run(Command::new("powershell").args(["-NoProfile", "-Command", script]).env("KNOT_CARD_PATH", path))
    } else if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let file = std::fs::File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
        run(Command::new("wl-copy").args(["--type", "image/png"]).stdin(file))
    } else {
        run(Command::new("xclip").args(["-selection", "clipboard", "-t", "image/png", "-i"]).arg(path))
    }
}

/// Escape for an AppleScript string literal, where only `\` and `"` are special.
fn applescript_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

fn run(command: &mut Command) -> Result<()> {
    let output = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("{} is not available", command.get_program().to_string_lossy()))?;
    if output.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&output.stderr);
    let why = why.lines().next().unwrap_or("").trim();
    bail!("{} failed{}{}", command.get_program().to_string_lossy(), if why.is_empty() { "" } else { ": " }, why)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applescript_literal_escapes_backslash_and_quote() {
        assert_eq!(applescript_escape(r#"/a "b"/c\d"#), r#"/a \"b\"/c\\d"#);
    }

    #[test]
    fn a_missing_tool_is_an_error_not_a_panic() {
        let err = run(&mut Command::new("knot-no-such-tool-xyz")).unwrap_err();
        assert!(err.to_string().contains("not available"));
    }
}
