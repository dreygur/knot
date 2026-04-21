use std::path::Path;

/// Returns the BLAKE3 hex digest of a file's contents.
pub fn calculate_hash(path: &Path) -> anyhow::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn known_hash() {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(b"hello world").unwrap();
        let hex = calculate_hash(f.path()).unwrap();
        assert_eq!(
            hex,
            blake3::hash(b"hello world").to_hex().to_string()
        );
    }

    #[test]
    fn empty_file() {
        let f = NamedTempFile::new().unwrap();
        let hex = calculate_hash(f.path()).unwrap();
        assert_eq!(hex, blake3::hash(b"").to_hex().to_string());
    }

    #[test]
    fn missing_file_errors() {
        assert!(calculate_hash(Path::new("/nonexistent/file.txt")).is_err());
    }
}
