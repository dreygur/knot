use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use tree_sitter::{Language, Node, Parser};

use super::types::{SkeletonItem, SymbolKind, SymbolNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedLang {
    Rust,
    Python,
    TypeScript,
}

impl SupportedLang {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "rs" => Some(SupportedLang::Rust),
            "py" => Some(SupportedLang::Python),
            "ts" | "tsx" | "js" | "jsx" => Some(SupportedLang::TypeScript),
            _ => None,
        }
    }

    pub fn tree_sitter_language(&self) -> Language {
        match self {
            SupportedLang::Rust => tree_sitter_rust::LANGUAGE.into(),
            SupportedLang::Python => tree_sitter_python::LANGUAGE.into(),
            SupportedLang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        }
    }
}

fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

fn node_text<'a>(source: &'a str, node: Node) -> String {
    source[node.start_byte()..node.end_byte()].trim().to_string()
}

fn first_line_of(lines: &[&str], line_1_indexed: usize) -> String {
    if line_1_indexed > 0 && line_1_indexed <= lines.len() {
        lines[line_1_indexed - 1].trim().to_string()
    } else {
        String::new()
    }
}

fn compute_crux(lines: &[&str], start_line: usize, end_line: usize) -> String {
    let start_idx = start_line.saturating_sub(1);
    let end_idx = end_line.min(lines.len());
    let total_lines = end_idx.saturating_sub(start_idx);

    if total_lines <= 8 {
        lines[start_idx..end_idx].join("\n")
    } else {
        let omitted = total_lines - 7;
        let mut crux_lines: Vec<String> = lines[start_idx..start_idx + 6]
            .iter()
            .map(|l| l.to_string())
            .collect();
        crux_lines.push(format!("    // ... +{omitted} lines ..."));
        if end_idx > 0 && end_idx <= lines.len() {
            crux_lines.push(lines[end_idx - 1].to_string());
        }
        crux_lines.join("\n")
    }
}

fn collect_outgoing_calls(source_code: &str, root: Node) -> Vec<String> {
    let mut calls = HashSet::new();
    let mut stack = vec![root];

    while let Some(current) = stack.pop() {
        if current.kind() == "call_expression" || current.kind() == "call" {
            if let Some(function_node) = current.child_by_field_name("function") {
                let call_text = node_text(source_code, function_node);
                let bare_call = call_text
                    .trim_start_matches("self.")
                    .trim_start_matches("this.")
                    .split('.')
                    .last()
                    .unwrap_or(&call_text)
                    .split("::")
                    .last()
                    .unwrap_or(&call_text);

                if !bare_call.is_empty() && bare_call.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    calls.insert(bare_call.to_string());
                }
            }
        }

        let mut cursor = current.walk();
        for child in current.children(&mut cursor) {
            stack.push(child);
        }
    }

    let mut list: Vec<String> = calls.into_iter().collect();
    list.sort();
    list
}

fn extract_rust_fn(
    file_rel_path: &str,
    source_code: &str,
    lines: &[&str],
    node: Node,
    parent_scope: Option<&str>,
) -> Option<SymbolNode> {
    let name_node = node.child_by_field_name("name")?;
    let raw_name = node_text(source_code, name_node);
    let name = match parent_scope {
        Some(scope) => format!("{scope}::{raw_name}"),
        None => raw_name,
    };

    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;

    let signature = if let Some(body) = node.child_by_field_name("body") {
        let sig_end = body.start_byte();
        source_code[node.start_byte()..sig_end].trim().to_string()
    } else {
        first_line_of(lines, start_line)
    };

    let full_text = &source_code[node.start_byte()..node.end_byte()];
    let crux = compute_crux(lines, start_line, end_line);
    let hash = blake3::hash(full_text.as_bytes()).to_hex().to_string();
    let outgoing_calls = collect_outgoing_calls(source_code, node);

    let kind = if parent_scope.is_some() {
        SymbolKind::Method
    } else {
        SymbolKind::Function
    };

    Some(SymbolNode {
        id: format!("{file_rel_path}::{name}"),
        file: file_rel_path.to_string(),
        name,
        kind,
        signature,
        span: (start_line, end_line),
        crux,
        hash,
        outgoing_calls,
    })
}

fn extract_rust_type(
    file_rel_path: &str,
    source_code: &str,
    lines: &[&str],
    node: Node,
    kind: SymbolKind,
) -> Option<SymbolNode> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(source_code, name_node);
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;

    let signature = first_line_of(lines, start_line);
    let full_text = &source_code[node.start_byte()..node.end_byte()];
    let crux = compute_crux(lines, start_line, end_line);
    let hash = blake3::hash(full_text.as_bytes()).to_hex().to_string();

    Some(SymbolNode {
        id: format!("{file_rel_path}::{name}"),
        file: file_rel_path.to_string(),
        name,
        kind,
        signature,
        span: (start_line, end_line),
        crux,
        hash,
        outgoing_calls: Vec::new(),
    })
}

fn extract_py_fn(
    file_rel_path: &str,
    source_code: &str,
    lines: &[&str],
    node: Node,
    parent_scope: Option<&str>,
) -> Option<SymbolNode> {
    let name_node = node.child_by_field_name("name")?;
    let raw_name = node_text(source_code, name_node);
    let name = match parent_scope {
        Some(scope) => format!("{scope}.{raw_name}"),
        None => raw_name,
    };

    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;

    let signature = first_line_of(lines, start_line);
    let full_text = &source_code[node.start_byte()..node.end_byte()];
    let crux = compute_crux(lines, start_line, end_line);
    let hash = blake3::hash(full_text.as_bytes()).to_hex().to_string();
    let outgoing_calls = collect_outgoing_calls(source_code, node);

    let kind = if parent_scope.is_some() {
        SymbolKind::Method
    } else {
        SymbolKind::Function
    };

    Some(SymbolNode {
        id: format!("{file_rel_path}::{name}"),
        file: file_rel_path.to_string(),
        name,
        kind,
        signature,
        span: (start_line, end_line),
        crux,
        hash,
        outgoing_calls,
    })
}

fn extract_py_class(
    file_rel_path: &str,
    source_code: &str,
    lines: &[&str],
    node: Node,
) -> Option<SymbolNode> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(source_code, name_node);
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;

    let signature = first_line_of(lines, start_line);
    let full_text = &source_code[node.start_byte()..node.end_byte()];
    let crux = compute_crux(lines, start_line, end_line);
    let hash = blake3::hash(full_text.as_bytes()).to_hex().to_string();

    Some(SymbolNode {
        id: format!("{file_rel_path}::{name}"),
        file: file_rel_path.to_string(),
        name,
        kind: SymbolKind::Class,
        signature,
        span: (start_line, end_line),
        crux,
        hash,
        outgoing_calls: Vec::new(),
    })
}

fn extract_ts_fn(
    file_rel_path: &str,
    source_code: &str,
    lines: &[&str],
    node: Node,
    parent_scope: Option<&str>,
) -> Option<SymbolNode> {
    let name_node = node.child_by_field_name("name")?;
    let raw_name = node_text(source_code, name_node);
    let name = match parent_scope {
        Some(scope) => format!("{scope}.{raw_name}"),
        None => raw_name,
    };

    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;

    let signature = if let Some(body) = node.child_by_field_name("body") {
        source_code[node.start_byte()..body.start_byte()].trim().to_string()
    } else {
        first_line_of(lines, start_line)
    };

    let full_text = &source_code[node.start_byte()..node.end_byte()];
    let crux = compute_crux(lines, start_line, end_line);
    let hash = blake3::hash(full_text.as_bytes()).to_hex().to_string();
    let outgoing_calls = collect_outgoing_calls(source_code, node);

    let kind = if parent_scope.is_some() || node.kind() == "method_definition" {
        SymbolKind::Method
    } else {
        SymbolKind::Function
    };

    Some(SymbolNode {
        id: format!("{file_rel_path}::{name}"),
        file: file_rel_path.to_string(),
        name,
        kind,
        signature,
        span: (start_line, end_line),
        crux,
        hash,
        outgoing_calls,
    })
}

fn extract_ts_type(
    file_rel_path: &str,
    source_code: &str,
    lines: &[&str],
    node: Node,
    kind: SymbolKind,
) -> Option<SymbolNode> {
    let name_node = node.child_by_field_name("name")?;
    let name = node_text(source_code, name_node);
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;

    let signature = first_line_of(lines, start_line);
    let full_text = &source_code[node.start_byte()..node.end_byte()];
    let crux = compute_crux(lines, start_line, end_line);
    let hash = blake3::hash(full_text.as_bytes()).to_hex().to_string();

    Some(SymbolNode {
        id: format!("{file_rel_path}::{name}"),
        file: file_rel_path.to_string(),
        name,
        kind,
        signature,
        span: (start_line, end_line),
        crux,
        hash,
        outgoing_calls: Vec::new(),
    })
}

fn collect_symbols_recursive(
    file_rel_path: &str,
    source_code: &str,
    lines: &[&str],
    node: Node,
    lang: SupportedLang,
    parent_scope: Option<&str>,
    symbols: &mut Vec<SymbolNode>,
) {
    let kind = node.kind();
    let mut current_scope = parent_scope;

    match lang {
        SupportedLang::Rust => match kind {
            "function_item" => {
                if let Some(sym) = extract_rust_fn(file_rel_path, source_code, lines, node, parent_scope) {
                    symbols.push(sym);
                }
            }
            "struct_item" => {
                if let Some(sym) = extract_rust_type(file_rel_path, source_code, lines, node, SymbolKind::Struct) {
                    current_scope = Some(leak_str(&sym.name));
                    symbols.push(sym);
                }
            }
            "enum_item" => {
                if let Some(sym) = extract_rust_type(file_rel_path, source_code, lines, node, SymbolKind::Enum) {
                    symbols.push(sym);
                }
            }
            "trait_item" => {
                if let Some(sym) = extract_rust_type(file_rel_path, source_code, lines, node, SymbolKind::Trait) {
                    current_scope = Some(leak_str(&sym.name));
                    symbols.push(sym);
                }
            }
            "impl_item" => {
                if let Some(type_node) = node.child_by_field_name("type") {
                    let type_name = node_text(source_code, type_node);
                    current_scope = Some(leak_str(&type_name));
                }
            }
            _ => {}
        },
        SupportedLang::Python => match kind {
            "function_definition" => {
                if let Some(sym) = extract_py_fn(file_rel_path, source_code, lines, node, parent_scope) {
                    symbols.push(sym);
                }
            }
            "class_definition" => {
                if let Some(sym) = extract_py_class(file_rel_path, source_code, lines, node) {
                    current_scope = Some(leak_str(&sym.name));
                    symbols.push(sym);
                }
            }
            _ => {}
        },
        SupportedLang::TypeScript => match kind {
            "function_declaration" | "method_definition" => {
                if let Some(sym) = extract_ts_fn(file_rel_path, source_code, lines, node, parent_scope) {
                    symbols.push(sym);
                }
            }
            "class_declaration" => {
                if let Some(sym) = extract_ts_type(file_rel_path, source_code, lines, node, SymbolKind::Class) {
                    current_scope = Some(leak_str(&sym.name));
                    symbols.push(sym);
                }
            }
            "interface_declaration" => {
                if let Some(sym) = extract_ts_type(file_rel_path, source_code, lines, node, SymbolKind::Interface) {
                    symbols.push(sym);
                }
            }
            "type_alias_declaration" => {
                if let Some(sym) = extract_ts_type(file_rel_path, source_code, lines, node, SymbolKind::TypeAlias) {
                    symbols.push(sym);
                }
            }
            _ => {}
        },
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_symbols_recursive(file_rel_path, source_code, lines, child, lang, current_scope, symbols);
    }
}

pub fn extract_file_skeleton(symbols: &[SymbolNode]) -> Vec<SkeletonItem> {
    symbols
        .iter()
        .map(|s| SkeletonItem {
            name: s.name.clone(),
            kind: s.kind,
            signature: s.signature.clone(),
            span: s.span,
        })
        .collect()
}

pub fn extract_file_symbols(
    file_rel_path: &str,
    source_code: &str,
    lang: SupportedLang,
) -> Result<Vec<SymbolNode>> {
    let mut parser = Parser::new();
    parser
        .set_language(&lang.tree_sitter_language())
        .context("Failed to set Tree-sitter language")?;

    let tree = parser
        .parse(source_code, None)
        .context("Failed to parse source code")?;

    let root_node = tree.root_node();
    let lines: Vec<&str> = source_code.lines().collect();

    let mut symbols = Vec::new();
    collect_symbols_recursive(
        file_rel_path,
        source_code,
        &lines,
        root_node,
        lang,
        None,
        &mut symbols,
    );

    Ok(symbols)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_symbol_extraction() {
        let code = r#"
pub struct AuthManager {
    secret: String,
}

impl AuthManager {
    pub fn new(secret: String) -> Self {
        Self { secret }
    }

    pub async fn validate_token(&self, token: &str) -> bool {
        self.check_signature(token)
    }

    fn check_signature(&self, token: &str) -> bool {
        !token.is_empty()
    }
}
"#;

        let symbols = extract_file_symbols("src/auth.rs", code, SupportedLang::Rust).unwrap();
        assert!(symbols.iter().any(|s| s.name == "AuthManager" && s.kind == SymbolKind::Struct));
        assert!(symbols.iter().any(|s| s.name == "AuthManager::new"));
        assert!(symbols.iter().any(|s| s.name == "AuthManager::validate_token"));

        let validate_fn = symbols.iter().find(|s| s.name == "AuthManager::validate_token").unwrap();
        assert!(validate_fn.outgoing_calls.contains(&"check_signature".to_string()));
    }

    #[test]
    fn test_python_symbol_extraction() {
        let code = r#"
class PaymentProcessor:
    def __init__(self, key: str):
        self.key = key

    def process(self, amount: float) -> bool:
        return self.verify_balance(amount)

    def verify_balance(self, amount: float) -> bool:
        return amount > 0
"#;

        let symbols = extract_file_symbols("payment.py", code, SupportedLang::Python).unwrap();
        assert!(symbols.iter().any(|s| s.name == "PaymentProcessor" && s.kind == SymbolKind::Class));
        assert!(symbols.iter().any(|s| s.name == "PaymentProcessor.process"));

        let process_fn = symbols.iter().find(|s| s.name == "PaymentProcessor.process").unwrap();
        assert!(process_fn.outgoing_calls.contains(&"verify_balance".to_string()));
    }
}
