use anyhow::Result;
use once_cell::sync::Lazy;
use regex::Regex;
use serde::Deserialize;
use std::fs;

#[derive(Debug, Deserialize)]
struct RedactionPattern {
    name: String,
    regex: String,
}

#[derive(Debug, Deserialize)]
struct PrivacyRules {
    redaction_patterns: Vec<RedactionPattern>,
}

/// Compiled redaction rules loaded from PRIVACY_RULES.json
static RULES: Lazy<Vec<(String, Regex)>> = Lazy::new(|| {
    load_rules().unwrap_or_else(|e| {
        tracing::warn!("Failed to load PRIVACY_RULES.json: {e}; using defaults");
        default_rules()
    })
});

fn load_rules() -> Result<Vec<(String, Regex)>> {
    // Walk up from cwd to find PRIVACY_RULES.json
    let candidates = [
        "PRIVACY_RULES.json",
        "../PRIVACY_RULES.json",
        "../../PRIVACY_RULES.json",
    ];
    for path in candidates {
        if let Ok(content) = fs::read_to_string(path) {
            let rules: PrivacyRules = serde_json::from_str(&content)?;
            return rules
                .redaction_patterns
                .into_iter()
                .map(|p| Ok((p.name, Regex::new(&p.regex)?)))
                .collect();
        }
    }
    anyhow::bail!("PRIVACY_RULES.json not found")
}

fn default_rules() -> Vec<(String, Regex)> {
    [
        ("Generic_API_Key", r"[a-zA-Z0-9]{32,}"),
        ("Bearer_Token", r"Bearer [a-zA-Z0-9\-\._~\+\/]+=*"),
        ("Env_Variable", r"([A-Z_]{3,15})=\S+"),
        ("Private_Key", r"-----BEGIN [A-Z ]+ PRIVATE KEY-----"),
    ]
    .into_iter()
    .filter_map(|(name, pattern)| {
        Regex::new(pattern)
            .ok()
            .map(|re| (name.to_string(), re))
    })
    .collect()
}

/// Scrub all privacy-sensitive patterns from content.
/// Matched spans are replaced with `[REDACTED:<rule_name>]`.
pub fn scrub(content: &str) -> String {
    let mut result = content.to_string();
    for (name, pattern) in RULES.iter() {
        let replacement = format!("[REDACTED:{}]", name);
        result = pattern.replace_all(&result, replacement.as_str()).into_owned();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrubs_bearer_token() {
        let input = "Authorization: Bearer eyJhbGciOiJSUzI1NiIsInR5cCI6IkpXVCJ9.abc";
        let out = scrub(input);
        assert!(!out.contains("eyJ"), "token should be redacted");
        assert!(out.contains("[REDACTED:"), "should contain redaction marker");
    }

    #[test]
    fn scrubs_env_variable() {
        let input = "DATABASE_URL=postgres://user:pass@host/db";
        let out = scrub(input);
        assert!(!out.contains("postgres://"), "value should be redacted");
    }

    #[test]
    fn passes_clean_content() {
        let input = "Use LanceDB for vector storage in this project.";
        let out = scrub(input);
        assert_eq!(out, input);
    }
}
