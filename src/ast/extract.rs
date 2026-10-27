use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use tree_sitter::{Language, Node, Parser};

use super::types::{ImportRef, SkeletonItem, SymbolKind, SymbolNode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportedLang {
    Rust,
    Python,
    TypeScript,
    /// TSX grammar: TypeScript plus JSX. Also used for plain JavaScript, which may contain JSX.
    Tsx,
    Go,
    Java,
}

impl SupportedLang {
    pub fn from_path(path: &Path) -> Option<Self> {
        match path.extension()?.to_str()? {
            "rs" => Some(SupportedLang::Rust),
            "py" => Some(SupportedLang::Python),
            "ts" | "mts" | "cts" => Some(SupportedLang::TypeScript),
            "tsx" | "js" | "jsx" | "mjs" | "cjs" => Some(SupportedLang::Tsx),
            "go" => Some(SupportedLang::Go),
            "java" => Some(SupportedLang::Java),
            _ => None,
        }
    }

    pub fn tree_sitter_language(&self) -> Language {
        match self {
            SupportedLang::Rust => tree_sitter_rust::LANGUAGE.into(),
            SupportedLang::Python => tree_sitter_python::LANGUAGE.into(),
            SupportedLang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            SupportedLang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
            SupportedLang::Go => tree_sitter_go::LANGUAGE.into(),
            SupportedLang::Java => tree_sitter_java::LANGUAGE.into(),
        }
    }
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

/// Reduce a call's function expression to a path such as `foo`, `Type::new` or
/// `module.func`, keeping the qualifier so resolution can use it. Receivers like
/// `self.` and `this.` are dropped; turbofish generics are removed. Calls on
/// arbitrary expressions (`get()()`, `(a || b)()`) yield `None`.
fn normalize_call_path(call_text: &str) -> Option<String> {
    let mut path = call_text.trim();
    for receiver in ["self.", "this.", "Self::"] {
        path = path.strip_prefix(receiver).unwrap_or(path);
    }

    let mut cleaned = String::with_capacity(path.len());
    let mut depth = 0usize;
    for c in path.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            _ if depth == 0 => cleaned.push(c),
            _ => {}
        }
    }
    // Dropping `<...>` from a turbofish (`Vec::<u8>::new`) leaves a doubled separator.
    let cleaned = cleaned.replace("::::", "::").trim_end_matches("::").replace('?', "");

    let valid = !cleaned.is_empty()
        && cleaned.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ':' || c == '.')
        && !cleaned.starts_with(['.', ':']);
    valid.then_some(cleaned)
}

fn collect_outgoing_calls(source_code: &str, root: Node) -> Vec<String> {
    let mut calls = HashSet::new();
    let mut stack = vec![root];

    while let Some(current) = stack.pop() {
        match current.kind() {
            "call_expression" | "call" => {
                if let Some(function_node) = current.child_by_field_name("function") {
                    if let Some(call) = normalize_call_path(&node_text(source_code, function_node)) {
                        calls.insert(call);
                    }
                }
            }
            // Java: `obj.name(...)` has separate object and name fields. When the object
            // is not a plain path (`build().run()`), keep the method name alone.
            "method_invocation" => {
                if let Some(name_node) = current.child_by_field_name("name") {
                    let name = node_text(source_code, name_node);
                    let qualified = current
                        .child_by_field_name("object")
                        .and_then(|obj| normalize_call_path(&format!("{}.{name}", node_text(source_code, obj))));
                    if let Some(call) = qualified.or_else(|| normalize_call_path(&name)) {
                        calls.insert(call);
                    }
                }
            }
            // Java: `new Type<>(...)` calls the type's constructor.
            "object_creation_expression" => {
                if let Some(call) = current
                    .child_by_field_name("type")
                    .and_then(|t| normalize_call_path(&node_text(source_code, t)))
                {
                    calls.insert(call);
                }
            }
            _ => {}
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

fn is_ts_function_value(node: Node) -> bool {
    matches!(node.kind(), "arrow_function" | "function_expression" | "function")
}

/// The function a TS/JS value defines: itself, or the first function argument of a
/// wrapper call such as `memo(() => ...)` or `forwardRef(function (props, ref) {...})`.
fn ts_function_of(value: Node) -> Option<Node> {
    if is_ts_function_value(value) {
        return Some(value);
    }
    if value.kind() != "call_expression" {
        return None;
    }
    let args = value.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    let found = args.named_children(&mut cursor).find(|a| is_ts_function_value(*a));
    found
}

/// A module level `const`/`let` declarator: `export const Button = (p) => ...`,
/// `const db = new PrismaClient()`. Locals inside functions are not symbols.
fn extract_ts_variable(file_rel_path: &str, source_code: &str, lines: &[&str], node: Node) -> Option<SymbolNode> {
    let declaration = node.parent()?;
    let statement = match declaration.parent() {
        Some(p) if p.kind() == "export_statement" => p,
        _ => declaration,
    };
    if statement.parent().map(|p| p.kind()) != Some("program") {
        return None;
    }
    let name_node = node.child_by_field_name("name")?;
    if name_node.kind() != "identifier" {
        return None;
    }
    let name = node_text(source_code, name_node);

    let function = node.child_by_field_name("value").and_then(ts_function_of);
    let (kind, signature) = match function.and_then(|f| f.child_by_field_name("body")) {
        Some(body) => (
            SymbolKind::Function,
            source_code[statement.start_byte()..body.start_byte()].trim().to_string(),
        ),
        None if function.is_some() => (SymbolKind::Function, first_line_of(lines, statement.start_position().row + 1)),
        None => (SymbolKind::Constant, first_line_of(lines, statement.start_position().row + 1)),
    };
    Some(build_symbol(file_rel_path, source_code, lines, node, name, kind, signature))
}

/// A class field holding a function, `handleClick = () => {...}`, is a method.
fn extract_ts_field_method(
    file_rel_path: &str,
    source_code: &str,
    lines: &[&str],
    node: Node,
    scope: &str,
) -> Option<SymbolNode> {
    let value = node.child_by_field_name("value").filter(|v| is_ts_function_value(*v))?;
    let name = node_text(source_code, node.child_by_field_name("name")?);
    let signature = match value.child_by_field_name("body") {
        Some(body) => source_code[node.start_byte()..body.start_byte()].trim().to_string(),
        None => first_line_of(lines, node.start_position().row + 1),
    };
    Some(build_symbol(file_rel_path, source_code, lines, node, format!("{scope}.{name}"), SymbolKind::Method, signature))
}

/// A symbol spanning `node`, named `name`. Callables collect their outgoing calls.
fn build_symbol(
    file_rel_path: &str,
    source_code: &str,
    lines: &[&str],
    node: Node,
    name: String,
    kind: SymbolKind,
    signature: String,
) -> SymbolNode {
    let start_line = node.start_position().row + 1;
    let end_line = node.end_position().row + 1;
    let full_text = &source_code[node.start_byte()..node.end_byte()];
    let outgoing_calls = match kind {
        SymbolKind::Function | SymbolKind::Method => collect_outgoing_calls(source_code, node),
        _ => Vec::new(),
    };

    SymbolNode {
        id: format!("{file_rel_path}::{name}"),
        file: file_rel_path.to_string(),
        name,
        kind,
        signature,
        span: (start_line, end_line),
        crux: compute_crux(lines, start_line, end_line),
        hash: blake3::hash(full_text.as_bytes()).to_hex().to_string(),
        outgoing_calls,
    }
}

/// Text from `start` up to the body, or the whole node when it has none.
fn signature_from(source_code: &str, node: Node, start: usize) -> String {
    let end = node.child_by_field_name("body").map_or(node.end_byte(), |b| b.start_byte());
    source_code[start.min(end)..end].trim().trim_end_matches(';').trim_end().to_string()
}

/// The type a Go method is declared on: `func (s *Server[T]) Run()` gives `Server`.
fn go_receiver_type(source_code: &str, node: Node) -> Option<String> {
    let receiver = node.child_by_field_name("receiver")?;
    let mut cursor = receiver.walk();
    let param = receiver.named_children(&mut cursor).find(|c| c.kind() == "parameter_declaration")?;
    let type_text = node_text(source_code, param.child_by_field_name("type")?);
    let bare = type_text.trim_start_matches('*').split('[').next()?.trim();
    (!bare.is_empty()).then(|| bare.to_string())
}

fn extract_go_symbol(file_rel_path: &str, source_code: &str, lines: &[&str], node: Node) -> Option<SymbolNode> {
    let name = node_text(source_code, node.child_by_field_name("name")?);
    let (name, kind) = match node.kind() {
        "function_declaration" => (name, SymbolKind::Function),
        "method_declaration" => match go_receiver_type(source_code, node) {
            Some(owner) => (format!("{owner}.{name}"), SymbolKind::Method),
            None => (name, SymbolKind::Method),
        },
        "type_spec" => {
            let kind = match node.child_by_field_name("type").map(|t| t.kind()) {
                Some("struct_type") => SymbolKind::Struct,
                Some("interface_type") => SymbolKind::Interface,
                _ => SymbolKind::TypeAlias,
            };
            (name, kind)
        }
        "type_alias" => (name, SymbolKind::TypeAlias),
        _ => return None,
    };
    let signature = match kind {
        SymbolKind::Function | SymbolKind::Method => signature_from(source_code, node, node.start_byte()),
        _ => first_line_of(lines, node.start_position().row + 1),
    };
    Some(build_symbol(file_rel_path, source_code, lines, node, name, kind, signature))
}

/// Where a Java declaration's signature starts: after any annotations in its modifiers,
/// so `@Override` lines stay out of it.
fn java_signature_start(node: Node) -> usize {
    let mut cursor = node.walk();
    let Some(modifiers) = node.children(&mut cursor).find(|c| c.kind() == "modifiers") else {
        return node.start_byte();
    };
    let mut mod_cursor = modifiers.walk();
    let first_keyword = modifiers
        .children(&mut mod_cursor)
        .find(|c| !matches!(c.kind(), "annotation" | "marker_annotation" | "line_comment" | "block_comment"));
    match first_keyword {
        Some(k) => k.start_byte(),
        None => modifiers.next_sibling().map_or(node.start_byte(), |n| n.start_byte()),
    }
}

fn extract_java_symbol(
    file_rel_path: &str,
    source_code: &str,
    lines: &[&str],
    node: Node,
    parent_scope: Option<&str>,
) -> Option<SymbolNode> {
    let raw_name = node_text(source_code, node.child_by_field_name("name")?);
    let kind = match node.kind() {
        "method_declaration" | "constructor_declaration" => SymbolKind::Method,
        "class_declaration" | "record_declaration" => SymbolKind::Class,
        "interface_declaration" | "annotation_type_declaration" => SymbolKind::Interface,
        "enum_declaration" => SymbolKind::Enum,
        _ => return None,
    };
    let name = match (kind, parent_scope) {
        (SymbolKind::Method, Some(scope)) => format!("{scope}.{raw_name}"),
        _ => raw_name,
    };
    let signature = signature_from(source_code, node, java_signature_start(node));
    // A type's "signature" is its header; keep it to one line like other languages
    let signature = match kind {
        SymbolKind::Method => signature,
        _ => signature.lines().next().unwrap_or_default().trim().to_string(),
    };
    Some(build_symbol(file_rel_path, source_code, lines, node, name, kind, signature))
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
    let mut current_scope: Option<String> = parent_scope.map(str::to_string);

    match lang {
        SupportedLang::Rust => match kind {
            "function_item" => {
                if let Some(sym) = extract_rust_fn(file_rel_path, source_code, lines, node, parent_scope) {
                    symbols.push(sym);
                }
            }
            "struct_item" => {
                if let Some(sym) = extract_rust_type(file_rel_path, source_code, lines, node, SymbolKind::Struct) {
                    current_scope = Some(sym.name.clone());
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
                    current_scope = Some(sym.name.clone());
                    symbols.push(sym);
                }
            }
            "impl_item" => {
                if let Some(type_node) = node.child_by_field_name("type") {
                    let type_name = node_text(source_code, type_node);
                    current_scope = Some(type_name);
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
                    current_scope = Some(sym.name.clone());
                    symbols.push(sym);
                }
            }
            _ => {}
        },
        SupportedLang::TypeScript | SupportedLang::Tsx => match kind {
            "function_declaration" | "method_definition" => {
                if let Some(sym) = extract_ts_fn(file_rel_path, source_code, lines, node, parent_scope) {
                    symbols.push(sym);
                }
            }
            "class_declaration" => {
                if let Some(sym) = extract_ts_type(file_rel_path, source_code, lines, node, SymbolKind::Class) {
                    current_scope = Some(sym.name.clone());
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
            "variable_declarator" if parent_scope.is_none() => {
                if let Some(sym) = extract_ts_variable(file_rel_path, source_code, lines, node) {
                    symbols.push(sym);
                }
            }
            "public_field_definition" => {
                if let Some(sym) =
                    parent_scope.and_then(|scope| extract_ts_field_method(file_rel_path, source_code, lines, node, scope))
                {
                    symbols.push(sym);
                }
            }
            _ => {}
        },
        // Go methods name their type in the receiver, so no scope is carried down.
        SupportedLang::Go => {
            if let Some(sym) = extract_go_symbol(file_rel_path, source_code, lines, node) {
                symbols.push(sym);
            }
        }
        SupportedLang::Java => {
            if let Some(sym) = extract_java_symbol(file_rel_path, source_code, lines, node, parent_scope) {
                if sym.kind != SymbolKind::Method {
                    current_scope = Some(sym.name.clone());
                }
                symbols.push(sym);
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_symbols_recursive(file_rel_path, source_code, lines, child, lang, current_scope.as_deref(), symbols);
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

/// Flatten a Rust use-tree (`a::b::{c, d as e, f::*}`) into imports.
fn flatten_rust_use(tree: &str, prefix: &str, out: &mut Vec<ImportRef>) {
    let tree = tree.trim();
    if tree.is_empty() {
        return;
    }

    if let Some(open) = tree.find('{') {
        let base = tree[..open].trim().trim_end_matches("::");
        let inner = tree[open + 1..].trim_end().strip_suffix('}').unwrap_or(&tree[open + 1..]);
        let new_prefix = join_rust_path(prefix, base);

        let mut depth = 0usize;
        let mut start = 0;
        for (i, c) in inner.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    flatten_rust_use(&inner[start..i], &new_prefix, out);
                    start = i + 1;
                }
                _ => {}
            }
        }
        flatten_rust_use(&inner[start..], &new_prefix, out);
        return;
    }

    let (path, alias) = match tree.split_once(" as ") {
        Some((p, a)) => (p.trim(), Some(a.trim())),
        None => (tree, None),
    };
    let full = join_rust_path(prefix, path);
    let (module, name) = match full.rsplit_once("::") {
        Some((m, n)) => (m.to_string(), n.to_string()),
        None => (String::new(), full.clone()),
    };
    if name == "self" {
        let module_name = module.rsplit("::").next().unwrap_or(&module).to_string();
        out.push(ImportRef { local: alias.unwrap_or(&module_name).to_string(), name: module_name, module });
    } else {
        out.push(ImportRef { local: alias.unwrap_or(&name).to_string(), name, module });
    }
}

fn join_rust_path(prefix: &str, path: &str) -> String {
    match (prefix.is_empty(), path.is_empty()) {
        (true, _) => path.to_string(),
        (_, true) => prefix.to_string(),
        _ => format!("{prefix}::{path}"),
    }
}

fn collect_imports(source_code: &str, root: Node, lang: SupportedLang) -> Vec<ImportRef> {
    let mut imports = Vec::new();
    let mut cursor = root.walk();

    for node in root.children(&mut cursor) {
        let text = node_text(source_code, node);
        match (lang, node.kind()) {
            (SupportedLang::Rust, "use_declaration") => {
                let body = text.trim_end_matches(';');
                let body = body.split_once("use ").map(|(_, b)| b).unwrap_or(body);
                flatten_rust_use(body, "", &mut imports);
            }
            (SupportedLang::Python, "import_from_statement") => {
                let Some(rest) = text.strip_prefix("from ") else { continue };
                let Some((module, names)) = rest.split_once(" import ") else { continue };
                let names = names.trim().trim_start_matches('(').trim_end_matches(')');
                for part in names.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    let (name, local) = part.split_once(" as ").unwrap_or((part, part));
                    imports.push(ImportRef {
                        local: local.trim().to_string(),
                        name: name.trim().to_string(),
                        module: module.trim().to_string(),
                    });
                }
            }
            (SupportedLang::Python, "import_statement") => {
                let rest = text.trim_start_matches("import ").trim();
                for part in rest.split(',').map(str::trim) {
                    let (module, local) = part.split_once(" as ").unwrap_or((part, part.split('.').next().unwrap_or(part)));
                    imports.push(ImportRef {
                        local: local.trim().to_string(),
                        name: "*".into(),
                        module: module.trim().to_string(),
                    });
                }
            }
            (SupportedLang::TypeScript | SupportedLang::Tsx, "import_statement") => {
                let Some(source) = node.child_by_field_name("source") else { continue };
                let module = node_text(source_code, source).trim_matches(['"', '\'', '`']).to_string();
                let clause = text
                    .trim_start_matches("import")
                    .trim_start()
                    .trim_start_matches("type ")
                    .split(" from ")
                    .next()
                    .unwrap_or("")
                    .trim();

                let (default_part, named_part) = match clause.find('{') {
                    Some(i) => (clause[..i].trim().trim_end_matches(','), Some(&clause[i..])),
                    None => (clause, None),
                };
                for item in default_part.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                    match item.strip_prefix("* as ") {
                        Some(ns) => imports.push(ImportRef { local: ns.trim().into(), name: "*".into(), module: module.clone() }),
                        None => imports.push(ImportRef { local: item.into(), name: "default".into(), module: module.clone() }),
                    }
                }
                if let Some(named) = named_part {
                    let named = named.trim().trim_start_matches('{').trim_end_matches('}');
                    for item in named.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        let item = item.trim_start_matches("type ").trim();
                        let (name, local) = item.split_once(" as ").unwrap_or((item, item));
                        imports.push(ImportRef { local: local.trim().into(), name: name.trim().into(), module: module.clone() });
                    }
                }
            }
            (SupportedLang::Go, "import_declaration") => collect_go_imports(source_code, node, &mut imports),
            (SupportedLang::Java, "import_declaration") => {
                let path = text.trim_start_matches("import").trim().trim_start_matches("static ").trim_end_matches(';').trim();
                match path.strip_suffix(".*") {
                    Some(module) => imports.push(ImportRef { local: "*".into(), name: "*".into(), module: module.into() }),
                    None => {
                        let (module, name) = path.rsplit_once('.').unwrap_or(("", path));
                        imports.push(ImportRef { local: name.into(), name: name.into(), module: module.into() });
                    }
                }
            }
            _ => {}
        }
    }

    imports
}

/// Go imports bind a whole package, by its alias or the path's last segment
/// (skipping a `/v2` major version suffix).
fn collect_go_imports(source_code: &str, node: Node, imports: &mut Vec<ImportRef>) {
    let mut stack = vec![node];
    while let Some(current) = stack.pop() {
        if current.kind() != "import_spec" {
            let mut cursor = current.walk();
            stack.extend(current.named_children(&mut cursor));
            continue;
        }
        let Some(path_node) = current.child_by_field_name("path") else { continue };
        let module = node_text(source_code, path_node).trim_matches(['"', '`']).to_string();
        let alias = current.child_by_field_name("name").map(|n| node_text(source_code, n));
        let mut segments = module.rsplit('/');
        let last = segments.next().unwrap_or(&module);
        let is_version = last.len() > 1 && last.starts_with('v') && last[1..].chars().all(|c| c.is_ascii_digit());
        let package = if is_version { segments.next().unwrap_or(last) } else { last };
        let local = match alias.as_deref() {
            Some("_") => continue,
            Some(a) => a.to_string(),
            None => package.to_string(),
        };
        imports.push(ImportRef { local, name: "*".into(), module });
    }
}

pub fn extract_file_symbols(
    file_rel_path: &str,
    source_code: &str,
    lang: SupportedLang,
) -> Result<Vec<SymbolNode>> {
    extract_file(file_rel_path, source_code, lang).map(|(symbols, _)| symbols)
}

/// Parse a file once and return both its symbols and its top-level imports.
pub fn extract_file(
    file_rel_path: &str,
    source_code: &str,
    lang: SupportedLang,
) -> Result<(Vec<SymbolNode>, Vec<ImportRef>)> {
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

    // Overloads (Java) and redefinitions share a name; give each its own id so none
    // is dropped from the graph. The name stays, so lookups by name find them all.
    let mut seen: HashMap<String, usize> = HashMap::new();
    for sym in &mut symbols {
        let count = seen.entry(sym.id.clone()).or_insert(0);
        *count += 1;
        if *count > 1 {
            sym.id = format!("{}#{count}", sym.id);
        }
    }

    let imports = collect_imports(source_code, root_node, lang);
    Ok((symbols, imports))
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
    fn test_tsx_component_extraction() {
        let code = r#"
export function Button(props: { label: string }) {
    return <button onClick={() => track(props.label)}>{props.label}</button>;
}

class Panel {
    render() {
        return <div><Button label="ok" /></div>;
    }
}
"#;
        let lang = SupportedLang::from_path(Path::new("ui/Button.tsx")).unwrap();
        assert_eq!(lang, SupportedLang::Tsx);
        let symbols = extract_file_symbols("ui/Button.tsx", code, lang).unwrap();

        let button = symbols.iter().find(|s| s.name == "Button").unwrap();
        assert_eq!(button.span, (2, 4));
        assert!(button.outgoing_calls.contains(&"track".to_string()));
        assert!(symbols.iter().any(|s| s.name == "Panel.render"));
    }

    #[test]
    fn test_ts_module_consts_and_arrow_components_are_symbols() {
        let code = r#"
/** Shared Prisma client. */
export const db = new PrismaClient();

export const Button = ({ label }: Props) => {
    return <button onClick={() => track(label)}>{label}</button>;
};

export const Card = memo(function Card(props: CardProps) {
    return render(props);
});

const { a, b } = load();

class Store {
    handleSave = async () => { await persist(); };
    count = 0;
}

function outer() {
    const local = () => helper();
    return local();
}
"#;
        let symbols = extract_file_symbols("ui/button.tsx", code, SupportedLang::Tsx).unwrap();
        let kind_of = |name: &str| symbols.iter().find(|s| s.name == name).map(|s| s.kind);

        assert_eq!(kind_of("db"), Some(SymbolKind::Constant));
        assert_eq!(kind_of("Button"), Some(SymbolKind::Function));
        assert_eq!(kind_of("Card"), Some(SymbolKind::Function));
        assert_eq!(kind_of("Store.handleSave"), Some(SymbolKind::Method));
        assert_eq!(kind_of("Store.count"), None, "plain fields are not methods");
        assert_eq!(kind_of("local"), None, "locals inside functions are not symbols");
        assert!(!symbols.iter().any(|s| s.name == "a" || s.name == "b"), "destructuring is skipped");

        let button = symbols.iter().find(|s| s.name == "Button").unwrap();
        assert_eq!(button.signature, "export const Button = ({ label }: Props) =>");
        assert_eq!(button.span, (5, 7));
        assert!(button.outgoing_calls.contains(&"track".to_string()));
        let card = symbols.iter().find(|s| s.name == "Card").unwrap();
        assert!(card.outgoing_calls.contains(&"render".to_string()));
        assert_eq!(symbols.iter().find(|s| s.name == "db").unwrap().signature, "export const db = new PrismaClient();");
    }

    fn import(local: &str, name: &str, module: &str) -> ImportRef {
        ImportRef { local: local.into(), name: name.into(), module: module.into() }
    }

    #[test]
    fn test_import_extraction_per_language() {
        let rust = "use crate::engine::{hybrid::{resolve_centrality, normalize as norm}, graph};\nuse std::path::Path;\nfn f() {}\n";
        let (_, imports) = extract_file("src/a.rs", rust, SupportedLang::Rust).unwrap();
        assert!(imports.contains(&import("resolve_centrality", "resolve_centrality", "crate::engine::hybrid")));
        assert!(imports.contains(&import("norm", "normalize", "crate::engine::hybrid")));
        assert!(imports.contains(&import("graph", "graph", "crate::engine")));
        assert!(imports.contains(&import("Path", "Path", "std::path")));

        let py = "from pkg.billing import charge, refund as undo\nimport pkg.util as u\n";
        let (_, imports) = extract_file("a.py", py, SupportedLang::Python).unwrap();
        assert!(imports.contains(&import("charge", "charge", "pkg.billing")));
        assert!(imports.contains(&import("undo", "refund", "pkg.billing")));
        assert!(imports.contains(&import("u", "*", "pkg.util")));

        let ts = "import Api, { fetchUser, save as persist } from './api';\nimport * as fmt from '../fmt';\n";
        let (_, imports) = extract_file("src/a.ts", ts, SupportedLang::TypeScript).unwrap();
        assert!(imports.contains(&import("Api", "default", "./api")));
        assert!(imports.contains(&import("fetchUser", "fetchUser", "./api")));
        assert!(imports.contains(&import("persist", "save", "./api")));
        assert!(imports.contains(&import("fmt", "*", "../fmt")));
    }

    #[test]
    fn test_call_paths_keep_qualifiers() {
        assert_eq!(normalize_call_path("self.check"), Some("check".into()));
        assert_eq!(normalize_call_path("Vec::<u8>::new"), Some("Vec::new".into()));
        assert_eq!(normalize_call_path("hybrid::resolve"), Some("hybrid::resolve".into()));
        assert_eq!(normalize_call_path("utils.format"), Some("utils.format".into()));
        assert_eq!(normalize_call_path("(a || b)"), None);
    }

    #[test]
    fn test_go_symbol_and_import_extraction() {
        let code = r#"package store

import (
	"fmt"
	cache "github.com/acme/app/internal/lru/v2"
	_ "embed"
)

// Store keeps rows in memory.
type Store[T any] struct {
	rows map[string]T
}

type Getter interface {
	Get(key string) (string, error)
}

type ID = string

func New() *Store[int] {
	return &Store[int]{rows: cache.Make()}
}

func (s *Store[T]) Get(key string) (T, error) {
	fmt.Println(key)
	return s.lookup(key)
}
"#;
        let lang = SupportedLang::from_path(Path::new("internal/store/store.go")).unwrap();
        let (symbols, imports) = extract_file("internal/store/store.go", code, lang).unwrap();

        let kind_of = |name: &str| symbols.iter().find(|s| s.name == name).map(|s| s.kind);
        assert_eq!(kind_of("Store"), Some(SymbolKind::Struct));
        assert_eq!(kind_of("Getter"), Some(SymbolKind::Interface));
        assert_eq!(kind_of("ID"), Some(SymbolKind::TypeAlias));
        assert_eq!(kind_of("New"), Some(SymbolKind::Function));

        let get = symbols.iter().find(|s| s.name == "Store.Get").unwrap();
        assert_eq!(get.kind, SymbolKind::Method);
        assert_eq!(get.signature, "func (s *Store[T]) Get(key string) (T, error)");
        assert!(get.outgoing_calls.contains(&"fmt.Println".to_string()));
        assert!(get.outgoing_calls.contains(&"s.lookup".to_string()));

        assert!(imports.contains(&import("fmt", "*", "fmt")));
        assert!(imports.contains(&import("cache", "*", "github.com/acme/app/internal/lru/v2")));
        assert!(!imports.iter().any(|i| i.module == "embed"), "blank imports bind nothing");
    }

    #[test]
    fn test_java_symbol_and_import_extraction() {
        let code = r#"package com.acme.billing;

import com.acme.store.Repository;
import static java.util.Objects.requireNonNull;
import java.util.*;

/** Charges customers. */
public class InvoiceService {
    private final Repository repo;

    public InvoiceService(Repository repo) {
        this.repo = requireNonNull(repo);
    }

    @Override
    public Invoice charge(String id) {
        Invoice invoice = new Invoice(id);
        repo.save(invoice);
        return build().finish();
    }

    public Invoice charge(String id, int cents) {
        return charge(id);
    }

    interface Hook {
        void run();
    }

    enum State { OPEN, PAID }
}
"#;
        let lang = SupportedLang::from_path(Path::new("src/main/java/com/acme/billing/InvoiceService.java")).unwrap();
        let (symbols, imports) = extract_file("InvoiceService.java", code, lang).unwrap();

        let kind_of = |name: &str| symbols.iter().find(|s| s.name == name).map(|s| s.kind);
        assert_eq!(kind_of("InvoiceService"), Some(SymbolKind::Class));
        assert_eq!(kind_of("InvoiceService.InvoiceService"), Some(SymbolKind::Method));
        assert_eq!(kind_of("Hook"), Some(SymbolKind::Interface));
        assert_eq!(kind_of("Hook.run"), Some(SymbolKind::Method));
        assert_eq!(kind_of("State"), Some(SymbolKind::Enum));

        let charge = symbols.iter().find(|s| s.name == "InvoiceService.charge").unwrap();
        assert_eq!(charge.signature, "public Invoice charge(String id)");
        assert_eq!(charge.span.0, 15, "span starts at the annotation");
        for call in ["Invoice", "repo.save", "build", "finish"] {
            assert!(charge.outgoing_calls.contains(&call.to_string()), "missing call {call}");
        }
        assert_eq!(symbols.iter().find(|s| s.name == "Hook.run").unwrap().signature, "void run()");

        let overload_ids: Vec<&str> =
            symbols.iter().filter(|s| s.name == "InvoiceService.charge").map(|s| s.id.as_str()).collect();
        assert_eq!(
            overload_ids,
            vec!["InvoiceService.java::InvoiceService.charge", "InvoiceService.java::InvoiceService.charge#2"]
        );

        assert!(imports.contains(&import("Repository", "Repository", "com.acme.store")));
        assert!(imports.contains(&import("requireNonNull", "requireNonNull", "java.util.Objects")));
        assert!(imports.contains(&import("*", "*", "java.util")));
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
