//! Put the card's PNG on the system clipboard.

use anyhow::{bail, Context, Result};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

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
    } else {
        copy_with(&linux_tools(std::env::var_os("WAYLAND_DISPLAY").is_some()), path, &run)
    }
}

/// A Linux clipboard tool, fed the PNG on stdin or as its last argument.
struct Tool {
    program: &'static str,
    args: &'static [&'static str],
    stdin: bool,
}

/// `wl-copy`, then `xclip` (through XWayland) on Wayland; `xclip` alone on X11.
fn linux_tools(wayland: bool) -> Vec<Tool> {
    let wl_copy = Tool { program: "wl-copy", args: &["--type", "image/png"], stdin: true };
    let xclip = Tool { program: "xclip", args: &["-selection", "clipboard", "-t", "image/png", "-i"], stdin: false };
    if wayland {
        vec![wl_copy, xclip]
    } else {
        vec![xclip]
    }
}

/// Try each tool in turn until one copies; the error names every failure.
fn copy_with(tools: &[Tool], path: &Path, run: &dyn Fn(&mut Command) -> Result<()>) -> Result<()> {
    let mut failures = Vec::new();
    for tool in tools {
        let mut command = Command::new(tool.program);
        command.args(tool.args);
        if tool.stdin {
            command.stdin(File::open(path).with_context(|| format!("cannot open {}", path.display()))?);
        } else {
            command.arg(path);
        }
        match run(&mut command) {
            Ok(()) => return Ok(()),
            Err(e) => failures.push(e.to_string()),
        }
    }
    bail!("{}", failures.join("; "))
}

/// Escape for an AppleScript string literal, where only `\` and `"` are special.
fn applescript_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Run `command` and wait for it alone. Its stderr goes to a file, not a pipe: xclip and
/// wl-copy fork a child that keeps serving the selection with the inherited fds, so
/// reading a pipe to EOF would wait until another app takes the clipboard.
fn run(command: &mut Command) -> Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let (errors, stderr) = match stderr_file() {
        Some((path, file)) => (Some(path), Stdio::from(file)),
        None => (None, Stdio::null()),
    };
    let status = command.stdout(Stdio::null()).stderr(stderr).status();
    let why = errors
        .map(|path| {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let _ = std::fs::remove_file(&path);
            text
        })
        .unwrap_or_default();
    let status = status.with_context(|| format!("{program} is not available"))?;
    if status.success() {
        return Ok(());
    }
    let why = why.lines().next().unwrap_or("").trim();
    bail!("{program} failed{}{why}", if why.is_empty() { "" } else { ": " })
}

/// A fresh file in the temp folder for one tool's stderr, or `None` when none can be made.
fn stderr_file() -> Option<(PathBuf, File)> {
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let name = format!("knot-clipboard-{}-{}.err", std::process::id(), COUNT.fetch_add(1, Ordering::Relaxed));
    let path = std::env::temp_dir().join(name);
    // create_new never follows a planted symlink or reuses someone's file
    let file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).ok()?;
    Some((path, file))
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

    #[test]
    fn a_tool_that_forks_a_server_does_not_hang_the_card() {
        // xclip and wl-copy leave a child serving the selection with the inherited fds
        let started = std::time::Instant::now();
        run(Command::new("sh").args(["-c", "sleep 5 & exit 0"])).unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "waited on the forked child");
    }

    #[test]
    fn a_failing_tool_reports_its_first_stderr_line() {
        let err = run(Command::new("sh").args(["-c", "echo 'no display' >&2; echo more >&2; exit 1"])).unwrap_err();
        assert_eq!(err.to_string(), "sh failed: no display");
    }

    #[test]
    fn linux_tries_wl_copy_then_xclip_on_wayland() {
        let programs = |wayland| linux_tools(wayland).iter().map(|t| t.program).collect::<Vec<_>>();
        assert_eq!(programs(true), ["wl-copy", "xclip"]);
        assert_eq!(programs(false), ["xclip"]);
    }

    #[test]
    fn linux_falls_back_when_the_first_tool_fails() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("card.png");
        std::fs::write(&png, b"png").unwrap();
        let tried = std::cell::RefCell::new(Vec::new());
        let result = copy_with(&linux_tools(true), &png, &|cmd: &mut Command| {
            let program = cmd.get_program().to_string_lossy().into_owned();
            tried.borrow_mut().push(program.clone());
            if program == "xclip" { Ok(()) } else { bail!("{program} is not available") }
        });
        assert!(result.is_ok());
        assert_eq!(*tried.borrow(), ["wl-copy", "xclip"]);

        let err = copy_with(&linux_tools(true), &png, &|cmd: &mut Command| bail!("{} failed", cmd.get_program().to_string_lossy())).unwrap_err();
        assert_eq!(err.to_string(), "wl-copy failed; xclip failed");
    }
}
