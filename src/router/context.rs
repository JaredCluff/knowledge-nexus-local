use regex::Regex;
use std::sync::LazyLock;

static PERSONAL_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)\b(my|mine|i|me)\b").expect("known valid regex"),
        Regex::new(r"(?i)\b(my notes|my files|my documents)\b").expect("known valid regex"),
        Regex::new(r"(?i)\b(personal|private)\b").expect("known valid regex"),
    ]
});

static FAMILY_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)\b(family|shared|everyone|our|household)\b").expect("known valid regex"),
        Regex::new(r"(?i)\b(family recipe|family photo|family budget)\b")
            .expect("known valid regex"),
        Regex::new(r"(?i)\b(mom|dad|brother|sister|spouse|partner|kid)\b")
            .expect("known valid regex"),
    ]
});

pub struct ContextClassifier;

impl ContextClassifier {
    pub fn new() -> Self {
        Self
    }

    /// Classify query into scope: "personal", "family", or "all"
    pub fn classify(&self, query: &str) -> String {
        let personal_score: usize = PERSONAL_PATTERNS
            .iter()
            .filter(|p| p.is_match(query))
            .count();
        let family_score: usize = FAMILY_PATTERNS.iter().filter(|p| p.is_match(query)).count();

        if family_score > 0 && family_score >= personal_score {
            "family".to_string()
        } else if personal_score > 0 {
            "personal".to_string()
        } else {
            "all".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_personal_queries() {
        let c = ContextClassifier::new();
        assert_eq!(c.classify("my notes about rust"), "personal");
        assert_eq!(c.classify("where are my documents"), "personal");
        assert_eq!(c.classify("show me my personal files"), "personal");
    }

    #[test]
    fn test_family_queries() {
        let c = ContextClassifier::new();
        assert_eq!(c.classify("family recipe for pasta"), "family");
        assert_eq!(c.classify("our shared budget"), "family");
        assert_eq!(c.classify("what did everyone decide"), "family");
    }

    #[test]
    fn test_general_queries() {
        let c = ContextClassifier::new();
        assert_eq!(c.classify("how to fix a leaky faucet"), "all");
        assert_eq!(c.classify("rust programming tutorial"), "all");
    }
}
