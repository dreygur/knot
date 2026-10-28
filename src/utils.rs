use std::path::Path;

/// `path` as the person reads it: under `~/` when it sits in `home`.
pub fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Whether `path` itself is a symlink; the link is never followed.
pub fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// A cloned repo can commit a file or folder Knot writes as a symlink to anywhere,
/// say `~/.ssh`; writing through it would let the repo overwrite files outside itself.
pub fn refuse_symlink(path: &Path) -> std::io::Result<()> {
    if is_symlink(path) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is a symlink; Knot does not write through it", path.display()),
        ));
    }
    Ok(())
}

/// Replace each `{{NAME}}` in `template` with `value(NAME)`, in one pass, so text that
/// was inserted is never scanned for slots. Unknown names are left as they are.
pub fn fill_slots(template: &str, value: impl Fn(&str) -> Option<String>) -> String {
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

/// Write to a sibling temp file and rename it into place, so a crash leaves the old file.
/// The temp file is created fresh, never opened through a planted link, and takes the
/// old file's mode, so a private config holding a token stays private.
pub fn write_atomic(path: &Path, content: &str) -> anyhow::Result<()> {
    use std::io::Write;
    refuse_symlink(path)?;
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".knot-tmp");
    let tmp = path.with_file_name(name);
    // A crash can leave one behind; removing a link removes the link, not its target
    match std::fs::remove_file(&tmp) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => anyhow::bail!("cannot replace {}: {e}", tmp.display()),
        _ => {}
    }

    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&tmp)?;
    if let Ok(old) = std::fs::metadata(path) {
        file.set_permissions(old.permissions())?;
    }
    file.write_all(content.as_bytes())?;
    drop(file);
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// `content` with `block` between the `start` and `end` markers in place of what an
/// earlier run put there, or appended when there is none yet.
pub fn upsert_block(content: &str, block: &str, start: &str, end: &str) -> String {
    match (content.find(start), content.find(end)) {
        (Some(s), Some(e)) if s < e => format!("{}{}{}", &content[..s], block.trim(), &content[e + end.len()..]),
        _ => format!("{content}\n{block}\n"),
    }
}

/// `1 file`, `3 files`.
pub fn count(n: usize, noun: &str) -> String {
    format!("{n} {noun}{}", if n == 1 { "" } else { "s" })
}

/// 999, 1.2k, 40k, 4.1M, 4M, 1.2B.
pub fn short(n: u64) -> String {
    const UNITS: [(f64, &str); 3] = [(1e3, "k"), (1e6, "M"), (1e9, "B")];
    if n < 1_000 {
        return n.to_string();
    }
    let mut unit = UNITS
        .iter()
        .rposition(|(size, _)| n as f64 >= *size)
        .unwrap_or(0);
    // 999,960 rounds to "1000.0k"; step up a unit instead
    if unit + 1 < UNITS.len() && (n as f64 / UNITS[unit].0 * 10.0).round() >= 10_000.0 {
        unit += 1;
    }
    let (size, suffix) = UNITS[unit];
    let number = format!("{:.1}", n as f64 / size);
    format!("{}{suffix}", number.strip_suffix(".0").unwrap_or(&number))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_formats() {
        assert_eq!(short(999), "999");
        assert_eq!(short(1_234), "1.2k");
        assert_eq!(short(4_100_000), "4.1M");
        assert_eq!(short(1_200_000_000), "1.2B");
        assert_eq!(short(999_960), "1M");
        assert_eq!(short(40_000), "40k");
        assert_eq!(short(4_000_000), "4M");
        assert_eq!(short(1_000), "1k");
    }

    #[test]
    fn count_says_one_or_many() {
        assert_eq!(count(1, "file"), "1 file");
        assert_eq!(count(0, "file"), "0 files");
        assert_eq!(count(3, "hit"), "3 hits");
    }

    #[test]
    fn block_is_replaced_not_repeated() {
        let once = upsert_block("# mine\n", "<a>\nold\n</a>", "<a>", "</a>");
        let twice = upsert_block(&once, "<a>\nnew\n</a>", "<a>", "</a>");
        assert_eq!(twice, "# mine\n\n<a>\nnew\n</a>\n");
    }

    #[test]
    fn slots_fill_once() {
        let filled = fill_slots("{{a}} and {{b}} {{", |k| (k == "a").then(|| "{{b}}".to_string()));
        assert_eq!(filled, "{{b}} and {{b}} {{");
    }
}
