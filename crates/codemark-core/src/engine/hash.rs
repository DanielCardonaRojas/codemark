use sha2::{Digest, Sha256};

/// Normalize text for hashing: strip per-line whitespace, collapse blank lines,
/// replace whitespace runs with single space.
pub fn normalize_for_hash(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_blank = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank && !result.is_empty() {
                result.push('\n');
            }
            prev_blank = true;
            continue;
        }
        prev_blank = false;
        if !result.is_empty() {
            result.push('\n');
        }
        // Collapse whitespace runs within the line
        let mut in_ws = false;
        for ch in trimmed.chars() {
            if ch.is_whitespace() {
                if !in_ws {
                    result.push(' ');
                    in_ws = true;
                }
            } else {
                result.push(ch);
                in_ws = false;
            }
        }
    }

    result
}

/// Normalize text and compute SHA-256 hash, returning "sha256:<first 16 hex chars>".
pub fn content_hash(text: &str) -> String {
    let normalized = normalize_for_hash(text);
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    let hash = hasher.finalize();
    let hex: String = hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_format() {
        let h = content_hash("fn hello() {}");
        assert!(h.starts_with("sha256:"));
        // sha256: prefix (7 chars) + 16 hex chars = 23
        assert_eq!(h.len(), 23);
    }

    #[test]
    fn hash_stable_across_indentation() {
        let a = "fn hello() {\n    let x = 1;\n}";
        let b = "fn hello() {\n        let x = 1;\n}";
        assert_eq!(content_hash(a), content_hash(b));
    }

    #[test]
    fn hash_stable_across_trailing_whitespace() {
        let a = "fn hello() {  \n    let x = 1;  \n}  ";
        let b = "fn hello() {\n    let x = 1;\n}";
        assert_eq!(content_hash(a), content_hash(b));
    }

    #[test]
    fn hash_stable_across_blank_lines() {
        let a = "fn hello() {\n\n\n    let x = 1;\n}";
        let b = "fn hello() {\n\n    let x = 1;\n}";
        assert_eq!(content_hash(a), content_hash(b));
    }

    #[test]
    fn hash_differs_for_different_content() {
        assert_ne!(content_hash("fn hello() {}"), content_hash("fn world() {}"));
    }

    #[test]
    fn normalize_collapses_whitespace() {
        let input = "fn   hello(  x:   Int  )  {  }";
        let normalized = normalize_for_hash(input);
        assert_eq!(normalized, "fn hello( x: Int ) { }");
    }

    #[test]
    fn normalize_handles_empty_input() {
        assert_eq!(normalize_for_hash(""), "");
        assert_eq!(normalize_for_hash("   "), "");
        assert_eq!(normalize_for_hash("\n\n\n"), "");
    }
}
