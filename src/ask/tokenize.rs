/// Words too common in questions and code to separate one symbol from another.
const STOP_WORDS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "by", "do", "does", "for", "from", "how", "i", "if",
    "in", "into", "is", "it", "its", "of", "on", "or", "so", "that", "the", "their", "then", "this",
    "to", "up", "was", "we", "what", "when", "where", "which", "who", "why", "with", "fn", "def",
    "function", "pub", "let", "const", "self", "mut", "return", "async", "await", "export", "impl",
    "my", "me", "out", "get", "set", "new", "use",
];

const MIN_TOKEN_LEN: usize = 2;

/// Light suffix stripping so `redirects`, `redirected` and `redirecting` meet `redirect`.
/// Deliberately weaker than Porter: identifiers are short and over-stemming merges
/// unrelated names.
fn stem(word: &str) -> String {
    let rules: &[(&str, &str, usize)] = &[
        ("ies", "y", 4),
        ("sses", "ss", 5),
        ("ing", "", 5),
        ("ed", "", 4),
        ("es", "", 5),
        ("s", "", 4),
    ];
    for (suffix, replacement, min_len) in rules {
        if word.len() >= *min_len && word.ends_with(suffix) && !word.ends_with("ss") {
            return format!("{}{replacement}", &word[..word.len() - suffix.len()]);
        }
    }
    word.to_string()
}

/// Split one identifier on `_` and case changes: `parseHTTPResponse2` gives
/// `parse`, `http`, `response`, `2`.
fn split_identifier(token: &str) -> Vec<String> {
    let mut parts = Vec::new();
    for chunk in token.split('_').filter(|c| !c.is_empty()) {
        let chars: Vec<char> = chunk.chars().collect();
        let mut current = String::new();
        for (i, &c) in chars.iter().enumerate() {
            let prev = i.checked_sub(1).map(|p| chars[p]);
            let next = chars.get(i + 1).copied();
            let boundary = match prev {
                Some(p) if c.is_uppercase() && p.is_lowercase() => true,
                Some(p) if c.is_uppercase() && p.is_uppercase() && next.is_some_and(|n| n.is_lowercase()) => true,
                Some(p) if c.is_ascii_digit() != p.is_ascii_digit() => true,
                _ => false,
            };
            if boundary && !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
            current.push(c);
        }
        if !current.is_empty() {
            parts.push(current);
        }
    }
    parts
}

/// Tokens for search: identifiers are split into words, lowercased, stemmed and
/// filtered. Whole compound identifiers are kept too, so an exact name still
/// matches strongly.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    for raw in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if raw.is_empty() {
            continue;
        }
        let parts = split_identifier(raw);
        if parts.len() > 1 {
            tokens.push(raw.to_lowercase());
        }
        for part in parts {
            let lower = part.to_lowercase();
            if lower.len() < MIN_TOKEN_LEN || STOP_WORDS.contains(&lower.as_str()) {
                continue;
            }
            tokens.push(stem(&lower));
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_camel_snake_and_acronyms() {
        assert_eq!(split_identifier("parseHTTPResponse2"), vec!["parse", "HTTP", "Response", "2"]);
        assert_eq!(split_identifier("check_git_diff"), vec!["check", "git", "diff"]);
        assert_eq!(split_identifier("XMLParser"), vec!["XML", "Parser"]);
    }

    #[test]
    fn tokenize_keeps_compounds_and_stems_words() {
        let tokens = tokenize("how are redirects followed in resolveRedirects?");
        assert!(tokens.contains(&"redirect".to_string()));
        assert!(tokens.contains(&"resolveredirects".to_string()));
        assert!(tokens.contains(&"resolve".to_string()));
        assert!(!tokens.contains(&"how".to_string()));
        assert!(!tokens.contains(&"are".to_string()));
    }

    #[test]
    fn stemming_is_conservative() {
        assert_eq!(stem("redirecting"), "redirect");
        assert_eq!(stem("redirected"), "redirect");
        assert_eq!(stem("entries"), "entry");
        assert_eq!(stem("class"), "class");
        assert_eq!(stem("process"), "process");
        assert_eq!(stem("bus"), "bus");
    }
}
