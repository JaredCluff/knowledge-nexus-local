//! Normalization + SHA-256 helper for article content_hash.
//!
//! Used by the SQLite → SurrealDB migrator and, in P3, by the dedup
//! pipeline. Keeping both users on the same function guarantees that
//! a migrated article hashes to the same value that a newly-ingested
//! one does.

use sha2::{Digest, Sha256};

/// Normalize then SHA-256 the content. Normalization:
///   - strip leading/trailing whitespace
///   - collapse internal runs of whitespace to a single space
///   - lowercase
///
/// This produces stable hashes across minor formatting differences
/// without throwing away enough information that genuinely different
/// articles collide.
pub fn content_hash(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_ws = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !last_was_ws && !out.is_empty() {
                out.push(' ');
            }
            last_was_ws = true;
        } else {
            out.extend(ch.to_lowercase());
            last_was_ws = false;
        }
    }
    while out.ends_with(' ') {
        out.pop();
    }

    let digest = Sha256::digest(out.as_bytes());
    hex::encode(digest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_hash_is_stable_across_formatting() {
        let a = content_hash("Hello World");
        let b = content_hash("  hello   world  ");
        let c = content_hash("HELLO\nWORLD");
        assert_eq!(a, b);
        assert_eq!(a, c);
    }

    #[test]
    fn test_content_hash_differs_for_different_content() {
        assert_ne!(content_hash("foo"), content_hash("bar"));
    }

    #[test]
    fn test_content_hash_hex_length() {
        assert_eq!(content_hash("anything").len(), 64);
    }
}
