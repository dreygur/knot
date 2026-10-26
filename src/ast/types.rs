use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Class,
    Interface,
    Trait,
    Enum,
    Module,
    Constant,
    TypeAlias,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Method => "method",
            SymbolKind::Struct => "struct",
            SymbolKind::Class => "class",
            SymbolKind::Interface => "interface",
            SymbolKind::Trait => "trait",
            SymbolKind::Enum => "enum",
            SymbolKind::Module => "module",
            SymbolKind::Constant => "constant",
            SymbolKind::TypeAlias => "type",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolNode {
    /// Qualified ID: e.g. "src/auth.rs::validate_jwt"
    pub id: String,
    /// Repo-relative file path
    pub file: String,
    /// Bare symbol name: e.g. "validate_jwt"
    pub name: String,
    pub kind: SymbolKind,
    /// Definition signature: e.g. "pub async fn validate_jwt(token: &str) -> Result<Claims>"
    pub signature: String,
    /// 1-indexed line span: (start_line, end_line)
    pub span: (usize, usize),
    /// Key crux excerpt (up to 8 lines)
    pub crux: String,
    /// BLAKE3 hash of the symbol's exact definition text
    pub hash: String,
    /// Function/method names called within this symbol's body
    pub outgoing_calls: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeKind {
    Calls,
    Imports,
    Implements,
    References,
}

impl EdgeKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            EdgeKind::Calls => "calls",
            EdgeKind::Imports => "imports",
            EdgeKind::Implements => "implements",
            EdgeKind::References => "references",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CallEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlastRadiusReport {
    pub target: String,
    pub depth: usize,
    pub impacted_symbols: Vec<SymbolNode>,
    pub impacted_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkeletonItem {
    pub name: String,
    pub kind: SymbolKind,
    pub signature: String,
    pub span: (usize, usize),
}
