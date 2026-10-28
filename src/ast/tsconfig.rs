//! TypeScript and JavaScript path aliases from `tsconfig.json` / `jsconfig.json`.
//!
//! `import { db } from "@/lib/db"` names no file by itself; the nearest config above
//! the importing file maps it through `compilerOptions.paths` (and `baseUrl`) to
//! `src/lib/db`. Without this, aliased imports resolve to nothing and calls through
//! them fall back to guessing by name.

use serde_json::Value;
use std::path::Path;

use super::resolve::ALWAYS_SKIPPED_DIRS;

const CONFIG_NAMES: &[&str] = &["tsconfig.json", "jsconfig.json"];
/// `extends` chains longer than this are cut, which also stops cycles.
const MAX_EXTENDS_DEPTH: usize = 5;
const TS_EXTENSIONS: &[&str] = &[".tsx", ".ts", ".jsx", ".js", ".mts", ".cts", ".mjs", ".cjs"];

/// One config's effective alias settings, with every path repo-relative.
#[derive(Debug, Clone, Default)]
struct AliasConfig {
    /// Directory holding the config ("" for the repo root).
    dir: String,
    /// Resolved `baseUrl`, when one is set here or inherited.
    base_url: Option<String>,
    /// `paths` patterns with their targets already joined onto their base directory.
    paths: Vec<(String, Vec<String>)>,
}

#[derive(Debug, Clone, Default)]
pub struct TsAliases {
    /// Deepest directories first, so the nearest config wins.
    configs: Vec<AliasConfig>,
}

/// Remove `//` and `/* */` comments outside strings, and commas before `}` or `]`,
/// which tsconfig allows and JSON does not.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => out.extend(chars.next()),
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match (c, chars.peek()) {
            ('"', _) => {
                in_string = true;
                out.push(c);
            }
            ('/', Some('/')) => {
                for n in chars.by_ref() {
                    if n == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            ('/', Some('*')) => {
                chars.next();
                let mut prev = ' ';
                for n in chars.by_ref() {
                    if prev == '*' && n == '/' {
                        break;
                    }
                    prev = n;
                }
            }
            _ => out.push(c),
        }
    }

    // Trailing commas: drop a comma whose next non-space character closes a container
    let bytes: Vec<char> = out.chars().collect();
    let mut cleaned = String::with_capacity(bytes.len());
    let mut in_string = false;
    let mut escaped = false;
    for (i, &c) in bytes.iter().enumerate() {
        if in_string {
            cleaned.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if c == '"' {
            in_string = true;
        }
        if c == ',' && bytes[i + 1..].iter().find(|n| !n.is_whitespace()).is_some_and(|n| *n == '}' || *n == ']') {
            continue;
        }
        cleaned.push(c);
    }
    cleaned
}

/// Join a relative path onto a repo-relative directory, resolving `.` and `..`.
/// `None` when `..` climbs above the repo root.
fn join(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

fn dir_of(rel_file: &str) -> &str {
    rel_file.rsplit_once('/').map_or("", |(d, _)| d)
}

/// What one config file says, with its `extends` chain applied.
struct Resolved {
    base_url: Option<String>,
    paths: Option<Vec<(String, Vec<String>)>>,
}

fn read_config(repo_root: &Path, rel_file: &str, depth: usize) -> Option<Resolved> {
    let text = std::fs::read_to_string(repo_root.join(rel_file)).ok()?;
    let json: Value = serde_json::from_str(&strip_jsonc(&text)).ok()?;
    let dir = dir_of(rel_file);

    // Only relative `extends` can be followed; package presets live in node_modules
    let mut resolved = match json.get("extends").and_then(Value::as_str) {
        Some(parent) if depth < MAX_EXTENDS_DEPTH && parent.starts_with('.') => {
            let parent = if parent.ends_with(".json") { parent.to_string() } else { format!("{parent}.json") };
            join(dir, &parent).and_then(|p| read_config(repo_root, &p, depth + 1))
        }
        _ => None,
    }
    .unwrap_or(Resolved { base_url: None, paths: None });

    let options = json.get("compilerOptions");
    if let Some(base) = options.and_then(|o| o.get("baseUrl")).and_then(Value::as_str) {
        resolved.base_url = join(dir, base);
    }
    if let Some(paths) = options.and_then(|o| o.get("paths")).and_then(Value::as_object) {
        // Targets are relative to baseUrl when there is one, else to this config
        let base = resolved.base_url.clone().unwrap_or_else(|| dir.to_string());
        let mapped = paths
            .iter()
            .map(|(pattern, targets)| {
                let targets = targets
                    .as_array()
                    .map(|list| list.iter().filter_map(Value::as_str).filter_map(|t| join(&base, t)).collect())
                    .unwrap_or_default();
                (pattern.clone(), targets)
            })
            .collect();
        resolved.paths = Some(mapped);
    }
    Some(resolved)
}

fn strip_ts_extension(path: &str) -> &str {
    TS_EXTENSIONS.iter().find_map(|ext| path.strip_suffix(ext)).unwrap_or(path)
}

impl TsAliases {
    /// Read every `tsconfig.json` and `jsconfig.json` under `repo_root`, skipping the
    /// same build and vendor directories the code index skips.
    pub fn load(repo_root: &Path) -> Self {
        let walker = ignore::WalkBuilder::new(repo_root)
            .follow_links(false)
            .parents(false)
            .filter_entry(|e| {
                let is_dir = e.file_type().is_some_and(|t| t.is_dir());
                !(is_dir && e.depth() > 0 && ALWAYS_SKIPPED_DIRS.contains(&e.file_name().to_string_lossy().as_ref()))
            })
            .build();

        let mut configs = Vec::new();
        for entry in walker.flatten() {
            let name = entry.file_name().to_string_lossy();
            if !CONFIG_NAMES.contains(&name.as_ref()) || !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(repo_root) else { continue };
            let rel = rel.to_string_lossy().replace('\\', "/");
            let Some(resolved) = read_config(repo_root, &rel, 0) else { continue };
            if resolved.base_url.is_none() && resolved.paths.is_none() {
                continue;
            }
            configs.push(AliasConfig {
                dir: dir_of(&rel).to_string(),
                base_url: resolved.base_url,
                paths: resolved.paths.unwrap_or_default(),
            });
        }
        configs.sort_by_key(|c| std::cmp::Reverse(c.dir.matches('/').count() + usize::from(!c.dir.is_empty())));
        Self { configs }
    }

    /// Repo-relative paths, without extension, that a non-relative `module` imported
    /// from `importer` may name. Empty when no config maps it.
    pub fn resolve(&self, module: &str, importer: &str) -> Vec<String> {
        let Some(config) = self
            .configs
            .iter()
            .find(|c| c.dir.is_empty() || importer.starts_with(&format!("{}/", c.dir)))
        else {
            return Vec::new();
        };

        // An exact pattern wins; otherwise the wildcard with the longest prefix
        let mut best: Option<(usize, &str, &[String])> = None;
        for (pattern, targets) in &config.paths {
            let capture = match pattern.split_once('*') {
                None if pattern == module => Some((usize::MAX, "")),
                None => None,
                Some((prefix, suffix)) => module
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.strip_suffix(suffix))
                    .map(|captured| (prefix.len(), captured)),
            };
            if let Some((rank, captured)) = capture {
                if best.is_none_or(|(r, _, _)| rank > r) {
                    best = Some((rank, captured, targets));
                }
            }
        }

        let mut out: Vec<String> = match best {
            Some((_, captured, targets)) => targets
                .iter()
                .map(|t| strip_ts_extension(&t.replace('*', captured)).to_string())
                .collect(),
            None => Vec::new(),
        };
        if out.is_empty() {
            if let Some(base) = &config.base_url {
                out.extend(join(base, module).map(|p| strip_ts_extension(&p).to_string()));
            }
        }
        out.retain(|p| !p.is_empty());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn jsonc_comments_and_trailing_commas_are_accepted() {
        let text = "{\n  // line comment\n  \"a\": \"http://x\", /* block */\n  \"b\": [1, 2,],\n}\n";
        let json: Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
        assert_eq!(json["a"], "http://x");
        assert_eq!(json["b"][1], 2);
    }

    #[test]
    fn paths_base_url_extends_and_nearest_config() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            root,
            "tsconfig.base.json",
            r##"{ "compilerOptions": { "baseUrl": ".", "paths": { "@/*": ["src/*"], "#db": ["src/lib/db.ts"] } } }"##,
        );
        write(root, "tsconfig.json", "{ \"extends\": \"./tsconfig.base\", // inherits\n }");
        write(
            root,
            "apps/web/tsconfig.json",
            r#"{ "compilerOptions": { "paths": { "~/*": ["./app/*", "./shared/*"] } } }"#,
        );
        write(root, "node_modules/pkg/tsconfig.json", r#"{ "compilerOptions": { "paths": { "x": ["y"] } } }"#);

        let aliases = TsAliases::load(root);
        assert_eq!(aliases.resolve("@/components/button", "src/app/page.tsx"), vec!["src/components/button"]);
        assert_eq!(aliases.resolve("#db", "src/a.ts"), vec!["src/lib/db"]);
        // baseUrl alone makes bare paths resolve from it
        assert_eq!(aliases.resolve("lib/auth", "src/a.ts"), vec!["lib/auth"]);
        // A nested config governs its own files; its paths are relative to it
        assert_eq!(
            aliases.resolve("~/ui/card", "apps/web/app/page.tsx"),
            vec!["apps/web/app/ui/card", "apps/web/shared/ui/card"]
        );
        assert!(aliases.resolve("@/x", "apps/web/app/page.tsx").is_empty(), "nearest config only");
        assert!(aliases.resolve("react", "apps/web/a.ts").is_empty());
    }
}
